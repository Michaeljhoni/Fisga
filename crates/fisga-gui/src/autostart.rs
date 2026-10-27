//! "Iniciar com a sessão": an XDG autostart entry that runs `fisga-gui
//! --background` at login. GNOME, KDE and the systemd XDG autostart generator
//! (used by niri, Sway and Hyprland sessions started through systemd) read it.

use std::path::{Path, PathBuf};

const FILE: &str = "io.fisga.Fisga.desktop";

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

pub fn path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("autostart").join(FILE))
}

pub fn is_enabled() -> bool {
    path().is_some_and(|path| path.exists())
}

/// The desktop entry; `Exec` has the absolute path because the session's
/// `PATH` often lacks `~/.local/bin`.
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

/// The entry from before the rename (LDM) runs a program that no longer
/// exists: it gives way to Fisga's.
pub fn migrate_legacy() {
    let Some(old) = config_dir().map(|dir| dir.join("autostart").join("io.ldm.Ldm.desktop")) else { return };
    if old.exists() && std::fs::remove_file(&old).is_ok() {
        let _ = set_enabled(true);
    }
}

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
    std::fs::write(&path, entry(&std::env::current_exe()?))?;
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
}
