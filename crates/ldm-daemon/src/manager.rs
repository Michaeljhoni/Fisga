//! Download list, queue and job lifecycle.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use ldm_core::{DownloadOptions, EngineEvent, Outcome, RequestSpec, Target};
use ldm_ipc::{AddRequest, Category, DownloadInfo, Response, Settings, SourceKind, State};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{notify, ytdlp};

pub enum JobResult {
    Completed(PathBuf),
    Paused,
    Failed(String),
}

struct Job {
    info: DownloadInfo,
    request: AddRequest,
    cancel: Option<CancellationToken>,
    /// "Baixar agora": start even when `max_parallel` downloads are running.
    forced: bool,
    /// The folder follows the category (no folder was chosen by the user), so it
    /// changes while the name of a pending download is still being found out.
    auto_dir: bool,
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
    /// Never locked while `inner` is held: take a copy first.
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    /// Speed limit shared by every direct download.
    limiter: Arc<ldm_core::RateLimiter>,
}

/// Where a download of `category` goes when the user did not pick a folder.
fn auto_dir(settings: &Settings, category: Category) -> PathBuf {
    let base = settings.download_dir.clone().unwrap_or_else(default_download_dir);
    match category.folder() {
        Some(folder) if settings.category_folders => base.join(folder),
        _ => base,
    }
}

fn load_settings(path: &Path) -> Settings {
    match std::fs::read(path) {
        Ok(data) => serde_json::from_slice::<Settings>(&data)
            .map(Settings::sanitized)
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "settings.json inválido; usando o padrão");
                Settings::default()
            }),
        Err(_) => Settings::default(),
    }
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
    pub fn load(store: PathBuf, settings_path: PathBuf) -> anyhow::Result<Arc<Self>> {
        let mut inner = Inner { next_id: 1, jobs: BTreeMap::new() };
        if let Ok(data) = std::fs::read(&store) {
            let stored: Stored = serde_json::from_slice(&data).context("downloads.json inválido")?;
            inner.next_id = stored.next_id;
            for StoredJob { mut info, request } in stored.jobs {
                if info.state == State::Running {
                    info.state = State::Paused;
                }
                info.speed = 0.0;
                info.probing = false;
                if info.category == Category::Other {
                    info.category = Category::detect(&info.name, info.kind);
                }
                inner.jobs.insert(info.id, Job { info, request, cancel: None, forced: false, auto_dir: false });
            }
        }
        Ok(Arc::new(Self {
            inner: Mutex::new(inner),
            updates: broadcast::channel(256).0,
            client: ldm_core::build_client()?,
            store,
            dirty: AtomicBool::new(false),
            limiter: Arc::new(ldm_core::RateLimiter::new(0)),
            settings: Mutex::new(load_settings(&settings_path)),
            settings_path,
        }))
        .map(|manager: Arc<Self>| {
            manager.limiter.set_rate(manager.settings().speed_limit);
            manager
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn set_settings(self: &Arc<Self>, settings: Settings) -> anyhow::Result<()> {
        let settings = settings.sanitized();
        if let Some(dir) = &settings.download_dir {
            if !dir.is_absolute() {
                bail!("a pasta de downloads precisa ser um caminho absoluto");
            }
        }
        write_json(&self.settings_path, &settings, 0o644)?;
        *self.settings.lock().unwrap() = settings.clone();
        self.limiter.set_rate(settings.speed_limit);
        tracing::info!(?settings, "configurações alteradas");
        let _ = self.updates.send(Response::Settings { settings });
        self.refresh_pending_dirs();
        self.schedule();
        Ok(())
    }

    /// Pending downloads without a chosen folder follow the current settings.
    fn refresh_pending_dirs(&self) {
        let settings = self.settings();
        let mut inner = self.inner.lock().unwrap();
        for job in inner.jobs.values_mut().filter(|j| j.auto_dir && j.info.state == State::Pending) {
            job.info.category = Category::detect(&job.info.name, job.info.kind);
            job.info.dir = Some(auto_dir(&settings, job.info.category));
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
        }
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
        let probe = request.confirm;
        let name = display_name(&request);
        let category = Category::detect(request.file_name.as_deref().unwrap_or(&name), request.kind);
        let auto = request.output_dir.is_none();
        let dir = request.output_dir.clone().unwrap_or_else(|| auto_dir(&self.settings(), category));
        if !request.confirm {
            request.output_dir = Some(dir.clone());
        }
        let id = {
            let mut inner = self.inner.lock().unwrap();
            let id = inner.next_id;
            inner.next_id += 1;
            let state = match (request.confirm, request.start) {
                (true, _) => State::Pending,
                (false, true) => State::Queued,
                (false, false) => State::Paused,
            };
            let info = DownloadInfo {
                id,
                url: request.url.clone(),
                kind: request.kind,
                name,
                path: None,
                state,
                downloaded: 0,
                total: None,
                speed: 0.0,
                connections: 0,
                resumable: false,
                error: None,
                created_at: now(),
                dir: Some(dir),
                probing: probe,
                category,
            };
            let _ = self.updates.send(Response::Update { item: info.clone() });
            inner.jobs.insert(id, Job { info, request, cancel: None, forced: false, auto_dir: auto });
            id
        };
        tracing::info!(id, "download adicionado");
        self.save();
        self.schedule();
        if probe {
            tokio::spawn(self.clone().probe_pending(id));
        }
        Ok(id)
    }

    pub fn state(&self, id: u64) -> Option<State> {
        self.inner.lock().unwrap().jobs.get(&id).map(|j| j.info.state)
    }

    /// Fills in the size and file name of a pending download, so the "Novo
    /// download" window can show them. Pages that hold many videos (feeds,
    /// channels, searches, playlists) are flagged with an error, which blocks
    /// confirming them.
    async fn probe_pending(self: Arc<Self>, id: u64) {
        let Some(request) = self.inner.lock().unwrap().jobs.get(&id).map(|j| j.request.clone()) else {
            return;
        };
        let keep_name = request.file_name.is_some();
        if request.kind == SourceKind::Direct {
            let spec = RequestSpec { url: request.url.clone(), headers: request_headers(&request) };
            let result = ldm_core::probe(&self.client, &spec).await;
            self.update(id, |info| {
                info.probing = false;
                match result {
                    Ok(probe) => {
                        info.total = probe.total;
                        info.resumable = probe.ranges;
                        if !keep_name && info.state == State::Pending {
                            info.name = probe.file_name;
                        }
                    }
                    Err(e) => tracing::info!(id, error = %e, "não foi possível sondar o download pendente"),
                }
            });
            self.refresh_pending_dirs();
            return;
        }
        let result = ytdlp::probe(id, &request).await;
        self.update(id, |info| {
            info.probing = false;
            match result {
                Ok(ytdlp::PageProbe::Video { name, size }) => {
                    info.total = size;
                    info.resumable = true;
                    if let Some(name) = name.filter(|_| !keep_name && info.state == State::Pending) {
                        info.name = name;
                    }
                }
                Ok(ytdlp::PageProbe::List { title }) => {
                    let what = title.map(|t| format!(" (\u{201c}{t}\u{201d})")).unwrap_or_default();
                    info.error = Some(format!(
                        "Este endereço é uma lista de vídeos{what}, não um vídeo: baixá-lo traria todos eles. \
                         Abra o vídeo desejado e clique em Baixar de novo."
                    ));
                }
                Err(e) => tracing::info!(id, error = %e, "o yt-dlp não conseguiu analisar o endereço"),
            }
        });
        self.refresh_pending_dirs();
    }

    /// Answers a pending download: queue it, or start it right away (`now`).
    pub fn confirm(
        self: &Arc<Self>,
        id: u64,
        now: bool,
        file_name: Option<String>,
        output_dir: Option<PathBuf>,
    ) -> anyhow::Result<()> {
        let settings = self.settings();
        {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.jobs.get_mut(&id).context("download não encontrado")?;
            if job.info.state != State::Pending {
                bail!("o download #{id} não está aguardando confirmação");
            }
            if let Some(error) = &job.info.error {
                bail!("{error}");
            }
            if let Some(name) = file_name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
                job.info.name = name.clone();
                job.request.file_name = Some(name);
            }
            job.info.category = Category::detect(&job.info.name, job.info.kind);
            match output_dir {
                Some(dir) => {
                    job.auto_dir = false;
                    job.info.dir = Some(dir);
                }
                None if job.auto_dir => job.info.dir = Some(auto_dir(&settings, job.info.category)),
                None => {}
            }
            // The folder is fixed from now on.
            job.request.output_dir = job.info.dir.clone();
            job.auto_dir = false;
            job.info.state = State::Queued;
            job.forced = now;
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
        }
        tracing::info!(id, now, "download confirmado");
        self.save();
        self.schedule();
        Ok(())
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

    /// Starts queued downloads while there is a free slot ("Baixar agora" ones
    /// start regardless).
    pub fn schedule(self: &Arc<Self>) {
        let max_parallel = self.settings().max_parallel;
        let mut inner = self.inner.lock().unwrap();
        let running = inner.jobs.values().filter(|j| j.info.state == State::Running).count();
        // Forced first, then by id.
        let mut queued: Vec<(bool, u64)> = inner
            .jobs
            .values()
            .filter(|j| j.info.state == State::Queued)
            .map(|j| (!j.forced, j.info.id))
            .collect();
        queued.sort();
        let forced = queued.iter().filter(|(not_forced, _)| !not_forced).count();
        let to_start = forced + max_parallel.saturating_sub(running + forced);
        for (_, id) in queued.into_iter().take(to_start) {
            let job = inner.jobs.get_mut(&id).expect("id from map");
            let cancel = CancellationToken::new();
            job.cancel = Some(cancel.clone());
            job.forced = false;
            job.info.state = State::Running;
            job.info.error = None;
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
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
        let settings = self.settings();
        let target = match path {
            Some(path) => Target::File(path),
            None => Target::Dir {
                dir: request.output_dir.clone().unwrap_or_else(|| auto_dir(&settings, Category::Other)),
                file_name: request.file_name.clone(),
            },
        };
        let opts = DownloadOptions {
            connections: request.connections.unwrap_or(settings.connections).clamp(1, 32),
            limiter: Some(self.limiter.clone()),
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
                // The folder is already chosen; the type may only now be known
                // (a URL without extension, a name from Content-Disposition).
                if info.category == Category::Other {
                    info.category = Category::detect(&info.name, info.kind);
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
        if let Err(e) = write_json(&self.store, &stored, 0o600) {
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

/// Atomic write (temporary file + rename).
fn write_json(path: &Path, value: &impl Serialize, mode: u32) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(mode).open(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    // Nothing listens on port 9 (discard), so jobs that do start fail fast.
    const URL: &str = "http://127.0.0.1:9/file.iso";

    fn manager() -> Arc<Manager> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ldmd-test-{}-{nanos}", std::process::id()));
        let manager = Manager::load(dir.join("downloads.json"), dir.join("settings.json")).unwrap();
        let settings = Settings { download_dir: Some(dir.join("Downloads")), ..Settings::default() };
        manager.set_settings(settings).unwrap();
        manager
    }

    fn add(manager: &Arc<Manager>, confirm: bool) -> u64 {
        manager.add(AddRequest { url: URL.into(), confirm, start: true, ..Default::default() }).unwrap()
    }

    #[tokio::test]
    async fn confirm_flag_creates_pending_download_that_does_not_start() {
        let manager = manager();
        let mut updates = manager.subscribe();
        let id = add(&manager, true);
        assert_eq!(manager.state(id), Some(State::Pending));
        let Ok(Response::Update { item }) = updates.try_recv() else { panic!("no update broadcast") };
        assert_eq!(item.state, State::Pending);
        assert!(item.probing, "direct downloads are probed for size");
        assert!(item.dir.is_some());
        manager.schedule();
        assert_eq!(manager.state(id), Some(State::Pending));
    }

    #[tokio::test]
    async fn pending_download_flagged_by_the_probe_cannot_be_confirmed() {
        let manager = manager();
        let id = add(&manager, true);
        manager.update(id, |info| info.error = Some("lista".into()));
        assert!(manager.confirm(id, true, None, None).is_err());
        assert_eq!(manager.state(id), Some(State::Pending));
    }

    #[tokio::test]
    async fn without_confirm_the_download_starts() {
        let manager = manager();
        let id = add(&manager, false);
        assert_eq!(manager.state(id), Some(State::Running));
    }

    #[tokio::test]
    async fn confirm_applies_name_and_folder() {
        let manager = manager();
        let id = add(&manager, true);
        manager.confirm(id, false, Some("outro.iso".into()), Some("/tmp/x".into())).unwrap();
        let info = manager.list().into_iter().find(|i| i.id == id).unwrap();
        assert_eq!(info.name, "outro.iso");
        assert_eq!(info.dir.as_deref(), Some(Path::new("/tmp/x")));
        // A free slot: queued downloads start immediately.
        assert_eq!(info.state, State::Running);
        // Only pending downloads can be confirmed.
        assert!(manager.confirm(id, true, None, None).is_err());
        assert!(manager.confirm(999, true, None, None).is_err());
    }

    #[tokio::test]
    async fn queue_waits_for_a_slot_but_download_now_does_not() {
        let manager = manager();
        for _ in 0..manager.settings().max_parallel {
            add(&manager, false);
        }
        let queued = add(&manager, true);
        let now = add(&manager, true);
        manager.confirm(queued, false, None, None).unwrap();
        manager.confirm(now, true, None, None).unwrap();
        assert_eq!(manager.state(queued), Some(State::Queued));
        assert_eq!(manager.state(now), Some(State::Running));
    }

    #[tokio::test]
    async fn cancel_removes_the_pending_download() {
        let manager = manager();
        let id = add(&manager, true);
        manager.remove(id, false).await.unwrap();
        assert_eq!(manager.state(id), None);
    }

    #[tokio::test]
    async fn pending_survives_a_restart() {
        let manager = manager();
        let id = add(&manager, true);
        manager.save();
        let reloaded = Manager::load(manager.store.clone(), manager.settings_path.clone()).unwrap();
        assert_eq!(reloaded.state(id), Some(State::Pending));
    }

    #[tokio::test]
    async fn category_folders() {
        let manager = manager();
        let base = manager.settings().download_dir.unwrap();
        let dir_of = |id| manager.list().into_iter().find(|i| i.id == id).unwrap().dir.unwrap();
        let iso = add(&manager, true);
        assert_eq!(manager.list()[0].category, Category::Program);
        assert_eq!(dir_of(iso), base.join("Programas"));

        // A page video goes to Vídeos; the user-chosen folder wins.
        let page = manager
            .add(AddRequest { url: "https://example.com/v".into(), kind: SourceKind::Page, confirm: true, ..Default::default() })
            .unwrap();
        assert_eq!(dir_of(page), base.join("Vídeos"));
        let chosen = manager
            .add(AddRequest { url: URL.into(), output_dir: Some("/tmp/aqui".into()), ..Default::default() })
            .unwrap();
        assert_eq!(dir_of(chosen), Path::new("/tmp/aqui"));

        // Turning the subfolders off moves pending downloads back to the base folder.
        manager.set_settings(Settings { category_folders: false, ..manager.settings() }).unwrap();
        assert_eq!(dir_of(iso), base);
        manager.confirm(iso, false, None, None).unwrap();
        manager.set_settings(Settings { category_folders: true, ..manager.settings() }).unwrap();
        assert_eq!(dir_of(iso), base, "confirmed downloads keep their folder");
    }

    #[tokio::test]
    async fn speed_limit_follows_the_settings() {
        let manager = manager();
        assert_eq!(manager.limiter.rate(), 0);
        manager.set_settings(Settings { speed_limit: 512 * 1024, ..manager.settings() }).unwrap();
        assert_eq!(manager.limiter.rate(), 512 * 1024);
        let reloaded = Manager::load(manager.store.clone(), manager.settings_path.clone()).unwrap();
        assert_eq!(reloaded.limiter.rate(), 512 * 1024);
    }

    #[tokio::test]
    async fn settings_are_saved_and_broadcast() {
        let manager = manager();
        let mut updates = manager.subscribe();
        manager.set_settings(Settings { max_parallel: 5, ..manager.settings() }).unwrap();
        assert!(matches!(updates.try_recv(), Ok(Response::Settings { settings }) if settings.max_parallel == 5));
        let reloaded = Manager::load(manager.store.clone(), manager.settings_path.clone()).unwrap();
        assert_eq!(reloaded.settings().max_parallel, 5);
        assert!(manager.set_settings(Settings { download_dir: Some("relativa".into()), ..Settings::default() }).is_err());
    }
}
