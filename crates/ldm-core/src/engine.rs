//! Segmented download with dynamic splitting and resume.
//!
//! The file is preallocated as `name.ldm-part`. Each connection owns one segment
//! and writes at its offset. When a connection has nothing left, it takes an
//! unowned segment or splits the largest one still in progress at the midpoint of
//! what remains. The connection that owned the split segment notices its new,
//! smaller `end` on its next chunk and stops there. A connection that stalls
//! (no data for `STALL`) hands the rest of its segment to an idle one, and a
//! server that answers 503/429 gets fewer connections.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, CONTENT_RANGE, IF_RANGE, RANGE, RETRY_AFTER};
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
/// Splits never create segments smaller than this. The split point is always
/// past what the owner still holds in its buffer (`Segment::buffered`).
const MIN_SPLIT_FLOOR: u64 = 256 * 1024;
/// A connection that received nothing for this long is stalled: an idle
/// connection takes over the rest of its segment (IDM: about 2 s).
const STALL: Duration = Duration::from_secs(3);
/// How often a connection waiting for the server looks around: it writes what
/// it holds (so its segment can be taken over exactly) and stops if it was.
const TICK: Duration = Duration::from_secs(1);
/// The longest wait a server's `Retry-After` gets.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(120);
/// The data is forced to disk, then the control file saved, this often.
const SAVE_EVERY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Maximum simultaneous connections for one file.
    pub connections: usize,
    /// Smallest segment produced by a split.
    pub min_split_size: u64,
    /// Failures in a row, without any progress between them, before a
    /// connection gives up.
    pub max_retries: u32,
    /// Abort a connection that receives nothing for this long (ten times as
    /// long without Range support, when giving up means starting over).
    pub idle_timeout: Duration,
    /// Delay between opening each additional connection.
    pub connection_stagger: Duration,
    /// Speed limits, all respected: the global one (every download) and this download's own.
    pub limiters: Vec<Arc<crate::RateLimiter>>,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            connections: 8,
            min_split_size: MIN_SPLIT_FLOOR,
            max_retries: 15,
            idle_timeout: Duration::from_secs(30),
            connection_stagger: Duration::from_millis(150),
            limiters: Vec::new(),
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
    // The data comes from the redirect's target: credentials only if it is the same host.
    let followed = spec.for_final_url(&probe.final_url);
    let spec = &followed;

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
            // Giving up here means starting over: wait much longer.
            next = tokio::time::timeout(opts.idle_timeout.saturating_mul(10), stream.next()) => next,
        };
        match next {
            Err(_) => return Err(Error::Other("tempo esgotado esperando dados do servidor".into())),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(e.into()),
            Ok(Some(Ok(chunk))) => {
                file.write_all(&chunk).await?;
                downloaded += chunk.len() as u64;
                for limiter in &opts.limiters {
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
    /// Received by the owner but not written yet; it follows `state.pos()`.
    buffered: u64,
    /// When the owner last received data (or took the segment).
    last_data: Instant,
    /// Taken over before a byte of it was written: the old owner drops it.
    /// Left out of the control file.
    revoked: bool,
    /// Its owner waits for the speed limit: slow on purpose, not stalled.
    held: bool,
    /// The server started sending it. Before that a slow server is waited
    /// for (up to the idle timeout), not taken over: a takeover would only
    /// wait for the same slow answer again.
    streaming: bool,
}

impl Segment {
    fn new(state: SegmentState, owned: bool) -> Self {
        Self { state, owned, buffered: 0, last_data: Instant::now(), revoked: false, held: false, streaming: false }
    }

    /// Where a split may start: past what the owner holds in memory.
    fn cursor(&self) -> u64 {
        self.state.pos() + self.buffered
    }

    /// What no one has received yet.
    fn unclaimed(&self) -> u64 {
        (self.state.end + 1).saturating_sub(self.cursor())
    }
}

enum Claim {
    Segment(usize),
    /// Everything is owned and too small to split; check again later in case a
    /// connection fails, stalls or releases its segment.
    Wait,
    Done,
}

/// How a connection ended without an error.
enum Exit {
    Finished,
    /// The server asked for fewer connections and this one left.
    Left,
}

struct Ctx {
    client: Client,
    spec: RequestSpec,
    url: Url,
    etag: Option<String>,
    file: Arc<File>,
    segments: Mutex<Vec<Segment>>,
    downloaded: AtomicU64,
    /// Connections still working; a busy server (503/429) brings it down.
    workers: AtomicUsize,
    /// Connections that left because the server was busy.
    left: AtomicUsize,
    min_split: u64,
    max_retries: u32,
    idle_timeout: Duration,
    limiters: Vec<Arc<crate::RateLimiter>>,
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
            segs[i].last_data = Instant::now();
            return Claim::Segment(i);
        }
        let largest = segs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.owned)
            .max_by_key(|(_, s)| s.unclaimed())
            .map(|(i, s)| (i, s.cursor(), s.unclaimed()));
        if let Some((i, cursor, unclaimed)) = largest {
            if unclaimed >= 2 * self.min_split {
                let mid = cursor + unclaimed / 2;
                let end = segs[i].state.end;
                segs[i].state.end = mid - 1;
                segs.push(Segment::new(SegmentState { start: mid, end, done: 0 }, true));
                return Claim::Segment(segs.len() - 1);
            }
        }
        // Too small to split: the rest of a stalled connection's segment. Its
        // owner has written all it received (see `TICK`) and stops when it notices.
        let stalled = segs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.owned && s.streaming && !s.held && s.buffered == 0 && s.state.remaining() > 0)
            .filter(|(_, s)| s.last_data.elapsed() >= STALL)
            .max_by_key(|(_, s)| s.state.remaining())
            .map(|(i, _)| i);
        if let Some(i) = stalled {
            let state = segs[i].state;
            if state.done == 0 {
                segs[i].revoked = true;
                segs[i].state.done = state.len();
            } else {
                segs[i].state.end = state.pos() - 1;
            }
            tracing::info!(segment = i, remaining = state.remaining(), "conexão parada; outra assume o resto do trecho");
            segs.push(Segment::new(SegmentState { start: state.pos(), end: state.end, done: 0 }, true));
            return Claim::Segment(segs.len() - 1);
        }
        Claim::Wait
    }

    fn release(&self, index: usize) {
        let mut segs = self.segments.lock().unwrap();
        segs[index].owned = false;
        segs[index].buffered = 0;
        segs[index].held = false;
        segs[index].streaming = false;
    }

    /// The server's answer for segment `index` arrived; its data follows.
    fn streaming(&self, index: usize) {
        let mut segs = self.segments.lock().unwrap();
        segs[index].streaming = true;
        segs[index].last_data = Instant::now();
    }

    /// The owner starts (or ends) waiting for the speed limit.
    fn hold(&self, index: usize, held: bool) {
        let mut segs = self.segments.lock().unwrap();
        segs[index].held = held;
        segs[index].last_data = Instant::now();
    }

    /// One connection fewer, unless it is the last one.
    #[allow(deprecated)] // try_update needs a newer Rust than our MSRV (1.80)
    fn leave(&self) -> bool {
        let left =
            self.workers.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| if n > 1 { Some(n - 1) } else { None }).is_ok();
        if left {
            self.left.fetch_add(1, Ordering::SeqCst);
        }
        left
    }

    /// One connection that left (`leave`) comes back, if any did.
    #[allow(deprecated)] // try_update needs a newer Rust than our MSRV (1.80)
    fn rejoin(&self) -> bool {
        let back = self.left.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
        if back {
            self.workers.fetch_add(1, Ordering::SeqCst);
        }
        back
    }

    fn bounds(&self, index: usize) -> (u64, u64) {
        let segs = self.segments.lock().unwrap();
        (segs[index].state.pos(), segs[index].state.end)
    }

    /// Nothing from `offset` on is this segment's any more (split or taken over).
    fn gone(&self, index: usize, offset: u64) -> bool {
        let segs = self.segments.lock().unwrap();
        segs[index].revoked || offset > segs[index].state.end
    }

    /// A chunk of `len` bytes arrived and the owner's data reaches `cursor`:
    /// how much of it is still the segment's, and whether that reaches its end.
    fn accept(&self, index: usize, cursor: u64, len: usize) -> (usize, bool) {
        let mut segs = self.segments.lock().unwrap();
        let seg = &mut segs[index];
        seg.last_data = Instant::now();
        if seg.revoked || cursor > seg.state.end {
            return (0, true);
        }
        let take = len.min((seg.state.end + 1 - cursor) as usize);
        seg.buffered += take as u64;
        (take, cursor + take as u64 > seg.state.end)
    }

    /// The segments to save and show; the revoked ones are covered by others.
    fn snapshot(&self) -> Vec<SegmentState> {
        self.segments.lock().unwrap().iter().filter(|s| !s.revoked).map(|s| s.state).collect()
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
            let seg = &mut segs[index];
            seg.state.done = (seg.state.done + len).min(seg.state.len());
            seg.buffered = seg.buffered.saturating_sub(len);
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
        if from > end || self.gone(index, from) {
            return Ok(());
        }
        let mut request = self
            .spec
            .apply(self.client.get(self.url.clone()))
            .header(RANGE, format!("bytes={from}-{end}"));
        if let Some(etag) = &self.etag {
            request = request.header(IF_RANGE, etag.as_str());
        }
        // A server that never answers: another connection may take the segment meanwhile.
        let asked = Instant::now();
        let send = request.send();
        tokio::pin!(send);
        let resp = loop {
            tokio::select! {
                _ = self.cancel.cancelled() => return Err(Error::Cancelled),
                r = &mut send => break r?,
                _ = tokio::time::sleep(TICK) => {
                    if self.gone(index, from) {
                        return Ok(());
                    }
                    if asked.elapsed() >= self.idle_timeout {
                        return Err(Error::Other("tempo esgotado esperando a resposta do servidor".into()));
                    }
                }
            }
        };
        match resp.status() {
            StatusCode::PARTIAL_CONTENT => {}
            StatusCode::OK => return Err(Error::RangeIgnored),
            status @ (StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE) => {
                return Err(Error::Throttled { status, retry_after: retry_after(resp.headers()) });
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
        self.streaming(index);

        let mut stream = resp.bytes_stream();
        let mut buf = Vec::with_capacity(FLUSH_SIZE);
        let mut offset = from;
        let mut last_data = Instant::now();
        loop {
            let next = tokio::select! {
                _ = self.cancel.cancelled() => {
                    self.flush(index, &mut buf, &mut offset).await?;
                    return Err(Error::Cancelled);
                }
                next = tokio::time::timeout(TICK, stream.next()) => next,
            };
            let chunk = match next {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(e))) => {
                    self.flush(index, &mut buf, &mut offset).await?;
                    return Err(e.into());
                }
                Ok(None) => {
                    self.flush(index, &mut buf, &mut offset).await?;
                    return if self.gone(index, offset) {
                        Ok(())
                    } else {
                        Err(Error::Other("conexão encerrada antes do fim do segmento".into()))
                    };
                }
                Err(_) => {
                    // Quiet for a moment: write what is held, so that a stalled
                    // segment can be taken over from exactly where it stopped.
                    self.flush(index, &mut buf, &mut offset).await?;
                    if self.gone(index, offset) {
                        return Ok(());
                    }
                    if last_data.elapsed() >= self.idle_timeout {
                        return Err(Error::Other("tempo esgotado esperando dados".into()));
                    }
                    continue;
                }
            };
            last_data = Instant::now();

            let limited = self.limiters.iter().any(|limiter| limiter.rate() > 0);
            if limited {
                self.hold(index, true);
            }
            for limiter in &self.limiters {
                limiter.consume(chunk.len()).await;
            }
            if limited {
                self.hold(index, false);
            }

            // Another connection may have taken the tail of this segment.
            let (take, reached_end) = self.accept(index, offset + buf.len() as u64, chunk.len());
            buf.extend_from_slice(&chunk[..take]);
            if buf.len() >= FLUSH_SIZE || reached_end {
                self.flush(index, &mut buf, &mut offset).await?;
            }
            if reached_end {
                return Ok(());
            }
        }
    }
}

/// `Retry-After` in seconds (the HTTP-date form is rare on file servers).
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers.get(RETRY_AFTER)?.to_str().ok()?.trim().parse().ok().map(Duration::from_secs)
}

/// The wait before the `failures`-th retry in a row: 2 s more each time, up to 30 s.
fn backoff(failures: u32) -> Duration {
    Duration::from_secs((2 * u64::from(failures)).min(30))
}

async fn sleep_or_cancel(cancel: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

async fn worker(ctx: Arc<Ctx>, delay: Duration) -> Result<()> {
    let result = work(&ctx, delay).await;
    if !matches!(result, Ok(Exit::Left)) {
        ctx.workers.fetch_sub(1, Ordering::SeqCst);
    }
    result.map(|_| ())
}

async fn work(ctx: &Ctx, delay: Duration) -> Result<Exit> {
    if !delay.is_zero() && !sleep_or_cancel(&ctx.cancel, delay).await {
        return Ok(Exit::Finished);
    }
    loop {
        if ctx.cancel.is_cancelled() {
            return Ok(Exit::Finished);
        }
        let index = match ctx.claim() {
            Claim::Done => return Ok(Exit::Finished),
            Claim::Wait => {
                if !sleep_or_cancel(&ctx.cancel, Duration::from_millis(500)).await {
                    return Ok(Exit::Finished);
                }
                continue;
            }
            Claim::Segment(index) => index,
        };

        let mut failures = 0u32;
        loop {
            let before = ctx.bounds(index).0;
            let e = match ctx.fetch_segment(index).await {
                Ok(()) => break,
                Err(_) if ctx.cancel.is_cancelled() => {
                    ctx.release(index);
                    return Ok(Exit::Finished);
                }
                Err(e) => e,
            };
            // It got somewhere: whatever went wrong is not lasting, count anew.
            if ctx.bounds(index).0 > before {
                failures = 0;
            }
            failures += 1;
            tracing::warn!(segment = index, attempt = failures, error = %e, "segmento falhou");
            if e.is_permanent() || failures > ctx.max_retries {
                ctx.release(index);
                return Err(e);
            }
            let wait = match &e {
                Error::Throttled { retry_after, .. } => {
                    // The server wants fewer connections: this one leaves, unless it is the last.
                    if ctx.leave() {
                        tracing::info!(segment = index, "servidor ocupado; uma conexão a menos");
                        ctx.release(index);
                        return Ok(Exit::Left);
                    }
                    retry_after.unwrap_or_else(|| backoff(failures)).min(MAX_RETRY_AFTER)
                }
                _ => backoff(failures),
            };
            if !sleep_or_cancel(&ctx.cancel, wait).await {
                ctx.release(index);
                return Ok(Exit::Finished);
            }
        }
        ctx.release(index);
    }
}

/// Forces the written data to disk, then saves the control file that says it
/// is there. After a power cut the control file never claims bytes the disk
/// lost, which would leave a silent hole of zeros in the finished file.
async fn persist(ctx: &Ctx, path: &Path, mut control: ControlFile) {
    // Before the sync: only bytes already written count.
    control.segments = ctx.snapshot();
    let file = ctx.file.clone();
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        file.sync_data()?;
        control::save(&path, &control)
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "não foi possível salvar o arquivo de controle"),
        Err(e) => tracing::warn!(error = %e, "não foi possível salvar o arquivo de controle"),
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

    let connections = opts.connections.max(1);
    let ctx = Arc::new(Ctx {
        client: client.clone(),
        spec: spec.clone(),
        url: probe.final_url.clone(),
        etag: probe.etag.clone(),
        file: Arc::new(file),
        segments: Mutex::new(states.into_iter().map(|state| Segment::new(state, false)).collect()),
        downloaded: AtomicU64::new(already),
        workers: AtomicUsize::new(connections),
        left: AtomicUsize::new(0),
        min_split: opts.min_split_size.max(MIN_SPLIT_FLOOR),
        max_retries: opts.max_retries,
        idle_timeout: opts.idle_timeout,
        limiters: opts.limiters.clone(),
        cancel: cancel.clone(),
    });

    let control = ControlFile {
        version: control::VERSION,
        url: spec.url.clone(),
        total,
        etag: probe.etag.clone(),
        segments: Vec::new(),
    };
    persist(&ctx, &control_file, control.clone()).await;

    let mut workers = JoinSet::new();
    for i in 0..connections {
        workers.spawn(worker(ctx.clone(), opts.connection_stagger * i as u32));
    }

    let mut meter = SpeedMeter::new(already);
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    let mut last_save = Instant::now();
    let mut last_error = None;
    loop {
        tokio::select! {
            joined = workers.join_next() => match joined {
                None => {
                    // The others failed while a busy server's connection was
                    // leaving: one comes back, rather than giving up with work left.
                    let unfinished = !cancel.is_cancelled() && ctx.snapshot().iter().any(|s| s.remaining() > 0);
                    let fatal = last_error.as_ref().is_some_and(Error::is_permanent);
                    if unfinished && !fatal && ctx.rejoin() {
                        workers.spawn(worker(ctx.clone(), Duration::ZERO));
                        continue;
                    }
                    break;
                }
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
                if last_save.elapsed() >= SAVE_EVERY {
                    last_save = Instant::now();
                    persist(&ctx, &control_file, control.clone()).await;
                }
            }
        }
    }

    persist(&ctx, &control_file, control).await;
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

    const MB: u64 = 1024 * 1024;

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
            segments: Mutex::new(
                segments.into_iter().map(|state| Segment { streaming: owned, ..Segment::new(state, owned) }).collect(),
            ),
            downloaded: AtomicU64::new(0),
            workers: AtomicUsize::new(2),
            left: AtomicUsize::new(0),
            min_split: MIN_SPLIT_FLOOR,
            max_retries: 0,
            idle_timeout: Duration::from_secs(1),
            limiters: Vec::new(),
            cancel: CancellationToken::new(),
        }
    }

    /// Makes segment `index` look silent since `ago`.
    fn silent(ctx: &Ctx, index: usize, ago: Duration) {
        ctx.segments.lock().unwrap()[index].last_data = Instant::now() - ago;
    }

    #[test]
    fn splits_largest_remaining_at_midpoint() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 10 * MB - 1, done: 2 * MB }], true);
        let Claim::Segment(i) = ctx.claim() else { panic!("expected a split") };
        let segs = ctx.snapshot();
        assert_eq!(i, 1);
        // 8 MiB remaining from 2 MiB: the new segment starts at 6 MiB.
        assert_eq!(segs[0], SegmentState { start: 0, end: 6 * MB - 1, done: 2 * MB });
        assert_eq!(segs[1], SegmentState { start: 6 * MB, end: 10 * MB - 1, done: 0 });
    }

    #[test]
    fn splits_past_the_owners_buffer() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 10 * MB - 1, done: 2 * MB }], true);
        ctx.segments.lock().unwrap()[0].buffered = MB;
        assert!(matches!(ctx.claim(), Claim::Segment(1)));
        // From 3 MiB (2 written + 1 in memory), 7 MiB left: split at 6.5 MiB.
        assert_eq!(ctx.snapshot()[1].start, 6 * MB + MB / 2);
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

    #[test]
    fn takes_over_a_stalled_connection() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 300 * 1024, done: 1000 }], true);
        // Silent, but not long enough, or still holding data: nothing to do.
        silent(&ctx, 0, STALL / 2);
        assert!(matches!(ctx.claim(), Claim::Wait));
        silent(&ctx, 0, STALL * 2);
        ctx.segments.lock().unwrap()[0].buffered = 10;
        assert!(matches!(ctx.claim(), Claim::Wait));
        ctx.segments.lock().unwrap()[0].buffered = 0;

        assert!(matches!(ctx.claim(), Claim::Segment(1)));
        let segs = ctx.snapshot();
        assert_eq!(segs[0], SegmentState { start: 0, end: 999, done: 1000 });
        assert_eq!(segs[1], SegmentState { start: 1000, end: 300 * 1024, done: 0 });
        assert!(ctx.gone(0, 1000), "the old owner stops");
        assert!(!ctx.gone(1, 1000));
    }

    #[test]
    fn a_slow_server_is_waited_for_not_taken_over() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 99, done: 0 }], true);
        ctx.segments.lock().unwrap()[0].streaming = false;
        silent(&ctx, 0, STALL * 2);
        assert!(matches!(ctx.claim(), Claim::Wait), "no answer yet: not stalled");
        ctx.streaming(0);
        silent(&ctx, 0, STALL * 2);
        assert!(matches!(ctx.claim(), Claim::Segment(1)));
    }

    #[test]
    fn a_connection_that_left_comes_back_once() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 99, done: 0 }], true);
        assert!(!ctx.rejoin(), "nobody left");
        assert!(ctx.leave());
        assert!(ctx.rejoin());
        assert!(!ctx.rejoin());
        assert_eq!(ctx.workers.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_connection_held_by_the_speed_limit_is_not_stalled() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 99, done: 10 }], true);
        ctx.hold(0, true);
        silent(&ctx, 0, STALL * 2);
        assert!(matches!(ctx.claim(), Claim::Wait));
        ctx.hold(0, false);
        silent(&ctx, 0, STALL * 2);
        assert!(matches!(ctx.claim(), Claim::Segment(1)));
    }

    #[test]
    fn a_stalled_connection_without_data_gives_up_its_whole_segment() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 99, done: 0 }], true);
        silent(&ctx, 0, STALL * 2);
        assert!(matches!(ctx.claim(), Claim::Segment(1)));
        // The old one is left out of the control file; the new one has it all.
        assert_eq!(ctx.snapshot(), vec![SegmentState { start: 0, end: 99, done: 0 }]);
        assert!(ctx.gone(0, 0));
        assert_eq!(ctx.accept(0, 0, 50), (0, true), "late data of the old owner is dropped");
        assert_eq!(ctx.accept(1, 0, 50), (50, false));
        assert_eq!(ctx.accept(1, 50, 80), (50, true));
    }

    #[test]
    fn only_the_last_connection_stays() {
        let ctx = ctx_with(vec![SegmentState { start: 0, end: 99, done: 0 }], true);
        assert!(ctx.leave());
        assert!(!ctx.leave(), "the last one keeps trying");
        assert_eq!(ctx.workers.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retry_waits() {
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(5), Duration::from_secs(10));
        assert_eq!(backoff(40), Duration::from_secs(30));
        let mut headers = HeaderMap::new();
        assert_eq!(retry_after(&headers), None);
        headers.insert(RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(7)));
        headers.insert(RETRY_AFTER, "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap());
        assert_eq!(retry_after(&headers), None);
    }
}
