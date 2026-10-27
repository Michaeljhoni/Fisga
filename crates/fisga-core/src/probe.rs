//! First request: size, file name and Range support.

use reqwest::header::{CONTENT_DISPOSITION, CONTENT_RANGE, CONTENT_TYPE, ETAG, RANGE};
use reqwest::{Client, StatusCode};
use url::Url;

use crate::{filename, Error, RequestSpec, Result};

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

/// Network errors, 5xx and 429 are usually temporary.
fn is_transient(error: &Error) -> bool {
    match error {
        Error::Http(e) => !e.is_builder() && !e.is_redirect(),
        Error::Status(s) => s.is_server_error() || *s == StatusCode::TOO_MANY_REQUESTS,
        _ => false,
    }
}

/// Probes the URL, retrying temporary failures (up to 4 attempts).
pub async fn probe(client: &Client, spec: &RequestSpec) -> Result<ProbeResult> {
    let mut attempt = 0;
    loop {
        match probe_once(client, spec).await {
            Err(e) if attempt < 3 && is_transient(&e) => {
                attempt += 1;
                tracing::warn!(attempt, error = %e, "sondagem falhou, tentando de novo");
                tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
            }
            result => return result,
        }
    }
}

async fn probe_once(client: &Client, spec: &RequestSpec) -> Result<ProbeResult> {
    let resp = spec
        .apply(client.get(&spec.url))
        .header(RANGE, "bytes=0-0")
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Status(status));
    }

    let headers = resp.headers();
    let header = |name| headers.get(name).and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok());

    let (total, ranges) = if status == StatusCode::PARTIAL_CONTENT {
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
    #[test]
    fn content_range() {
        assert_eq!(super::content_range_total("bytes 0-0/12345"), Some(12345));
        assert_eq!(super::content_range_total("bytes 0-0/*"), None);
    }
}
