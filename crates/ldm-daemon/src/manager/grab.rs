//! IDM's site grabber: the files a page links to and, up to a depth, the
//! files of the pages it links to, on the same site and below the starting
//! folder (a directory listing's "Parent Directory" is not followed).
//!
//! The HTML is read without a parser: the `href`/`src` attributes are enough
//! for links, images, video and audio sources.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use anyhow::{bail, Context};
use ldm_ipc::{AddRequest, Category, GrabbedLink, SourceKind};
use url::Url;

use super::Manager;

/// The most pages read in one search.
const MAX_PAGES: usize = 100;
/// Bigger pages are cut there.
const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
const PAGE_TIMEOUT: Duration = Duration::from_secs(15);

/// Extensions of pages (followed), not files (listed).
const PAGE_EXTENSIONS: &[&str] = &["html", "htm", "xhtml", "shtml", "php", "asp", "aspx", "jsp", "cgi"];

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// The `href` and `src` values of a page, in order.
pub fn extract_links(html: &str) -> Vec<String> {
    let bytes = html.as_bytes();
    let lower: Vec<u8> = bytes.iter().map(u8::to_ascii_lowercase).collect();
    let skip_spaces = |mut at: usize| {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        at
    };
    let mut links = Vec::new();
    let mut i = 0;
    while i < lower.len() {
        let name = match &lower[i..] {
            rest if rest.starts_with(b"href") => 4,
            rest if rest.starts_with(b"src") => 3,
            _ => {
                i += 1;
                continue;
            }
        };
        let at = skip_spaces(i + name);
        if bytes.get(at) != Some(&b'=') {
            i += name;
            continue;
        }
        let start = skip_spaces(at + 1);
        // Quoted, or up to a space or the end of the tag. All the bounds are
        // ASCII characters, so the slice below is on character boundaries.
        let (start, end) = match bytes.get(start) {
            Some(&quote @ (b'"' | b'\'')) => {
                let from = start + 1;
                let end = bytes[from..].iter().position(|b| *b == quote).map_or(bytes.len(), |k| from + k);
                (from, end)
            }
            _ => {
                let end = bytes[start..]
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || *b == b'>')
                    .map_or(bytes.len(), |k| start + k);
                (start, end)
            }
        };
        let value = decode_entities(html[start..end].trim());
        if !value.is_empty() {
            links.push(value);
        }
        i = end.max(i + 1);
    }
    links
}

/// What a link points to, once resolved against its page.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    Page(Url),
    File(GrabbedLink),
}

fn classify(page: &Url, link: &str) -> Option<Found> {
    let mut url = page.join(link).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "ftp") {
        return None;
    }
    url.set_fragment(None);
    let name = ldm_core::filename::from_url(&url).unwrap_or_default();
    let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    let page_like = url.path().ends_with('/')
        || extension.as_deref().map_or(true, |ext| PAGE_EXTENSIONS.contains(&ext));
    Some(match page_like {
        true => Found::Page(url),
        false => {
            let category = Category::detect(&name, SourceKind::Direct);
            Found::File(GrabbedLink { url: url.to_string(), name, category })
        }
    })
}

/// Pages worth following: the same host, at or below the starting folder.
fn inside(start: &Url, url: &Url) -> bool {
    let folder = match start.path().rfind('/') {
        Some(at) => &start.path()[..=at],
        None => "/",
    };
    url.host_str() == start.host_str() && url.port_or_known_default() == start.port_or_known_default() && url.path().starts_with(folder)
}

impl Manager {
    /// Reads `start` and, up to `depth` levels, its pages: every file linked,
    /// once, in the order found; and how many pages were read.
    pub async fn grab(&self, start: &str, depth: u8) -> anyhow::Result<(Vec<GrabbedLink>, usize)> {
        let start = Url::parse(start).context("URL inválida")?;
        if !matches!(start.scheme(), "http" | "https") {
            bail!("o grabber lê páginas http ou https");
        }
        let depth = depth.min(ldm_ipc::MAX_GRAB_DEPTH);
        let client = self.client();
        let mut queue = VecDeque::from([(start.clone(), 0u8)]);
        let mut seen_pages = HashSet::from([start.to_string()]);
        let mut seen_files = HashSet::new();
        let (mut files, mut pages, mut first_error) = (Vec::new(), 0, None);
        while let Some((page, level)) = queue.pop_front() {
            if pages >= MAX_PAGES {
                tracing::info!(start = %start, "grabber: limite de páginas");
                break;
            }
            let html = match tokio::time::timeout(PAGE_TIMEOUT, self.fetch_page(&client, &page)).await {
                Ok(Ok(html)) => html,
                Ok(Err(e)) => {
                    tracing::info!(page = %page, error = %e, "grabber: página ignorada");
                    first_error.get_or_insert(e);
                    continue;
                }
                Err(_) => {
                    first_error.get_or_insert_with(|| anyhow::anyhow!("{page} demorou demais"));
                    continue;
                }
            };
            pages += 1;
            for link in extract_links(&html) {
                match classify(&page, &link) {
                    Some(Found::File(file)) => {
                        if seen_files.insert(file.url.clone()) {
                            files.push(file);
                        }
                    }
                    Some(Found::Page(next)) => {
                        if level < depth && inside(&start, &next) && seen_pages.insert(next.to_string()) {
                            queue.push_back((next, level + 1));
                        }
                    }
                    None => {}
                }
            }
        }
        // Nothing read at all: say why (the first page failed).
        if pages == 0 {
            if let Some(e) = first_error {
                return Err(e.context(format!("não foi possível ler {start}")));
            }
        }
        tracing::info!(start = %start, pages, files = files.len(), "grabber");
        Ok((files, pages))
    }

    async fn fetch_page(&self, client: &reqwest::Client, url: &Url) -> anyhow::Result<String> {
        let headers = self.headers(&AddRequest { url: url.to_string(), ..Default::default() });
        let mut request = client.get(url.clone());
        for (name, value) in &headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let mut response = request.send().await?;
        if !response.status().is_success() {
            bail!("{url}: o servidor respondeu {}", response.status());
        }
        let html = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map_or(true, |kind| kind.contains("html"));
        if !html {
            bail!("{url} não é uma página");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() >= MAX_PAGE_BYTES {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::manager;
    use super::*;

    #[test]
    fn links_in_html() {
        let html = r#"<a href="a.iso">A</a> <A HREF='b.zip'>B</A> <img src=foto.jpg alt=x>
            <a href = "c.pdf?x=1&amp;y=2">C</a> <video data-src="v.mp4"></video> <a href="">vazio</a>
            <p>texto com src sem igual</p> <a href="ç/ação.txt">acentos</a>"#;
        assert_eq!(extract_links(html), ["a.iso", "b.zip", "foto.jpg", "c.pdf?x=1&y=2", "v.mp4", "ç/ação.txt"]);
    }

    #[test]
    fn pages_and_files() {
        let page = Url::parse("https://e.com/pub/iso/").unwrap();
        let file = |link: &str| match classify(&page, link) {
            Some(Found::File(file)) => Some((file.url, file.name, file.category)),
            _ => None,
        };
        assert_eq!(
            file("debian.iso#topo"),
            Some(("https://e.com/pub/iso/debian.iso".into(), "debian.iso".into(), Category::Program))
        );
        assert_eq!(file("/docs/manual%20v2.pdf").map(|f| f.1), Some("manual v2.pdf".into()));
        assert!(matches!(classify(&page, "antigos/"), Some(Found::Page(_))));
        assert!(matches!(classify(&page, "index.html"), Some(Found::Page(_))));
        assert!(matches!(classify(&page, "?C=N;O=D"), Some(Found::Page(_))));
        assert_eq!(classify(&page, "mailto:a@e.com"), None);
        assert_eq!(classify(&page, "javascript:void(0)"), None);

        let start = Url::parse("https://e.com/pub/iso/index.html").unwrap();
        assert!(inside(&start, &Url::parse("https://e.com/pub/iso/antigos/").unwrap()));
        assert!(!inside(&start, &Url::parse("https://e.com/pub/").unwrap()), "not up the tree");
        assert!(!inside(&start, &Url::parse("https://outro.com/pub/iso/").unwrap()));
    }

    /// Serves a small site: /pub/ links to a file, a subfolder, its parent and
    /// another site; /pub/sub/ links to two files, one of them already seen.
    async fn site() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..n]);
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    let body = match path {
                        "/pub/" => r#"<a href="../">Parent</a> <a href="a.iso">a</a> <a href="sub/">sub</a> <a href="http://outro.invalid/x.zip">x</a>"#,
                        "/pub/sub/" => r#"<a href="../a.iso">a</a> <a href="b.pdf">b</a> <a href="mais/">mais</a>"#,
                        "/pub/sub/mais/" => r#"<a href="c.mp3">c</a>"#,
                        "/" => r#"<a href="segredo.zip">fora</a>"#,
                        _ => "",
                    };
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                });
            }
        });
        base
    }

    #[tokio::test]
    async fn grabs_below_the_start_up_to_the_depth() {
        let manager = manager();
        let base = site().await;
        let names = |links: &[GrabbedLink]| links.iter().map(|l| l.name.clone()).collect::<Vec<_>>();

        let (links, pages) = manager.grab(&format!("{base}/pub/"), 0).await.unwrap();
        assert_eq!((names(&links), pages), (vec!["a.iso".to_string(), "x.zip".to_string()], 1));

        let (links, pages) = manager.grab(&format!("{base}/pub/"), 1).await.unwrap();
        assert_eq!(names(&links), ["a.iso", "x.zip", "b.pdf"], "a.iso once; nothing from / (above the start)");
        assert_eq!(pages, 2);

        let (links, _) = manager.grab(&format!("{base}/pub/"), 2).await.unwrap();
        assert_eq!(names(&links).last().map(String::as_str), Some("c.mp3"));
        assert!(manager.grab("ftp://e.com/", 1).await.is_err());
        assert!(manager.grab("http://127.0.0.1:9/", 0).await.is_err(), "nothing read: an error");
    }
}
