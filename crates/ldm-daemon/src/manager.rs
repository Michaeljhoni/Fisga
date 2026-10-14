//! Download list, queue and job lifecycle.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use ldm_core::{DownloadOptions, EngineEvent, Outcome, RequestSpec, Target};
use ldm_ipc::{AddRequest, DownloadInfo, Response, SourceKind, State};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{notify, ytdlp};

/// Downloads running at the same time; the rest wait in the queue.
const MAX_PARALLEL: usize = 3;
const DEFAULT_CONNECTIONS: usize = 8;

pub enum JobResult {
    Completed(PathBuf),
    Paused,
    Failed(String),
}

struct Job {
    info: DownloadInfo,
    request: AddRequest,
    cancel: Option<CancellationToken>,
}

struct Inner {
    next_id: u64,
    jobs: BTreeMap<u64, Job>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    next_id: u64,
    jobs: Vec<StoredJob>,
}

#[derive(Serialize, Deserialize)]
struct StoredJob {
    info: DownloadInfo,
    request: AddRequest,
}

pub struct Manager {
    inner: Mutex<Inner>,
    updates: broadcast::Sender<Response>,
    client: reqwest::Client,
    store: PathBuf,
    dirty: AtomicBool,
}

pub fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn looks_like_manifest(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    path.ends_with(".m3u8") || path.ends_with(".mpd")
}

fn display_name(request: &AddRequest) -> String {
    request
        .file_name
        .clone()
        .or_else(|| request.title.clone().filter(|t| !t.trim().is_empty()))
        .or_else(|| url::Url::parse(&request.url).ok().as_ref().and_then(ldm_core::filename::from_url))
        .unwrap_or_else(|| request.url.clone())
}

impl Manager {
    pub fn load(store: PathBuf) -> anyhow::Result<Arc<Self>> {
        let mut inner = Inner { next_id: 1, jobs: BTreeMap::new() };
        if let Ok(data) = std::fs::read(&store) {
            let stored: Stored = serde_json::from_slice(&data).context("downloads.json inválido")?;
            inner.next_id = stored.next_id;
            for StoredJob { mut info, request } in stored.jobs {
                if info.state == State::Running {
                    info.state = State::Paused;
                }
                info.speed = 0.0;
                inner.jobs.insert(info.id, Job { info, request, cancel: None });
            }
        }
        Ok(Arc::new(Self {
            inner: Mutex::new(inner),
            updates: broadcast::channel(256).0,
            client: ldm_core::build_client()?,
            store,
            dirty: AtomicBool::new(false),
        }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Response> {
        self.updates.subscribe()
    }

    pub fn list(&self) -> Vec<DownloadInfo> {
        self.inner.lock().unwrap().jobs.values().map(|j| j.info.clone()).collect()
    }

    pub fn add(self: &Arc<Self>, mut request: AddRequest) -> anyhow::Result<u64> {
        let parsed = url::Url::parse(&request.url).context("URL inválida")?;
        if !matches!(parsed.scheme(), "http" | "https" | "ftp") {
            bail!("esquema não suportado: {}", parsed.scheme());
        }
        if request.kind == SourceKind::Direct && looks_like_manifest(&request.url) {
            request.kind = SourceKind::Stream;
        }
        let id = {
            let mut inner = self.inner.lock().unwrap();
            let id = inner.next_id;
            inner.next_id += 1;
            let info = DownloadInfo {
                id,
                url: request.url.clone(),
                kind: request.kind,
                name: display_name(&request),
                path: None,
                state: if request.start { State::Queued } else { State::Paused },
                downloaded: 0,
                total: None,
                speed: 0.0,
                connections: 0,
                resumable: false,
                error: None,
                created_at: now(),
            };
            let _ = self.updates.send(Response::Update { item: info.clone() });
            inner.jobs.insert(id, Job { info, request, cancel: None });
            id
        };
        tracing::info!(id, "download adicionado");
        self.save();
        self.schedule();
        Ok(id)
    }

    pub fn pause(&self, id: u64) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let job = inner.jobs.get_mut(&id).context("download não encontrado")?;
        match job.info.state {
            // The job task notices the cancellation and reports Paused.
            State::Running => {
                if let Some(cancel) = &job.cancel {
                    cancel.cancel();
                }
            }
            State::Queued => {
                job.info.state = State::Paused;
                let _ = self.updates.send(Response::Update { item: job.info.clone() });
                self.dirty.store(true, Ordering::Relaxed);
            }
            _ => {}
        }
        Ok(())
    }

    pub fn resume(self: &Arc<Self>, id: u64) -> anyhow::Result<()> {
        {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.jobs.get_mut(&id).context("download não encontrado")?;
            if matches!(job.info.state, State::Paused | State::Failed) {
                job.info.state = State::Queued;
                job.info.error = None;
                let _ = self.updates.send(Response::Update { item: job.info.clone() });
            }
        }
        self.save();
        self.schedule();
        Ok(())
    }

    pub async fn remove(self: &Arc<Self>, id: u64, delete_file: bool) -> anyhow::Result<()> {
        let job = self.inner.lock().unwrap().jobs.remove(&id).context("download não encontrado")?;
        if let Some(cancel) = &job.cancel {
            cancel.cancel();
            // Give the engine a moment to flush before files are deleted.
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        if delete_file {
            if let Some(path) = &job.info.path {
                if job.info.state == State::Completed {
                    let _ = tokio::fs::remove_file(path).await;
                }
                let _ = tokio::fs::remove_file(ldm_core::control::part_path(path)).await;
                let _ = tokio::fs::remove_file(ldm_core::control::control_path(path)).await;
            }
        }
        let _ = self.updates.send(Response::Removed { id });
        self.save();
        self.schedule();
        Ok(())
    }

    /// Starts queued downloads while there is a free slot.
    pub fn schedule(self: &Arc<Self>) {
        let mut inner = self.inner.lock().unwrap();
        let mut running = inner.jobs.values().filter(|j| j.info.state == State::Running).count();
        let queued: Vec<u64> =
            inner.jobs.values().filter(|j| j.info.state == State::Queued).map(|j| j.info.id).collect();
        for id in queued {
            if running >= MAX_PARALLEL {
                break;
            }
            let job = inner.jobs.get_mut(&id).expect("id from map");
            let cancel = CancellationToken::new();
            job.cancel = Some(cancel.clone());
            job.info.state = State::Running;
            job.info.error = None;
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
            running += 1;
            tokio::spawn(self.clone().run_job(id, cancel));
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Applies a change to a download and broadcasts it.
    pub fn update(&self, id: u64, change: impl FnOnce(&mut DownloadInfo)) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(job) = inner.jobs.get_mut(&id) {
            change(&mut job.info);
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    async fn run_job(self: Arc<Self>, id: u64, cancel: CancellationToken) {
        let Some((request, path)) = ({
            let inner = self.inner.lock().unwrap();
            inner.jobs.get(&id).map(|j| (j.request.clone(), j.info.path.clone()))
        }) else {
            return;
        };
        let result = match request.kind {
            SourceKind::Direct => self.run_direct(id, &request, path, cancel.clone()).await,
            SourceKind::Stream | SourceKind::Page => ytdlp::run(&self, id, &request, cancel.clone()).await,
        };
        self.finish(id, result);
    }

    async fn run_direct(
        &self,
        id: u64,
        request: &AddRequest,
        path: Option<PathBuf>,
        cancel: CancellationToken,
    ) -> JobResult {
        let spec = RequestSpec { url: request.url.clone(), headers: request_headers(request) };
        let target = match path {
            Some(path) => Target::File(path),
            None => Target::Dir {
                dir: request.output_dir.clone().unwrap_or_else(default_download_dir),
                file_name: request.file_name.clone(),
            },
        };
        let opts = DownloadOptions {
            connections: request.connections.unwrap_or(DEFAULT_CONNECTIONS).clamp(1, 32),
            ..Default::default()
        };
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let download = ldm_core::download(&self.client, &spec, target, &opts, events_tx, cancel);
        tokio::pin!(download);
        loop {
            tokio::select! {
                Some(event) = events.recv() => self.apply_event(id, event),
                outcome = &mut download => {
                    while let Ok(event) = events.try_recv() {
                        self.apply_event(id, event);
                    }
                    return match outcome {
                        Ok(Outcome::Completed(path)) => JobResult::Completed(path),
                        Ok(Outcome::Paused(_)) | Err(ldm_core::Error::Cancelled) => JobResult::Paused,
                        Err(e) => JobResult::Failed(e.to_string()),
                    };
                }
            }
        }
    }

    fn apply_event(&self, id: u64, event: EngineEvent) {
        match event {
            EngineEvent::Started { path, total, resumable } => self.update(id, |info| {
                if let Some(name) = path.file_name() {
                    info.name = name.to_string_lossy().into_owned();
                }
                info.path = Some(path);
                info.total = total;
                info.resumable = resumable;
            }),
            EngineEvent::Progress(p) => self.update(id, |info| {
                info.downloaded = p.downloaded;
                info.total = p.total.or(info.total);
                info.speed = p.speed;
                info.connections = p.active_connections;
            }),
        }
    }

    fn finish(self: &Arc<Self>, id: u64, result: JobResult) {
        let mut message = None;
        self.update(id, |info| {
            info.speed = 0.0;
            info.connections = 0;
            match &result {
                JobResult::Completed(path) => {
                    info.state = State::Completed;
                    if let Some(name) = path.file_name() {
                        info.name = name.to_string_lossy().into_owned();
                    }
                    info.path = Some(path.clone());
                    if let Some(total) = info.total {
                        info.downloaded = total;
                    }
                    message = Some(("Download concluído".to_string(), info.name.clone()));
                }
                JobResult::Paused => info.state = State::Paused,
                JobResult::Failed(error) => {
                    info.state = State::Failed;
                    info.error = Some(error.clone());
                    message = Some(("Falha no download".to_string(), format!("{}\n{}", info.name, error)));
                }
            }
        });
        if let Some(job) = self.inner.lock().unwrap().jobs.get_mut(&id) {
            job.cancel = None;
        }
        if let Some((summary, body)) = message {
            notify::send(&summary, &body);
        }
        self.save();
        self.schedule();
    }

    pub fn save(&self) {
        let stored = {
            let inner = self.inner.lock().unwrap();
            Stored {
                next_id: inner.next_id,
                jobs: inner
                    .jobs
                    .values()
                    .map(|j| StoredJob {
                        info: j.info.clone(),
                        // Browser cookies stay in memory only; they are not written to disk.
                        request: AddRequest { cookies: Vec::new(), ..j.request.clone() },
                    })
                    .collect(),
            }
        };
        self.dirty.store(false, Ordering::Relaxed);
        if let Err(e) = write_store(&self.store, &stored) {
            tracing::warn!(error = %e, "não foi possível salvar a lista de downloads");
        }
    }

    pub async fn autosave(self: Arc<Self>, shutdown: CancellationToken) {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = ticker.tick() => if self.dirty.load(Ordering::Relaxed) { self.save() },
            }
        }
    }

    /// Pauses every running download and waits briefly for them to stop.
    pub async fn shutdown(&self) {
        let tokens: Vec<CancellationToken> =
            self.inner.lock().unwrap().jobs.values().filter_map(|j| j.cancel.clone()).collect();
        for token in &tokens {
            token.cancel();
        }
        for _ in 0..30 {
            if self.inner.lock().unwrap().jobs.values().all(|j| j.cancel.is_none()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.save();
    }
}

fn write_store(path: &Path, stored: &Stored) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(stored)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Referer, User-Agent, Cookie and extra headers captured by the browser.
pub fn request_headers(request: &AddRequest) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    if let Some(user_agent) = &request.user_agent {
        headers.push(("User-Agent".to_string(), user_agent.clone()));
    }
    if let Some(referrer) = &request.referrer {
        headers.push(("Referer".to_string(), referrer.clone()));
    }
    if !request.cookies.is_empty() {
        let cookie = request.cookies.iter().map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; ");
        headers.push(("Cookie".to_string(), cookie));
    }
    headers.extend(request.headers.iter().cloned());
    headers
}
