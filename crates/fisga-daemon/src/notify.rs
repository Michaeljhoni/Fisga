//! Desktop notifications through `notify-send` (libnotify), which works with any
//! notification daemon (mako, dunst, GNOME, KDE...). Missing tool = silently skipped.

pub fn send(summary: &str, body: &str) {
    let mut command = tokio::process::Command::new("notify-send");
    command
        .args(["--app-name=Fisga", "--icon=folder-download", summary, body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(mut child) = command.spawn() {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}
