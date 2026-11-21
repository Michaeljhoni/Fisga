//! File name detection and sanitizing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    // Windows opens a device for these names, with any extension: the file
    // could not be created there, nor copied there later.
    if is_device_name(trimmed) {
        out.push('_');
    }
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

/// CON, PRN, AUX, NUL, COM1-9 and LPT1-9, in any case, before the first dot.
fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    match stem.as_bytes() {
        b"CON" | b"PRN" | b"AUX" | b"NUL" => true,
        [b'C', b'O', b'M', digit] | [b'L', b'P', b'T', digit] => (b'1'..=b'9').contains(digit),
        _ => false,
    }
}

/// `dir/name`, then `dir/name (1).ext`, `dir/name (2).ext`...
pub fn candidates<'a>(dir: &'a Path, name: &'a str) -> impl Iterator<Item = PathBuf> + 'a {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 10 => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    std::iter::once(dir.join(name)).chain((1..).map(move |n| dir.join(format!("{stem} ({n}){ext}"))))
}

/// Returns `dir/name`, or `dir/name (n).ext` if that file (or an unrelated
/// partial download of it) already exists.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    candidates(dir, name).find(|p| !p.exists() && !control::part_path(p).exists()).expect("infinite iterator")
}

/// The final paths of the downloads of one list (the daemon's), each held by
/// one download from the moment it is chosen: two downloads never take the
/// same name, nor write into each other's `.fisga-part`, even before the file
/// exists or while one of them is paused.
#[derive(Debug, Default)]
pub struct Paths {
    held: Mutex<HashMap<PathBuf, u64>>,
}

impl Paths {
    /// Gives `path` to download `id`, which lets go of the one it held;
    /// false when another download holds it.
    pub fn take(&self, id: u64, path: &Path) -> bool {
        let mut held = self.held.lock().unwrap();
        match held.get(path) {
            Some(owner) => *owner == id,
            None => {
                held.retain(|_, owner| *owner != id);
                held.insert(path.to_path_buf(), id);
                true
            }
        }
    }

    /// Download `id` no longer needs its path (finished or removed).
    pub fn release(&self, id: u64) {
        self.held.lock().unwrap().retain(|_, owner| *owner != id);
    }
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

    #[test]
    fn windows_device_names_are_not_used() {
        for (name, safe) in [
            ("CON", "_CON"),
            ("nul.txt", "_nul.txt"),
            ("Aux.tar.gz", "_Aux.tar.gz"),
            ("com1.jpg", "_com1.jpg"),
            ("LPT9", "_LPT9"),
            ("prn .txt", "_prn .txt"),
            ("com10.jpg", "com10.jpg"),
            ("com0", "com0"),
            ("console.log", "console.log"),
            ("lpt.txt", "lpt.txt"),
            ("my con.txt", "my con.txt"),
        ] {
            assert_eq!(sanitize(name), safe, "{name}");
        }
    }

    #[test]
    fn candidate_names() {
        let dir = Path::new("/d");
        let names: Vec<PathBuf> = candidates(dir, "a.tar.gz").take(3).collect();
        assert_eq!(names, [dir.join("a.tar.gz"), dir.join("a.tar (1).gz"), dir.join("a.tar (2).gz")]);
        assert_eq!(candidates(dir, "LEIAME").nth(1), Some(dir.join("LEIAME (1)")));
    }

    #[test]
    fn a_path_has_one_holder() {
        let paths = Paths::default();
        let (a, b) = (Path::new("/d/a.jpg"), Path::new("/d/a (1).jpg"));
        assert!(paths.take(1, a));
        assert!(paths.take(1, a), "again: still its own");
        assert!(!paths.take(2, a), "another download's");
        assert!(paths.take(2, b));
        // Taking another path lets go of the old one.
        assert!(paths.take(1, Path::new("/d/b.jpg")));
        assert!(paths.take(3, a));
        paths.release(3);
        assert!(paths.take(2, a), "released");
        assert!(!paths.take(3, a));
    }
}
