//! Posts, profiles and boards of social and art sites, sent by the browser's
//! media gallery (`SourceKind::Gallery`). gallery-dl lists their files, as it
//! knows each site's way to them (every photo of a carousel or a slideshow,
//! the originals), and each file becomes a download of its own, by Fisga's
//! engine, in its type's folder inside the site's (`Imagens/instagram.com`);
//! the gallery then leaves the list.
//!
//! gallery-dl is a separate program: Fisga starts it and reads what it
//! prints, as with yt-dlp. It downloads nothing itself here.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fisga_i18n::{tr, trf};
use fisga_ipc::{AddRequest, ProxySettings, SourceKind};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::manager::{JobResult, Manager};

/// What gallery-dl prints for each file instead of downloading it: a mark,
/// then the file's address, name and extension, each in JSON (which has no
/// tab or line break inside), apart by tabs.
const PRINT: &str = "prepare:FISGAFILE\t{_url!j}\t{filename!j}\t{extension!j}";
const MARK: &str = "FISGAFILE\t";

/// How often the count of files found is shown.
const NOTE_EVERY: Duration = Duration::from_millis(500);
/// gallery-dl's last lines on stderr, kept to tell what went wrong.
const LOG_LINES: usize = 40;

/// A file of the gallery, as gallery-dl found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub url: String,
    /// `Direct`, or `Page` for a video gallery-dl leaves to yt-dlp (its
    /// `ytdl:` addresses).
    pub kind: SourceKind,
    /// Its name and extension, as the site gives them.
    pub name: Option<String>,
}

/// A line of gallery-dl's output: a file it found (see `PRINT`), or `None`.
pub fn parse_line(line: &str) -> Option<Found> {
    let mut fields = line.trim_end_matches(['\r', '\n']).strip_prefix(MARK)?.split('\t');
    // A missing field is printed as None, which is not JSON.
    let text = |field: Option<&str>| match serde_json::from_str::<Value>(field?).ok()? {
        Value::String(text) => Some(text).filter(|text| !text.trim().is_empty()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    };
    let address = text(fields.next())?;
    let (kind, url) = match address.strip_prefix("ytdl:") {
        Some(page) => (SourceKind::Page, page.to_string()),
        None => (SourceKind::Direct, address),
    };
    // Not "text:" (a post's text) nor anything else that is not a file on the web.
    if !matches!(url::Url::parse(&url).ok()?.scheme(), "http" | "https") {
        return None;
    }
    let name = match (text(fields.next()), text(fields.next())) {
        (Some(stem), Some(ext)) => Some(format!("{stem}.{ext}")),
        (stem, _) => stem,
    };
    let name = name.map(|name| fisga_core::filename::sanitize(&name)).filter(|name| !name.is_empty());
    Some(Found { url, kind, name })
}

/// The message of gallery-dl's last error in its log (`[instagram][error]
/// ...`), else of its last warning.
fn last_problem(log: &VecDeque<String>) -> Option<String> {
    let after = |level: &str| {
        let tag = format!("][{level}] ");
        log.iter().rev().find_map(|line| line.split_once(tag.as_str()).map(|(_, message)| message.trim().to_string()))
    };
    after("error").or_else(|| after("warning")).filter(|message| !message.is_empty())
}

/// gallery-dl's complaint, with what to do when it is a known one.
fn explain(problem: Option<String>) -> String {
    let Some(problem) = problem else {
        return tr!("o gallery-dl não encontrou nenhum arquivo nesta página").into();
    };
    let lower = problem.to_ascii_lowercase();
    if lower.contains("unsupported url") {
        return trf!("o gallery-dl não sabe baixar desta página ({})", problem);
    }
    if ["login", "authoriz", "authrequired", "auth required", "401", "403"].iter().any(|word| lower.contains(word)) {
        return trf!("o site pediu login: entre na sua conta no navegador e baixe de novo pela galeria (gallery-dl: {})", problem);
    }
    format!("gallery-dl: {problem}")
}

/// What came of asking gallery-dl.
enum Listing {
    Paused,
    Failed(String),
    /// What it found, and whether it went to the end without an error.
    Done { files: Vec<Found>, complete: bool, problem: Option<String> },
}

/// Lists the gallery's files with gallery-dl and puts each in the queue in
/// its place.
pub async fn run(manager: &Arc<Manager>, id: u64, request: &AddRequest, cancel: CancellationToken) -> JobResult {
    let prepared = tokio::select! {
        prepared = manager.prepare_gallery(id) => prepared,
        _ = cancel.cancelled() => return JobResult::Paused,
    };
    if let Err(error) = prepared {
        return JobResult::Failed(error);
    }
    manager.update(id, |info| {
        info.note = Some(tr!("Procurando os arquivos com o gallery-dl…").into());
        info.connections = 1;
    });
    let (files, complete, problem) = match list(manager, id, request, &cancel).await {
        Listing::Paused => return JobResult::Paused,
        Listing::Failed(error) => return JobResult::Failed(error),
        Listing::Done { files, complete, problem } => (files, complete, problem),
    };
    if files.is_empty() {
        return JobResult::Failed(explain(problem));
    }
    let found = files.len();
    let added = match manager.add_gallery_files(id, files) {
        Ok(added) => added,
        Err(e) => return JobResult::Failed(e.to_string()),
    };
    match (complete, added) {
        (false, _) => JobResult::Failed(trf!(
            "{} de {} arquivos foram para a fila, mas o gallery-dl parou antes do fim; tente de novo para buscar o resto. ({})",
            added,
            found,
            explain(problem)
        )),
        (true, 0) => JobResult::Replaced(trf!("Nada novo: os {} arquivos já estão na pasta ou na lista.", found)),
        (true, _) => JobResult::Replaced(trf!("{} arquivos na fila.", added)),
    }
}

/// The proxy for gallery-dl: in the environment, as for yt-dlp (a command
/// line is visible to other users, and the address may hold a password).
fn apply_proxy(command: &mut Command, proxy: &ProxySettings) {
    match proxy.for_ytdlp() {
        None => {}
        Some(url) if url.is_empty() => {
            // Nor the system's, which Windows keeps in the registry.
            command.args(["-o", "proxy-env=false"]);
            for name in crate::ytdlp::PROXY_VARIABLES {
                command.env_remove(name);
            }
        }
        Some(url) => {
            for name in crate::ytdlp::PROXY_VARIABLES {
                command.env(name, &url);
            }
            command.env_remove("no_proxy").env_remove("NO_PROXY");
        }
    }
}

/// The command that lists the files at the gallery's address.
fn command(request: &AddRequest, cookies: Option<&Path>, proxy: &ProxySettings) -> Command {
    let mut command = crate::tools::gallery_dl();
    command
        // One line per file, instead of the file; nothing else on stdout.
        .args(["--print", PRINT])
        // UTF-8, whatever the system's code page (Windows), and each line as
        // it comes (into a pipe, Python would hold them back): the count of
        // files found moves while a profile is read.
        .args(["-o", r#"output.stdout={"encoding": "utf-8", "line_buffering": true}"#, "-o", "output.stderr=utf-8"])
        // A profile may hold tens of thousands: as many as a batch takes.
        .args(["--range", &format!("1-{}", fisga_ipc::MAX_BATCH)]);
    if let Some(user_agent) = &request.user_agent {
        command.args(["--user-agent", user_agent]);
    }
    if let Some(file) = cookies {
        command.arg("--cookies").arg(file);
    }
    apply_proxy(&mut command, proxy);
    command.arg("--").arg(&request.url);
    command
}

async fn list(manager: &Arc<Manager>, id: u64, request: &AddRequest, cancel: &CancellationToken) -> Listing {
    // The browser's cookies for the site (its login), deleted when this ends.
    let cookies = match request.cookies.is_empty() {
        true => None,
        false => match crate::ytdlp::write_cookies_file(&format!("{id}-gallery"), &request.cookies) {
            Ok(file) => Some(file),
            Err(e) => {
                tracing::warn!(error = %e, "não foi possível gravar os cookies para o gallery-dl");
                None
            }
        },
    };
    let mut command = command(request, cookies.as_ref().map(|file| file.path()), &manager.settings().proxy);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // Its own program starts another one (PyInstaller): both stop together.
    crate::shell::Group::prepare(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Listing::Failed(
                tr!(
                    "o gallery-dl não está instalado. Instale pelo gerenciador de pacotes da sua distro \
                     (ex.: pacman -S gallery-dl)."
                )
                .into(),
            )
        }
        Err(e) => return Listing::Failed(trf!("não foi possível iniciar o gallery-dl: {}", e)),
    };
    let mut group = crate::shell::Group::of(&child);

    // stdout (the files) and stderr (its log), line by line.
    let (lines_tx, mut lines) = mpsc::unbounded_channel::<(bool, String)>();
    let streams = [
        child.stdout.take().map(|s| (true, Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)),
        child.stderr.take().map(|s| (false, Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)),
    ];
    for (out, stream) in streams.into_iter().flatten() {
        let tx = lines_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stream).split(b'\n');
            while let Ok(Some(line)) = reader.next_segment().await {
                if tx.send((out, String::from_utf8_lossy(&line).trim_end_matches('\r').to_string())).is_err() {
                    break;
                }
            }
        });
    }
    drop(lines_tx);

    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let mut log = VecDeque::new();
    let mut said = Instant::now();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                group.stop(&mut child).await;
                return Listing::Paused;
            }
            line = lines.recv() => {
                let Some((out, line)) = line else { break };
                if !out {
                    tracing::debug!(id, "gallery-dl: {line}");
                    if log.len() == LOG_LINES {
                        log.pop_front();
                    }
                    log.push_back(strip_colors(&line));
                    continue;
                }
                let Some(found) = parse_line(&line) else { continue };
                if seen.insert(found.url.clone()) {
                    files.push(found);
                }
                if said.elapsed() >= NOTE_EVERY {
                    said = Instant::now();
                    let count = files.len();
                    manager.update(id, |info| info.note = Some(trf!("Procurando os arquivos com o gallery-dl… {} encontrados", count)));
                }
            }
        }
    }
    let status = child.wait().await;
    drop(cookies);
    let problem = last_problem(&log);
    match status {
        Ok(status) => {
            if !status.success() {
                tracing::warn!(id, %status, problem = problem.as_deref().unwrap_or(""), "o gallery-dl terminou com erro");
            }
            Listing::Done { files, complete: status.success(), problem }
        }
        Err(e) => Listing::Failed(e.to_string()),
    }
}

/// A log line without the terminal's color codes, should gallery-dl send them.
fn strip_colors(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // ESC [ ... a letter
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &Command) -> Vec<String> {
        command.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn the_files_gallery_dl_prints() {
        // What gallery-dl 1.32 prints for an Instagram post with two photos and a video.
        let photo = parse_line("FISGAFILE\t\"https://scontent.cdninstagram.com/v/t51/1_n.jpg?stp=x&_nc_ht=a\"\t\"1_n\"\t\"jpg\"").unwrap();
        assert_eq!(photo.url, "https://scontent.cdninstagram.com/v/t51/1_n.jpg?stp=x&_nc_ht=a");
        assert_eq!(photo.kind, SourceKind::Direct);
        assert_eq!(photo.name.as_deref(), Some("1_n.jpg"));
        // Names with what Windows refuses, and accents in JSON escapes.
        let odd = parse_line("FISGAFILE\t\"https://e.com/a\"\t\"Caf\\u00e9: \\\"praia\\\"\"\t\"png\"\r").unwrap();
        let expected = if cfg!(windows) { "Café_ _praia_.png" } else { "Café: \"praia\".png" };
        assert_eq!(odd.name.as_deref(), Some(expected));
        // Without a name (None is what gallery-dl prints for a missing field).
        let bare = parse_line("FISGAFILE\t\"https://pbs.twimg.com/media/Gx1?format=jpg&name=orig\"\tNone\tNone").unwrap();
        assert_eq!(bare.name, None);
        let numbered = parse_line("FISGAFILE\t\"https://e.com/x\"\t12345\t\"mp4\"").unwrap();
        assert_eq!(numbered.name.as_deref(), Some("12345.mp4"));
        // A video gallery-dl hands to yt-dlp.
        let video = parse_line("FISGAFILE\t\"ytdl:https://v.redd.it/abc\"\t\"abc\"\t\"mp4\"").unwrap();
        assert_eq!((video.url.as_str(), video.kind), ("https://v.redd.it/abc", SourceKind::Page));
        // Not files: other lines, a post's text, broken JSON.
        assert_eq!(parse_line("https://e.com/a.jpg"), None);
        assert_eq!(parse_line("FISGAFILE\t\"text:Hello\"\t\"a\"\t\"txt\""), None);
        assert_eq!(parse_line("FISGAFILE\t\"file:///etc/passwd\"\t\"a\"\t\"txt\""), None);
        assert_eq!(parse_line("FISGAFILE\t\"https://e.com/a"), None);
        assert_eq!(parse_line("FISGAFILE\tNone\tNone\tNone"), None);
    }

    #[test]
    fn what_went_wrong_from_the_log() {
        let log = |lines: &[&str]| lines.iter().map(|l| l.to_string()).collect::<VecDeque<_>>();
        let auth = log(&[
            "[instagram][warning] HTTP redirect to login page (https://www.instagram.com/accounts/login/)",
            "[instagram][error] AuthorizationError: Login required",
        ]);
        assert_eq!(last_problem(&auth).as_deref(), Some("AuthorizationError: Login required"));
        assert!(explain(last_problem(&auth)).starts_with("o site pediu login"));
        let warned = log(&["[twitter][info] Requesting guest token", "[twitter][warning] Tweet 1 unavailable"]);
        assert_eq!(last_problem(&warned).as_deref(), Some("Tweet 1 unavailable"));
        let unsupported = log(&["[gallery-dl][error] Unsupported URL 'https://example.com/'"]);
        assert!(explain(last_problem(&unsupported)).starts_with("o gallery-dl não sabe baixar desta página"));
        assert_eq!(explain(None), "o gallery-dl não encontrou nenhum arquivo nesta página");
        assert_eq!(strip_colors("\u{1b}[1;31m[x][error] Falhou\u{1b}[0m"), "[x][error] Falhou");
    }

    #[test]
    fn the_command_line() {
        let cookies = std::env::temp_dir().join("cookies-7-gallery.txt");
        let request = AddRequest {
            url: "https://www.instagram.com/p/C1a2b3/".into(),
            user_agent: Some("Mozilla/5.0 (X11; Linux x86_64)".into()),
            ..Default::default()
        };
        let none = ProxySettings { mode: fisga_ipc::ProxyMode::None, ..Default::default() };
        let line = args(&command(&request, Some(&cookies), &none));
        let at = |flag: &str| line.iter().position(|a| a == flag).map(|i| line[i + 1].as_str());
        assert_eq!(at("--print"), Some(PRINT));
        assert_eq!(at("--user-agent"), Some("Mozilla/5.0 (X11; Linux x86_64)"));
        assert_eq!(at("--cookies"), Some(cookies.to_string_lossy().as_ref()));
        assert_eq!(at("--range"), Some("1-5000"));
        assert!(line.windows(2).any(|w| w == ["-o", "proxy-env=false"]), "{line:?}");
        assert_eq!(&line[line.len() - 2..], ["--", "https://www.instagram.com/p/C1a2b3/"], "the address cannot pass for an option");
        // A manual proxy goes in the environment, never on the command line.
        let manual = ProxySettings { mode: fisga_ipc::ProxyMode::Manual, url: "http://u:segredo@proxy:3128".into() };
        let command = command(&request, None, &manual);
        assert!(!args(&command).iter().any(|a| a.contains("segredo") || a == "--cookies"));
        let https = command.as_std().get_envs().find(|(name, _)| *name == "https_proxy").and_then(|(_, value)| value);
        assert_eq!(https.map(|v| v.to_string_lossy().into_owned()).as_deref(), Some("http://u:segredo@proxy:3128"));
    }
}
