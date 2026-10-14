//! Registers the native messaging host (`ldm-native-host`) in every browser found
//! in the user's home. Manifests are per-user, so no root is needed.

use std::path::{Path, PathBuf};

use anyhow::Context;
use ldm_ipc::{CHROME_EXTENSION_ID, FIREFOX_EXTENSION_ID, NATIVE_HOST_NAME};
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

fn write_manifest(dir: &Path, manifest: &serde_json::Value) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("criar {}", dir.display()))?;
    let path = dir.join(format!("{NATIVE_HOST_NAME}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(manifest)?)?;
    Ok(path)
}

pub fn install(extra_chrome_ids: &[String], uninstall: bool) -> anyhow::Result<()> {
    let home = dirs::home_dir().context("pasta pessoal não encontrada")?;
    let config = dirs::config_dir().unwrap_or_else(|| home.join(".config"));

    let host = ldm_ipc::sibling_executable("ldm-native-host");
    let host = host
        .canonicalize()
        .with_context(|| format!("ldm-native-host não encontrado ({}); instale-o junto com o ldm", host.display()))?;

    let mut origins = vec![format!("chrome-extension://{CHROME_EXTENSION_ID}/")];
    origins.extend(extra_chrome_ids.iter().map(|id| format!("chrome-extension://{id}/")));
    let chrome_manifest = json!({
        "name": NATIVE_HOST_NAME,
        "description": "LDM - integração com o navegador",
        "path": host,
        "type": "stdio",
        "allowed_origins": origins,
    });
    let firefox_manifest = json!({
        "name": NATIVE_HOST_NAME,
        "description": "LDM - integração com o navegador",
        "path": host,
        "type": "stdio",
        "allowed_extensions": [FIREFOX_EXTENSION_ID],
    });

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
