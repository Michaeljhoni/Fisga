//! "Iniciar com a sessão": runs `fisga-gui --background` at login.
//!
//! - Linux: an XDG autostart entry. GNOME, KDE and the systemd XDG autostart
//!   generator (used by niri, Sway and Hyprland sessions started through
//!   systemd) read it.
//! - Windows: the "Fisga" value of `HKCU\...\CurrentVersion\Run` (the
//!   installer's checkbox writes the same one).
//! - macOS: a LaunchAgent in `~/Library/LaunchAgents`.

use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;

#[cfg(not(any(windows, target_os = "macos")))]
const FILE: &str = "io.fisga.Fisga.desktop";

#[cfg(not(any(windows, target_os = "macos")))]
fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("autostart").join(FILE))
}

#[cfg(target_os = "macos")]
fn path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/LaunchAgents").join(format!("{}.plist", crate::APP_ID)))
}

#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    path().is_some_and(|path| path.exists())
}

/// The desktop entry; `Exec` has the absolute path because the session's
/// `PATH` often lacks `~/.local/bin`.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
pub fn entry(program: &Path) -> String {
    let program = program.display().to_string();
    let exec = match program.contains([' ', '"', '\\']) {
        true => format!("\"{}\"", program.replace('\\', "\\\\").replace('"', "\\\"")),
        false => program,
    };
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Fisga\n\
         Comment=Gerenciador de downloads, em segundo plano na bandeja\n\
         Exec={exec} --background\n\
         Icon=io.fisga.Fisga\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// The LaunchAgent: runs once at login, and is not restarted when it quits.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn launch_agent(program: &Path) -> String {
    let xml = |text: &str| text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{}</string>\n\
         \t\t<string>--background</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n",
        crate::APP_ID,
        xml(&program.display().to_string()),
    )
}

/// The entry from before the rename (LDM) runs a program that no longer
/// exists: it gives way to Fisga's.
pub fn migrate_legacy() {
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let Some(old) = config_dir().map(|dir| dir.join("autostart").join("io.ldm.Ldm.desktop")) else { return };
        if old.exists() && std::fs::remove_file(&old).is_ok() {
            let _ = set_enabled(true);
        }
    }
}

#[cfg(not(windows))]
pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let path = path().ok_or_else(|| anyhow::anyhow!("não encontrei a pasta de configuração (HOME)"))?;
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(not(target_os = "macos"))]
    let contents = entry(&std::env::current_exe()?);
    #[cfg(target_os = "macos")]
    let contents = launch_agent(&std::env::current_exe()?);
    std::fs::write(&path, contents)?;
    Ok(())
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// `reg.exe`, without the console window a GUI program would open for it.
#[cfg(windows)]
fn reg(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("reg")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
pub fn is_enabled() -> bool {
    reg(&["query", RUN_KEY, "/v", "Fisga"])
}

#[cfg(windows)]
pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    if !enabled {
        if is_enabled() && !reg(&["delete", RUN_KEY, "/v", "Fisga", "/f"]) {
            anyhow::bail!("não foi possível remover a entrada de {RUN_KEY}");
        }
        return Ok(());
    }
    let command = format!("\"{}\" --background", std::env::current_exe()?.display());
    anyhow::ensure!(reg(&["add", RUN_KEY, "/v", "Fisga", "/t", "REG_SZ", "/d", &command, "/f"]), "não foi possível gravar em {RUN_KEY}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry() {
        let entry = entry(Path::new("/home/ana/.local/bin/fisga-gui"));
        assert!(entry.contains("\nExec=/home/ana/.local/bin/fisga-gui --background\n"), "{entry}");
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\n"));
        let spaced = super::entry(Path::new("/opt/meus apps/fisga-gui"));
        assert!(spaced.contains("\nExec=\"/opt/meus apps/fisga-gui\" --background\n"), "{spaced}");
    }

    #[test]
    fn launch_agent_plist() {
        let plist = launch_agent(Path::new("/Applications/Fisga & Cia.app/Contents/MacOS/fisga-gui"));
        assert!(plist.contains("<string>io.fisga.Fisga</string>"), "{plist}");
        assert!(plist.contains("<string>/Applications/Fisga &amp; Cia.app/Contents/MacOS/fisga-gui</string>"), "{plist}");
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"), "{plist}");
    }
}
