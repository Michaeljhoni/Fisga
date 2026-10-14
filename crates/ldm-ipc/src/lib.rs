//! Protocol between `ldmd` (daemon) and its clients: one JSON object per line over
//! the Unix socket `$XDG_RUNTIME_DIR/ldm/ldmd.sock`.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::OwnedReadHalf;
use tokio::net::UnixStream;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Native messaging host name registered in the browsers.
pub const NATIVE_HOST_NAME: &str = "io.ldm.native";
/// Firefox extension id (`browser_specific_settings.gecko.id`).
pub const FIREFOX_EXTENSION_ID: &str = "ldm@ldm.local";
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
}

fn yes() -> bool {
    true
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
        /// (the GUI). While one is connected, the daemon does not launch `ldm-gui`.
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
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong { version: String },
    Added { id: u64 },
    Downloads { items: Vec<DownloadInfo> },
    Update { item: DownloadInfo },
    Removed { id: u64 },
    Ok,
    Error { message: String },
}

/// `$XDG_RUNTIME_DIR/ldm`, or `/tmp/ldm-<uid>` when the variable is unset.
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("ldm"),
        // SAFETY: getuid has no preconditions and cannot fail.
        _ => std::env::temp_dir().join(format!("ldm-{}", unsafe { libc::getuid() })),
    }
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("ldmd.sock")
}

pub struct Connection {
    lines: Lines<BufReader<OwnedReadHalf>>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl Connection {
    pub async fn open() -> anyhow::Result<Self> {
        let stream = UnixStream::connect(socket_path())
            .await
            .context("o daemon ldmd não está rodando")?;
        let (reader, writer) = stream.into_split();
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
        .map(|exe| exe.with_file_name(name))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Starts `ldmd` in its own session if it is not running, and waits for its socket.
pub async fn ensure_daemon() -> anyhow::Result<()> {
    if is_running().await {
        return Ok(());
    }
    let mut command = std::process::Command::new(sibling_executable("ldmd"));
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
    command.spawn().context("não foi possível iniciar o ldmd")?;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if is_running().await {
            return Ok(());
        }
    }
    bail!("o ldmd não respondeu após ser iniciado")
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
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""type":"confirm""#), "{json}");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Confirm { id: 7, now: true, .. }));
        let minimal: Request = serde_json::from_str(r#"{"type":"confirm","id":3}"#).unwrap();
        assert!(matches!(minimal, Request::Confirm { id: 3, now: false, file_name: None, output_dir: None }));
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
    }
}
