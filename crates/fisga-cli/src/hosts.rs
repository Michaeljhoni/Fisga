//! Registers the native messaging host (`fisga-native-host`) in every browser found
//! in the user's home. Manifests are per-user, so no root is needed. Packages
//! (the PKGBUILD) use `install_system` instead: system-wide manifests for every
//! user, written under the package's staging folder.

use std::path::{Path, PathBuf};

use anyhow::Context;
use fisga_ipc::{CHROME_EXTENSION_ID, FIREFOX_EXTENSION_ID, NATIVE_HOST_NAME};
use serde_json::json;

/// Config dirs (under `~/.config`) of Chromium-based browsers.
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
const FIREFOX_FAMILY: &[&str] = &[".mozilla", ".librewolf", ".waterfox", ".floorp", ".zen"];

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
    let home = dirs::home_dir().context("pasta pessoal não encontrada")?;
    let config = dirs::config_dir().unwrap_or_else(|| home.join(".config"));

    let host = fisga_ipc::sibling_executable("fisga-native-host");
    let host = host
        .canonicalize()
        .with_context(|| format!("fisga-native-host não encontrado ({}); instale-o junto com o fisga", host.display()))?;

    let (chrome_manifest, firefox_manifest) = manifests(&host, extra_chrome_ids);

    let mut targets: Vec<(PathBuf, &serde_json::Value)> = Vec::new();
    for browser in CHROMIUM_FAMILY {
        let root = config.join(browser);
        if root.is_dir() {
            targets.push((root.join("NativeMessagingHosts"), &chrome_manifest));
        }
    }
    for browser in FIREFOX_FAMILY {
        let root = home.join(browser);
        if root.is_dir() || *browser == ".mozilla" {
            targets.push((root.join("native-messaging-hosts"), &firefox_manifest));
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
    if !uninstall {
        println!("\nhost: {}", host.display());
        println!("Recarregue a extensão no navegador para ela encontrar o app.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
