//! LDM download engine.
//!
//! - [`probe`] asks the server for size, file name and Range support.
//! - [`download`] runs a download: several connections write directly at their
//!   offsets in a preallocated `*.ldm-part` file, and a connection that finishes
//!   early splits the largest remaining segment in half (dynamic segmentation).
//!   Progress is persisted in `*.ldm-part.json` so the download can be resumed.

pub mod control;
pub mod engine;
pub mod filename;
mod fileio;
pub mod limit;
pub mod probe;

use std::time::Duration;

pub use control::SegmentState;
pub use engine::{download, DownloadOptions, EngineEvent, Outcome, Progress, Target};
pub use limit::RateLimiter;
pub use probe::{probe, ProbeResult};

/// What to fetch and which extra headers (Referer, Cookie, User-Agent...) to send.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RequestSpec {
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

impl RequestSpec {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), headers: Vec::new() }
    }

    /// The spec for `final_url`, where the first request was redirected: the
    /// credentials (`Authorization`, `Cookie`) go only to the host they were
    /// meant for, and never from https down to http, as reqwest does for the
    /// redirect itself. The engine downloads from `final_url` directly.
    pub fn for_final_url(&self, final_url: &url::Url) -> RequestSpec {
        let keep = url::Url::parse(&self.url).is_ok_and(|from| {
            let upgrade = from.scheme() == "http" && final_url.scheme() == "https";
            let downgrade = from.scheme() == "https" && final_url.scheme() == "http";
            from.host_str() == final_url.host_str()
                && !downgrade
                && (upgrade || from.port_or_known_default() == final_url.port_or_known_default())
        });
        if keep {
            return self.clone();
        }
        let headers = self
            .headers
            .iter()
            .filter(|(name, _)| !name.eq_ignore_ascii_case("authorization") && !name.eq_ignore_ascii_case("cookie"))
            .cloned()
            .collect();
        RequestSpec { url: self.url.clone(), headers }
    }

    pub(crate) fn apply(&self, mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.headers {
            rb = rb.header(name.as_str(), value.as_str());
        }
        rb.header(reqwest::header::ACCEPT_ENCODING, "identity")
    }
}

/// `reqwest::Error` hides its cause (DNS, TLS, connection reset...) behind
/// `source()`; show the whole chain.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("erro HTTP: {}", error_chain(.0))]
    Http(#[from] reqwest::Error),
    #[error("erro de E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("o servidor respondeu com status {0}")]
    Status(reqwest::StatusCode),
    #[error("URL inválida: {0}")]
    Url(#[from] url::ParseError),
    /// 503 or 429: too many connections, or the server is overloaded.
    #[error("o servidor está ocupado (status {status})")]
    Throttled { status: reqwest::StatusCode, retry_after: Option<Duration> },
    #[error("download cancelado")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Retrying will not help: the server refuses this request (403, 404, 410...).
    pub fn is_permanent(&self) -> bool {
        matches!(self, Error::Status(s) if s.is_client_error()
            && *s != reqwest::StatusCode::REQUEST_TIMEOUT
            && *s != reqwest::StatusCode::TOO_MANY_REQUESTS)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub const DEFAULT_USER_AGENT: &str = concat!("ldm/", env!("CARGO_PKG_VERSION"));

/// The proxy the HTTP client goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proxy<'a> {
    /// `http_proxy`, `https_proxy`, `all_proxy` and `no_proxy` from the environment.
    System,
    /// None, whatever the environment says.
    Direct,
    /// `http://host:port`, `socks5://user:password@host:port`, `socks5h://...`.
    Url(&'a str),
}

/// Builds the HTTP client shared by all downloads, with the system's proxy.
/// Per-download headers come from [`RequestSpec`].
pub fn build_client() -> Result<reqwest::Client> {
    build_client_with(Proxy::System)
}

pub fn build_client_with(proxy: Proxy) -> Result<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .user_agent(DEFAULT_USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(20))
        // HTTP/2 would carry every segment over one TCP connection, sharing one
        // congestion window: the point of several connections is several sockets.
        .http1_only()
        // Notices a connection a NAT or Wi-Fi change silently dropped.
        .tcp_keepalive(Duration::from_secs(60))
        .pool_max_idle_per_host(16);
    let builder = match proxy {
        Proxy::System => builder,
        Proxy::Direct => builder.no_proxy(),
        Proxy::Url(url) => builder.proxy(reqwest::Proxy::all(url)?),
    };
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_stay_with_their_host() {
        let spec = RequestSpec {
            url: "http://arquivos.exemplo.com/a.iso".into(),
            headers: vec![
                ("Authorization".into(), "Basic x".into()),
                ("cookie".into(), "s=1".into()),
                ("Referer".into(), "https://exemplo.com/".into()),
            ],
        };
        let names = |spec: RequestSpec| spec.headers.into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        let at = |url: &str| spec.for_final_url(&url::Url::parse(url).unwrap());
        assert_eq!(names(at("http://arquivos.exemplo.com/b.iso")).len(), 3, "same host");
        assert_eq!(names(at("https://arquivos.exemplo.com/a.iso")).len(), 3, "upgraded to https");
        assert_eq!(names(at("https://cdn.outro.net/a.iso")), ["Referer"], "another host");
        assert_eq!(names(at("http://arquivos.exemplo.com:8080/a.iso")), ["Referer"], "another port");
        let secure = RequestSpec { url: "https://arquivos.exemplo.com/a.iso".into(), ..spec.clone() };
        let down = secure.for_final_url(&url::Url::parse("http://arquivos.exemplo.com/a.iso").unwrap());
        assert_eq!(names(down), ["Referer"], "never down to http");
    }
}
