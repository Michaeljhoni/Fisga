//! Protocol between `fisgad` (daemon) and its clients: one JSON object per line over
//! a local connection (see [`transport`]).

pub mod transport;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Native messaging host name registered in the browsers.
pub const NATIVE_HOST_NAME: &str = "io.fisga.native";
/// Firefox extension id (`browser_specific_settings.gecko.id`).
pub const FIREFOX_EXTENSION_ID: &str = "fisga@fisga.local";
/// Chrome extension id, derived from the public `key` in `manifest.chrome.json`.
pub const CHROME_EXTENSION_ID: &str = "ocifdbhjaijkdgcnpigjlemkmahkojcm";

/// How the daemon should fetch a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A plain file: segmented HTTP download.
    #[default]
    Direct,
    /// An HLS/DASH manifest (`.m3u8`/`.mpd`).
    Stream,
    /// A web page whose media must be extracted (YouTube, Vimeo...), via yt-dlp.
    Page,
}

/// A browser cookie, as returned by `cookies.getAll()` in the extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cookie {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub host_only: bool,
    #[serde(default)]
    pub expiration_date: Option<f64>,
}

fn root_path() -> String {
    "/".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRequest {
    pub url: String,
    #[serde(default)]
    pub kind: SourceKind,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub page_url: Option<String>,
    /// Page or media title, used as a display name until the file name is known.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub cookies: Vec<Cookie>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub connections: Option<usize>,
    /// Start right away (true) or only add to the list (false).
    #[serde(default = "yes")]
    pub start: bool,
    /// Ask the user first: the download is created as `State::Pending` and the
    /// GUI shows the "Novo download" window. `start` is ignored.
    #[serde(default)]
    pub confirm: bool,
    /// Quality of a site video: a `MediaFormat::key` ("1080", "audio"...);
    /// `None` = the best one.
    #[serde(default)]
    pub format: Option<String>,
    /// A `Queue::id`; `None` = the main queue.
    #[serde(default)]
    pub queue: Option<u64>,
    /// The checksum the download page published, checked when it finishes.
    #[serde(default)]
    pub checksum: Option<Checksum>,
}

fn yes() -> bool {
    true
}

/// A quality the user can pick for a site video, found by the daemon's probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFormat {
    /// Sent back in `Request::Confirm::format`: a video height ("1080") or "audio".
    pub key: String,
    /// "1080p", "Somente áudio"...
    pub label: String,
    /// Extension of the file it produces, when known.
    #[serde(default)]
    pub ext: Option<String>,
    /// Estimated size in bytes.
    #[serde(default)]
    pub size: Option<u64>,
}

/// What to do when the destination file already exists (IDM's "arquivo já existe").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Conflict {
    /// Save as "name (1).ext".
    Rename,
    /// Delete the existing file (and any partial download) first.
    Overwrite,
    /// Continue the partial download that is there.
    Resume,
}

/// What is already at a download's destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    Nothing,
    File,
    /// An unfinished download: `name.fisga-part` (Fisga's engine) or `name.part` (yt-dlp).
    Partial,
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(suffix);
    PathBuf::from(os)
}

/// Partial files that may sit next to `path` (see `fisga_core::control::part_path`),
/// the engine's from before the rename (LDM) included.
pub fn partial_paths(path: &Path) -> [PathBuf; 5] {
    [".fisga-part", ".fisga-part.json", ".part", ".ldm-part", ".ldm-part.json"].map(|suffix| with_suffix(path, suffix))
}

pub fn existing(path: &Path) -> Existing {
    let [part, _, ytdlp_part, legacy_part, _] = partial_paths(path);
    if part.exists() || ytdlp_part.exists() || legacy_part.exists() {
        Existing::Partial
    } else if path.exists() {
        Existing::File
    } else {
        Existing::Nothing
    }
}

/// `name`, or `name (1).ext`, `name (2).ext`... whichever is free in `dir`.
pub fn unique_name(dir: &Path, name: &str) -> String {
    if existing(&dir.join(name)) == Existing::Nothing {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 10 => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (1..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| existing(&dir.join(candidate)) == Existing::Nothing)
        .expect("infinite iterator")
}

/// File type groups, as in IDM: the main window filters by them and, by
/// default, each one is saved in its own subfolder of the download folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Video,
    Music,
    Image,
    Document,
    Compressed,
    Program,
    #[default]
    Other,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Video,
        Category::Music,
        Category::Image,
        Category::Document,
        Category::Compressed,
        Category::Program,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Video => "Vídeos",
            Category::Music => "Músicas",
            Category::Image => "Imagens",
            Category::Document => "Documentos",
            Category::Compressed => "Compactados",
            Category::Program => "Programas",
            Category::Other => "Outros",
        }
    }

    /// Subfolder of the download folder; `Other` stays at its root.
    pub fn folder(self) -> Option<&'static str> {
        (self != Category::Other).then(|| self.label())
    }

    pub fn from_extension(ext: &str) -> Category {
        match ext.to_ascii_lowercase().as_str() {
            "mp4" | "m4v" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "flv" | "mpg" | "mpeg" | "3gp" | "ts"
            | "m2ts" | "mts" | "ogv" | "vob" | "rm" | "rmvb" => Category::Video,
            "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "oga" | "opus" | "wma" | "alac" | "aiff" | "aif"
            | "ape" | "mka" | "mid" | "midi" => Category::Music,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "tif" | "tiff" | "heic" | "heif" | "avif"
            | "ico" | "psd" | "raw" | "cr2" | "nef" | "xcf" => Category::Image,
            "pdf" | "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "epub" | "mobi" | "azw3" | "djvu" | "xls"
            | "xlsx" | "ods" | "csv" | "ppt" | "pptx" | "odp" | "tex" => Category::Document,
            "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "tbz" | "tbz2" | "xz" | "txz" | "zst" | "tzst"
            | "lz" | "lzma" | "lz4" | "cab" | "arj" | "z" => Category::Compressed,
            "appimage" | "deb" | "rpm" | "flatpak" | "flatpakref" | "snap" | "run" | "bin" | "sh" | "exe"
            | "msi" | "apk" | "pkg" | "dmg" | "jar" | "iso" | "img" => Category::Program,
            _ => Category::Other,
        }
    }

    /// From the file name; videos from sites (yt-dlp) are videos unless the
    /// name says otherwise (e.g. an audio-only `.m4a`).
    pub fn detect(name: &str, kind: SourceKind) -> Category {
        let lower = name.to_ascii_lowercase();
        // Compound extensions first: "x.tar.gz" is an archive, not "gz" only.
        if [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"].iter().any(|ext| lower.ends_with(ext)) {
            return Category::Compressed;
        }
        let category = name.rsplit_once('.').map(|(_, ext)| Category::from_extension(ext)).unwrap_or_default();
        match (category, kind) {
            (Category::Other, SourceKind::Page | SourceKind::Stream) => Category::Video,
            (category, _) => category,
        }
    }
}

/// Daemon settings, kept in `$XDG_CONFIG_HOME/fisga/settings.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Base download folder; `None` = the system's Downloads folder.
    pub download_dir: Option<PathBuf>,
    /// Save each category in its own subfolder (Vídeos, Músicas, Programas...).
    pub category_folders: bool,
    /// Downloads running at the same time; the rest wait in the queue.
    pub max_parallel: usize,
    /// Connections per file for direct downloads.
    pub connections: usize,
    /// Open the progress window when a confirmed download starts.
    pub show_progress: bool,
    /// Speed limit for all downloads together, in bytes per second; 0 = none.
    pub speed_limit: u64,
    /// A copied link to a file (video, music, program, archive...) opens the
    /// "Novo download" window, like IDM's clipboard monitor.
    pub watch_clipboard: bool,
    /// Icon in the system tray (StatusNotifierItem); closing the main window
    /// keeps `fisga-gui` running there.
    pub tray: bool,
    /// When the main queue runs (IDM's scheduler). `max_parallel` and this
    /// field are the main queue's settings; `queues` holds the other ones.
    pub schedule: Schedule,
    /// Queues created by the user, besides the main one.
    pub queues: Vec<Queue>,
    /// The highest queue id ever used: ids are never reused, so a download
    /// (or an open window) never ends up in a newer queue that took the id.
    pub last_queue_id: u64,
    pub proxy: ProxySettings,
    /// Columns of the main window's list the user hid ("size", "added"...).
    pub hidden_columns: Vec<String>,
    /// The main window shows the downloads as a list instead of cards.
    pub list_view: bool,
    /// The main window shows the newest downloads on top.
    pub newest_first: bool,
    pub on_complete: OnComplete,
    pub browser: BrowserSettings,
    /// The look of the windows.
    pub theme: Theme,
    /// In the weeks of Carnaval, Easter, Christmas and other holidays, the
    /// holiday's theme: its colors and a scene (confetti, snow, bats...)
    /// moving behind the main window's panels.
    pub festive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// The browser downloads Fisga takes over (IDM's "Tipos de arquivo") and the
/// key that sends a clicked link to Fisga whatever it is. The extension reads
/// them through the native host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BrowserSettings {
    /// Extensions, lowercase and without the dot.
    pub file_types: Vec<String>,
    pub force_key: ForceKey,
}

/// The extension's own copy of this list (`extension/src/background.js`)
/// serves until it hears from the app: keep them alike.
pub const DEFAULT_FILE_TYPES: &str = "3gp 7z aac apk appimage avi bin bz2 ckpt deb dmg epub exe flac flatpak flv \
     gguf gz img iso jar m4a m4v mkv mov mp3 mp4 mpeg mpg msi ogg ogv onnx opus pkg pt pth rar rpm run \
     safetensors tar tgz wav webm wma wmv xz zip zst";

impl Default for BrowserSettings {
    fn default() -> Self {
        Self { file_types: Self::parse_file_types(DEFAULT_FILE_TYPES), force_key: ForceKey::default() }
    }
}

impl BrowserSettings {
    /// `"MP4, .zip *.Safetensors"` → `["mp4", "zip", "safetensors"]`: lowercase,
    /// without dots, each once.
    pub fn parse_file_types(text: &str) -> Vec<String> {
        let mut types: Vec<String> = Vec::new();
        for word in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
            let word = word.trim_start_matches(['*', '.']).to_ascii_lowercase();
            let valid = !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if valid && !types.contains(&word) {
                types.push(word);
            }
        }
        types
    }
}

/// Held while clicking a link in the browser: the link goes to Fisga.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForceKey {
    None,
    /// As in IDM. Alt+click otherwise saves the link with the browser.
    #[default]
    Alt,
    CtrlAlt,
    ShiftAlt,
    CtrlShift,
}

impl ForceKey {
    pub const ALL: [ForceKey; 5] = [ForceKey::None, ForceKey::Alt, ForceKey::CtrlAlt, ForceKey::ShiftAlt, ForceKey::CtrlShift];

    pub fn label(self) -> &'static str {
        match self {
            ForceKey::None => "Nenhuma",
            ForceKey::Alt => "Alt",
            ForceKey::CtrlAlt => "Ctrl+Alt",
            ForceKey::ShiftAlt => "Shift+Alt",
            ForceKey::CtrlShift => "Ctrl+Shift",
        }
    }
}

/// What happens when a download finishes, in this order: the antivirus
/// (an infected file stops here), the user's command, the sound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OnComplete {
    /// The desktop's "complete" sound.
    pub sound: bool,
    /// Scan the file with ClamAV (`clamdscan`, else `clamscan`).
    pub antivirus: bool,
    /// Run in `sh -c`, with the file in `$FISGA_FILE` (and `$FISGA_NAME`,
    /// `$FISGA_URL`, `$FISGA_DIR`); empty = nothing.
    pub command: String,
}

impl Default for OnComplete {
    fn default() -> Self {
        Self { sound: true, antivirus: false, command: String::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// `http_proxy`/`https_proxy`/`all_proxy` from the environment, as before.
    #[default]
    System,
    /// Straight to the sites, whatever the environment says.
    None,
    /// `ProxySettings::url`.
    Manual,
}

/// The proxy for every download (the engine and yt-dlp).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProxySettings {
    pub mode: ProxyMode,
    /// `http://host:3128`, `socks5://host:1080`, with an optional `user:password@`.
    pub url: String,
}

/// Hides the password of `user:password@host`.
fn redact_url(text: &str) -> String {
    match url::Url::parse(text) {
        Ok(mut url) if url.password().is_some() => {
            let _ = url.set_password(Some("***"));
            url.to_string()
        }
        _ => text.to_string(),
    }
}

/// The settings are logged: the proxy password must not be.
impl std::fmt::Debug for ProxySettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxySettings").field("mode", &self.mode).field("url", &redact_url(&self.url)).finish()
    }
}

impl ProxySettings {
    /// What is wrong with a manual proxy address, if anything.
    pub fn check(&self) -> Result<(), String> {
        if self.mode != ProxyMode::Manual {
            return Ok(());
        }
        let url = url::Url::parse(self.url.trim())
            .map_err(|_| format!("endereço de proxy inválido: {} (use http://host:porta ou socks5://host:porta)", redact_url(&self.url)))?;
        if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") {
            return Err(format!("tipo de proxy não suportado: {} (use http, https, socks5 ou socks5h)", url.scheme()));
        }
        if url.host_str().map_or(true, str::is_empty) {
            return Err("o endereço do proxy precisa de um host".into());
        }
        Ok(())
    }

    /// `--proxy` for yt-dlp (an empty value means no proxy); `None`: the environment's.
    pub fn for_ytdlp(&self) -> Option<String> {
        match self.mode {
            ProxyMode::System => None,
            ProxyMode::None => Some(String::new()),
            ProxyMode::Manual => Some(self.url.trim().to_string()),
        }
    }

    pub fn describe(&self) -> String {
        match self.mode {
            ProxyMode::System => "o do sistema (variáveis http_proxy/https_proxy)".into(),
            ProxyMode::None => "nenhum".into(),
            ProxyMode::Manual => redact_url(self.url.trim()),
        }
    }
}

/// IDM's "site logins": direct downloads from `host` send this user and
/// password (HTTP Basic); `*.host` also covers its subdomains. Only over
/// https (or to this computer): plain http would show the password to the
/// network. Kept apart from the settings, in a file only the user can read.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteLogin {
    pub host: String,
    pub user: String,
    pub password: String,
}

impl std::fmt::Debug for SiteLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteLogin").field("host", &self.host).field("user", &self.user).finish_non_exhaustive()
    }
}

impl SiteLogin {
    /// `https://Arquivos.Exemplo.com/pasta` → `arquivos.exemplo.com`; `*.Exemplo.com` stays a wildcard.
    pub fn normalize_host(text: &str) -> String {
        let text = text.trim();
        let (wildcard, text) = match text.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let host = match text.contains("://") {
            true => url::Url::parse(text).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default(),
            false => text.split(['/', ':']).next().unwrap_or_default().to_string(),
        };
        let host = host.trim_start_matches('.').to_ascii_lowercase();
        match wildcard && !host.is_empty() {
            true => format!("*.{host}"),
            false => host,
        }
    }

    /// The host itself; with `*.`, its subdomains too.
    pub fn matches(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        match self.host.strip_prefix("*.") {
            Some(base) => !base.is_empty() && (host == base || host.ends_with(&format!(".{base}"))),
            None => !self.host.is_empty() && host == self.host,
        }
    }

    /// The login for `url`, if the password can go there: an exact host wins
    /// over a wildcard, a longer wildcard over a shorter one.
    pub fn find<'a>(logins: &'a [SiteLogin], url: &str) -> Option<&'a SiteLogin> {
        let url = url::Url::parse(url).ok()?;
        let local = match url.host()? {
            url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(ip) => ip.is_loopback(),
            url::Host::Ipv6(ip) => ip.is_loopback(),
        };
        if url.scheme() != "https" && !local {
            return None;
        }
        let host = url.host_str()?.to_string();
        logins
            .iter()
            .filter(|login| login.matches(&host))
            .max_by_key(|login| (!login.host.starts_with("*."), login.host.len()))
    }
}

/// The main queue: always there, it cannot be removed or renamed.
pub const MAIN_QUEUE: u64 = 0;
pub const MAIN_QUEUE_NAME: &str = "Fila principal";

/// A download queue, as in IDM: its own number of downloads at the same time
/// and its own schedule. Every download belongs to one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Queue {
    pub id: u64,
    pub name: String,
    /// Downloads of this queue running at the same time.
    pub max_parallel: usize,
    pub schedule: Schedule,
}

impl Default for Queue {
    fn default() -> Self {
        Self { id: MAIN_QUEUE, name: MAIN_QUEUE_NAME.into(), max_parallel: 3, schedule: Schedule::default() }
    }
}

/// Where to move a download inside its queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Up,
    Down,
    Top,
    Bottom,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: None,
            category_folders: true,
            max_parallel: 3,
            connections: 8,
            show_progress: true,
            speed_limit: 0,
            watch_clipboard: true,
            tray: true,
            schedule: Schedule::default(),
            queues: Vec::new(),
            last_queue_id: MAIN_QUEUE,
            proxy: ProxySettings::default(),
            hidden_columns: vec!["added".into()],
            list_view: false,
            newest_first: true,
            on_complete: OnComplete::default(),
            browser: BrowserSettings::default(),
            theme: Theme::Dark,
            festive: true,
        }
    }
}

fn sanitize_days(days: &mut Vec<u8>) {
    days.retain(|day| *day < 7);
    days.sort_unstable();
    days.dedup();
}

impl Settings {
    /// Keeps numbers in a usable range, and the queues unique and named.
    pub fn sanitized(mut self) -> Self {
        self.max_parallel = self.max_parallel.clamp(1, 20);
        self.connections = self.connections.clamp(1, 32);
        self.browser.file_types = BrowserSettings::parse_file_types(&self.browser.file_types.join(" "));
        sanitize_days(&mut self.schedule.days);
        let mut seen = vec![MAIN_QUEUE];
        self.queues.retain(|queue| {
            let new = !seen.contains(&queue.id);
            seen.push(queue.id);
            new
        });
        for queue in &mut self.queues {
            queue.max_parallel = queue.max_parallel.clamp(1, 20);
            sanitize_days(&mut queue.schedule.days);
            queue.name = queue.name.trim().to_string();
            if queue.name.is_empty() {
                queue.name = format!("Fila {}", queue.id);
            }
            self.last_queue_id = self.last_queue_id.max(queue.id);
        }
        self
    }

    /// Every queue, the main one first.
    pub fn all_queues(&self) -> Vec<Queue> {
        let main = Queue {
            id: MAIN_QUEUE,
            name: MAIN_QUEUE_NAME.into(),
            max_parallel: self.max_parallel,
            schedule: self.schedule.clone(),
        };
        std::iter::once(main).chain(self.queues.iter().cloned()).collect()
    }

    pub fn queue(&self, id: u64) -> Option<Queue> {
        self.all_queues().into_iter().find(|queue| queue.id == id)
    }

    pub fn queue_name(&self, id: u64) -> String {
        self.queue(id).map(|queue| queue.name).unwrap_or_else(|| format!("fila {id}"))
    }

    /// Adds or replaces a queue (for the main one: its limit and schedule).
    pub fn set_queue(&mut self, queue: Queue) {
        if queue.id == MAIN_QUEUE {
            self.max_parallel = queue.max_parallel;
            self.schedule = queue.schedule;
        } else if let Some(existing) = self.queues.iter_mut().find(|q| q.id == queue.id) {
            *existing = queue;
        } else {
            self.last_queue_id = self.last_queue_id.max(queue.id);
            self.queues.push(queue);
        }
    }

    /// The main queue stays.
    pub fn remove_queue(&mut self, id: u64) {
        self.queues.retain(|queue| queue.id != id);
    }

    pub fn new_queue_id(&self) -> u64 {
        self.queues.iter().map(|queue| queue.id).fold(self.last_queue_id, u64::max) + 1
    }
}

/// A time of day, written as "HH:MM" in JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TimeOfDay(u16);

impl TimeOfDay {
    pub fn new(hour: u16, minute: u16) -> Option<Self> {
        (hour < 24 && minute < 60).then_some(Self(hour * 60 + minute))
    }

    /// "2:00", "02:00", "23:59".
    pub fn parse(text: &str) -> Option<Self> {
        let (hour, minute) = text.trim().split_once(':')?;
        if minute.len() != 2 || hour.is_empty() || hour.len() > 2 {
            return None;
        }
        Self::new(hour.parse().ok()?, minute.parse().ok()?)
    }

    /// Minutes since midnight.
    pub fn minutes(self) -> u16 {
        self.0
    }
}

impl std::fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.0 / 60, self.0 % 60)
    }
}

impl TryFrom<String> for TimeOfDay {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text).ok_or_else(|| format!("horário inválido: {text} (use HH:MM)"))
    }
}

impl From<TimeOfDay> for String {
    fn from(time: TimeOfDay) -> String {
        time.to_string()
    }
}

/// What to do when a scheduled queue finishes (ordered: powering off wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhenDone {
    #[default]
    Nothing,
    Suspend,
    Poweroff,
}

impl WhenDone {
    pub const ALL: [WhenDone; 3] = [WhenDone::Nothing, WhenDone::Suspend, WhenDone::Poweroff];

    pub fn label(self) -> &'static str {
        match self {
            WhenDone::Nothing => "Não fazer nada",
            WhenDone::Suspend => "Suspender o computador",
            WhenDone::Poweroff => "Desligar o computador",
        }
    }
}

/// Short weekday names, indexed like `Schedule::days` (0 = Sunday).
pub const WEEKDAYS: [&str; 7] = ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"];

/// IDM's scheduler: the queue starts at `start` on the chosen days and runs
/// until `stop` (running downloads go back to the queue) or, without `stop`,
/// until it is empty. Outside that window queued downloads wait; "Baixar
/// agora" and resuming a download by hand still start right away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Schedule {
    pub enabled: bool,
    pub start: TimeOfDay,
    pub stop: Option<TimeOfDay>,
    /// Days the queue starts on: 0 = Sunday ... 6 = Saturday.
    pub days: Vec<u8>,
    pub when_done: WhenDone,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            start: TimeOfDay(2 * 60),
            stop: None,
            days: (0..7).collect(),
            when_done: WhenDone::Nothing,
        }
    }
}

impl Schedule {
    /// The window that contains `now`, identified by the local day it started
    /// on (a window that crosses midnight belongs to the day before).
    pub fn window_at(&self, now: LocalTime) -> Option<i64> {
        const DAY: u16 = 24 * 60;
        let start = self.start.minutes();
        let length = match self.stop {
            Some(stop) => match (stop.minutes() + DAY - start) % DAY {
                0 => DAY,
                length => length,
            },
            // Until the queue is empty, at most until the next start.
            None => DAY,
        };
        let (day, weekday, since_start) = if now.minutes >= start {
            (now.day, now.weekday, now.minutes - start)
        } else {
            (now.day - 1, (now.weekday + 6) % 7, now.minutes + DAY - start)
        };
        (self.days.contains(&weekday) && since_start < length).then_some(day)
    }

    /// "todos os dias, das 02:00 às 08:00", "seg, qua, sex, às 23:00 até terminar a fila".
    pub fn describe(&self) -> String {
        let days = match self.days.len() {
            7 => "todos os dias".to_string(),
            0 => "nenhum dia".to_string(),
            _ => self.days.iter().map(|d| WEEKDAYS[*d as usize % 7]).collect::<Vec<_>>().join(", "),
        };
        match self.stop {
            Some(stop) => format!("{days}, das {} às {stop}", self.start),
            None => format!("{days}, às {} até terminar a fila", self.start),
        }
    }
}

/// Local wall-clock time, as the scheduler sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    /// Days since 1970-01-01 in local time: tells days apart.
    pub day: i64,
    /// 0 = Sunday ... 6 = Saturday.
    pub weekday: u8,
    /// Minutes since midnight.
    pub minutes: u16,
}

impl LocalTime {
    #[cfg(unix)]
    pub fn now() -> Self {
        // SAFETY: time and localtime_r only write to the values we pass.
        let (time, tm) = unsafe {
            let time = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&time, &mut tm);
            (time, tm)
        };
        // time_t and c_long are i32 on some 32-bit targets.
        #[allow(clippy::unnecessary_cast)]
        let local = time as i64 + tm.tm_gmtoff as i64;
        Self {
            day: local.div_euclid(86_400),
            weekday: tm.tm_wday as u8,
            minutes: (tm.tm_hour * 60 + tm.tm_min) as u16,
        }
    }

    /// The system clock turned into local time by Windows, which knows the
    /// time zone and its daylight saving rules.
    #[cfg(windows)]
    pub fn now() -> Self {
        #[repr(C)]
        #[derive(Default)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetSystemTimeAsFileTime(time: *mut FileTime);
            fn FileTimeToLocalFileTime(utc: *const FileTime, local: *mut FileTime) -> i32;
        }
        let (mut utc, mut local) = (FileTime::default(), FileTime::default());
        // SAFETY: both only write to the values we pass.
        unsafe {
            GetSystemTimeAsFileTime(&mut utc);
            FileTimeToLocalFileTime(&utc, &mut local);
        }
        // 100 ns ticks since 1601-01-01.
        let ticks = (u64::from(local.high) << 32) | u64::from(local.low);
        let local = (ticks / 10_000_000) as i64 - 11_644_473_600;
        let day = local.div_euclid(86_400);
        Self {
            day,
            // 1970-01-01 was a Thursday.
            weekday: (day + 4).rem_euclid(7) as u8,
            minutes: (local.rem_euclid(86_400) / 60) as u16,
        }
    }
}

/// A scheduled queue finished and the computer will be suspended or turned
/// off at `at`, unless someone cancels (`Request::CancelPower`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerPlan {
    pub action: WhenDone,
    /// Unix seconds.
    pub at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Waiting for the user to confirm it in the "Novo download" window.
    Pending,
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadInfo {
    pub id: u64,
    pub url: String,
    pub kind: SourceKind,
    /// Best available display name (file name, else title, else URL).
    pub name: String,
    pub path: Option<PathBuf>,
    pub state: State,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Bytes per second.
    pub speed: f64,
    pub connections: usize,
    pub resumable: bool,
    pub error: Option<String>,
    /// Unix seconds.
    pub created_at: u64,
    /// Destination folder.
    #[serde(default)]
    pub dir: Option<PathBuf>,
    /// True while the daemon fetches the size and name of a pending download.
    #[serde(default)]
    pub probing: bool,
    #[serde(default)]
    pub category: Category,
    /// Qualities of a site video, best first; empty for other downloads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<MediaFormat>,
    /// The `Queue::id` it belongs to.
    #[serde(default)]
    pub queue: u64,
    /// Position in its queue: lower starts first.
    #[serde(default)]
    pub order: i64,
    /// The page the download came from (to get a fresh link from it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// Waiting for a new address: the next download of the same file sent
    /// by the browser replaces this one's link (`Request::AwaitUrl`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub awaiting_url: bool,
    /// This download's own speed limit, bytes per second; 0 = only the global one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub speed_limit: u64,
    /// The checksum the file must have, from the download page; checked when
    /// it finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<Checksum>,
    /// The last check of the file (`Request::Verify`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<Hashing>,
    /// How much of each of the `MAP_PARTS` stretches of the file is
    /// downloaded, 0–100: the file map of a segmented download. Empty for
    /// the others.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub map: Vec<u8>,
}

/// The stretches of a download's file map.
pub const MAP_PARTS: usize = 8;

/// The file map of a segmented download: for each of `MAP_PARTS` equal
/// stretches of the file, the percentage downloaded. `segments` are
/// (start, inclusive end, bytes done from start).
pub fn file_map(segments: impl IntoIterator<Item = (u64, u64, u64)>, total: u64) -> Vec<u8> {
    if total == 0 {
        return Vec::new();
    }
    let bounds = |part: usize| total * part as u64 / MAP_PARTS as u64;
    let mut done = vec![0u64; MAP_PARTS];
    for (start, end, written) in segments {
        let (from, to) = (start, (start + written).min(end + 1));
        for (part, sum) in done.iter_mut().enumerate() {
            let (a, b) = (bounds(part), bounds(part + 1));
            *sum += to.min(b).saturating_sub(from.max(a));
        }
    }
    done.iter()
        .enumerate()
        .map(|(part, sum)| match bounds(part + 1) - bounds(part) {
            0 => 100,
            len => (sum * 100 / len).min(100) as u8,
        })
        .collect()
}

impl DownloadInfo {
    /// `Some(false)`: the file is not the one the download page published.
    pub fn checksum_matches(&self) -> Option<bool> {
        match (&self.checksum, &self.hash) {
            (Some(expected), Some(Hashing::Done { digest })) => Some(expected == digest),
            _ => None,
        }
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// The algorithms download pages publish checksums in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgo {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlgo {
    pub const ALL: [HashAlgo; 4] = [HashAlgo::Sha1, HashAlgo::Sha256, HashAlgo::Sha384, HashAlgo::Sha512];

    pub fn name(self) -> &'static str {
        match self {
            HashAlgo::Sha1 => "sha1",
            HashAlgo::Sha256 => "sha256",
            HashAlgo::Sha384 => "sha384",
            HashAlgo::Sha512 => "sha512",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HashAlgo::Sha1 => "SHA-1",
            HashAlgo::Sha256 => "SHA-256",
            HashAlgo::Sha384 => "SHA-384",
            HashAlgo::Sha512 => "SHA-512",
        }
    }

    /// Hex digits in a digest.
    pub fn hex_len(self) -> usize {
        match self {
            HashAlgo::Sha1 => 40,
            HashAlgo::Sha256 => 64,
            HashAlgo::Sha384 => 96,
            HashAlgo::Sha512 => 128,
        }
    }
}

/// A file's checksum: the algorithm and the digest in lowercase hex, written
/// "sha256:3a7b…".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Checksum {
    pub algo: HashAlgo,
    pub hex: String,
}

impl Checksum {
    /// A checksum as download pages show it: "sha256:3A7B…", "SHA-256: 3a7b…",
    /// a line of a SHA256SUMS file ("3a7b…  file.iso", or "SHA256 (file.iso) =
    /// 3a7b…"), or the digits alone, the algorithm told by how many there are.
    pub fn parse(text: &str) -> Result<Checksum, String> {
        let words: Vec<String> = text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        let named = words.iter().find_map(|word| {
            let word = word.replace('-', "");
            HashAlgo::ALL.into_iter().find(|algo| algo.name() == word)
        });
        let digits = words.iter().find(|word| {
            word.chars().all(|c| c.is_ascii_hexdigit()) && HashAlgo::ALL.iter().any(|algo| algo.hex_len() == word.len())
        });
        let Some(hex) = digits else {
            return Err(match named {
                Some(algo) => format!("o {} tem {} dígitos hexadecimais", algo.label(), algo.hex_len()),
                None => "não há uma soma SHA-1, SHA-256, SHA-384 ou SHA-512 no texto".into(),
            });
        };
        let by_length = HashAlgo::ALL.into_iter().find(|algo| algo.hex_len() == hex.len()).expect("checked above");
        match named {
            Some(algo) if algo != by_length => {
                Err(format!("o {} tem {} dígitos hexadecimais; este tem {}", algo.label(), algo.hex_len(), hex.len()))
            }
            _ => Ok(Checksum { algo: by_length, hex: hex.clone() }),
        }
    }
}

impl std::fmt::Display for Checksum {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}", self.algo.name(), self.hex)
    }
}

impl From<Checksum> for String {
    fn from(checksum: Checksum) -> String {
        checksum.to_string()
    }
}

impl TryFrom<String> for Checksum {
    type Error = String;

    fn try_from(text: String) -> Result<Checksum, String> {
        Checksum::parse(&text)
    }
}

/// Reading a finished download's file to get its checksum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Hashing {
    /// Bytes read so far.
    Running { done: u64 },
    Done { digest: Checksum },
    Failed { error: String },
}

/// The most addresses a batch may have.
pub const MAX_BATCH: usize = 5000;

/// IDM's batch download: `https://site/aula_*.mp4` from `1` to `20` (or `a`
/// to `z`). A start with leading zeros (`01`) pads every number to its width,
/// unless `width` says otherwise (0 = no padding beyond that).
pub fn expand_batch(template: &str, from: &str, to: &str, width: usize) -> Result<Vec<String>, String> {
    let template = template.trim();
    if template.matches('*').count() != 1 {
        return Err("o endereço precisa de exatamente um * no lugar do número ou da letra".into());
    }
    let (from, to) = (from.trim(), to.trim());
    let letters = |text: &str| {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(c),
            _ => None,
        }
    };
    let items: Vec<String> = match (from.parse::<u64>(), to.parse::<u64>(), letters(from), letters(to)) {
        (Ok(first), Ok(last), _, _) => {
            if first > last {
                return Err(format!("o início ({first}) passa do fim ({last})"));
            }
            if last - first >= MAX_BATCH as u64 {
                return Err(format!("são {} endereços; o máximo é {MAX_BATCH}", last - first + 1));
            }
            let width = match width {
                0 if from.len() > 1 && from.starts_with('0') => from.len(),
                width => width,
            };
            (first..=last).map(|n| format!("{n:0width$}")).collect()
        }
        (_, _, Some(first), Some(last)) if first.is_ascii_lowercase() == last.is_ascii_lowercase() => {
            if first > last {
                return Err(format!("o início ({first}) passa do fim ({last})"));
            }
            (first..=last).map(String::from).collect()
        }
        _ => return Err("use números (1 a 20, 01 a 20) ou letras (a a z) no início e no fim".into()),
    };
    Ok(items.into_iter().map(|item| template.replacen('*', &item, 1)).collect())
}

/// Every link in a text (one per line, or among other words), in order and
/// without repeats: the "lista de endereços" of a batch.
pub fn parse_url_list(text: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let word = word.trim_matches(|c: char| matches!(c, '<' | '>' | '"' | '\'' | '(' | ')' | ',' | ';'));
        let link = ["http://", "https://", "ftp://"].iter().any(|scheme| {
            word.get(..scheme.len()).is_some_and(|start| start.eq_ignore_ascii_case(scheme))
        });
        if link && url::Url::parse(word).is_ok() && !urls.iter().any(|u| u == word) {
            urls.push(word.to_string());
        }
    }
    urls
}

/// A file the grabber found on a site (IDM's "Site Grabber").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrabbedLink {
    pub url: String,
    /// The file name, from the address.
    pub name: String,
    pub category: Category,
}

/// The deepest the grabber follows links (0 = only the page given).
pub const MAX_GRAB_DEPTH: u8 = 3;

/// `*` (any text) and `?` (one character), ignoring case: `*.pdf`, `aula_??.mp4`.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();
    // Classic wildcard matching with one backtrack point.
    let (mut p, mut n, mut star, mut mark) = (0, 0, None, 0);
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = n;
            p += 1;
        } else if let Some(at) = star {
            p = at + 1;
            mark += 1;
            n = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// The grabber's filters: the categories wanted (none = all) and name
/// patterns (any of them; none = all), separated by spaces, commas or `;`.
pub fn grab_filter(link: &GrabbedLink, categories: &[Category], patterns: &str) -> bool {
    let patterns: Vec<&str> = patterns.split([' ', ',', ';']).filter(|p| !p.is_empty()).collect();
    (categories.is_empty() || categories.contains(&link.category))
        && (patterns.is_empty() || patterns.iter().any(|pattern| glob_matches(pattern, &link.name)))
}

/// The limit a download gets from the global one and its own (0 = none):
/// the tighter one.
pub fn combined_limit(global: u64, own: u64) -> u64 {
    match (global, own) {
        (0, limit) | (limit, 0) => limit,
        (global, own) => global.min(own),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Add(AddRequest),
    List,
    Pause { id: u64 },
    Resume { id: u64 },
    Remove {
        id: u64,
        #[serde(default)]
        delete_file: bool,
    },
    /// Keep the connection open and receive `Update` messages.
    Subscribe {
        /// This client shows the "Novo download" window for pending downloads
        /// (the GUI). While one is connected, the daemon does not launch `fisga-gui`.
        #[serde(default)]
        confirmer: bool,
    },
    /// Answer to a pending download: start it now or put it in the queue,
    /// optionally with another file name or folder. To cancel, use `Remove`.
    Confirm {
        id: u64,
        /// Start immediately, even when the queue is full.
        #[serde(default)]
        now: bool,
        #[serde(default)]
        file_name: Option<String>,
        #[serde(default)]
        output_dir: Option<PathBuf>,
        /// A `MediaFormat::key` of the download.
        #[serde(default)]
        format: Option<String>,
        /// The file already exists at the destination: what to do with it.
        #[serde(default)]
        conflict: Option<Conflict>,
        /// Another queue for it (a `Queue::id`).
        #[serde(default)]
        queue: Option<u64>,
        /// The checksum the file must have.
        #[serde(default)]
        checksum: Option<Checksum>,
    },
    /// Moves a download to another queue, at its end.
    SetQueue { id: u64, queue: u64 },
    /// Moves a download inside its queue.
    Reorder { id: u64, direction: Direction },
    /// IDM's "Refresh download address": another link to the same file (an
    /// expired one renewed); the download continues where it stopped.
    RefreshUrl { id: u64, url: String },
    /// Waits for (or stops waiting for) the browser to send the same file
    /// again, and takes its link.
    AwaitUrl { id: u64, waiting: bool },
    /// A download's own speed limit in bytes per second (0 = none), on top of
    /// the global one. Applies at once to direct downloads.
    SetSpeedLimit { id: u64, limit: u64 },
    /// Checks a finished download's file against `checksum`, which becomes
    /// the one it must have; `None` keeps the one it has, or only computes
    /// the SHA-256. An unfinished download is checked when it finishes.
    Verify {
        id: u64,
        #[serde(default)]
        checksum: Option<Checksum>,
    },
    /// IDM's site grabber: the files linked from `url` and, up to `depth`
    /// levels, from its pages below it on the same site. Answered with
    /// `Response::Grabbed`; adding them is an `AddBatch`.
    Grab { url: String, depth: u8 },
    /// IDM's batch download: many addresses at once, straight to a queue
    /// (no "Novo download" window for each). Answered with `Response::AddedBatch`.
    AddBatch {
        urls: Vec<String>,
        #[serde(default)]
        output_dir: Option<PathBuf>,
        #[serde(default)]
        queue: Option<u64>,
        /// Into the queue (true) or paused (false).
        #[serde(default = "yes")]
        start: bool,
    },
    Shutdown,
    GetSettings,
    /// Replaces the settings; subscribers receive `Response::Settings`.
    SetSettings { settings: Settings },
    /// Calls off the suspend/power off planned after a scheduled queue.
    CancelPower,
    /// The site logins (`Response::Logins`). Not part of the settings: they are
    /// never broadcast nor logged.
    GetLogins,
    SetLogins { logins: Vec<SiteLogin> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong { version: String },
    Added { id: u64 },
    AddedBatch { ids: Vec<u64> },
    /// The grabber's findings, and how many pages it read.
    Grabbed { links: Vec<GrabbedLink>, pages: usize },
    Downloads { items: Vec<DownloadInfo> },
    Update { item: DownloadInfo },
    Removed { id: u64 },
    Settings { settings: Settings },
    /// The planned suspend/power off changed (`None`: nothing planned). Sent to
    /// subscribers, and right after subscribing while one is planned.
    Power { plan: Option<PowerPlan> },
    Logins { logins: Vec<SiteLogin> },
    Ok,
    Error { message: String },
}

/// The daemon's private folder for its socket and temporary files:
/// `$XDG_RUNTIME_DIR/fisga`, or `fisga-<uid>` in the temporary folder when the
/// variable is unset (macOS, where that folder is already per user).
#[cfg(unix)]
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("fisga"),
        // SAFETY: getuid has no preconditions and cannot fail.
        _ => std::env::temp_dir().join(format!("fisga-{}", unsafe { libc::getuid() })),
    }
}

/// `%LOCALAPPDATA%\fisga\run`, which only the user can open.
#[cfg(windows)]
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("fisga")
        .join("run")
}

#[cfg(unix)]
pub fn socket_path() -> PathBuf {
    runtime_dir().join("fisgad.sock")
}

/// Fisga was called LDM: its folders (settings and logins, the download
/// list, the logs) take the new name the first time Fisga starts.
pub fn migrate_legacy_dirs() {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from);
    for (variable, default) in [("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"), ("XDG_STATE_HOME", ".local/state")] {
        let base = std::env::var_os(variable)
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(default)));
        let Some(base) = base else { continue };
        let (old, new) = (base.join("ldm"), base.join("fisga"));
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(&old, &new);
        }
    }
}

pub struct Connection {
    lines: Lines<BufReader<transport::ReadHalf>>,
    writer: transport::WriteHalf,
}

impl Connection {
    pub async fn open() -> anyhow::Result<Self> {
        let stream = transport::connect().await.context("o daemon fisgad não está rodando")?;
        let (reader, writer) = tokio::io::split(stream);
        Ok(Self { lines: BufReader::new(reader).lines(), writer })
    }

    pub async fn send(&mut self, request: &Request) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(request)?;
        line.push(b'\n');
        self.writer.write_all(&line).await?;
        Ok(())
    }

    /// Next message, or None when the daemon closed the connection.
    pub async fn recv(&mut self) -> anyhow::Result<Option<Response>> {
        match self.lines.next_line().await? {
            Some(line) => Ok(Some(serde_json::from_str(&line)?)),
            None => Ok(None),
        }
    }

    pub async fn call(&mut self, request: &Request) -> anyhow::Result<Response> {
        self.send(request).await?;
        match self.recv().await? {
            Some(Response::Error { message }) => bail!(message),
            Some(response) => Ok(response),
            None => bail!("o daemon fechou a conexão"),
        }
    }
}

/// One request, one response.
pub async fn call(request: &Request) -> anyhow::Result<Response> {
    Connection::open().await?.call(request).await
}

async fn is_running() -> bool {
    matches!(call(&Request::Ping).await, Ok(Response::Pong { .. }))
}

/// Path of a sibling executable (installed next to the current one), falling
/// back to a `$PATH` lookup.
pub fn sibling_executable(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Starts `fisgad` in its own session if it is not running, and waits for its socket.
pub async fn ensure_daemon() -> anyhow::Result<()> {
    if is_running().await {
        return Ok(());
    }
    let mut command = std::process::Command::new(sibling_executable("fisgad"));
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A new session keeps the daemon alive when the browser kills the
        // short-lived native host process (and its process group).
        // SAFETY: setsid is async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // No console window, and out of the browser's job object, which kills
        // what the native host started when it closes; a job that forbids
        // leaving it makes the first try fail.
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if command.spawn().is_err() {
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            command.spawn().context("não foi possível iniciar o fisgad")?;
        }
    }
    #[cfg(not(windows))]
    command.spawn().context("não foi possível iniciar o fisgad")?;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if is_running().await {
            return Ok(());
        }
    }
    bail!("o fisgad não respondeu após ser iniciado")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_request_defaults() {
        let add: AddRequest = serde_json::from_str(r#"{"url":"https://example.com/a.iso"}"#).unwrap();
        assert!(add.start);
        assert!(!add.confirm);
        assert_eq!(add.kind, SourceKind::Direct);
    }

    #[test]
    fn checksums_as_download_pages_write_them() {
        let sha256 = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        let parsed = Checksum::parse(&sha256.to_uppercase()).unwrap();
        assert_eq!(parsed, Checksum { algo: HashAlgo::Sha256, hex: sha256.into() });
        for text in [
            format!("sha256:{sha256}"),
            format!("SHA-256: {sha256}"),
            format!("{sha256}  ubuntu-24.04-desktop-amd64.iso"),
            format!("SHA256 (ubuntu.iso) = {sha256}"),
            format!("  {sha256}\n"),
        ] {
            assert_eq!(Checksum::parse(&text).unwrap(), parsed, "{text}");
        }
        let sha1 = Checksum::parse("SHA1: a94a8fe5ccb19ba61c4c0873d391e987982fbbd3").unwrap();
        assert_eq!(sha1.algo, HashAlgo::Sha1);
        assert_eq!(Checksum::parse(&"ab".repeat(64)).unwrap().algo, HashAlgo::Sha512);
        assert_eq!(Checksum::parse(&"cd".repeat(48)).unwrap().algo, HashAlgo::Sha384);

        // The name says SHA-256 but the digits are a SHA-1.
        let wrong = Checksum::parse("sha256: a94a8fe5ccb19ba61c4c0873d391e987982fbbd3").unwrap_err();
        assert!(wrong.contains("64"), "{wrong}");
        assert!(Checksum::parse("MD5: 098f6bcd4621d373cade4e832627b4f6").is_err());
        assert!(Checksum::parse("sha256: 9f86d08").unwrap_err().contains("64"));
        assert!(Checksum::parse("").is_err());

        // On the wire it is "algorithm:digits".
        let json = serde_json::to_string(&parsed).unwrap();
        assert_eq!(json, format!("\"sha256:{sha256}\""));
        assert_eq!(serde_json::from_str::<Checksum>(&json).unwrap(), parsed);
        assert!(serde_json::from_str::<Checksum>("\"nada\"").is_err());
    }

    #[test]
    fn file_map_of_segments() {
        // 800 bytes: stretches of 100. The first segment wrote 150 of 0..=399,
        // the second all of 400..=799.
        let map = file_map([(0, 399, 150), (400, 799, 400)], 800);
        assert_eq!(map, vec![100, 50, 0, 0, 100, 100, 100, 100]);
        assert_eq!(file_map([], 0), Vec::<u8>::new());
        // Fewer bytes than stretches: the empty stretches count as done.
        assert_eq!(file_map([(0, 2, 3)], 3).len(), MAP_PARTS);
        assert!(file_map([(0, 2, 3)], 3).iter().all(|p| *p == 100));
    }

    #[test]
    fn checksum_matches_only_a_finished_check() {
        let mut info: DownloadInfo = serde_json::from_str(
            r#"{"id":1,"url":"https://e.com/a","kind":"direct","name":"a","path":null,"state":"completed",
               "downloaded":1,"total":1,"speed":0,"connections":0,"resumable":true,"error":null,"createdAt":0}"#,
        )
        .unwrap();
        assert_eq!(info.checksum_matches(), None);
        let digest = Checksum::parse(&"ab".repeat(32)).unwrap();
        info.checksum = Some(digest.clone());
        info.hash = Some(Hashing::Running { done: 1 });
        assert_eq!(info.checksum_matches(), None);
        info.hash = Some(Hashing::Done { digest: digest.clone() });
        assert_eq!(info.checksum_matches(), Some(true));
        info.hash = Some(Hashing::Done { digest: Checksum::parse(&"cd".repeat(32)).unwrap() });
        assert_eq!(info.checksum_matches(), Some(false));
        let json = serde_json::to_string(&info.hash).unwrap();
        assert!(json.contains(r#""state":"done""#), "{json}");
    }

    #[test]
    fn subscribe_without_fields_is_not_a_confirmer() {
        let request: Request = serde_json::from_str(r#"{"type":"subscribe"}"#).unwrap();
        assert!(matches!(request, Request::Subscribe { confirmer: false }));
        let request: Request = serde_json::from_str(r#"{"type":"subscribe","confirmer":true}"#).unwrap();
        assert!(matches!(request, Request::Subscribe { confirmer: true }));
    }

    #[test]
    fn confirm_roundtrip() {
        let request = Request::Confirm {
            id: 7,
            now: true,
            file_name: Some("a.iso".into()),
            output_dir: Some("/tmp".into()),
            format: None,
            conflict: None,
            queue: None,
            checksum: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""type":"confirm""#), "{json}");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Confirm { id: 7, now: true, .. }));
        let minimal: Request = serde_json::from_str(r#"{"type":"confirm","id":3}"#).unwrap();
        assert!(matches!(
            minimal,
            Request::Confirm {
                id: 3,
                now: false,
                file_name: None,
                output_dir: None,
                format: None,
                conflict: None,
                queue: None,
                checksum: None
            }
        ));
        let full: Request =
            serde_json::from_str(r#"{"type":"confirm","id":3,"format":"720","conflict":"overwrite"}"#).unwrap();
        assert!(matches!(
            full,
            Request::Confirm { format: Some(ref f), conflict: Some(Conflict::Overwrite), .. } if f == "720"
        ));
    }

    #[test]
    fn existing_files_and_unique_names() {
        let dir = std::env::temp_dir().join(format!("fisga-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(existing(&dir.join("a.iso")), Existing::Nothing);
        assert_eq!(unique_name(&dir, "a.iso"), "a.iso");
        std::fs::write(dir.join("a.iso"), b"x").unwrap();
        assert_eq!(existing(&dir.join("a.iso")), Existing::File);
        assert_eq!(unique_name(&dir, "a.iso"), "a (1).iso");
        std::fs::write(dir.join("a (1).iso.fisga-part"), b"x").unwrap();
        assert_eq!(existing(&dir.join("a (1).iso")), Existing::Partial);
        assert_eq!(unique_name(&dir, "a.iso"), "a (2).iso");
        std::fs::write(dir.join("v.mp4.part"), b"x").unwrap();
        assert_eq!(existing(&dir.join("v.mp4")), Existing::Partial, "yt-dlp partial");
        assert_eq!(unique_name(&dir, "sem-extensao"), "sem-extensao");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn categories_from_names() {
        assert_eq!(Category::detect("filme.MKV", SourceKind::Direct), Category::Video);
        assert_eq!(Category::detect("musica.opus", SourceKind::Direct), Category::Music);
        assert_eq!(Category::detect("linux.iso", SourceKind::Direct), Category::Program);
        assert_eq!(Category::detect("app.AppImage", SourceKind::Direct), Category::Program);
        assert_eq!(Category::detect("src.tar.gz", SourceKind::Direct), Category::Compressed);
        assert_eq!(Category::detect("foto.jpeg", SourceKind::Direct), Category::Image);
        assert_eq!(Category::detect("livro.pdf", SourceKind::Direct), Category::Document);
        assert_eq!(Category::detect("download", SourceKind::Direct), Category::Other);
        // Site videos: video unless the name says audio.
        assert_eq!(Category::detect("Meu vídeo", SourceKind::Page), Category::Video);
        assert_eq!(Category::detect("podcast.m4a", SourceKind::Page), Category::Music);
        assert_eq!(Category::Other.folder(), None);
        assert_eq!(Category::Program.folder(), Some("Programas"));
    }

    #[test]
    fn settings_defaults_and_partial_json() {
        let settings: Settings = serde_json::from_str(r#"{"maxParallel": 50}"#).unwrap();
        assert!(settings.category_folders);
        assert!(settings.watch_clipboard);
        // A settings.json from before the window remembered its look: cards, newest on top.
        assert!(!settings.list_view && settings.newest_first);
        // And from before the themes: dark, with the holidays' effects.
        assert!(settings.theme == Theme::Dark && settings.festive);
        assert_eq!(serde_json::from_str::<Settings>(r#"{"theme":"light"}"#).unwrap().theme, Theme::Light);
        assert_eq!(settings.sanitized().max_parallel, 20);
        let request: Request = serde_json::from_str(r#"{"type":"set_settings","settings":{"connections":4}}"#).unwrap();
        assert!(matches!(request, Request::SetSettings { settings } if settings.connections == 4));
    }

    #[test]
    fn browser_file_types_and_force_key() {
        // A settings.json from before: the default types (models included) and Alt.
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.browser.file_types.iter().any(|t| t == "safetensors"));
        assert_eq!(settings.browser.force_key, ForceKey::Alt);
        assert_eq!(BrowserSettings::parse_file_types("MP4, .zip *.Safetensors;mp4  x/y"), ["mp4", "zip", "safetensors"]);
        let settings: Settings =
            serde_json::from_str(r#"{"browser":{"fileTypes":["GGUF",".pt"],"forceKey":"ctrl_alt"}}"#).unwrap();
        assert_eq!(settings.browser.force_key, ForceKey::CtrlAlt);
        assert_eq!(settings.sanitized().browser.file_types, ["gguf", "pt"]);
    }

    #[test]
    fn times_of_day() {
        assert_eq!(TimeOfDay::parse("2:05").map(TimeOfDay::minutes), Some(125));
        assert_eq!(TimeOfDay::parse(" 23:59 ").map(|t| t.to_string()).as_deref(), Some("23:59"));
        for bad in ["24:00", "12:60", "1200", "12:5", ":30", "aa:bb", "123:00"] {
            assert_eq!(TimeOfDay::parse(bad), None, "{bad}");
        }
        let schedule: Schedule = serde_json::from_str(r#"{"enabled":true,"start":"23:00","stop":"06:30"}"#).unwrap();
        assert_eq!(schedule.stop, TimeOfDay::new(6, 30));
        assert_eq!(schedule.days.len(), 7, "every day by default");
        assert!(serde_json::to_string(&schedule).unwrap().contains(r#""start":"23:00""#));
        assert!(serde_json::from_str::<Schedule>(r#"{"start":"25:00"}"#).is_err());
        // settings.json written before the scheduler existed still loads.
        let settings: Settings = serde_json::from_str(r#"{"connections":4}"#).unwrap();
        assert!(!settings.schedule.enabled && settings.tray);
        let request: Request = serde_json::from_str(r#"{"type":"cancel_power"}"#).unwrap();
        assert!(matches!(request, Request::CancelPower));
    }

    fn at(day: i64, weekday: u8, hour: u16, minute: u16) -> LocalTime {
        LocalTime { day, weekday, minutes: hour * 60 + minute }
    }

    #[test]
    fn schedule_windows() {
        let time = |text| TimeOfDay::parse(text).unwrap();
        // Weekdays only, 09:00 to 17:00. Day 10 is a Monday (weekday 1).
        let office = Schedule { enabled: true, start: time("09:00"), stop: Some(time("17:00")), days: (1..6).collect(), ..Default::default() };
        assert_eq!(office.window_at(at(10, 1, 8, 59)), None);
        assert_eq!(office.window_at(at(10, 1, 9, 0)), Some(10));
        assert_eq!(office.window_at(at(10, 1, 16, 59)), Some(10));
        assert_eq!(office.window_at(at(10, 1, 17, 0)), None);
        assert_eq!(office.window_at(at(9, 0, 12, 0)), None, "not on Sunday");
        assert_eq!(office.describe(), "seg, ter, qua, qui, sex, das 09:00 às 17:00");

        // Across midnight: Friday 23:00 to Saturday 06:00 belongs to Friday.
        let night = Schedule { start: time("23:00"), stop: Some(time("06:00")), days: vec![5], ..office.clone() };
        assert_eq!(night.window_at(at(14, 5, 23, 30)), Some(14));
        assert_eq!(night.window_at(at(15, 6, 5, 59)), Some(14));
        assert_eq!(night.window_at(at(15, 6, 6, 0)), None);
        assert_eq!(night.window_at(at(15, 6, 23, 30)), None, "Saturday is not a start day");

        // Without a stop time the window lasts until the next start.
        let open_ended = Schedule { start: time("02:00"), stop: None, days: (0..7).collect(), ..office };
        assert_eq!(open_ended.window_at(at(20, 3, 1, 59)), Some(19));
        assert_eq!(open_ended.window_at(at(20, 3, 2, 0)), Some(20));
        assert_eq!(open_ended.describe(), "todos os dias, às 02:00 até terminar a fila");
        let now = LocalTime::now();
        assert!(now.weekday < 7 && now.minutes < 24 * 60);
    }

    #[test]
    fn queues() {
        let mut settings = Settings::default();
        assert_eq!(settings.all_queues().len(), 1);
        assert_eq!(settings.queue_name(MAIN_QUEUE), MAIN_QUEUE_NAME);
        let id = settings.new_queue_id();
        assert_eq!(id, 1);
        settings.set_queue(Queue { id, name: " Noite ".into(), max_parallel: 99, ..Queue::default() });
        settings.set_queue(Queue { id: MAIN_QUEUE, max_parallel: 5, ..Queue::default() });
        // Duplicates and a fake "main" are dropped; empty names get one.
        settings.queues.push(Queue { id, name: "outra".into(), ..Queue::default() });
        settings.queues.push(Queue { id: MAIN_QUEUE, ..Queue::default() });
        settings.queues.push(Queue { id: 7, name: "  ".into(), ..Queue::default() });
        let settings = settings.sanitized();
        assert_eq!(settings.max_parallel, 5, "the main queue's limit is the old setting");
        let names: Vec<String> = settings.all_queues().into_iter().map(|q| q.name).collect();
        assert_eq!(names, [MAIN_QUEUE_NAME, "Noite", "Fila 7"]);
        assert_eq!(settings.queue(id).unwrap().max_parallel, 20);
        assert_eq!(settings.new_queue_id(), 8);
        let mut settings = settings;
        settings.remove_queue(id);
        settings.remove_queue(MAIN_QUEUE);
        assert_eq!(settings.all_queues().len(), 2);
        settings.remove_queue(7);
        assert_eq!(settings.new_queue_id(), 8, "the id of a removed queue is not reused");
        assert!(WhenDone::Poweroff > WhenDone::Suspend && WhenDone::Suspend > WhenDone::Nothing);
        let request: Request = serde_json::from_str(r#"{"type":"reorder","id":4,"direction":"top"}"#).unwrap();
        assert!(matches!(request, Request::Reorder { id: 4, direction: Direction::Top }));
    }

    #[test]
    fn proxy_settings() {
        let manual = |url: &str| ProxySettings { mode: ProxyMode::Manual, url: url.into() };
        assert!(manual("http://proxy.local:3128").check().is_ok());
        assert!(manual("socks5h://ana:senha@10.0.0.1:1080").check().is_ok());
        assert!(manual("ftp://proxy:21").check().is_err());
        assert!(manual("proxy.local:3128").check().is_err(), "needs the scheme");
        assert!(ProxySettings { mode: ProxyMode::System, url: "lixo".into() }.check().is_ok(), "unused url");
        let secret = manual("socks5://ana:senha@host:1080");
        assert!(!format!("{secret:?}").contains("senha"), "{secret:?}");
        assert!(!secret.describe().contains("senha"));
        assert_eq!(secret.for_ytdlp().as_deref(), Some("socks5://ana:senha@host:1080"));
        assert_eq!(ProxySettings { mode: ProxyMode::None, url: String::new() }.for_ytdlp().as_deref(), Some(""));
        assert_eq!(ProxySettings::default().for_ytdlp(), None);
        // settings.json written before the proxy existed still loads.
        let settings: Settings = serde_json::from_str(r#"{"connections":4}"#).unwrap();
        assert_eq!(settings.proxy.mode, ProxyMode::System);
    }

    #[test]
    fn site_logins() {
        assert_eq!(SiteLogin::normalize_host("https://Arquivos.Exemplo.com:8443/pasta"), "arquivos.exemplo.com");
        assert_eq!(SiteLogin::normalize_host(" *.Exemplo.com "), "*.exemplo.com");
        assert_eq!(SiteLogin::normalize_host("exemplo.com/x"), "exemplo.com");
        let login = |host: &str| SiteLogin { host: host.into(), user: "ana".into(), password: "segredo".into() };
        let logins = [login("*.exemplo.com"), login("cdn.exemplo.com"), login("127.0.0.1")];
        let found = |url: &str| SiteLogin::find(&logins, url).map(|login| login.host.as_str());
        assert_eq!(found("https://cdn.exemplo.com/a.iso"), Some("cdn.exemplo.com"), "the exact host wins");
        assert_eq!(found("https://www.EXEMPLO.com/a.iso"), Some("*.exemplo.com"));
        assert_eq!(found("https://exemplo.com/a.iso"), Some("*.exemplo.com"));
        assert_eq!(found("https://outroexemplo.com/a.iso"), None);
        assert_eq!(found("http://cdn.exemplo.com/a.iso"), None, "never over plain http");
        assert_eq!(found("http://127.0.0.1:8080/a.iso"), Some("127.0.0.1"), "this computer is fine");
        let exact = [login("arquivos.exemplo.com")];
        assert!(SiteLogin::find(&exact, "https://sub.arquivos.exemplo.com/a").is_none(), "subdomains need *.");
        assert!(!format!("{:?}", logins[0]).contains("segredo"));
    }

    #[test]
    fn batch_patterns() {
        let urls = expand_batch("https://e.com/aula_*.mp4", "1", "3", 0).unwrap();
        assert_eq!(urls, ["https://e.com/aula_1.mp4", "https://e.com/aula_2.mp4", "https://e.com/aula_3.mp4"]);
        let padded = expand_batch("https://e.com/*.jpg", "08", "10", 0).unwrap();
        assert_eq!(padded, ["https://e.com/08.jpg", "https://e.com/09.jpg", "https://e.com/10.jpg"]);
        assert_eq!(expand_batch("https://e.com/*.jpg", "9", "10", 3).unwrap()[0], "https://e.com/009.jpg");
        assert_eq!(expand_batch("https://e.com/parte-*.rar", "a", "c", 0).unwrap().len(), 3);
        assert_eq!(expand_batch("https://e.com/*.rar", "X", "Z", 0).unwrap()[2], "https://e.com/Z.rar");
        for (template, from, to) in [
            ("https://e.com/a.iso", "1", "2"),
            ("https://e.com/*/*.iso", "1", "2"),
            ("https://e.com/*.iso", "5", "2"),
            ("https://e.com/*.iso", "a", "Z"),
            ("https://e.com/*.iso", "1", "z"),
            ("https://e.com/*.iso", "1", "999999"),
        ] {
            assert!(expand_batch(template, from, to, 0).is_err(), "{template} {from}..{to}");
        }
    }

    #[test]
    fn grabber_filters() {
        assert!(glob_matches("*.pdf", "Apostila.PDF"));
        assert!(glob_matches("aula_??.mp4", "aula_07.mp4"));
        assert!(!glob_matches("aula_??.mp4", "aula_7.mp4"));
        assert!(glob_matches("*parte*", "livro-parte-2.zip"));
        assert!(glob_matches("*", "qualquer"));
        assert!(!glob_matches("*.zip", "a.zip.part"));
        let link = |name: &str, category| GrabbedLink { url: format!("https://e.com/{name}"), name: name.into(), category };
        let pdf = link("apostila.pdf", Category::Document);
        let iso = link("debian.iso", Category::Program);
        assert!(grab_filter(&pdf, &[], ""));
        assert!(grab_filter(&pdf, &[Category::Document], ""));
        assert!(!grab_filter(&iso, &[Category::Document], ""));
        assert!(grab_filter(&iso, &[], "*.pdf, *.iso"));
        assert!(!grab_filter(&iso, &[], "*.pdf"));
        assert!(!grab_filter(&pdf, &[Category::Program], "*.pdf"), "both filters apply");
    }

    #[test]
    fn url_lists() {
        let text = "Baixe <https://e.com/a.iso>, depois\nhttps://e.com/b.zip\n\nhttps://e.com/a.iso (de novo)\nFTP://e.com/c.tar ou e.com/d.iso";
        assert_eq!(parse_url_list(text), ["https://e.com/a.iso", "https://e.com/b.zip", "FTP://e.com/c.tar"]);
        assert!(parse_url_list("nada aqui").is_empty());
        let request: Request = serde_json::from_str(r#"{"type":"add_batch","urls":["https://e.com/a"]}"#).unwrap();
        assert!(matches!(request, Request::AddBatch { start: true, queue: None, .. }));
    }

    #[test]
    fn sanitized_days() {
        let mut settings = Settings::default();
        settings.schedule.days = vec![6, 1, 9, 1];
        assert_eq!(settings.sanitized().schedule.days, vec![1, 6]);
    }

    #[test]
    fn pending_state_and_old_download_info() {
        assert_eq!(serde_json::to_string(&State::Pending).unwrap(), r#""pending""#);
        // downloads.json written before `dir`/`probing` existed still loads.
        let info: DownloadInfo = serde_json::from_str(
            r#"{"id":1,"url":"https://e.com/a","kind":"direct","name":"a","path":null,"state":"paused",
                "downloaded":0,"total":null,"speed":0.0,"connections":0,"resumable":false,"error":null,"createdAt":1}"#,
        )
        .unwrap();
        assert_eq!(info.dir, None);
        assert!(!info.probing);
        assert_eq!(info.category, Category::Other);
        assert!(info.formats.is_empty());
        assert_eq!((info.queue, info.order), (MAIN_QUEUE, 0));
        assert!(!info.awaiting_url && info.page.is_none());
        assert_eq!(info.speed_limit, 0);
        assert_eq!(combined_limit(0, 0), 0);
        assert_eq!(combined_limit(1000, 0), 1000);
        assert_eq!(combined_limit(0, 500), 500);
        assert_eq!(combined_limit(1000, 500), 500);
        assert_eq!(combined_limit(300, 500), 300);
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("awaitingUrl") && !json.contains("page") && !json.contains("speedLimit"), "{json}");
        let request: Request =
            serde_json::from_str(r#"{"type":"refresh_url","id":2,"url":"https://e.com/novo"}"#).unwrap();
        assert!(matches!(request, Request::RefreshUrl { id: 2, .. }));
        assert!(!serde_json::to_string(&info).unwrap().contains("formats"), "empty list not written");
    }
}
