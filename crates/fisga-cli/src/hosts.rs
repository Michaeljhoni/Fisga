//! Registers the native messaging host (`fisga-native-host`) in every browser found
//! in the user's home. Manifests are per-user, so no root is needed. Packages
//! (the PKGBUILD) use `install_system` instead: system-wide manifests for every
//! user, written under the package's staging folder.
//!
//! Linux and macOS browsers find the manifest in a folder of their own; on
//! Windows it lives in Fisga's folder and a registry key (under HKCU) points
//! each browser to it.

use std::path::{Path, PathBuf};

use anyhow::Context;
use fisga_ipc::{CHROME_EXTENSION_ID, FIREFOX_EXTENSION_ID, NATIVE_HOST_NAME};
use serde_json::json;

/// Config dirs (under `~/.config`) of Chromium-based browsers.
#[cfg(not(any(windows, target_os = "macos")))]
const CHROMIUM_FAMILY: &[&str] = &[
    "google-chrome",
    "google-chrome-beta",
    "google-chrome-unstable",
    "chromium",
    "BraveSoftware/Brave-Browser",
    "BraveSoftware/Brave-Browser-Beta",
    "microsoft-edge",
    "microsoft-edge-beta",
    "vivaldi",
    "thorium",
    "net.imput.helium",
];

/// Profile roots (under `~`) of Firefox-based browsers.
#[cfg(not(any(windows, target_os = "macos")))]
const FIREFOX_FAMILY: &[&str] = &[".mozilla", ".librewolf", ".waterfox", ".floorp", ".zen"];

/// Folders (under `~/Library/Application Support`) of Chromium-based browsers.
#[cfg(target_os = "macos")]
const CHROMIUM_FAMILY: &[&str] = &[
    "Google/Chrome",
    "Google/Chrome Beta",
    "Google/Chrome Canary",
    "Chromium",
    "BraveSoftware/Brave-Browser",
    "Microsoft Edge",
    "Vivaldi",
    "Thorium",
];

/// Folders (under `~/Library/Application Support`) of Firefox-based browsers.
#[cfg(target_os = "macos")]
const FIREFOX_FAMILY: &[&str] = &["Mozilla", "LibreWolf", "Waterfox", "Floorp", "zen"];

/// Registry keys (under `HKCU\Software`) where Chromium-based browsers look.
#[cfg(windows)]
const CHROMIUM_KEYS: &[&str] =
    &[r"Google\Chrome", "Chromium", r"Microsoft\Edge", r"BraveSoftware\Brave-Browser", "Vivaldi", "Thorium"];

/// Firefox and its forks read Mozilla's key.
#[cfg(windows)]
const FIREFOX_KEYS: &[&str] = &["Mozilla"];

/// System-wide manifest folders of Chromium-based browsers.
const CHROMIUM_SYSTEM: &[&str] = &[
    "etc/chromium/native-messaging-hosts",
    "etc/opt/chrome/native-messaging-hosts",
    "etc/opt/brave.com/brave/native-messaging-hosts",
    "etc/opt/edge/native-messaging-hosts",
    "etc/opt/vivaldi/native-messaging-hosts",
];

/// System-wide manifest folders of Firefox-based browsers.
const FIREFOX_SYSTEM: &[&str] = &["usr/lib/mozilla/native-messaging-hosts", "usr/lib/librewolf/native-messaging-hosts"];

fn manifests(host: &Path, extra_chrome_ids: &[String]) -> (serde_json::Value, serde_json::Value) {
    let mut origins = vec![format!("chrome-extension://{CHROME_EXTENSION_ID}/")];
    origins.extend(extra_chrome_ids.iter().map(|id| format!("chrome-extension://{id}/")));
    let chrome = json!({
        "name": NATIVE_HOST_NAME,
        "description": "Fisga - integração com o navegador",
        "path": host,
        "type": "stdio",
        "allowed_origins": origins,
    });
    let firefox = json!({
        "name": NATIVE_HOST_NAME,
        "description": "Fisga - integração com o navegador",
        "path": host,
        "type": "stdio",
        "allowed_extensions": [FIREFOX_EXTENSION_ID],
    });
    (chrome, firefox)
}

/// For packages: the system-wide manifests under `root` (the package's
/// staging folder, `$pkgdir`), pointing to `host` as installed (`/usr/bin/...`).
pub fn install_system(root: &Path, host: &Path, extra_chrome_ids: &[String]) -> anyhow::Result<()> {
    anyhow::ensure!(host.is_absolute(), "--host-path precisa ser um caminho absoluto: {}", host.display());
    let (chrome, firefox) = manifests(host, extra_chrome_ids);
    let targets = CHROMIUM_SYSTEM.iter().map(|dir| (dir, &chrome)).chain(FIREFOX_SYSTEM.iter().map(|dir| (dir, &firefox)));
    for (dir, manifest) in targets {
        println!("instalado {}", write_manifest(&root.join(dir), manifest)?.display());
    }
    Ok(())
}

fn write_manifest(dir: &Path, manifest: &serde_json::Value) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("criar {}", dir.display()))?;
    let path = dir.join(format!("{NATIVE_HOST_NAME}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(manifest)?)?;
    Ok(path)
}

pub fn install(extra_chrome_ids: &[String], uninstall: bool) -> anyhow::Result<()> {
    let host = fisga_ipc::sibling_executable("fisga-native-host");
    let host = host
        .canonicalize()
        .with_context(|| format!("fisga-native-host não encontrado ({}); instale-o junto com o fisga", host.display()))?;
    let host = plain_path(host);

    let (chrome_manifest, firefox_manifest) = manifests(&host, extra_chrome_ids);
    register(&chrome_manifest, &firefox_manifest, uninstall)?;
    if !uninstall {
        println!("\nhost: {}", host.display());
        println!("Recarregue a extensão no navegador para ela encontrar o app.");
    }
    Ok(())
}

/// Windows' `canonicalize` gives `\\?\C:\...`, which browsers do not take as
/// the host's path.
fn plain_path(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|p| p.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => path,
    }
}

/// The manifest goes in each browser's own folder, for those found.
#[cfg(not(windows))]
fn register(chrome_manifest: &serde_json::Value, firefox_manifest: &serde_json::Value, uninstall: bool) -> anyhow::Result<()> {
    let home = dirs::home_dir().context("pasta pessoal não encontrada")?;
    let mut targets: Vec<(PathBuf, &serde_json::Value)> = Vec::new();

    #[cfg(not(target_os = "macos"))]
    let (chromium_root, firefox_root, folder, firefox_always) = (
        dirs::config_dir().unwrap_or_else(|| home.join(".config")),
        home,
        ("NativeMessagingHosts", "native-messaging-hosts"),
        ".mozilla",
    );
    #[cfg(target_os = "macos")]
    let (chromium_root, firefox_root, folder, firefox_always) = (
        home.join("Library").join("Application Support"),
        home.join("Library").join("Application Support"),
        ("NativeMessagingHosts", "NativeMessagingHosts"),
        "Mozilla",
    );

    for browser in CHROMIUM_FAMILY {
        let root = chromium_root.join(browser);
        if root.is_dir() {
            targets.push((root.join(folder.0), chrome_manifest));
        }
    }
    for browser in FIREFOX_FAMILY {
        let root = firefox_root.join(browser);
        if root.is_dir() || *browser == firefox_always {
            targets.push((root.join(folder.1), firefox_manifest));
        }
    }

    for (dir, manifest) in targets {
        // The host from before the rename (LDM) points to a program that no longer exists.
        let legacy = dir.join("io.ldm.native.json");
        if std::fs::remove_file(&legacy).is_ok() {
            println!("removido  {}", legacy.display());
        }
        if uninstall {
            let path = dir.join(format!("{NATIVE_HOST_NAME}.json"));
            if std::fs::remove_file(&path).is_ok() {
                println!("removido  {}", path.display());
            }
        } else {
            println!("instalado {}", write_manifest(&dir, manifest)?.display());
        }
    }
    Ok(())
}

/// The manifests go to `%LOCALAPPDATA%\fisga\native-messaging-hosts`; each
/// browser's key under `HKCU\Software\...\NativeMessagingHosts` names one.
/// Through `reg.exe`, which every Windows has.
#[cfg(windows)]
fn register(chrome_manifest: &serde_json::Value, firefox_manifest: &serde_json::Value, uninstall: bool) -> anyhow::Result<()> {
    let dir = dirs::data_local_dir().context("pasta LOCALAPPDATA não encontrada")?.join("fisga").join("native-messaging-hosts");
    let families = [("chrome", chrome_manifest, CHROMIUM_KEYS), ("firefox", firefox_manifest, FIREFOX_KEYS)];
    for (family, manifest, keys) in families {
        let file = dir.join(format!("{NATIVE_HOST_NAME}.{family}.json"));
        if uninstall {
            if std::fs::remove_file(&file).is_ok() {
                println!("removido  {}", file.display());
            }
        } else {
            std::fs::create_dir_all(&dir).with_context(|| format!("criar {}", dir.display()))?;
            std::fs::write(&file, serde_json::to_vec_pretty(manifest)?)?;
            println!("instalado {}", file.display());
        }
        for key in keys {
            let key = format!(r"HKCU\Software\{key}\NativeMessagingHosts\{NATIVE_HOST_NAME}");
            let mut reg = std::process::Command::new("reg");
            match uninstall {
                true => reg.args(["delete", &key, "/f"]),
                false => reg.args(["add", &key, "/ve", "/t", "REG_SZ", "/d"]).arg(&file).arg("/f"),
            };
            let done = reg
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            match (uninstall, done) {
                (false, false) => anyhow::bail!("não foi possível gravar {key} no registro"),
                (false, true) => println!("registrado {key}"),
                (true, true) => println!("removido  {key}"),
                // The key was not there.
                (true, false) => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_windows_paths_become_plain() {
        let host = PathBuf::from(r"\\?\C:\Fisga\fisga-native-host.exe");
        assert_eq!(plain_path(host), PathBuf::from(r"C:\Fisga\fisga-native-host.exe"));
        let unc = PathBuf::from(r"\\?\UNC\server\share\host.exe");
        assert_eq!(plain_path(unc.clone()), unc);
        let unix = PathBuf::from("/usr/bin/fisga-native-host");
        assert_eq!(plain_path(unix.clone()), unix);
    }

    /// The registry, as the browsers read it: the key's default value is the
    /// manifest, which names the host. Only in CI: it replaces, then removes,
    /// the computer's own registration.
    #[cfg(windows)]
    #[test]
    fn registry_keys_point_to_the_manifests() {
        if std::env::var_os("CI").is_none() {
            return;
        }
        let query = |key: &str| {
            let output = std::process::Command::new("reg").args(["query", key, "/ve"]).output().unwrap();
            output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
        };
        let (chrome, firefox) = manifests(Path::new(r"C:\Fisga\fisga-native-host.exe"), &[]);
        register(&chrome, &firefox, false).unwrap();
        let key = format!(r"HKCU\Software\Google\Chrome\NativeMessagingHosts\{NATIVE_HOST_NAME}");
        let value = query(&key).expect("Chrome's key");
        let file = value.lines().find_map(|line| line.split_once("REG_SZ")).map(|(_, file)| file.trim()).unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        assert_eq!(manifest["path"], r"C:\Fisga\fisga-native-host.exe");
        assert!(query(&format!(r"HKCU\Software\Mozilla\NativeMessagingHosts\{NATIVE_HOST_NAME}")).is_some());
        register(&chrome, &firefox, true).unwrap();
        assert!(query(&key).is_none());
    }

    // Linux packages: the paths are Unix ones.
    #[cfg(unix)]
    #[test]
    fn system_manifests_for_packages() {
        let root = std::env::temp_dir().join(format!("fisga-hosts-{}", std::process::id()));
        install_system(&root, Path::new("/usr/bin/fisga-native-host"), &[]).unwrap();
        let chromium: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("etc/chromium/native-messaging-hosts/io.fisga.native.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(chromium["path"], "/usr/bin/fisga-native-host");
        assert_eq!(chromium["allowed_origins"][0], format!("chrome-extension://{CHROME_EXTENSION_ID}/"));
        let firefox: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("usr/lib/mozilla/native-messaging-hosts/io.fisga.native.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(firefox["allowed_extensions"][0], FIREFOX_EXTENSION_ID);
        assert!(install_system(&root, Path::new("relativo/fisga-native-host"), &[]).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
