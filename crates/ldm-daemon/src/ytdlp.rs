//! Streams (HLS/DASH) and media pages (YouTube, Vimeo...) are handed to yt-dlp,
//! which keeps site extractors up to date and merges audio/video with ffmpeg.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use ldm_ipc::{AddRequest, Cookie};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::manager::{default_download_dir, JobResult, Manager};

const PROGRESS_TEMPLATE: &str = "download:LDMPROG %(progress.downloaded_bytes)s %(progress.total_bytes)s \
     %(progress.total_bytes_estimate)s %(progress.speed)s";

/// How long the pre-download check (`yt-dlp -J`) may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Netscape cookies.txt, readable only by the user, deleted after the run.
fn write_cookies_file(name: &str, cookies: &[Cookie]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = ldm_ipc::runtime_dir().join(format!("cookies-{name}.txt"));
    let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&path)?;
    writeln!(file, "# Netscape HTTP Cookie File")?;
    for c in cookies {
        let domain = if !c.host_only && !c.domain.starts_with('.') { format!(".{}", c.domain) } else { c.domain.clone() };
        let subdomains = if domain.starts_with('.') { "TRUE" } else { "FALSE" };
        let secure = if c.secure { "TRUE" } else { "FALSE" };
        let expires = c.expiration_date.map(|e| e as i64).unwrap_or(0);
        writeln!(file, "{domain}\t{subdomains}\t{}\t{secure}\t{expires}\t{}\t{}", c.path, c.name, c.value)?;
    }
    Ok(path)
}

fn parse_number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|v| v.is_finite())
}

pub async fn run(manager: &Arc<Manager>, id: u64, request: &AddRequest, cancel: CancellationToken) -> JobResult {
    let dir = request.output_dir.clone().unwrap_or_else(default_download_dir);
    let template = match &request.file_name {
        Some(name) => {
            let name = ldm_core::filename::sanitize(name);
            let stem = name.rsplit_once('.').map(|(stem, _)| stem.to_string()).unwrap_or(name);
            format!("{stem}.%(ext)s")
        }
        None => "%(title).150B [%(id)s].%(ext)s".to_string(),
    };

    let mut command = Command::new("yt-dlp");
    command
        .args(["--newline", "--no-colors", "--no-playlist", "--progress"])
        // One media file per download, always: a feed or channel URL that slips
        // through must not turn into an endless series of downloads.
        .args(["--max-downloads", "1"])
        .args(["--progress-template", PROGRESS_TEMPLATE])
        .args(["--print", "before_dl:LDMNAME %(title)s"])
        .args(["--print", "after_move:LDMFILE %(filepath)s"])
        .args(["--concurrent-fragments", "4"])
        .arg("--paths")
        .arg(&dir)
        .args(["--output", &template]);
    let cookies_file = add_request_options(&mut command, &id.to_string(), request);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);

    let result = run_child(manager, id, command, cancel).await;
    if let Some(path) = cookies_file {
        let _ = tokio::fs::remove_file(path).await;
    }
    result
}

/// Referer, user agent, cookies and the URL itself. Returns the cookies file to delete.
fn add_request_options(command: &mut Command, name: &str, request: &AddRequest) -> Option<PathBuf> {
    if let Some(referrer) = &request.referrer {
        command.args(["--referer", referrer]);
    }
    if let Some(user_agent) = &request.user_agent {
        command.args(["--user-agent", user_agent]);
    }
    let cookies_file = if request.cookies.is_empty() {
        None
    } else {
        match write_cookies_file(name, &request.cookies) {
            Ok(path) => {
                command.arg("--cookies").arg(&path);
                Some(path)
            }
            Err(e) => {
                tracing::warn!(error = %e, "não foi possível gravar os cookies para o yt-dlp");
                None
            }
        }
    };
    command.arg("--").arg(&request.url);
    cookies_file
}

/// What a page URL holds, according to `yt-dlp -J`.
#[derive(Debug, PartialEq)]
pub enum PageProbe {
    Video { name: Option<String>, size: Option<u64> },
    /// A feed, channel, search or playlist: many videos.
    List { title: Option<String> },
}

fn as_size(value: &serde_json::Value) -> Option<u64> {
    value.as_f64().filter(|v| v.is_finite() && *v > 0.0).map(|v| v as u64)
}

pub fn parse_probe(json: &serde_json::Value) -> PageProbe {
    let text = |key: &str| json.get(key).and_then(|v| v.as_str()).map(str::to_string).filter(|s| !s.is_empty());
    let kind = json.get("_type").and_then(|v| v.as_str()).unwrap_or("video");
    if matches!(kind, "playlist" | "multi_video") || json.get("entries").is_some() {
        return PageProbe::List { title: text("title") };
    }
    let size = as_size(&json["filesize"]).or_else(|| as_size(&json["filesize_approx"])).or_else(|| {
        let formats = json.get("requested_formats")?.as_array()?;
        let sizes: Vec<u64> = formats
            .iter()
            .filter_map(|f| as_size(&f["filesize"]).or_else(|| as_size(&f["filesize_approx"])))
            .collect();
        (!sizes.is_empty()).then(|| sizes.iter().sum())
    });
    let name = text("title").map(|title| match text("ext") {
        Some(ext) => ldm_core::filename::sanitize(&format!("{title}.{ext}")),
        None => ldm_core::filename::sanitize(&title),
    });
    PageProbe::Video { name, size }
}

/// Asks yt-dlp what the URL is before the user confirms it.
pub async fn probe(id: u64, request: &AddRequest) -> Result<PageProbe, String> {
    let mut command = Command::new("yt-dlp");
    command.args(["-J", "--flat-playlist", "--no-playlist", "--no-warnings", "--no-colors"]);
    let cookies_file = add_request_options(&mut command, &format!("{id}-probe"), request);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output()).await;
    if let Some(path) = cookies_file {
        let _ = tokio::fs::remove_file(path).await;
    }
    let output = match output {
        Err(_) => return Err("o yt-dlp demorou demais para responder".into()),
        Ok(Err(e)) => return Err(format!("não foi possível iniciar o yt-dlp: {e}")),
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error = stderr.lines().rev().find(|l| l.starts_with("ERROR:")).unwrap_or("o yt-dlp falhou");
        return Err(error.trim_start_matches("ERROR:").trim().to_string());
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("resposta inválida do yt-dlp: {e}"))?;
    Ok(parse_probe(&json))
}

async fn run_child(manager: &Arc<Manager>, id: u64, mut command: Command, cancel: CancellationToken) -> JobResult {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return JobResult::Failed(
                "yt-dlp não está instalado. Instale pelo gerenciador de pacotes da sua distro \
                 (ex.: pacman -S yt-dlp ffmpeg)."
                    .into(),
            )
        }
        Err(e) => return JobResult::Failed(format!("não foi possível iniciar o yt-dlp: {e}")),
    };

    let (lines_tx, mut lines) = mpsc::unbounded_channel::<String>();
    for stream in [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = lines_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stream).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    drop(lines_tx);

    manager.update(id, |info| {
        info.resumable = true;
        info.connections = 1;
    });
    let mut final_path: Option<PathBuf> = None;
    let mut last_error: Option<String> = None;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // yt-dlp keeps its .part files and continues them on the next run.
                let _ = child.kill().await;
                return JobResult::Paused;
            }
            line = lines.recv() => {
                let Some(line) = line else { break };
                if let Some(rest) = line.strip_prefix("LDMPROG ") {
                    let fields: Vec<&str> = rest.split_whitespace().collect();
                    let downloaded = fields.first().and_then(|v| parse_number(v));
                    let total = fields.get(1).and_then(|v| parse_number(v))
                        .or_else(|| fields.get(2).and_then(|v| parse_number(v)));
                    let speed = fields.get(3).and_then(|v| parse_number(v));
                    manager.update(id, |info| {
                        if let Some(d) = downloaded { info.downloaded = d as u64; }
                        if let Some(t) = total { info.total = Some(t as u64); }
                        info.speed = speed.unwrap_or(0.0);
                    });
                } else if let Some(title) = line.strip_prefix("LDMNAME ") {
                    let title = title.trim().to_string();
                    if !title.is_empty() && title != "NA" {
                        manager.update(id, |info| info.name = title);
                    }
                } else if let Some(path) = line.strip_prefix("LDMFILE ") {
                    let path = PathBuf::from(path.trim());
                    manager.update(id, |info| info.path = Some(path.clone()));
                    final_path = Some(path);
                } else if line.starts_with("ERROR:") {
                    tracing::warn!(id, "{line}");
                    last_error = Some(line.trim_start_matches("ERROR:").trim().to_string());
                }
            }
        }
    }

    match child.wait().await {
        // 101: stopped by --max-downloads after the one file we wanted.
        Ok(status) if status.success() || status.code() == Some(101) => match final_path {
            Some(path) => JobResult::Completed(path),
            None => JobResult::Failed("o yt-dlp terminou sem informar o arquivo gerado".into()),
        },
        Ok(status) => {
            JobResult::Failed(last_error.unwrap_or_else(|| format!("o yt-dlp terminou com {status}")))
        }
        Err(e) => JobResult::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn youtube_home_is_a_list() {
        // Trimmed output of `yt-dlp -J --flat-playlist "https://www.youtube.com/?gl=BR&tab=w1"`.
        let home = json!({"id": "recommended", "title": "recommended", "_type": "playlist", "entries": [],
            "extractor": "youtube:tab", "webpage_url": "https://www.youtube.com/feed/recommended"});
        assert_eq!(parse_probe(&home), PageProbe::List { title: Some("recommended".into()) });
    }

    #[test]
    fn single_video_with_separate_audio_and_video() {
        let video = json!({"id": "abc", "title": "Meu vídeo: parte 1/2", "ext": "webm",
            "requested_formats": [{"filesize": 1000}, {"filesize_approx": 500}]});
        assert_eq!(
            parse_probe(&video),
            PageProbe::Video { name: Some("Meu vídeo: parte 1_2.webm".into()), size: Some(1500) }
        );
    }

    #[test]
    fn single_video_size_fields() {
        let video = json!({"_type": "video", "title": "x", "ext": "mp4", "filesize_approx": 2048.0});
        assert_eq!(parse_probe(&video), PageProbe::Video { name: Some("x.mp4".into()), size: Some(2048) });
        let unknown = json!({"title": "x"});
        assert_eq!(parse_probe(&unknown), PageProbe::Video { name: Some("x".into()), size: None });
    }
}
