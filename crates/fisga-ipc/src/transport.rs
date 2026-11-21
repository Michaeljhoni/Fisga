//! The local connection between `fisgad` and its clients. On Linux and macOS
//! it is the Unix socket `fisgad.sock` in [`runtime_dir`], a folder only the
//! user can open. On Windows it is a named pipe; pipe names are visible to
//! every user of the machine, so the name is random and kept in the file
//! `pipe` of the runtime folder (in `%LOCALAPPDATA%`, private to the user).
//! Only the user can open the pipe, and both ends check that the other one's
//! server runs as the user: a name left from an earlier session may by then
//! be another user's pipe.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::runtime_dir;
#[cfg(unix)]
use crate::socket_path;

/// A connected stream, the same type on both sides and on every system.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Stream for T {}

pub type BoxStream = Box<dyn Stream>;
pub type ReadHalf = tokio::io::ReadHalf<BoxStream>;
pub type WriteHalf = tokio::io::WriteHalf<BoxStream>;

#[cfg(unix)]
pub async fn connect() -> io::Result<BoxStream> {
    check_private(&runtime_dir())?;
    Ok(Box::new(tokio::net::UnixStream::connect(socket_path()).await?))
}

/// The runtime folder must be a folder (not a link to one), the user's, and
/// closed to everyone else: in the shared temporary folder (with no
/// `$XDG_RUNTIME_DIR`) anyone could have made it first.
#[cfg(unix)]
fn check_private(dir: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let ours = meta.uid() == unsafe { libc::getuid() };
    if meta.file_type().is_dir() && ours && meta.mode() & 0o777 == 0o700 {
        return Ok(());
    }
    let message = fisga_i18n::trf!("{} não é uma pasta só deste usuário (dono e permissão 700)", dir.display());
    Err(io::Error::new(io::ErrorKind::PermissionDenied, message))
}

#[cfg(windows)]
pub async fn connect() -> io::Result<BoxStream> {
    use std::os::windows::io::AsRawHandle;
    use tokio::net::windows::named_pipe::ClientOptions;
    /// Every instance is taken: the daemon is about to create the next one.
    const ERROR_PIPE_BUSY: i32 = 231;
    let name = std::fs::read_to_string(pipe_file())?;
    for _ in 0..40 {
        match ClientOptions::new().open(name.trim()) {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(e),
            Ok(client) if win::served_by_user(client.as_raw_handle()) => return Ok(Box::new(client)),
            Ok(_) => {
                let message = fisga_i18n::tr!("o pipe do fisgad pertence a outro usuário");
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, message));
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, fisga_i18n::tr!("o fisgad está ocupado")))
}

#[cfg(windows)]
fn pipe_file() -> std::path::PathBuf {
    runtime_dir().join("pipe")
}

/// A pipe instance only this user can open, and only from this computer.
/// `first` fails rather than join a pipe someone else made.
#[cfg(windows)]
fn create_instance(name: &str, first: bool) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    let descriptor = win::user_only()?;
    let mut attributes = win::SecurityAttributes {
        length: std::mem::size_of::<win::SecurityAttributes>() as u32,
        descriptor: descriptor.0,
        inherit: 0,
    };
    let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
    options.reject_remote_clients(true).first_pipe_instance(first);
    // SAFETY: a SECURITY_ATTRIBUTES whose descriptor lives until the call returns.
    unsafe { options.create_with_security_attributes_raw(name, std::ptr::addr_of_mut!(attributes).cast()) }
}

/// The daemon's end.
pub struct Listener {
    #[cfg(unix)]
    listener: tokio::net::UnixListener,
    #[cfg(windows)]
    server: tokio::net::windows::named_pipe::NamedPipeServer,
    #[cfg(windows)]
    name: String,
    /// Accepts that failed in a row.
    failures: u32,
}

impl Listener {
    /// Starts listening, or `None` when another daemon already answers.
    #[cfg(unix)]
    pub async fn bind() -> io::Result<Option<Self>> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let dir = runtime_dir();
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            // The umask may have taken more than the group's and others' bits.
            Ok(()) => std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        check_private(&dir)?;
        if connect().await.is_ok() {
            return Ok(None);
        }
        let socket = socket_path();
        let _ = std::fs::remove_file(&socket);
        let listener = tokio::net::UnixListener::bind(&socket)
            .map_err(|e| io::Error::new(e.kind(), format!("bind {}: {e}", socket.display())))?;
        Ok(Some(Self { listener, failures: 0 }))
    }

    /// Another user's pipe under the name kept from before does not count: a
    /// new name replaces it.
    #[cfg(windows)]
    pub async fn bind() -> io::Result<Option<Self>> {
        std::fs::create_dir_all(runtime_dir())?;
        if connect().await.is_ok() {
            return Ok(None);
        }
        let name = format!(r"\\.\pipe\fisga-{}", random_hex());
        let server = create_instance(&name, true)?;
        let tmp = pipe_file().with_extension("tmp");
        std::fs::write(&tmp, &name)?;
        std::fs::rename(&tmp, pipe_file())?;
        Ok(Some(Self { server, name, failures: 0 }))
    }

    /// The next client. After a failure the next try waits, longer the more
    /// failures in a row, so a lasting error does not spin.
    pub async fn accept(&mut self) -> io::Result<BoxStream> {
        if self.failures > 0 {
            tokio::time::sleep(backoff(self.failures)).await;
        }
        let accepted = self.accept_one().await;
        self.failures = if accepted.is_ok() { 0 } else { self.failures.saturating_add(1) };
        accepted
    }

    #[cfg(unix)]
    async fn accept_one(&mut self) -> io::Result<BoxStream> {
        let (stream, _) = self.listener.accept().await?;
        Ok(Box::new(stream))
    }

    /// The connected instance goes to the client and a new one waits for the
    /// next; a failed one is replaced too, as it cannot connect again. The new
    /// one comes first, so the name never goes free for someone else to take.
    #[cfg(windows)]
    async fn accept_one(&mut self) -> io::Result<BoxStream> {
        let connected = self.server.connect().await;
        let next = create_instance(&self.name, false)?;
        let server = std::mem::replace(&mut self.server, next);
        connected?;
        Ok(Box::new(server))
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

/// The wait before the next accept: 100 ms per failure in a row, up to 2 s.
fn backoff(failures: u32) -> Duration {
    Duration::from_millis(100 * u64::from(failures.min(20)))
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

/// The Windows calls for the pipe's security, declared here like the rest of
/// Fisga's (kernel32 and advapi32 come with every Windows).
#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::io;
    use std::ptr::null_mut;

    type Handle = *mut c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const TOKEN_QUERY: u32 = 0x0008;
    /// `TOKEN_INFORMATION_CLASS::TokenUser`.
    const TOKEN_USER: i32 = 1;
    const SDDL_REVISION_1: u32 = 1;

    /// `SECURITY_ATTRIBUTES`.
    #[repr(C)]
    pub struct SecurityAttributes {
        pub length: u32,
        pub descriptor: *mut c_void,
        pub inherit: i32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetNamedPipeServerProcessId(pipe: Handle, process_id: *mut u32) -> i32;
        fn GetCurrentProcess() -> Handle;
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn GetTokenInformation(token: Handle, class: i32, information: *mut c_void, length: u32, needed: *mut u32) -> i32;
        fn EqualSid(a: *mut c_void, b: *mut c_void) -> i32;
        fn ConvertSidToStringSidW(sid: *mut c_void, text: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
    }

    /// Memory Windows allocated, given back when dropped.
    pub struct Local(pub *mut c_void);

    impl Drop for Local {
        fn drop(&mut self) {
            // SAFETY: the pointer came from a call that allocates with LocalAlloc.
            unsafe { LocalFree(self.0) };
        }
    }

    /// A process's `TOKEN_USER`, which holds the SID of the user it runs as.
    /// In `u64`s: the structure starts with a pointer, so is aligned like one.
    fn token_user(process: Handle) -> Option<Vec<u64>> {
        let mut token = null_mut();
        // SAFETY: valid out-pointers and lengths; the token is closed here.
        unsafe {
            if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let mut size = 0;
            GetTokenInformation(token, TOKEN_USER, null_mut(), 0, &mut size);
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let read = GetTokenInformation(token, TOKEN_USER, buffer.as_mut_ptr().cast(), size, &mut size) != 0;
            CloseHandle(token);
            read.then_some(buffer)
        }
    }

    /// The SID of a `TOKEN_USER`: its first field, pointing into the same buffer.
    fn sid(token_user: &[u64]) -> *mut c_void {
        // SAFETY: the buffer holds a TOKEN_USER and is aligned for a pointer.
        unsafe { token_user.as_ptr().cast::<*mut c_void>().read() }
    }

    /// The process serving `pipe` runs as this process's user.
    pub fn served_by_user(pipe: Handle) -> bool {
        let mut process_id = 0;
        // SAFETY: an open pipe handle and valid out-pointers; the process
        // handle is closed here.
        unsafe {
            if GetNamedPipeServerProcessId(pipe, &mut process_id) == 0 {
                return false;
            }
            let Some(ours) = token_user(GetCurrentProcess()) else { return false };
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
            if process.is_null() {
                return false;
            }
            let theirs = token_user(process);
            CloseHandle(process);
            theirs.is_some_and(|theirs| EqualSid(sid(&ours), sid(&theirs)) != 0)
        }
    }

    /// A security descriptor that lets only this process's user in: a
    /// protected DACL (nothing inherited) with full access for its SID alone.
    pub fn user_only() -> io::Result<Local> {
        // SAFETY: the pseudo handle of this process needs no closing.
        let user = token_user(unsafe { GetCurrentProcess() }).ok_or_else(io::Error::last_os_error)?;
        let mut text: *mut u16 = null_mut();
        // SAFETY: a SID from the token and a valid out-pointer.
        if unsafe { ConvertSidToStringSidW(sid(&user), &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let text = Local(text.cast());
        // SAFETY: Windows wrote a NUL-terminated UTF-16 string there.
        let sid_text = unsafe {
            let start: *const u16 = text.0.cast();
            let len = (0..).take_while(|&i| *start.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(start, len))
        };
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid_text})").encode_utf16().chain([0]).collect();
        let mut descriptor = null_mut();
        // SAFETY: a NUL-terminated string and a valid out-pointer.
        let made = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, null_mut())
        };
        if made == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Local(descriptor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_backs_off_after_failures() {
        assert_eq!(backoff(1), Duration::from_millis(100));
        assert_eq!(backoff(5), Duration::from_millis(500));
        assert_eq!(backoff(1000), Duration::from_secs(2));
    }

    /// The runtime folder: the user's own, mode 700, and not through a link.
    #[cfg(unix)]
    #[test]
    fn runtime_folder_must_be_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("fisga-private-{}", std::process::id()));
        let dir = root.join("run");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(check_private(&dir).is_ok());
        let link = root.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert_eq!(check_private(&link).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(check_private(&dir).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        let file = root.join("file");
        std::fs::write(&file, b"").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(check_private(&file).is_err(), "a file is not a folder");
        assert_eq!(check_private(&root.join("none")).unwrap_err().kind(), io::ErrorKind::NotFound);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The pipe's security, through the real Windows calls: this user's
    /// client sees its own server, and a second first instance is refused.
    #[cfg(windows)]
    #[tokio::test]
    async fn pipe_served_by_this_user() {
        use std::os::windows::io::AsRawHandle;
        use tokio::net::windows::named_pipe::ClientOptions;
        let name = format!(r"\\.\pipe\fisga-test-{}", random_hex());
        let server = create_instance(&name, true).unwrap();
        let client = ClientOptions::new().open(&name).unwrap();
        server.connect().await.unwrap();
        assert!(win::served_by_user(client.as_raw_handle()));
        assert!(create_instance(&name, true).is_err(), "the name is taken");
        let next = create_instance(&name, false).unwrap();
        drop(next);
    }
}
