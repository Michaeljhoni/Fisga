//! The local connection between `fisgad` and its clients. On Linux and macOS
//! it is the Unix socket `fisgad.sock` in [`runtime_dir`], a folder only the
//! user can open. On Windows it is a named pipe; pipe names are visible to
//! every user of the machine, so the name is random and kept in the file
//! `pipe` of the runtime folder (in `%LOCALAPPDATA%`, private to the user):
//! another user can neither reach the daemon nor create the pipe first to
//! impersonate it.

use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(unix)]
use crate::socket_path;
use crate::runtime_dir;

/// A connected stream, the same type on both sides and on every system.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Stream for T {}

pub type BoxStream = Box<dyn Stream>;
pub type ReadHalf = tokio::io::ReadHalf<BoxStream>;
pub type WriteHalf = tokio::io::WriteHalf<BoxStream>;

#[cfg(unix)]
pub async fn connect() -> io::Result<BoxStream> {
    Ok(Box::new(tokio::net::UnixStream::connect(socket_path()).await?))
}

#[cfg(windows)]
pub async fn connect() -> io::Result<BoxStream> {
    use tokio::net::windows::named_pipe::ClientOptions;
    /// Every instance is taken: the daemon is about to create the next one.
    const ERROR_PIPE_BUSY: i32 = 231;
    let name = std::fs::read_to_string(pipe_file())?;
    for _ in 0..40 {
        match ClientOptions::new().open(name.trim()) {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            result => return Ok(Box::new(result?)),
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "o fisgad está ocupado"))
}

#[cfg(windows)]
fn pipe_file() -> std::path::PathBuf {
    runtime_dir().join("pipe")
}

/// The daemon's end.
pub struct Listener {
    #[cfg(unix)]
    listener: tokio::net::UnixListener,
    #[cfg(windows)]
    server: tokio::net::windows::named_pipe::NamedPipeServer,
    #[cfg(windows)]
    name: String,
}

impl Listener {
    /// Starts listening, or `None` when another daemon already answers.
    #[cfg(unix)]
    pub async fn bind() -> io::Result<Option<Self>> {
        use std::os::unix::fs::PermissionsExt;
        let dir = runtime_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        if connect().await.is_ok() {
            return Ok(None);
        }
        let socket = socket_path();
        let _ = std::fs::remove_file(&socket);
        let listener = tokio::net::UnixListener::bind(&socket)
            .map_err(|e| io::Error::new(e.kind(), format!("bind {}: {e}", socket.display())))?;
        Ok(Some(Self { listener }))
    }

    #[cfg(windows)]
    pub async fn bind() -> io::Result<Option<Self>> {
        use tokio::net::windows::named_pipe::ServerOptions;
        std::fs::create_dir_all(runtime_dir())?;
        if connect().await.is_ok() {
            return Ok(None);
        }
        let name = format!(r"\\.\pipe\fisga-{}", random_hex());
        // first_pipe_instance: fails rather than join a pipe someone else made.
        let server = ServerOptions::new().first_pipe_instance(true).create(&name)?;
        let tmp = pipe_file().with_extension("tmp");
        std::fs::write(&tmp, &name)?;
        std::fs::rename(&tmp, pipe_file())?;
        Ok(Some(Self { server, name }))
    }

    #[cfg(unix)]
    pub async fn accept(&mut self) -> io::Result<BoxStream> {
        let (stream, _) = self.listener.accept().await?;
        Ok(Box::new(stream))
    }

    /// The connected instance goes to the client; a new one waits for the next.
    #[cfg(windows)]
    pub async fn accept(&mut self) -> io::Result<BoxStream> {
        use tokio::net::windows::named_pipe::ServerOptions;
        self.server.connect().await?;
        let next = ServerOptions::new().create(&self.name)?;
        Ok(Box::new(std::mem::replace(&mut self.server, next)))
    }

    /// Where the clients find it, for the log.
    #[cfg(unix)]
    pub fn address(&self) -> String {
        socket_path().display().to_string()
    }

    #[cfg(windows)]
    pub fn address(&self) -> String {
        self.name.clone()
    }

    /// Removes the socket (or the pipe's name), so clients see no daemon.
    pub fn remove(self) {
        #[cfg(unix)]
        let _ = std::fs::remove_file(socket_path());
        #[cfg(windows)]
        let _ = std::fs::remove_file(pipe_file());
    }
}

/// 128 unpredictable bits: std's hasher keys come from the system's random source.
#[cfg(windows)]
fn random_hex() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..2)
        .map(|_| {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            format!("{:016x}", hasher.finish())
        })
        .collect()
}
