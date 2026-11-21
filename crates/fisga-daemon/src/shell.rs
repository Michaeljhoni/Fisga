//! The programs the daemon starts. The user's commands (on completion, the
//! power command, the clipboard watcher) and the tests' stand-ins run in the
//! system's shell: `sh -c` on Linux and macOS, `cmd /C` on Windows.
//!
//! On Windows the daemon has no console, so each console program it starts
//! (yt-dlp, cmd, PowerShell...) would open a window of its own: they all get
//! `CREATE_NO_WINDOW`.

use tokio::process::{Child, Command};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How long a group asked to stop has before it is killed.
#[cfg(unix)]
const GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// A program and everything it starts, stopped together: yt-dlp runs
/// ffmpeg, which killing yt-dlp alone leaves running (and on Windows
/// yt-dlp.exe is a launcher whose own child does the work). On Unix it is
/// the program's own process group; on Windows, a job object.
pub struct Group {
    #[cfg(unix)]
    leader: Option<u32>,
    #[cfg(windows)]
    job: Option<Job>,
}

impl Group {
    /// For `command`, before it starts.
    pub fn prepare(command: &mut Command) {
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(not(unix))]
        let _ = command;
    }

    /// The group of `child`, just started from a prepared command.
    pub fn of(child: &Child) -> Group {
        #[cfg(not(any(unix, windows)))]
        let _ = child;
        Group {
            #[cfg(unix)]
            leader: child.id(),
            #[cfg(windows)]
            job: Job::assign(child),
        }
    }

    /// Asks the whole group to stop (SIGINT, as Ctrl+C: yt-dlp keeps its
    /// partial files), then kills what is left after a moment.
    pub async fn stop(&mut self, child: &mut Child) {
        #[cfg(unix)]
        if let Some(leader) = self.leader.take().and_then(|pid| i32::try_from(pid).ok()) {
            // SAFETY: kill() only sends a signal, to the group this child leads.
            unsafe { libc::kill(-leader, libc::SIGINT) };
            let _ = tokio::time::timeout(GRACE, child.wait()).await;
            // SAFETY: as above.
            unsafe { libc::kill(-leader, libc::SIGKILL) };
        }
        let _ = child.kill().await;
        // Closing the job ends what is left in it.
        #[cfg(windows)]
        drop(self.job.take());
    }
}

/// A job object that ends its processes when it is closed.
#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a job handle is not tied to the thread that opened it.
#[cfg(windows)]
unsafe impl Send for Job {}

#[cfg(windows)]
impl Job {
    /// A new job holding `child` and what it starts from now on; `None`
    /// when Windows refuses (then only `child` is killed).
    fn assign(child: &Child) -> Option<Job> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        let process = child.raw_handle()?;
        // SAFETY: plain Win32 calls on the job's own handle and the live
        // child's; `limits` outlives the call that reads it.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let job = Job(job);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of_val(&limits) as u32;
            let info = std::ptr::from_ref(&limits).cast::<std::ffi::c_void>();
            if SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, info, size) == 0
                || AssignProcessToJobObject(job.0, process) == 0
            {
                return None;
            }
            Some(job)
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle is this job's own, closed once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// `program`, without a console window on Windows.
pub fn program(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(unix)]
pub fn command(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

#[cfg(windows)]
pub fn command(script: &str) -> Command {
    let mut command = program("cmd");
    // cmd reads its command line as it is: the usual argument quoting would
    // break the script's own quotes.
    command.arg("/C").raw_arg(script);
    command
}

/// A PowerShell script, run as given: `-EncodedCommand` (UTF-16LE in base64)
/// needs no quoting. Values from outside (file names...) go in environment
/// variables, never into the script's text.
#[cfg(windows)]
pub fn powershell(script: &str) -> Command {
    let mut command = program("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"]);
    command.arg(base64_utf16(script));
    command
}

#[cfg(any(windows, test))]
fn base64_utf16(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            match i <= chunk.len() {
                true => out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char),
                false => out.push('='),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[tokio::test]
    async fn stopping_a_group_stops_what_it_started() {
        use super::*;
        use std::time::Duration;
        let beat = std::env::temp_dir().join(format!("fisgad-group-{}", std::process::id()));
        let _ = std::fs::remove_file(&beat);
        // A program that starts another one, as yt-dlp starts ffmpeg; the
        // other one writes a count while it runs.
        let mut command = Command::new("sh");
        command.arg("-c").arg(r#"(i=0; while :; do i=$((i+1)); echo $i > "$1"; sleep 0.05; done) & wait"#);
        command.arg("sh").arg(&beat);
        Group::prepare(&mut command);
        let mut child = command.spawn().unwrap();
        let mut group = Group::of(&child);
        for _ in 0..100 {
            if beat.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let started = std::time::Instant::now();
        group.stop(&mut child).await;
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        tokio::time::sleep(Duration::from_millis(200)).await;
        let count = std::fs::read_to_string(&beat).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(std::fs::read_to_string(&beat).unwrap(), count, "what it started still runs");
        let _ = std::fs::remove_file(&beat);
    }

    #[test]
    fn powershell_encoding() {
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(...)) in PowerShell.
        assert_eq!(super::base64_utf16("a"), "YQA=");
        assert_eq!(super::base64_utf16("dir"), "ZABpAHIA");
        assert_eq!(super::base64_utf16("é!"), "6QAhAA==");
    }
}
