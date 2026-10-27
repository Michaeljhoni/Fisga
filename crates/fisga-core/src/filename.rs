//! File name detection and sanitizing.

use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;
use url::Url;

use crate::control;

/// Extracts the file name from a `Content-Disposition` header value.
/// `filename*=` (RFC 5987) wins over `filename=`.
pub fn from_content_disposition(value: &str) -> Option<String> {
    let mut plain = None;
    for part in value.split(';').map(str::trim) {
        let Some((key, val)) = part.split_once('=') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim();
        if key == "filename*" {
            // charset'lang'percent-encoded
            let encoded = val.splitn(3, '\'').nth(2).unwrap_or(val);
            let decoded = percent_decode_str(encoded.trim_matches('"')).decode_utf8_lossy();
            if !decoded.is_empty() {
                return Some(decoded.into_owned());
            }
        } else if key == "filename" {
            let unquoted = val.trim_matches('"').replace("\\\"", "\"");
            if !unquoted.is_empty() {
                plain = Some(unquoted);
            }
        }
    }
    plain
}

/// Last non-empty path segment of the URL, percent-decoded.
pub fn from_url(url: &Url) -> Option<String> {
    let segment = url.path_segments()?.rfind(|s| !s.is_empty())?;
    let decoded = percent_decode_str(segment).decode_utf8_lossy().into_owned();
    (!decoded.is_empty()).then_some(decoded)
}

/// Extension for common content types, used when the name has none.
pub fn extension_for_mime(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next()?.trim().to_ascii_lowercase();
    Some(match mime.as_str() {
        "application/zip" => "zip",
        "application/pdf" => "pdf",
        "application/x-7z-compressed" => "7z",
        "application/vnd.rar" | "application/x-rar-compressed" => "rar",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-xz" => "xz",
        "application/x-tar" => "tar",
        "application/vnd.debian.binary-package" => "deb",
        "application/x-iso9660-image" => "iso",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        _ => return None,
    })
}

/// Makes a name safe to use as a single path component on Linux (and Windows).
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            c if c.is_control() => '_',
            ':' | '*' | '?' | '"' | '<' | '>' | '|' if cfg!(windows) => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    let mut out = String::new();
    for c in trimmed.chars() {
        if out.len() + c.len_utf8() > 200 {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "download".to_string()
    } else {
        out
    }
}

/// Returns `dir/name`, or `dir/name (n).ext` if that file (or an unrelated
/// partial download of it) already exists.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() && !control::part_path(&candidate).exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 10 => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists() && !control::part_path(p).exists())
        .expect("infinite iterator")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_variants() {
        assert_eq!(from_content_disposition("attachment; filename=\"a b.zip\"").as_deref(), Some("a b.zip"));
        assert_eq!(from_content_disposition("attachment; filename=x.iso").as_deref(), Some("x.iso"));
        assert_eq!(
            from_content_disposition("attachment; filename=\"fallback.txt\"; filename*=UTF-8''ma%C3%A7%C3%A3.txt").as_deref(),
            Some("maçã.txt")
        );
        assert_eq!(from_content_disposition("inline"), None);
    }

    #[test]
    fn url_names() {
        let url = Url::parse("https://example.com/files/My%20File.tar.gz?x=1").unwrap();
        assert_eq!(from_url(&url).as_deref(), Some("My File.tar.gz"));
        assert_eq!(from_url(&Url::parse("https://example.com/").unwrap()), None);
    }

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize("../etc/passwd"), "_etc_passwd");
        assert_eq!(sanitize("  "), "download");
        assert_eq!(sanitize("a\nb.mp4"), "a_b.mp4");
    }
}
