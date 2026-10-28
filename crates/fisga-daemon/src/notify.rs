//! Desktop notifications. Linux: `notify-send` (libnotify), which works with
//! any notification daemon (mako, dunst, GNOME, KDE...). Windows: a toast
//! through PowerShell. macOS: `osascript`. Missing tool = silently skipped.

use crate::shell;

/// Windows only shows toasts of an app with a Start menu shortcut: Fisga's,
/// which the installer creates with the AppUserModelID `io.fisga.Fisga`, or
/// else PowerShell's (built from the sources, say).
#[cfg(windows)]
const TOAST: &str = r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
$installed = Test-Path (Join-Path ([Environment]::GetFolderPath('Programs')) 'Fisga.lnk')
$xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$text = $xml.GetElementsByTagName('text')
if ($installed) {
  $app = 'io.fisga.Fisga'
  $title = $env:FISGA_SUMMARY
} else {
  $app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
  $title = "Fisga: $env:FISGA_SUMMARY"
}
$text.Item(0).AppendChild($xml.CreateTextNode($title)) > $null
$text.Item(1).AppendChild($xml.CreateTextNode($env:FISGA_BODY)) > $null
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show([Windows.UI.Notifications.ToastNotification]::new($xml))
"#;

#[cfg(windows)]
fn command(summary: &str, body: &str) -> tokio::process::Command {
    let mut command = shell::powershell(TOAST);
    command.env("FISGA_SUMMARY", summary).env("FISGA_BODY", body);
    command
}

#[cfg(target_os = "macos")]
fn command(summary: &str, body: &str) -> tokio::process::Command {
    let mut command = shell::program("osascript");
    // The texts are the script's arguments, not part of its source.
    command
        .args(["-e", "on run argv", "-e", "display notification (item 2 of argv) with title \"Fisga\" subtitle (item 1 of argv)"])
        .args(["-e", "end run", summary, body]);
    command
}

#[cfg(not(any(windows, target_os = "macos")))]
fn command(summary: &str, body: &str) -> tokio::process::Command {
    let mut command = shell::program("notify-send");
    command.args(["--app-name=Fisga", "--icon=folder-download", summary, body]);
    command
}

pub fn send(summary: &str, body: &str) {
    let mut command = command(summary, body);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(mut child) = command.spawn() {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}
