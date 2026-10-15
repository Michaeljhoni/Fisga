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
    #[error("download cancelado")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub const DEFAULT_USER_AGENT: &str = concat!("ldm/", env!("CARGO_PKG_VERSION"));

/// Builds the HTTP client shared by all downloads. Per-download headers come from
/// [`RequestSpec`].
pub fn build_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(DEFAULT_USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(20))
        .pool_max_idle_per_host(16)
        .build()?)
}
