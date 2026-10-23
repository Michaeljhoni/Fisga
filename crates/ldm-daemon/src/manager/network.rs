//! How downloads reach the sites: the proxy of the HTTP client (rebuilt when
//! the setting changes) and IDM's "site logins", sent as HTTP Basic
//! authentication to their host and its subdomains.
//!
//! The logins live in `logins.json` next to `settings.json`, readable only by
//! the user, and never in the settings (which are broadcast and logged).

use std::path::{Path, PathBuf};

use anyhow::Context;
use ldm_ipc::{AddRequest, ProxyMode, ProxySettings, SiteLogin};

use super::{request_headers, write_json, Manager};

pub fn build_client(proxy: &ProxySettings) -> anyhow::Result<reqwest::Client> {
    proxy.check().map_err(anyhow::Error::msg)?;
    let choice = match proxy.mode {
        ProxyMode::System => ldm_core::Proxy::System,
        ProxyMode::None => ldm_core::Proxy::Direct,
        ProxyMode::Manual => ldm_core::Proxy::Url(proxy.url.trim()),
    };
    ldm_core::build_client_with(choice).context("proxy inválido")
}

pub fn logins_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name("logins.json")
}

pub fn load_logins(path: &Path) -> Vec<SiteLogin> {
    match std::fs::read(path) {
        Ok(data) => serde_json::from_slice(&data).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "logins.json inválido; ignorado");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Standard base64 (RFC 4648, with padding), for the `Authorization` header.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | (bytes[2] as u32);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            match i <= chunk.len() {
                true => out.push(ALPHABET[((n >> shift) & 63) as usize] as char),
                false => out.push('='),
            }
        }
    }
    out
}

pub fn basic_auth(login: &SiteLogin) -> String {
    format!("Basic {}", base64(format!("{}:{}", login.user, login.password).as_bytes()))
}

impl Manager {
    /// The HTTP client, with the current proxy.
    pub fn client(&self) -> reqwest::Client {
        self.client.lock().unwrap().clone()
    }

    pub(super) fn set_client(&self, client: reqwest::Client) {
        *self.client.lock().unwrap() = client;
    }

    /// The request's headers, plus the site login of its host (unless the
    /// browser already sent an `Authorization`).
    pub fn headers(&self, request: &AddRequest) -> Vec<(String, String)> {
        let mut headers = request_headers(request);
        if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("authorization")) {
            if let Some(login) = SiteLogin::find(&self.logins.lock().unwrap(), &request.url) {
                headers.push(("Authorization".into(), basic_auth(login)));
            }
        }
        headers
    }

    pub fn logins(&self) -> Vec<SiteLogin> {
        self.logins.lock().unwrap().clone()
    }

    pub fn set_logins(&self, logins: Vec<SiteLogin>) -> anyhow::Result<()> {
        let mut logins: Vec<SiteLogin> = logins
            .into_iter()
            .map(|login| SiteLogin { host: SiteLogin::normalize_host(&login.host), ..login })
            .filter(|login| !login.host.is_empty())
            .collect();
        // One login per host: the last one given wins.
        let mut seen = Vec::new();
        logins.reverse();
        logins.retain(|login| {
            let new = !seen.contains(&login.host);
            seen.push(login.host.clone());
            new
        });
        logins.reverse();
        write_json(&self.logins_path, &logins, 0o600)?;
        tracing::info!(hosts = ?logins.iter().map(|l| l.host.as_str()).collect::<Vec<_>>(), "logins por site alterados");
        *self.logins.lock().unwrap() = logins;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::manager;
    use super::*;

    #[test]
    fn base64_encoding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"ana:segredo"), "YW5hOnNlZ3JlZG8=");
        assert_eq!(base64("joão:ç".as_bytes()), "am/Do286w6c=");
    }

    #[tokio::test]
    async fn logins_go_to_their_host_and_are_kept_private() {
        use std::os::unix::fs::PermissionsExt;
        let manager = manager();
        let login = |host: &str, user: &str| SiteLogin { host: host.into(), user: user.into(), password: "segredo".into() };
        manager
            .set_logins(vec![login("https://Arquivos.Exemplo.com/x", "velho"), login("arquivos.exemplo.com", "ana"), login(" ", "x")])
            .unwrap();
        assert_eq!(manager.logins(), vec![login("arquivos.exemplo.com", "ana")]);
        let mode = std::fs::metadata(&manager.logins_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(load_logins(&manager.logins_path), manager.logins());

        let request = |url: &str| AddRequest { url: url.into(), ..Default::default() };
        let auth = |headers: Vec<(String, String)>| headers.into_iter().find(|(n, _)| n == "Authorization").map(|(_, v)| v);
        assert_eq!(auth(manager.headers(&request("https://arquivos.exemplo.com/a.iso"))).as_deref(), Some("Basic YW5hOnNlZ3JlZG8="));
        assert_eq!(auth(manager.headers(&request("https://exemplo.com/a.iso"))), None, "only the host and its subdomains");
        let mut from_browser = request("https://arquivos.exemplo.com/a.iso");
        from_browser.headers.push(("authorization".into(), "Bearer x".into()));
        assert_eq!(manager.headers(&from_browser).iter().filter(|(n, _)| n.eq_ignore_ascii_case("authorization")).count(), 1);
    }

    #[tokio::test]
    async fn a_wrong_proxy_is_refused_and_not_saved() {
        let manager = manager();
        let mut settings = manager.settings();
        settings.proxy = ProxySettings { mode: ProxyMode::Manual, url: "ftp://proxy:21".into() };
        assert!(manager.set_settings(settings.clone()).is_err());
        assert_eq!(manager.settings().proxy.mode, ProxyMode::System);
        settings.proxy.url = "socks5://127.0.0.1:1080".into();
        manager.set_settings(settings).unwrap();
        assert_eq!(manager.settings().proxy.mode, ProxyMode::Manual);
    }
}
