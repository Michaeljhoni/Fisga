//! The user's commands (on completion, the power command, the clipboard
//! watcher) and the tests' stand-ins run in the system's shell: `sh -c` on
//! Linux and macOS, `cmd /C` on Windows.

use tokio::process::Command;

#[cfg(unix)]
pub fn command(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

#[cfg(windows)]
pub fn command(script: &str) -> Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new("cmd");
    // cmd reads its command line as it is: the usual argument quoting would
    // break the script's own quotes.
    command.arg("/C").raw_arg(script).creation_flags(CREATE_NO_WINDOW);
    command
}
