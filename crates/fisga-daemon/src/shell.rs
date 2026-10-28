//! The programs the daemon starts. The user's commands (on completion, the
//! power command, the clipboard watcher) and the tests' stand-ins run in the
//! system's shell: `sh -c` on Linux and macOS, `cmd /C` on Windows.
//!
//! On Windows the daemon has no console, so each console program it starts
//! (yt-dlp, cmd, PowerShell...) would open a window of its own: they all get
//! `CREATE_NO_WINDOW`.

use tokio::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `program`, without a console window on Windows.
pub fn program(program: &str) -> Command {
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
    #[test]
    fn powershell_encoding() {
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(...)) in PowerShell.
        assert_eq!(super::base64_utf16("a"), "YQA=");
        assert_eq!(super::base64_utf16("dir"), "ZABpAHIA");
        assert_eq!(super::base64_utf16("é!"), "6QAhAA==");
    }
}
