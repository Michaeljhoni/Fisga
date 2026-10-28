//! What happens when a download finishes: the checksum check when the
//! download has one (a different file is flagged and nothing else runs),
//! then `Settings::on_complete`: the ClamAV scan (an infected file is
//! flagged and nothing else runs), the user's command, and the "complete" sound.
//!
//! The user's command runs in the system's shell (`sh -c`, `cmd /C` on
//! Windows) with the file in environment variables (`$FISGA_FILE`,
//! `%FISGA_FILE%`...), never pasted into the command text: a file name sent by
//! a website must not become shell code.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use fisga_ipc::DownloadInfo;
use tokio::process::Command;

use super::Manager;
use crate::{notify, shell};

#[cfg(not(any(windows, target_os = "macos")))]
const SOUND_FILE: &str = "/usr/share/sounds/freedesktop/stereo/complete.oga";

#[derive(Debug, PartialEq, Eq)]
enum Scan {
    Clean,
    /// The signature ClamAV found.
    Infected(String),
    /// No scanner, or it failed: the reason.
    Unavailable(String),
}

/// "/x/a.zip: Win.Test.EICAR_HDB-1 FOUND" → "Win.Test.EICAR_HDB-1".
fn signature(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_suffix(" FOUND"))
        .and_then(|line| line.rsplit_once(": ").map(|(_, name)| name.to_string()))
        .unwrap_or_else(|| "um vírus".into())
}

fn last_line(text: &[u8]) -> String {
    String::from_utf8_lossy(text).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

impl Manager {
    pub(super) async fn after_complete(self: Arc<Self>, id: u64, path: PathBuf) {
        let actions = self.settings().on_complete;
        let Some(info) = self.list().into_iter().find(|info| info.id == id) else { return };
        if info.checksum.is_some() && !self.check_file(id, &path).await {
            return;
        }
        if actions.antivirus {
            match self.scan(&path).await {
                Scan::Clean => tracing::info!(id, "ClamAV: limpo"),
                Scan::Infected(found) => {
                    tracing::warn!(id, found, "ClamAV encontrou um vírus");
                    self.update(id, |info| info.error = Some(format!("o ClamAV encontrou {found}; não abra este arquivo")));
                    notify::send("Vírus encontrado", &format!("{}\nO ClamAV encontrou {found}.", info.name));
                    self.save();
                    return;
                }
                Scan::Unavailable(why) => {
                    tracing::warn!(id, why, "ClamAV indisponível");
                    notify::send("Antivírus indisponível", &format!("{} não foi verificado: {why}", info.name));
                }
            }
        }
        let command = actions.command.trim();
        if !command.is_empty() {
            self.run_command(command, &info, &path).await;
        }
        if actions.sound {
            self.play_sound().await;
        }
    }

    async fn scan(&self, path: &Path) -> Scan {
        let run = |program: &str, args: &[&str]| {
            let mut command = shell::program(program);
            command.args(args).arg(path).stdin(Stdio::null());
            command
        };
        let output = match &self.scan_command {
            // Tests: the file goes as "$1", as for the real scanners.
            Some(custom) => {
                #[cfg(unix)]
                let mut command = shell::command(&format!("{custom} \"$1\""));
                #[cfg(unix)]
                command.arg("sh").arg(path);
                #[cfg(windows)]
                let mut command = shell::command(&format!("{custom} \"{}\"", path.display()));
                command.stdin(Stdio::null());
                command.output().await
            }
            None => match run("clamdscan", &["--no-summary", "--fdpass"]).output().await {
                // clamd not installed or not running: the standalone scanner (slower).
                Ok(output) if output.status.code() == Some(2) => run("clamscan", &["--no-summary"]).output().await,
                Err(_) => run("clamscan", &["--no-summary"]).output().await,
                other => other,
            },
        };
        match output {
            Err(e) => Scan::Unavailable(format!("o ClamAV não está instalado ({e})")),
            Ok(output) => match output.status.code() {
                Some(0) => Scan::Clean,
                Some(1) => Scan::Infected(signature(&String::from_utf8_lossy(&output.stdout))),
                _ => Scan::Unavailable(match last_line(&output.stderr) {
                    line if line.is_empty() => "o ClamAV falhou".into(),
                    line => line,
                }),
            },
        }
    }

    async fn run_command(&self, command: &str, info: &DownloadInfo, path: &Path) {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut child = shell::command(command);
        child
            .env("FISGA_FILE", path)
            .env("FISGA_NAME", &info.name)
            .env("FISGA_URL", &info.url)
            .env("FISGA_DIR", &dir)
            // Their names from before the rename, for commands written then.
            .env("LDM_FILE", path)
            .env("LDM_NAME", &info.name)
            .env("LDM_URL", &info.url)
            .env("LDM_DIR", &dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if dir.is_dir() {
            child.current_dir(&dir);
        }
        let result = child.output().await;
        match result {
            Ok(output) if output.status.success() => tracing::info!(id = info.id, "comando ao concluir executado"),
            Ok(output) => {
                let why = last_line(&output.stderr);
                tracing::warn!(id = info.id, status = ?output.status, why, "o comando ao concluir falhou");
                notify::send("O comando ao concluir falhou", &format!("{}\n{why}", info.name));
            }
            Err(e) => tracing::warn!(id = info.id, error = %e, "não foi possível rodar o comando ao concluir"),
        }
    }

    async fn play_sound(&self) {
        let quiet = |command: &mut Command| {
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        };
        if let Some(custom) = &self.sound_command {
            let mut command = shell::command(custom);
            quiet(&mut command);
            let _ = command.status().await;
            return;
        }
        play_system_sound(quiet).await;
    }
}

/// The desktop theme's sound, else the freedesktop file through PulseAudio or PipeWire.
#[cfg(not(any(windows, target_os = "macos")))]
async fn play_system_sound(quiet: impl Fn(&mut Command)) {
    let players: [(&str, &[&str]); 3] =
        [("canberra-gtk-play", &["-i", "complete", "-d", "Fisga"]), ("paplay", &[SOUND_FILE]), ("pw-play", &[SOUND_FILE])];
    for (program, args) in players {
        if program != "canberra-gtk-play" && !Path::new(SOUND_FILE).exists() {
            continue;
        }
        let mut command = shell::program(program);
        command.args(args);
        quiet(&mut command);
        if matches!(command.status().await, Ok(status) if status.success()) {
            return;
        }
    }
}

/// Windows' own notification sound, played to the end.
#[cfg(windows)]
async fn play_system_sound(quiet: impl Fn(&mut Command)) {
    let mut command = shell::powershell(
        r"$file = Join-Path $env:SystemRoot 'Media\Windows Notify System Generic.wav'
          if (-not (Test-Path $file)) { $file = Join-Path $env:SystemRoot 'Media\tada.wav' }
          (New-Object System.Media.SoundPlayer $file).PlaySync()",
    );
    quiet(&mut command);
    let _ = command.status().await;
}

#[cfg(target_os = "macos")]
async fn play_system_sound(quiet: impl Fn(&mut Command)) {
    let mut command = shell::program("afplay");
    command.arg("/System/Library/Sounds/Glass.aiff");
    quiet(&mut command);
    let _ = command.status().await;
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::super::tests::manager;
    use super::*;

    #[test]
    fn clamav_output() {
        assert_eq!(signature("/tmp/a.zip: Win.Test.EICAR_HDB-1 FOUND\n"), "Win.Test.EICAR_HDB-1");
        assert_eq!(signature("/tmp/pasta: com: dois pontos/x.exe: Eicar-Signature FOUND"), "Eicar-Signature");
        assert_eq!(signature(""), "um vírus");
    }

    /// A manager whose scanner and sound are shell commands writing to `dir`.
    #[cfg(unix)]
    fn with_commands(scanner: &str, sound: &str) -> Arc<Manager> {
        let mut manager = Arc::try_unwrap(manager()).ok().expect("only reference");
        manager.scan_command = Some(scanner.into());
        manager.sound_command = Some(sound.into());
        Arc::new(manager)
    }

    // The commands here are sh scripts.
    #[cfg(unix)]
    #[tokio::test]
    async fn clean_file_runs_the_command_with_its_path_then_the_sound() {
        let dir = std::env::temp_dir().join(format!("fisga-complete-{}-a", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A name that would be shell code if pasted into the command.
        let file = dir.join("x$(touch pwned).bin");
        std::fs::write(&file, b"data").unwrap();
        let log = dir.join("log");
        let manager = with_commands("true", &format!("echo som >> {}", log.display()));
        let mut settings = manager.settings();
        settings.on_complete.antivirus = true;
        settings.on_complete.command = format!("printf '%s\\n' \"$FISGA_FILE\" >> {}", log.display());
        manager.set_settings(settings).unwrap();
        let id = manager.add(fisga_ipc::AddRequest { url: "https://e.com/x.bin".into(), ..Default::default() }).unwrap();

        manager.clone().after_complete(id, file.clone()).await;
        let lines = std::fs::read_to_string(&log).unwrap();
        assert_eq!(lines, format!("{}\nsom\n", file.display()));
        assert!(!dir.join("pwned").exists(), "the file name was not run as code");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn infected_file_is_flagged_and_nothing_else_runs() {
        let dir = std::env::temp_dir().join(format!("fisga-complete-{}-b", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        let scanner = "f() { echo \"$1: Eicar-Test FOUND\"; exit 1; }; f";
        let manager = with_commands(scanner, &format!("echo som >> {}", log.display()));
        let mut settings = manager.settings();
        settings.on_complete.antivirus = true;
        settings.on_complete.command = format!("echo comando >> {}", log.display());
        manager.set_settings(settings).unwrap();
        let id = manager.add(fisga_ipc::AddRequest { url: "https://e.com/v.exe".into(), ..Default::default() }).unwrap();

        manager.clone().after_complete(id, dir.join("v.exe")).await;
        assert!(!log.exists(), "neither the command nor the sound ran");
        let info = manager.list().into_iter().find(|i| i.id == id).unwrap();
        assert!(info.error.unwrap().contains("Eicar-Test"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
