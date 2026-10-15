//! Segmented download with dynamic splitting and resume.
//!
//! The file is preallocated as `name.ldm-part`. Each connection owns one segment
//! and writes at its offset. When a connection has nothing left, it takes an
//! unowned segment or splits the largest one still in progress at the midpoint of
//! what remains. The connection that owned the split segment notices its new,
//! smaller `end` on its next chunk and stops there.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, IF_RANGE, RANGE};
use reqwest::{Client, StatusCode};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::control::{self, ControlFile, SegmentState};
use crate::fileio::write_all_at;
use crate::probe::{probe, ProbeResult};
use crate::{filename, Error, RequestSpec, Result};

/// Bytes buffered per connection before a positional write.
const FLUSH_SIZE: usize = 256 * 1024;
/// Splits never create segments smaller than this. It must stay well above
/// `FLUSH_SIZE` plus one network chunk, so a split point is always beyond the
/// data a connection may still hold in its buffer.
const MIN_SPLIT_FLOOR: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Maximum simultaneous connections for one file.
    pub connections: usize,
    /// Smallest segment produced by a split.
    pub min_split_size: u64,
    /// Retries per segment before the connection gives up.
    pub max_retries: u32,
    /// Abort a connection that receives nothing for this long.
    pub idle_timeout: Duration,
    /// Delay between opening each additional connection.
    pub connection_stagger: Duration,
    /// Shared speed limit (all connections, possibly all downloads).
    pub limiter: Option<Arc<crate::RateLimiter>>,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            connections: 8,
            min_split_size: MIN_SPLIT_FLOOR,
            max_retries: 8,
            idle_timeout: Duration::from_secs(30),
            connection_stagger: Duration::from_millis(150),
            limiter: None,
        }
    }
}

/// Where to save.
#[derive(Debug, Clone)]
pub enum Target {
    /// Pick the name (given or detected) inside `dir`, avoiding collisions, or
    /// resume an existing `.ldm-part` with that name.
    Dir { dir: PathBuf, file_name: Option<String> },
    /// Exact final path (used when resuming a known download).
    File(PathBuf),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Bytes per second (smoothed).
    pub speed: f64,
    pub active_connections: usize,
    pub segments: Vec<SegmentState>,
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Started {
        path: PathBuf,
        total: Option<u64>,
        /// False when the server has no Range support: pausing restarts from zero.
        resumable: bool,
    },
    Progress(Progress),
}

#[derive(Debug)]
pub enum Outcome {
    Completed(PathBuf),
    /// Stopped by the cancellation token; the partial data is kept when resumable.
    Paused(PathBuf),
}

/// Runs one download to completion, pause (via `cancel`) or error.
pub async fn download(
    client: &Client,
    spec: &RequestSpec,
    target: Target,
    opts: &DownloadOptions,
    events: mpsc::UnboundedSender<EngineEvent>,
    cancel: CancellationToken,
) -> Result<Outcome> {
    let probe = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::Cancelled),
        p = probe(client, spec) => p?,
    };

    let final_path = match target {
        Target::File(path) => path,
        Target::Dir { dir, file_name } => {
            let name = file_name
                .as_deref()
                .map(filename::sanitize)
                .unwrap_or_else(|| probe.file_name.clone());
            let candidate = dir.join(&name);
            if control::part_path(&candidate).exists() {
                candidate
            } else {
                filename::unique_path(&dir, &name)
            }
        }
    };
    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let segmented = probe.ranges && probe.total.is_some_and(|t| t > 0);
    let _ = events.send(EngineEvent::Started {
        path: final_path.clone(),
        total: probe.total,
        resumable: segmented,
    });

    if probe.total == Some(0) {
        tokio::fs::write(&final_path, b"").await?;
        return Ok(Outcome::Completed(final_path));
    }
    if segmented {
        segmented_download(client, spec, &probe, final_path, opts, events, cancel).await
    } else {
        single_stream(client, spec, &probe, final_path, opts, events, cancel).await
    }
}

/// Moves the finished `.ldm-part` to its final name, picking a new name if a file
/// appeared there in the meantime.
async fn finalize(part: &Path, final_path: PathBuf) -> Result<PathBuf> {
    let destination = if tokio::fs::try_exists(&final_path).await.unwrap_or(false) {
        let dir = final_path.parent().unwrap_or(Path::new("."));
        let name = final_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        filename::unique_path(dir, &name)
    } else {
        final_path.clone()
    };
    tokio::fs::rename(part, &destination).await?;
    let _ = tokio::fs::remove_file(control::control_path(&final_path)).await;
    Ok(destination)
}

struct SpeedMeter {
    last_bytes: u64,
    last_at: Instant,
    speed: f64,
}

impl SpeedMeter {
    fn new(bytes: u64) -> Self {
        Self { last_bytes: bytes, last_at: Instant::now(), speed: 0.0 }
    }

    fn update(&mut self, bytes: u64) -> f64 {
        let elapsed = self.last_at.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            let instant = bytes.saturating_sub(self.last_bytes) as f64 / elapsed;
            self.speed = if self.speed == 0.0 { instant } else { self.speed * 0.7 + instant * 0.3 };
            self.last_bytes = bytes;
            self.last_at = Instant::now();
        }
        self.speed
    }
}

// ---------------------------------------------------------------------------
// Single connection (no Range support or unknown size)
// ---------------------------------------------------------------------------

async fn single_stream(
    client: &Client,
    spec: &RequestSpec,
    probe: &ProbeResult,
    final_path: PathBuf,
    opts: &DownloadOptions,
    events: mpsc::UnboundedSender<EngineEvent>,
    cancel: CancellationToken,
) -> Result<Outcome> {
    use tokio::io::AsyncWriteExt;

    let part = control::part_path(&final_path);
    let resp = spec.apply(client.get(probe.final_url.clone())).send().await?;
    if !resp.status().is_success() {
        return Err(Error::Status(resp.status()));
    }
    let total = resp.content_length().or(probe.total);
    let mut file = tokio::fs::File::create(&part).await?;
    let mut stream = resp.bytes_stream();
    let mut downloaded = 0u64;
    let mut meter = SpeedMeter::new(0);
    let mut last_report = Instant::now();

    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => {
                drop(file);
                let _ = tokio::fs::remove_file(&part).await;
                return Ok(Outcome::Paused(final_path));
            }
            next = tokio::time::timeout(opts.idle_timeout, stream.next()) => next,
        };
        match next {
            Err(_) => return Err(Error::Other("tempo esgotado esperando dados do servidor".into())),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(e.into()),
            Ok(Some(Ok(chunk))) => {
                file.write_all(&chunk).await?;
                downloaded += chunk.len() as u64;
                if let Some(limiter) = &opts.limiter {
                    limiter.consume(chunk.len()).await;
                }
            }
        }
        if last_report.elapsed() >= Duration::from_millis(500) {
            last_report = Instant::now();
            let _ = events.send(EngineEvent::Progress(Progress {
                downloaded,
                total,
                speed: meter.update(downloaded),
                active_connections: 1,
                segments: Vec::new(),
            }));
        }
    }

    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    if let Some(total) = total {
        if downloaded != total {
            return Err(Error::Other(format!("conexão encerrada em {downloaded} de {total} bytes")));
        }
    }
    let _ = events.send(EngineEvent::Progress(Progress {
        downloaded,
        total: Some(downloaded),
        speed: 0.0,
        active_connections: 0,
        segments: Vec::new(),
    }));
    Ok(Outcome::Completed(finalize(&part, final_path).await?))
}

// ---------------------------------------------------------------------------
// Segmented download
// ---------------------------------------------------------------------------

struct Segment {
    state: SegmentState,
    owned: bool,
}

enum Claim {
    Segment(usize),
    /// Everything is owned and too small to split; check again later in case a
    /// connection fails and releases its segment.
    Wait,
    Done,
}

struct Ctx {
    client: Client,
    spec: RequestSpec,
    url: Url,
    etag: Option<String>,
    file: Arc<File>,
    segments: Mutex<Vec<Segment>>,
    downloaded: AtomicU64,
    min_split: u64,
    max_retries: u32,
    idle_timeout: Duration,
    limiter: Option<Arc<crate::RateLimiter>>,
    cancel: CancellationToken,
}

impl Ctx {
    fn claim(&self) -> Claim {
        let mut segs = self.segments.lock().unwrap();
        if segs.iter().all(|s| s.state.remaining() == 0) {
            return Claim::Done;
        }
        if let Some(i) = segs.iter().position(|s| !s.owned && s.state.remaining() > 0) {
            segs[i].owned = true;
            return Claim::Segment(i);
        }
        let largest = segs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.owned)
            .max_by_key(|(_, s)| s.state.remaining())
            .map(|(i, s)| (i, s.state));
        if let Some((i, state)) = largest {
            let remaining = state.remaining();
            if remaining >= 2 * self.min_split {
                let mid = state.pos() + remaining / 2;
                segs[i].state.end = mid - 1;
                segs.push(Segment {
                    state: SegmentState { start: mid, end: state.end, done: 0 },
                    owned: true,
                });
                return Claim::Segment(segs.len() - 1);
            }
        }
        Claim::Wait
    }

    fn release(&self, index: usize) {
        self.segments.lock().unwrap()[index].owned = false;
    }

    fn bounds(&self, index: usize) -> (u64, u64) {
        let segs = self.segments.lock().unwrap();
        (segs[index].state.pos(), segs[index].state.end)
    }

    fn end_of(&self, index: usize) -> u64 {
        self.segments.lock().unwrap()[index].state.end
    }

    fn snapshot(&self) -> Vec<SegmentState> {
        self.segments.lock().unwrap().iter().map(|s| s.state).collect()
    }

    fn active(&self) -> usize {
        self.segments
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.owned && s.state.remaining() > 0)
            .count()
    }

    /// Writes the buffer at `*offset`, then records the progress.
    async fn flush(&self, index: usize, buf: &mut Vec<u8>, offset: &mut u64) -> Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(buf);
        let len = data.len() as u64;
        let file = self.file.clone();
        let at = *offset;
        let mut data = tokio::task::spawn_blocking(move || write_all_at(&file, &data, at).map(|_| data))
            .await
            .map_err(|e| Error::Other(format!("falha na escrita: {e}")))??;
        {
            let mut segs = self.segments.lock().unwrap();
            let state = &mut segs[index].state;
            state.done = (state.done + len).min(state.len());
        }
        self.downloaded.fetch_add(len, Ordering::Relaxed);
        *offset += len;
        data.clear();
        *buf = data;
        Ok(())
    }

    /// Downloads the rest of segment `index` with one HTTP request.
    async fn fetch_segment(&self, index: usize) -> Result<()> {
        let (from, end) = self.bounds(index);
        if from > end {
            return Ok(());
        }
        let mut request = self
            .spec
            .apply(self.client.get(self.url.clone()))
            .header(RANGE, format!("bytes={from}-{end}"));
        if let Some(etag) = &self.etag {
            request = request.header(IF_RANGE, etag.as_str());
        }
        let resp = tokio::select! {
            _ = self.cancel.cancelled() => return Err(Error::Cancelled),
            r = request.send() => r?,
        };
        match resp.status() {
            StatusCode::PARTIAL_CONTENT => {}
            StatusCode::OK => {
                return Err(Error::Other("o servidor ignorou o Range (o arquivo mudou?)".into()));
            }
            status => return Err(Error::Status(status)),
        }
        let range_start = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes "))
            .and_then(|v| v.split('-').next())
            .and_then(|v| v.trim().parse::<u64>().ok());
        if range_start.is_some_and(|s| s != from) {
            return Err(Error::Other("Content-Range diferente do pedido".into()));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = Vec::with_capacity(FLUSH_SIZE);
        let mut offset = from;
        loop {
            let next = tokio::select! {
                _ = self.cancel.cancelled() => {
                    self.flush(index, &mut buf, &mut offset).await?;
                    return Err(Error::Cancelled);
                }
                next = tokio::time::timeout(self.idle_timeout, stream.next()) => next,
            };
            let chunk = match next {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(e))) => {
                    self.flush(index, &mut buf, &mut offset).await?;
                    return Err(e.into());
                }
                Ok(None) => {
                    self.flush(index, &mut buf, &mut offset).await?;
                    return if offset > self.end_of(index) {
                        Ok(())
                    } else {
                        Err(Error::Other("conexão encerrada antes do fim do segmento".into()))
                    };
                }
                Err(_) => {
                    self.flush(index, &mut buf, &mut offset).await?;
                    return Err(Error::Other("tempo esgotado esperando dados".into()));
                }
            };

            if let Some(limiter) = &self.limiter {
                limiter.consume(chunk.len()).await;
            }

            // Another connection may have taken the tail of this segment.
            let end_now = self.end_of(index);
            let cursor = offset + buf.len() as u64;
            if cursor > end_now {
                self.flush(index, &mut buf, &mut offset).await?;
                return Ok(());
            }
            let allowed = (end_now + 1 - cursor) as usize;
            let take = chunk.len().min(allowed);
            buf.extend_from_slice(&chunk[..take]);
            let reached_end = cursor + take as u64 > end_now;
            if buf.len() >= FLUSH_SIZE || reached_end {
                self.flush(index, &mut buf, &mut offset).await?;
            }
            if reached_end {
                return Ok(());
            }
        }
    }
}

async fn sleep_or_cancel(cancel: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

async fn worker(ctx: Arc<Ctx>, delay: Duration) -> Result<()> {
    if !delay.is_zero() && !sleep_or_cancel(&ctx.cancel, delay).await {
        return Ok(());
    }
    loop {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let index = match ctx.claim() {
            Claim::Done => return Ok(()),
            Claim::Wait => {
                if !sleep_or_cancel(&ctx.cancel, Duration::from_millis(500)).await {
                    return Ok(());
                }
                continue;
            }
            Claim::Segment(index) => index,
        };

        let mut failures = 0u32;
        loop {
            match ctx.fetch_segment(index).await {
                Ok(()) => break,
                Err(_) if ctx.cancel.is_cancelled() => {
                    ctx.release(index);
                    return Ok(());
                }
                Err(e) => {
                    failures += 1;
                    tracing::warn!(segment = index, attempt = failures, error = %e, "segmento falhou");
                    if failures > ctx.max_retries {
                        ctx.release(index);
                        return Err(e);
                    }
                    let backoff = Duration::from_secs(1 << failures.min(4));
                    if !sleep_or_cancel(&ctx.cancel, backoff).await {
                        ctx.release(index);
                        return Ok(());
                    }
                }
            }
        }
        ctx.release(index);
    }
}

async fn segmented_download(
    client: &Client,
    spec: &RequestSpec,
    probe: &ProbeResult,
    final_path: PathBuf,
    opts: &DownloadOptions,
    events: mpsc::UnboundedSender<EngineEvent>,
    cancel: CancellationToken,
) -> Result<Outcome> {
    let total = probe.total.expect("segmented downloads have a known size");
    let part = control::part_path(&final_path);
    let control_file = control::control_path(&final_path);

    let saved = control::load(&control_file)
        .filter(|c| c.total == total && part.exists())
        .filter(|c| c.etag.is_none() || probe.etag.is_none() || c.etag == probe.etag);
    let states = match saved {
        Some(c) => c.segments,
        None => vec![SegmentState { start: 0, end: total - 1, done: 0 }],
    };
    let already: u64 = states.iter().map(|s| s.done).sum();

    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&part)?;
    if file.metadata()?.len() != total {
        file.set_len(total)?;
    }

    let ctx = Arc::new(Ctx {
        client: client.clone(),
        spec: spec.clone(),
        url: probe.final_url.clone(),
        etag: probe.etag.clone(),
        file: Arc::new(file),
        segments: Mutex::new(states.into_iter().map(|state| Segment { state, owned: false }).collect()),
        downloaded: AtomicU64::new(already),
        min_split: opts.min_split_size.max(MIN_SPLIT_FLOOR),
        max_retries: opts.max_retries,
        idle_timeout: opts.idle_timeout,
        limiter: opts.limiter.clone(),
        cancel: cancel.clone(),
    });

    let save = |ctx: &Ctx| {
        let control = ControlFile {
            version: control::VERSION,
            url: spec.url.clone(),
            total,
            etag: probe.etag.clone(),
            segments: ctx.snapshot(),
        };
        if let Err(e) = control::save(&control_file, &control) {
            tracing::warn!(error = %e, "não foi possível salvar o arquivo de controle");
        }
    };
    save(&ctx);

    let mut workers = JoinSet::new();
    for i in 0..opts.connections.max(1) {
        workers.spawn(worker(ctx.clone(), opts.connection_stagger * i as u32));
    }

    let mut meter = SpeedMeter::new(already);
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    let mut last_save = Instant::now();
    let mut last_error = None;
    loop {
        tokio::select! {
            joined = workers.join_next() => match joined {
                None => break,
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(e))) => last_error = Some(e),
                Some(Err(e)) => last_error = Some(Error::Other(format!("conexão abortou: {e}"))),
            },
            _ = ticker.tick() => {
                let downloaded = ctx.downloaded.load(Ordering::Relaxed);
                let _ = events.send(EngineEvent::Progress(Progress {
                    downloaded,
                    total: Some(total),
                    speed: meter.update(downloaded),
                    active_connections: ctx.active(),
                    segments: ctx.snapshot(),
                }));
                if last_save.elapsed() >= Duration::from_secs(1) {
                    last_save = Instant::now();
                    save(&ctx);
                }
            }
        }
    }

    save(&ctx);
    let complete = ctx.snapshot().iter().all(|s| s.remaining() == 0);
    let _ = events.send(EngineEvent::Progress(Progress {
        downloaded: ctx.downloaded.load(Ordering::Relaxed),
        total: Some(total),
        speed: 0.0,
        active_connections: 0,
        segments: ctx.snapshot(),
    }));

    if complete {
        let file = ctx.file.clone();
        drop(ctx);
        tokio::task::spawn_blocking(move || file.sync_all())
            .await
            .map_err(|e| Error::Other(e.to_string()))??;
        return Ok(Outcome::Completed(finalize(&part, final_path).await?));
    }
    if cancel.is_cancelled() {
        return Ok(Outcome::Paused(final_path));
    }
    Err(last_error.unwrap_or_else(|| Error::Other("download incompleto".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(segments: Vec<SegmentState>, owned: bool) -> Ctx {
        let dir = std::env::temp_dir().join(format!("ldm-claim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = File::create(dir.join("x")).unwrap();
        Ctx {
            client: Client::new(),
            spec: RequestSpec::new("http://localhost/"),
            url: Url::parse("http://localhost/").unwrap(),
            etag: None,
            file: Arc::new(file),
            segments: Mutex::new(segments.into_iter().map(|state| Segment { state, owned }).collect()),
            downloaded: AtomicU64::new(0),
            min_split: MIN_SPLIT_FLOOR,
            max_retries: 0,
            idle_timeout: Duration::from_secs(1),
            limiter: None,
            cancel: CancellationToken::new(),
        }
    }

    #[test]
    fn splits_largest_remaining_at_midpoint() {
        let mb = 1024 * 1024;
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 10 * mb - 1, done: 2 * mb }], true);
        let Claim::Segment(i) = ctx.claim() else { panic!("expected a split") };
        let segs = ctx.snapshot();
        assert_eq!(i, 1);
        // 8 MiB remaining from 2 MiB: the new segment starts at 6 MiB.
        assert_eq!(segs[0], SegmentState { start: 0, end: 6 * mb - 1, done: 2 * mb });
        assert_eq!(segs[1], SegmentState { start: 6 * mb, end: 10 * mb - 1, done: 0 });
    }

    #[test]
    fn does_not_split_small_segments() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: MIN_SPLIT_FLOOR, done: 0 }], true);
        assert!(matches!(ctx.claim(), Claim::Wait));
    }

    #[test]
    fn prefers_unowned_segments() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 99, done: 10 }], false);
        assert!(matches!(ctx.claim(), Claim::Segment(0)));
    }
}
