//! Clipboard monitor, like IDM's: copying the address of a file (video, music,
//! program, archive, PDF...) anywhere opens the "Novo download" window.
//!
//! On Wayland it runs `wl-paste --watch` (wl-clipboard), which needs the
//! data-control protocol (niri, Sway, Hyprland, KDE...). Windows and macOS
//! have no such tool: a small PowerShell or `sh` loop looks at the clipboard
//! every second and prints what changed. `FISGA_CLIPBOARD_CMD` replaces the
//! command: it must print one line per clipboard change.

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fisga_ipc::{AddRequest, Category};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::gui::{Gui, Unanswered};
use crate::manager::Manager;

/// `wl-paste --watch` reports what is already in the clipboard when it starts:
/// that was copied before, not a new link to catch.
const SETTLE: Duration = Duration::from_millis(1500);
/// A watcher that ran at least this long is restarted when it stops (e.g. the
/// compositor restarted); one that fails right away is not supported here.
const RESTART_AFTER: Duration = Duration::from_secs(10);

/// One line per change: line breaks inside the copied text become spaces.
#[cfg(not(any(windows, target_os = "macos")))]
const WL_PASTE_HANDLER: &str = "tr '\\r\\n' '  '; echo";

/// Clipboard.GetText needs a single-threaded apartment, PowerShell 5's default.
/// What was copied before it started is not printed (PowerShell may take
/// longer than `SETTLE` to start).
#[cfg(windows)]
const POWERSHELL_WATCHER: &str = r"
Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$last = try { [System.Windows.Forms.Clipboard]::GetText() } catch { $null }
while ($true) {
    try { $text = [System.Windows.Forms.Clipboard]::GetText() } catch { $text = $last }
    if ($text -ne $last) {
        $last = $text
        [Console]::Out.WriteLine(($text -replace '[\r\n]', ' '))
        [Console]::Out.Flush()
    }
    Start-Sleep -Milliseconds 1000
}";

#[cfg(target_os = "macos")]
const PBPASTE_WATCHER: &str = r#"last=$(pbpaste 2>/dev/null | tr '\r\n' '  ')
while :; do
    text=$(pbpaste 2>/dev/null | tr '\r\n' '  ')
    if [ "$text" != "$last" ]; then last=$text; printf '%s\n' "$text"; fi
    sleep 1
done"#;

fn watch_command() -> Option<Command> {
    if let Some(custom) = std::env::var("FISGA_CLIPBOARD_CMD").ok().filter(|c| !c.is_empty()) {
        return Some(crate::shell::command(&custom));
    }
    system_watcher()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_watcher() -> Option<Command> {
    std::env::var_os("WAYLAND_DISPLAY").filter(|d| !d.is_empty())?;
    let mut command = crate::shell::program("wl-paste");
    command.args(["--type", "text", "--watch", "sh", "-c", WL_PASTE_HANDLER]);
    Some(command)
}

#[cfg(windows)]
fn system_watcher() -> Option<Command> {
    Some(crate::shell::powershell(POWERSHELL_WATCHER))
}

#[cfg(target_os = "macos")]
fn system_watcher() -> Option<Command> {
    Some(crate::shell::command(PBPASTE_WATCHER))
}

pub fn spawn(manager: Arc<Manager>, gui: Arc<Gui>) {
    let Some(_) = watch_command() else {
        tracing::info!("sem sessão Wayland: a área de transferência não é monitorada");
        return;
    };
    tokio::spawn(async move {
        loop {
            let Some(mut command) = watch_command() else { return };
            command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
            let started = Instant::now();
            match command.spawn() {
                Ok(mut child) => {
                    if let Some(stdout) = child.stdout.take() {
                        watch(&manager, &gui, BufReader::new(stdout)).await;
                    }
                    let _ = child.wait().await;
                }
                Err(e) => tracing::warn!(error = %e, "não foi possível monitorar a área de transferência (instale o wl-clipboard)"),
            }
            if started.elapsed() < RESTART_AFTER {
                tracing::warn!("o monitor da área de transferência parou; links copiados não serão capturados");
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn watch(manager: &Arc<Manager>, gui: &Arc<Gui>, reader: BufReader<tokio::process::ChildStdout>) {
    let started = Instant::now();
    let mut last: Option<String> = None;
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Some(url) = download_url(&line) else { continue };
        // The same link copied again (or reported twice) is not a new download.
        if last.as_deref() == Some(url.as_str()) {
            continue;
        }
        last = Some(url.clone());
        if started.elapsed() < SETTLE || !manager.settings().watch_clipboard {
            continue;
        }
        tracing::info!(url, "link copiado");
        match manager.add(AddRequest { url, confirm: true, ..Default::default() }) {
            Ok(id) => gui.request_confirmation(manager, id, Unanswered::Drop),
            Err(e) => tracing::info!(error = %e, "link copiado ignorado"),
        }
    }
}

/// The copied text, if it is just the address of a file worth downloading.
pub fn download_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() > 2048 || text.contains(char::is_whitespace) {
        return None;
    }
    let url = url::Url::parse(text).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "ftp") || url.host_str().is_none() {
        return None;
    }
    let name = url.path_segments()?.rfind(|s| !s.is_empty())?;
    let (_, ext) = name.rsplit_once('.')?;
    let wanted = match Category::from_extension(ext) {
        Category::Video | Category::Music | Category::Compressed | Category::Program => true,
        // Copying links to pages and pictures is common; only documents people download.
        Category::Document => {
            matches!(ext.to_ascii_lowercase().as_str(), "pdf" | "epub" | "mobi" | "djvu" | "doc" | "docx" | "odt")
        }
        Category::Image | Category::Other => false,
    };
    wanted.then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_files_are_caught() {
        for url in [
            "https://example.com/dist/app-1.2.AppImage",
            "  https://cdn.example.com/video.mp4?token=abc  ",
            "http://example.com/a/b/archive.tar.gz",
            "ftp://ftp.example.com/pub/linux.iso",
            "https://example.com/livro.PDF",
            "https://example.com/música%20boa.mp3",
        ] {
            assert!(download_url(url).is_some(), "{url}");
        }
    }

    #[test]
    fn other_text_is_ignored() {
        for text in [
            "https://example.com/",
            "https://example.com/page.html",
            "https://example.com/foto.jpg",
            "https://www.youtube.com/watch?v=abc",
            "veja https://example.com/a.zip",
            "https://example.com/a.zip\nhttps://example.com/b.zip",
            "example.com/a.zip",
            "file:///home/u/a.zip",
            "/home/u/a.zip",
            "texto qualquer",
            "",
        ] {
            assert_eq!(download_url(text), None, "{text}");
        }
    }
}
