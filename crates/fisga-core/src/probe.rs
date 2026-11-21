//! First request: size, file name and Range support.

use std::time::Duration;

use reqwest::header::{CONTENT_DISPOSITION, CONTENT_RANGE, CONTENT_TYPE, ETAG, RANGE};
use reqwest::{Client, StatusCode};
use url::Url;

use crate::{filename, Error, RequestSpec, Result};

/// How long a server may take to answer (the engine's default idle timeout).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ProbeResult {
    /// URL after redirects.
    pub final_url: Url,
    pub total: Option<u64>,
    /// True when the server honors `Range` and the size is known, so the file can
    /// be split across connections and resumed.
    pub ranges: bool,
    pub file_name: String,
    pub content_type: Option<String>,
    pub etag: Option<String>,
}

/// Parses the total from `bytes 0-0/12345`. Returns None for `*`.
pub(crate) fn content_range_total(value: &str) -> Option<u64> {
    value.rsplit_once('/')?.1.trim().parse().ok()
}

/// Network errors, timeouts, 5xx and 429 are usually temporary.
fn is_transient(error: &Error) -> bool {
    match error {
        Error::Http(e) => !e.is_builder() && !e.is_redirect(),
        Error::Status(s) => s.is_server_error() || *s == StatusCode::TOO_MANY_REQUESTS,
        Error::Timeout => true,
        _ => false,
    }
}

/// Probes the URL, retrying temporary failures (up to 4 attempts).
pub async fn probe(client: &Client, spec: &RequestSpec) -> Result<ProbeResult> {
    probe_within(client, spec, ANSWER_TIMEOUT).await
}

/// `probe`, giving the server `timeout` to answer each attempt.
pub(crate) async fn probe_within(client: &Client, spec: &RequestSpec, timeout: Duration) -> Result<ProbeResult> {
    if crate::ftp::is_ftp(&spec.url) {
        return crate::ftp::probe(spec).await;
    }
    let mut attempt = 0;
    loop {
        match probe_once(client, spec, timeout).await {
            Err(e) if attempt < 3 && is_transient(&e) => {
                attempt += 1;
                tracing::warn!(attempt, error = %e, "sondagem falhou, tentando de novo");
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            }
            result => return result,
        }
    }
}

async fn probe_once(client: &Client, spec: &RequestSpec, timeout: Duration) -> Result<ProbeResult> {
    // A server that takes the connection and never answers would hold the download forever.
    let send = spec.apply(client.get(&spec.url)).header(RANGE, "bytes=0-0").send();
    let resp = tokio::time::timeout(timeout, send).await.map_err(|_| Error::Timeout)??;
    let status = resp.status();
    let headers = resp.headers();
    let header = |name| headers.get(name).and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok());
    // An empty file has no byte 0: the server answers 416 with `bytes */0`.
    let empty = status == StatusCode::RANGE_NOT_SATISFIABLE && header(CONTENT_RANGE).and_then(content_range_total) == Some(0);
    if !status.is_success() && !empty {
        return Err(Error::Status(status));
    }

    let (total, ranges) = if empty {
        (Some(0), true)
    } else if status == StatusCode::PARTIAL_CONTENT {
        let total = header(CONTENT_RANGE).and_then(content_range_total);
        (total, total.is_some())
    } else {
        (resp.content_length(), false)
    };

    let content_type = header(CONTENT_TYPE).map(str::to_owned);
    let etag = header(ETAG).filter(|e| !e.starts_with("W/")).map(str::to_owned);
    let final_url = resp.url().clone();

    let mut name = header(CONTENT_DISPOSITION)
        .and_then(filename::from_content_disposition)
        .or_else(|| filename::from_url(&final_url))
        .or_else(|| Url::parse(&spec.url).ok().as_ref().and_then(filename::from_url))
        .unwrap_or_else(|| "download".to_string());
    if !name.contains('.') {
        if let Some(ext) = content_type.as_deref().and_then(filename::extension_for_mime) {
            name = format!("{name}.{ext}");
        }
    }

    Ok(ProbeResult {
        final_url,
        total,
        ranges,
        file_name: filename::sanitize(&name),
        content_type,
        etag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let spec = RequestSpec::new(format!("http://{}/a.iso", listener.local_addr().unwrap()));
        // Takes the connection, then says nothing.
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let started = std::time::Instant::now();
        let error = probe_once(&Client::new(), &spec, Duration::from_millis(300)).await.unwrap_err();
        assert!(matches!(error, Error::Timeout), "{error:?}");
        assert!(is_transient(&error), "worth another attempt");
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn content_range() {
        assert_eq!(super::content_range_total("bytes 0-0/12345"), Some(12345));
        assert_eq!(super::content_range_total("bytes 0-0/*"), None);
        assert_eq!(super::content_range_total("bytes */0"), Some(0));
    }
}
