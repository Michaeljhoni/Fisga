//! New versions of Fisga itself. Once a day, and when asked, the daemon looks
//! at the latest release on GitHub; one newer than this is offered in the main
//! window and announced once with a notification (for who only has the tray).
//!
//! Installing it: on Windows the release's installer, checked against its
//! SHA256SUMS, runs without questions; it closes Fisga (the downloads pause and
//! resume afterwards) and opens it again. On macOS the disk image is opened,
//! for the app to be dragged to Applications. On Linux the system's packages
//! bring it: Fisga only says there is one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fisga_ipc::{NewVersion, Response, UpdateStage};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::manager::Manager;

const LATEST: &str = "https://api.github.com/repos/Michaeljhoni/Fisga/releases/latest";
/// Not right at startup, which has enough to do (the session just began).
const FIRST_LOOK: Duration = Duration::from_secs(120);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the daemon sees whether a day went by: it may run for weeks.
/// Also the retry after a look that failed (offline).
const WAKE: Duration = Duration::from_secs(60 * 60);
const TIMEOUT: Duration = Duration::from_secs(30);
const SUMS: &str = "SHA256SUMS";

/// How this system's installer's name ends, among the release's files.
#[cfg(windows)]
const INSTALLER: Option<&str> = Some("-setup.exe");
#[cfg(target_os = "macos")]
const INSTALLER: Option<&str> = Some(".dmg");
#[cfg(not(any(windows, target_os = "macos")))]
const INSTALLER: Option<&str> = None;

/// What GitHub's API says of a release (the fields used).
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// A newer version, and where its files are.
#[derive(Debug, Clone)]
struct Found {
    info: NewVersion,
    installer: Option<Asset>,
    sums: Option<String>,
}

pub struct Updater {
    found: Mutex<Option<Found>>,
    /// `$FISGA_RELEASES_URL` replaces GitHub's (tests).
    url: String,
    /// The installer downloaded, and when the last look was.
    dir: PathBuf,
}

impl Updater {
    pub fn new() -> Arc<Self> {
        let url = std::env::var("FISGA_RELEASES_URL").ok().filter(|url| !url.is_empty());
        Arc::new(Self {
            found: Mutex::new(None),
            url: url.unwrap_or_else(|| LATEST.into()),
            dir: dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("fisga").join("update"),
        })
    }

    /// What a subscriber is told on arrival.
    pub fn current(&self) -> Option<NewVersion> {
        self.found.lock().unwrap().as_ref().map(|found| found.info.clone())
    }

    /// Looks at the latest release. Subscribers are told the answer too.
    pub async fn check(&self, manager: &Manager) -> Result<Option<NewVersion>, String> {
        let release = latest(&manager.client(), &self.url).await?;
        let found = found(release, fisga_ipc::VERSION, INSTALLER);
        let info = {
            let mut current = self.found.lock().unwrap();
            // The version being installed keeps its progress.
            let installing = match (&found, current.as_ref()) {
                (Some(new), Some(old)) => new.info.version == old.info.version && old.info.stage.as_ref().is_some_and(UpdateStage::busy),
                _ => false,
            };
            if !installing {
                *current = found;
            }
            current.as_ref().map(|found| found.info.clone())
        };
        manager.announce(Response::NewVersion { release: info.clone() });
        Ok(info)
    }

    fn set_stage(&self, manager: &Manager, stage: UpdateStage) {
        let info = {
            let mut found = self.found.lock().unwrap();
            let Some(found) = found.as_mut() else { return };
            found.info.stage = Some(stage);
            found.info.clone()
        };
        manager.announce(Response::NewVersion { release: Some(info) });
    }

    /// Starts installing the version found; how it goes reaches subscribers.
    pub fn install(self: &Arc<Self>, manager: &Arc<Manager>) -> Result<(), String> {
        let (installer, sums) = {
            let mut found = self.found.lock().unwrap();
            let Some(found) = found.as_mut() else { return Err("não há versão nova a instalar".into()) };
            if found.info.stage.as_ref().is_some_and(UpdateStage::busy) {
                return Err("a versão nova já está sendo instalada".into());
            }
            let (Some(installer), Some(sums)) = (found.installer.clone(), found.sums.clone()) else {
                return Err("aqui quem atualiza o Fisga é o gerenciador de pacotes do sistema".into());
            };
            (installer, sums)
        };
        self.set_stage(manager, UpdateStage::Downloading { done: 0, total: None });
        tracing::info!(file = %installer.name, "baixando a versão nova do Fisga");
        let (this, manager) = (self.clone(), manager.clone());
        tokio::spawn(async move {
            let say = |done, total| this.set_stage(&manager, UpdateStage::Downloading { done, total });
            let fetched = download(&manager.client(), &this.dir, &installer, &sums, &say).await;
            let stage = match fetched {
                Ok(file) => launch(&file).await,
                Err(e) => Err(e),
            };
            let stage = stage.unwrap_or_else(|error| {
                tracing::warn!(%error, "não foi possível atualizar o Fisga");
                UpdateStage::Failed { error }
            });
            tracing::info!(?stage, "atualização do Fisga");
            this.set_stage(&manager, stage);
        });
        Ok(())
    }

    /// The last look was over a day ago (or never happened).
    fn due(&self) -> bool {
        let checked = std::fs::metadata(self.dir.join("checked")).and_then(|file| file.modified()).ok();
        let age = checked.and_then(|at| SystemTime::now().duration_since(at).ok());
        age.map_or(true, |age| age >= EVERY)
    }

    fn mark_checked(&self) {
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(self.dir.join("checked"), "");
    }

    /// One notification per version, however many days it waits.
    fn notify_once(&self, info: &NewVersion) {
        let said = self.dir.join("notified");
        if std::fs::read_to_string(&said).is_ok_and(|version| version == info.version) {
            return;
        }
        let _ = std::fs::write(&said, &info.version);
        let body = match info.installable {
            true => "Abra o Fisga para ver as novidades e atualizar.",
            false => "Abra o Fisga para ver as novidades.",
        };
        crate::notify::send(&format!("Fisga {} disponível", info.version), body);
    }

    /// Installers of earlier updates (one still running is left for next time).
    fn clean(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name != "checked" && name != "notified" {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Looks once a day while the setting allows it.
pub async fn run(updater: Arc<Updater>, manager: Arc<Manager>, shutdown: CancellationToken) {
    updater.clean();
    let mut wait = FIRST_LOOK;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
        wait = WAKE;
        if !manager.settings().check_updates || !updater.due() {
            continue;
        }
        match updater.check(&manager).await {
            Ok(found) => {
                updater.mark_checked();
                if let Some(info) = found {
                    tracing::info!(version = %info.version, "há uma versão nova do Fisga");
                    updater.notify_once(&info);
                }
            }
            Err(e) => tracing::info!(error = %e, "não foi possível procurar uma versão nova do Fisga"),
        }
    }
}

async fn latest(client: &reqwest::Client, url: &str) -> Result<Release, String> {
    let request = client.get(url).header(reqwest::header::ACCEPT, "application/vnd.github+json").timeout(TIMEOUT);
    let response = request.send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    let text = response.text().await.map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("resposta inesperada do GitHub: {e}"))
}

/// The release, when newer than `current`, with this system's installer
/// (its name ends in `installer`) and the list of sums.
fn found(release: Release, current: &str, installer: Option<&str>) -> Option<Found> {
    let version = release.tag_name.trim_start_matches('v').to_string();
    if !newer(&version, current) {
        return None;
    }
    // The name becomes a file's: nothing that leaves the folder.
    let plain = |asset: &&Asset| !asset.name.contains(['/', '\\']) && !asset.name.starts_with('.');
    let file = installer.and_then(|end| release.assets.iter().filter(plain).find(|asset| asset.name.ends_with(end))).cloned();
    let sums = release.assets.iter().find(|asset| asset.name == SUMS).map(|asset| asset.browser_download_url.clone());
    let info = NewVersion {
        version,
        page: release.html_url,
        notes: release.body.unwrap_or_default(),
        installable: file.is_some() && sums.is_some(),
        stage: None,
    };
    Some(Found { info, installer: file, sums })
}

/// "1.10.0" is newer than "1.9.2"; "1.2" is "1.2.0". Anything after a `-` or
/// `+` is left out; what does not read as numbers is never newer.
fn newer(latest: &str, current: &str) -> bool {
    let parse = |version: &str| -> Option<Vec<u64>> {
        let numbers = version.trim().trim_start_matches('v').split(['-', '+']).next()?;
        numbers.split('.').map(|part| part.parse().ok()).collect()
    };
    let (Some(mut latest), Some(mut current)) = (parse(latest), parse(current)) else { return false };
    let len = latest.len().max(current.len());
    latest.resize(len, 0);
    current.resize(len, 0);
    latest > current
}

/// Fetches the installer and checks it against the release's sums.
async fn download(
    client: &reqwest::Client,
    dir: &Path,
    installer: &Asset,
    sums: &str,
    say: &(impl Fn(u64, Option<u64>) + Send + Sync),
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let response = client.get(sums).timeout(TIMEOUT).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    let list = response.text().await.map_err(|e| e.to_string())?;
    let named = list.lines().any(|line| line.split_whitespace().any(|word| word.trim_start_matches('*') == installer.name));
    let expected = crate::tools::find_sum(&list, &installer.name).filter(|_| named);
    let expected = expected.ok_or_else(|| format!("{} não está na lista de somas", installer.name))?;
    let part = dir.join(format!("{}.part", installer.name));
    let sum = crate::tools::fetch(client, &installer.browser_download_url, &part, say).await;
    let file = dir.join(&installer.name);
    let placed = match sum {
        Ok(sum) if sum == expected => std::fs::rename(&part, &file).map_err(|e| e.to_string()),
        Ok(_) => Err("a soma do instalador baixado não confere".into()),
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&part);
    placed.map(|()| file)
}

/// Inno Setup's silent install: a window with its progress, no questions.
/// It stops the daemon and the interface, and `/relaunch=yes` has it open
/// the interface again (fisga.iss). Detached: it outlives the daemon.
#[cfg(windows)]
async fn launch(file: &Path) -> Result<UpdateStage, String> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    std::process::Command::new(file)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/relaunch=yes"])
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("não foi possível iniciar o instalador: {e}"))?;
    Ok(UpdateStage::Installing)
}

/// Opens the disk image in Finder.
#[cfg(target_os = "macos")]
async fn launch(file: &Path) -> Result<UpdateStage, String> {
    match crate::shell::program("open").arg(file).status().await {
        Ok(status) if status.success() => Ok(UpdateStage::Opened),
        Ok(status) => Err(format!("o disco da versão nova não abriu ({status})")),
        Err(e) => Err(format!("o disco da versão nova não abriu: {e}")),
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
async fn launch(_: &Path) -> Result<UpdateStage, String> {
    Err("aqui quem atualiza o Fisga é o gerenciador de pacotes do sistema".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const RELEASE: &str = r#"{
        "tag_name": "v1.2.0",
        "html_url": "https://github.com/Michaeljhoni/Fisga/releases/tag/v1.2.0",
        "body": "- Atualização automática",
        "draft": false,
        "assets": [
            {"name": "fisga-1.2.0-x86_64.tar.gz", "browser_download_url": "https://example.invalid/fisga-1.2.0-x86_64.tar.gz"},
            {"name": "Fisga-1.2.0-setup.exe", "browser_download_url": "https://example.invalid/Fisga-1.2.0-setup.exe"},
            {"name": "Fisga-1.2.0.dmg", "browser_download_url": "https://example.invalid/Fisga-1.2.0.dmg"},
            {"name": "SHA256SUMS", "browser_download_url": "https://example.invalid/SHA256SUMS"}
        ]
    }"#;

    fn release() -> Release {
        serde_json::from_str(RELEASE).unwrap()
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("1.2.0", "1.1.0"));
        assert!(newer("v1.10.0", "1.9.2"));
        assert!(newer("2.0", "1.99.99"));
        assert!(!newer("1.1.0", "1.1.0"));
        assert!(!newer("1.2", "1.2.0"));
        assert!(!newer("1.0.9", "1.1.0"));
        assert!(newer("1.2.0-rc1", "1.1.0"));
        assert!(!newer("latest", "1.1.0"));
        assert!(!newer("1.2.0", "dev"));
    }

    #[test]
    fn picks_this_systems_installer() {
        let windows = found(release(), "1.1.0", Some("-setup.exe")).unwrap();
        assert_eq!(windows.info.version, "1.2.0");
        assert_eq!(windows.info.notes, "- Atualização automática");
        assert!(windows.info.installable);
        assert_eq!(windows.installer.unwrap().name, "Fisga-1.2.0-setup.exe");
        assert_eq!(windows.sums.as_deref(), Some("https://example.invalid/SHA256SUMS"));
        assert_eq!(found(release(), "1.1.0", Some(".dmg")).unwrap().installer.unwrap().name, "Fisga-1.2.0.dmg");
        // Linux: told, not installed.
        let linux = found(release(), "1.1.0", None).unwrap();
        assert!(!linux.info.installable && linux.installer.is_none());
        assert!(found(release(), "1.2.0", Some("-setup.exe")).is_none(), "the same version is not new");
        assert!(found(release(), "1.3.0", Some("-setup.exe")).is_none());
    }

    #[test]
    fn a_name_that_leaves_the_folder_is_not_taken() {
        let text = RELEASE.replace("\"Fisga-1.2.0-setup.exe\"", "\"..\\\\..\\\\Fisga-1.2.0-setup.exe\"");
        let release: Release = serde_json::from_str(&text).unwrap();
        let found = found(release, "1.1.0", Some("-setup.exe")).unwrap();
        assert!(!found.info.installable && found.installer.is_none());
    }

    /// Serves `files` by path, 404 otherwise; the address it listens on.
    async fn server(files: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = vec![0; 2048];
                let n = socket.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..n]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
                let body = files.iter().find(|(name, _)| path == format!("/{name}")).map(|(_, body)| body.clone());
                let head = match &body {
                    Some(body) => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                };
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body.unwrap_or_default()).await;
            }
        });
        format!("http://{address}")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fisga-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn client() -> reqwest::Client {
        fisga_core::build_client_with(fisga_core::Proxy::Direct).unwrap()
    }

    const SUM_OF_HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[tokio::test]
    async fn reads_the_latest_release() {
        let base = server(vec![("latest", RELEASE.as_bytes().to_vec())]).await;
        let release = latest(&client(), &format!("{base}/latest")).await.unwrap();
        assert_eq!(release.tag_name, "v1.2.0");
        assert_eq!(release.assets.len(), 4);
        assert!(latest(&client(), &format!("{base}/missing")).await.is_err());
    }

    #[tokio::test]
    async fn downloads_and_checks_the_installer() {
        let sums = format!("{}  fisga-1.2.0-x86_64.tar.gz\n{SUM_OF_HELLO}  Fisga-1.2.0-setup.exe\n", "0".repeat(64));
        let base = server(vec![("SHA256SUMS", sums.into_bytes()), ("setup.exe", b"hello".to_vec())]).await;
        let dir = scratch("ok");
        let asset = Asset { name: "Fisga-1.2.0-setup.exe".into(), browser_download_url: format!("{base}/setup.exe") };
        let file = download(&client(), &dir, &asset, &format!("{base}/SHA256SUMS"), &|_, _| {}).await.unwrap();
        assert_eq!(file, dir.join("Fisga-1.2.0-setup.exe"));
        assert_eq!(std::fs::read(&file).unwrap(), b"hello");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_wrong_or_missing_sum_keeps_nothing() {
        let wrong = format!("{}  Fisga-1.2.0-setup.exe\n", "0".repeat(64));
        let other = format!("{SUM_OF_HELLO}  Fisga-1.2.0.dmg\n");
        let base = server(vec![("wrong", wrong.into_bytes()), ("other", other.into_bytes()), ("setup.exe", b"hello".to_vec())]).await;
        let asset = Asset { name: "Fisga-1.2.0-setup.exe".into(), browser_download_url: format!("{base}/setup.exe") };
        for (list, error) in [("wrong", "não confere"), ("other", "não está na lista")] {
            let dir = scratch(list);
            let result = download(&client(), &dir, &asset, &format!("{base}/{list}"), &|_, _| {}).await;
            assert!(result.as_ref().is_err_and(|e| e.contains(error)), "{list}: {result:?}");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "{list}");
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
