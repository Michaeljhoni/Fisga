//! Download list, queues and job lifecycle.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use ldm_core::{DownloadOptions, EngineEvent, Outcome, RequestSpec, Target};
use ldm_ipc::{
    AddRequest, Category, Conflict, Direction, DownloadInfo, LocalTime, PowerPlan, Response, Settings, SourceKind,
    State, MAIN_QUEUE,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{notify, ytdlp};

mod network;
mod refresh;
mod scheduler;

pub use scheduler::tick_interval;

/// The user's answer in the "Novo download" window.
#[derive(Debug, Default)]
pub struct Answer {
    /// "Baixar agora": start even when the queue is full.
    pub now: bool,
    pub file_name: Option<String>,
    pub output_dir: Option<PathBuf>,
    /// A `MediaFormat::key` (quality of a site video).
    pub format: Option<String>,
    pub conflict: Option<Conflict>,
    /// Another queue (a `Queue::id`).
    pub queue: Option<u64>,
}

pub enum JobResult {
    Completed(PathBuf),
    Paused,
    Failed(String),
}

struct Job {
    info: DownloadInfo,
    request: AddRequest,
    cancel: Option<CancellationToken>,
    /// "Baixar agora": start even when its queue is full or outside its hours.
    forced: bool,
    /// The folder follows the category (no folder was chosen by the user), so it
    /// changes while the name of a pending download is still being found out.
    auto_dir: bool,
    /// Started by the queue (not by "Baixar agora", nor resumed by hand outside
    /// the scheduled window): the scheduler's stop time puts it back in the queue.
    from_queue: bool,
    /// Paused by the scheduler: goes back to `Queued` instead of `Paused`.
    requeue: bool,
}

impl Job {
    fn new(info: DownloadInfo, request: AddRequest, auto_dir: bool) -> Self {
        Self { info, request, cancel: None, forced: false, auto_dir, from_queue: false, requeue: false }
    }
}

struct Inner {
    next_id: u64,
    jobs: BTreeMap<u64, Job>,
}

impl Inner {
    /// A position after every download (new ones and ones moved to another
    /// queue go to its end), never equal to an existing one.
    fn next_order(&self) -> i64 {
        let last = self.jobs.values().map(|j| j.info.order).max().unwrap_or(0);
        (last + 1).max(self.next_id as i64)
    }
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
    /// Rebuilt when the proxy changes (see network.rs).
    client: Mutex<reqwest::Client>,
    logins: Mutex<Vec<ldm_ipc::SiteLogin>>,
    logins_path: PathBuf,
    store: PathBuf,
    dirty: AtomicBool,
    /// Never locked while `inner` is held: take a copy first.
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    /// Speed limit shared by every direct download.
    limiter: Arc<ldm_core::RateLimiter>,
    /// Never locked while `inner` is held.
    scheduler: Mutex<scheduler::Scheduler>,
    scheduler_path: PathBuf,
    power: Mutex<Option<PowerPlan>>,
    /// Time to cancel a planned suspend/power off (`$LDM_POWER_DELAY`, seconds).
    power_delay: Duration,
    /// `$LDM_POWER_CMD` replaces `systemctl` (tests); it gets "suspend" or "poweroff".
    power_command: Option<String>,
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
                // Saved before queues existed: the order is the order of arrival.
                if info.order == 0 {
                    info.order = info.id as i64;
                }
                if info.page.is_none() {
                    info.page = request.page_url.clone().or(request.referrer.clone());
                }
                inner.jobs.insert(info.id, Job::new(info, request, false));
            }
        }
        let settings = load_settings(&settings_path);
        for job in inner.jobs.values_mut() {
            if settings.queue(job.info.queue).is_none() {
                job.info.queue = MAIN_QUEUE;
            }
        }
        let power_delay = std::env::var("LDM_POWER_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
        let scheduler_path = scheduler::saved_path(&store);
        let saved = scheduler::load_saved(&scheduler_path);
        let client = network::build_client(&settings.proxy).or_else(|e| {
            // Never around the proxy the user chose: downloads fail until it is fixed.
            tracing::error!(error = %e, "proxy das configurações inválido; os downloads vão falhar até ele ser corrigido");
            ldm_core::build_client_with(ldm_core::Proxy::Url(network::UNREACHABLE_PROXY))
        })?;
        let logins_path = network::logins_path(&settings_path);
        Ok(Arc::new(Self {
            inner: Mutex::new(inner),
            updates: broadcast::channel(256).0,
            client: Mutex::new(client),
            logins: Mutex::new(network::load_logins(&logins_path)),
            logins_path,
            store,
            dirty: AtomicBool::new(false),
            limiter: Arc::new(ldm_core::RateLimiter::new(settings.speed_limit)),
            scheduler: Mutex::new(scheduler::Scheduler::new(&settings, LocalTime::now(), saved)),
            scheduler_path,
            settings: Mutex::new(settings),
            settings_path,
            power: Mutex::new(None),
            power_delay: Duration::from_secs(power_delay),
            power_command: std::env::var("LDM_POWER_CMD").ok().filter(|c| !c.is_empty()),
        }))
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
        let old = self.settings();
        // A new proxy needs a new client; a wrong one is refused before anything is saved.
        let client = match settings.proxy != old.proxy {
            true => Some(network::build_client(&settings.proxy)?),
            false => None,
        };
        // 0600: the proxy address may hold a password.
        write_json(&self.settings_path, &settings, 0o600)?;
        if let Some(client) = client {
            self.set_client(client);
        }
        let changed: Vec<u64> = settings
            .all_queues()
            .into_iter()
            .filter(|queue| old.queue(queue.id).map(|q| q.schedule) != Some(queue.schedule.clone()))
            .map(|queue| queue.id)
            .collect();
        *self.settings.lock().unwrap() = settings.clone();
        self.limiter.set_rate(settings.speed_limit);
        tracing::info!(?settings, "configurações alteradas");
        let _ = self.updates.send(Response::Settings { settings: settings.clone() });
        // Downloads of a removed queue go to the main one.
        {
            let mut inner = self.inner.lock().unwrap();
            for job in inner.jobs.values_mut().filter(|j| settings.queue(j.info.queue).is_none()) {
                job.info.queue = MAIN_QUEUE;
                let _ = self.updates.send(Response::Update { item: job.info.clone() });
                self.dirty.store(true, Ordering::Relaxed);
            }
        }
        self.refresh_pending_dirs();
        self.scheduler_check(LocalTime::now(), &changed);
        self.schedule();
        Ok(())
    }

    fn check_queue(&self, queue: u64) -> anyhow::Result<()> {
        match self.settings().queue(queue) {
            Some(_) => Ok(()),
            None => bail!("fila não encontrada: {queue}"),
        }
    }

    /// Moves a download to the end of another queue.
    pub fn set_queue(self: &Arc<Self>, id: u64, queue: u64) -> anyhow::Result<()> {
        self.check_queue(queue)?;
        {
            let mut inner = self.inner.lock().unwrap();
            let order = inner.next_order();
            let job = inner.jobs.get_mut(&id).context("download não encontrado")?;
            if job.info.queue == queue {
                return Ok(());
            }
            job.info.queue = queue;
            job.info.order = order;
            // "Baixar agora" was for the old queue: the new one's limit applies.
            job.forced = false;
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
        }
        tracing::info!(id, queue, "download movido de fila");
        self.save();
        self.schedule();
        Ok(())
    }

    /// Moves a download up or down in its queue (among the unfinished ones).
    pub fn reorder(self: &Arc<Self>, id: u64, direction: Direction) -> anyhow::Result<()> {
        {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.jobs.get(&id).context("download não encontrado")?;
            let queue = job.info.queue;
            let mut line: Vec<(i64, u64)> = inner
                .jobs
                .values()
                .filter(|j| j.info.queue == queue && j.info.state != State::Completed)
                .map(|j| (j.info.order, j.info.id))
                .collect();
            line.sort();
            let Some(at) = line.iter().position(|(_, other)| *other == id) else { return Ok(()) };
            let target = match direction {
                Direction::Up => at.checked_sub(1),
                Direction::Down => (at + 1 < line.len()).then_some(at + 1),
                Direction::Top => (at > 0).then_some(0),
                Direction::Bottom => (at + 1 < line.len()).then_some(line.len() - 1),
            };
            let Some(target) = target else { return Ok(()) };
            let moved = line.remove(at);
            line.insert(target, moved);
            // Renumber the line, keeping the positions it already used (made
            // distinct first: equal positions could not be swapped).
            let mut orders: Vec<i64> = line.iter().map(|(order, _)| *order).collect();
            orders.sort();
            for i in 1..orders.len() {
                orders[i] = orders[i].max(orders[i - 1] + 1);
            }
            for ((_, other), order) in line.into_iter().zip(orders) {
                let job = inner.jobs.get_mut(&other).expect("id from map");
                if job.info.order != order {
                    job.info.order = order;
                    let _ = self.updates.send(Response::Update { item: job.info.clone() });
                }
            }
        }
        self.save();
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
        let queue = request.queue.unwrap_or(MAIN_QUEUE);
        self.check_queue(queue)?;
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
            let order = inner.next_order();
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
                formats: Vec::new(),
                queue,
                order,
                page: request.page_url.clone().or(request.referrer.clone()),
                awaiting_url: false,
            };
            let _ = self.updates.send(Response::Update { item: info.clone() });
            inner.jobs.insert(id, Job::new(info, request, auto));
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
            let spec = RequestSpec { url: request.url.clone(), headers: self.headers(&request) };
            let result = ldm_core::probe(&self.client(), &spec).await;
            // A download waiting for a new address takes this one's link (see refresh.rs).
            if let Ok(probe) = &result {
                if self.try_take_address(id, probe.total, &probe.file_name).await {
                    return;
                }
            }
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
        let result = ytdlp::probe(id, &request, &self.settings().proxy).await;
        self.update(id, |info| {
            info.probing = false;
            match result {
                Ok(ytdlp::PageProbe::Video { name, size, formats }) => {
                    info.total = size;
                    info.formats = formats;
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
    pub fn confirm(self: &Arc<Self>, id: u64, answer: Answer) -> anyhow::Result<()> {
        let Answer { now, file_name, output_dir, format, conflict, queue } = answer;
        let settings = self.settings();
        if let Some(queue) = queue {
            self.check_queue(queue)?;
        }
        let mut to_delete = Vec::new();
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
            if let Some(format) = format {
                if !job.info.formats.iter().any(|f| f.key == format) {
                    let known: Vec<&str> = job.info.formats.iter().map(|f| f.key.as_str()).collect();
                    match known.is_empty() {
                        true => bail!("este download não tem opções de qualidade"),
                        false => bail!("qualidade desconhecida: {format} (opções: {})", known.join(", ")),
                    }
                }
                job.request.format = Some(format);
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
            if let (Some(conflict), Some(dir)) = (conflict, job.info.dir.clone()) {
                let name = ldm_core::filename::sanitize(&job.info.name);
                let name = match conflict {
                    Conflict::Rename => ldm_ipc::unique_name(&dir, &name),
                    Conflict::Overwrite => {
                        let path = dir.join(&name);
                        to_delete.extend(ldm_ipc::partial_paths(&path));
                        to_delete.push(path);
                        name
                    }
                    // The engine (and yt-dlp) continue a partial file with the same name.
                    Conflict::Resume => name,
                };
                job.info.name = name.clone();
                job.request.file_name = Some(name);
            }
            // Last, after every check that can refuse the answer.
            if let Some(queue) = queue {
                job.info.queue = queue;
            }
            job.auto_dir = false;
            job.info.state = State::Queued;
            job.forced = now;
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
        }
        for path in to_delete {
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!(id, path = %path.display(), "arquivo existente substituído"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(id, path = %path.display(), error = %e, "não foi possível apagar"),
            }
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
                // Paused by the user: stays paused, even at the scheduler's stop time.
                job.requeue = false;
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
        // Outside the scheduled window the queue waits, but a download resumed
        // by hand starts now, like "Baixar agora".
        let queue = self.inner.lock().unwrap().jobs.get(&id).map_or(MAIN_QUEUE, |j| j.info.queue);
        let forced = !self.queue_open(queue);
        {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.jobs.get_mut(&id).context("download não encontrado")?;
            // Resumed by hand: no longer waiting for a new address.
            job.info.awaiting_url = false;
            if matches!(job.info.state, State::Paused | State::Failed) {
                job.info.state = State::Queued;
                job.info.error = None;
                job.forced = forced;
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

    /// Starts queued downloads while their queue has a free slot ("Baixar
    /// agora" ones start regardless). Outside its scheduled window a queue only
    /// starts those.
    pub fn schedule(self: &Arc<Self>) {
        let queues = self.settings().all_queues();
        let limits: HashMap<u64, usize> = queues.iter().map(|q| (q.id, q.max_parallel)).collect();
        let open: HashMap<u64, bool> = queues.iter().map(|q| (q.id, self.queue_open(q.id))).collect();
        let mut inner = self.inner.lock().unwrap();
        let mut running: HashMap<u64, usize> = HashMap::new();
        for job in inner.jobs.values().filter(|j| j.info.state == State::Running) {
            *running.entry(job.info.queue).or_default() += 1;
        }
        // Forced first, then in queue order.
        let mut queued: Vec<(bool, i64, u64, u64)> = inner
            .jobs
            .values()
            .filter(|j| j.info.state == State::Queued)
            .map(|j| (!j.forced, j.info.order, j.info.id, j.info.queue))
            .collect();
        queued.sort();
        let mut to_start = Vec::new();
        for (not_forced, _, id, queue) in queued {
            let slots = running.entry(queue).or_default();
            let free = open.get(&queue).copied().unwrap_or(true) && *slots < limits.get(&queue).copied().unwrap_or(1);
            if !not_forced || free {
                *slots += 1;
                to_start.push(id);
            }
        }
        for id in to_start {
            let job = inner.jobs.get_mut(&id).expect("id from map");
            let cancel = CancellationToken::new();
            job.cancel = Some(cancel.clone());
            job.from_queue = !job.forced;
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
        let spec = RequestSpec { url: request.url.clone(), headers: self.headers(request) };
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
        let client = self.client();
        let download = ldm_core::download(&client, &spec, target, &opts, events_tx, cancel);
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
        let (requeue, from_queue, queue) = self
            .inner
            .lock()
            .unwrap()
            .jobs
            .get_mut(&id)
            .map(|job| {
                // Before the state changes: a resume right after the job reports
                // Paused installs a new token that must not be cleared here.
                job.cancel = None;
                let from_queue = job.from_queue;
                // A paused download the queue started still belongs to it (see `queue_busy`).
                if !matches!(result, JobResult::Paused) {
                    job.from_queue = false;
                }
                (std::mem::take(&mut job.requeue), from_queue, job.info.queue)
            })
            .unwrap_or_default();
        if from_queue && matches!(result, JobResult::Completed(_)) {
            self.scheduler.lock().unwrap().completed(queue);
        }
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
                JobResult::Paused if requeue => info.state = State::Queued,
                JobResult::Paused => info.state = State::Paused,
                JobResult::Failed(error) => {
                    info.state = State::Failed;
                    info.error = Some(error.clone());
                    message = Some(("Falha no download".to_string(), format!("{}\n{}", info.name, error)));
                }
            }
        });
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

    pub(super) fn manager() -> Arc<Manager> {
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
        assert!(manager.confirm(id, Answer { now: true, ..Default::default() }).is_err());
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
        manager.confirm(id, Answer { file_name: Some("outro.iso".into()), output_dir: Some("/tmp/x".into()), ..Default::default() }).unwrap();
        let info = manager.list().into_iter().find(|i| i.id == id).unwrap();
        assert_eq!(info.name, "outro.iso");
        assert_eq!(info.dir.as_deref(), Some(Path::new("/tmp/x")));
        // A free slot: queued downloads start immediately.
        assert_eq!(info.state, State::Running);
        // Only pending downloads can be confirmed.
        assert!(manager.confirm(id, Answer { now: true, ..Default::default() }).is_err());
        assert!(manager.confirm(999, Answer { now: true, ..Default::default() }).is_err());
    }

    #[tokio::test]
    async fn queue_waits_for_a_slot_but_download_now_does_not() {
        let manager = manager();
        for _ in 0..manager.settings().max_parallel {
            add(&manager, false);
        }
        let queued = add(&manager, true);
        let now = add(&manager, true);
        manager.confirm(queued, Answer::default()).unwrap();
        manager.confirm(now, Answer { now: true, ..Default::default() }).unwrap();
        assert_eq!(manager.state(queued), Some(State::Queued));
        assert_eq!(manager.state(now), Some(State::Running));
    }

    #[tokio::test]
    async fn existing_file_renamed_or_overwritten() {
        let manager = manager();
        let dir = manager.settings().download_dir.unwrap().join("Programas");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.iso"), b"old").unwrap();
        let info = |id| manager.list().into_iter().find(|i| i.id == id).unwrap();

        let renamed = add(&manager, true);
        manager.confirm(renamed, Answer { conflict: Some(Conflict::Rename), ..Default::default() }).unwrap();
        assert_eq!(info(renamed).name, "file (1).iso");
        assert_eq!(std::fs::read(dir.join("file.iso")).unwrap(), b"old");

        std::fs::write(dir.join("file.iso.ldm-part"), b"partial").unwrap();
        let overwritten = add(&manager, true);
        manager.confirm(overwritten, Answer { conflict: Some(Conflict::Overwrite), ..Default::default() }).unwrap();
        assert_eq!(info(overwritten).name, "file.iso");
        assert!(!dir.join("file.iso").exists() && !dir.join("file.iso.ldm-part").exists());
    }

    #[tokio::test]
    async fn quality_must_be_one_of_the_probed_formats() {
        let manager = manager();
        let id = add(&manager, true);
        assert!(manager.confirm(id, Answer { format: Some("720".into()), ..Default::default() }).is_err());
        manager.update(id, |info| {
            info.formats = vec![ldm_ipc::MediaFormat { key: "720".into(), label: "720p".into(), ext: None, size: None }]
        });
        manager.confirm(id, Answer { format: Some("720".into()), ..Default::default() }).unwrap();
        let request = manager.inner.lock().unwrap().jobs[&id].request.clone();
        assert_eq!(request.format.as_deref(), Some("720"));
    }

    fn add_to(manager: &Arc<Manager>, queue: u64, start: bool) -> u64 {
        let request = AddRequest { url: URL.into(), start, queue: Some(queue), ..Default::default() };
        manager.add(request).unwrap()
    }

    fn order_of(manager: &Arc<Manager>, queue: u64) -> Vec<u64> {
        let mut line: Vec<(i64, u64)> =
            manager.list().into_iter().filter(|i| i.queue == queue).map(|i| (i.order, i.id)).collect();
        line.sort();
        line.into_iter().map(|(_, id)| id).collect()
    }

    #[tokio::test]
    async fn queues_have_their_own_limit() {
        let manager = manager();
        let mut settings = manager.settings();
        settings.set_queue(ldm_ipc::Queue { id: 1, name: "Uma por vez".into(), max_parallel: 1, ..Default::default() });
        manager.set_settings(settings).unwrap();
        assert!(manager.add(AddRequest { url: URL.into(), queue: Some(9), ..Default::default() }).is_err());

        // Paused so nothing runs (and fails) while the queue is set up.
        let first = add_to(&manager, 1, false);
        let second = add_to(&manager, 1, false);
        let main = add_to(&manager, MAIN_QUEUE, false);
        manager.reorder(second, Direction::Top).unwrap();
        assert_eq!(order_of(&manager, 1), [second, first]);
        manager.reorder(second, Direction::Down).unwrap();
        assert_eq!(order_of(&manager, 1), [first, second]);
        manager.reorder(first, Direction::Up).unwrap();
        assert_eq!(order_of(&manager, 1), [first, second], "already at the top");

        // One at a time in queue 1, the first in line first; the main queue is apart.
        manager.reorder(second, Direction::Top).unwrap();
        for id in [second, first, main] {
            manager.resume(id).unwrap();
        }
        assert_eq!(manager.state(second), Some(State::Running));
        assert_eq!(manager.state(first), Some(State::Queued));
        assert_eq!(manager.state(main), Some(State::Running));

        // Moving it goes to the end of the other queue; removing a queue moves its downloads to the main one.
        manager.set_queue(first, MAIN_QUEUE).unwrap();
        assert_eq!(order_of(&manager, MAIN_QUEUE).last(), Some(&first));
        assert!(manager.set_queue(first, 9).is_err());
        let mut settings = manager.settings();
        settings.remove_queue(1);
        manager.set_settings(settings).unwrap();
        assert!(manager.list().iter().all(|i| i.queue == MAIN_QUEUE));
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
        manager.confirm(iso, Answer::default()).unwrap();
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
