//! Streams (HLS/DASH) and media pages (YouTube, Vimeo...) are handed to yt-dlp,
//! which keeps site extractors up to date and merges audio/video with ffmpeg.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use ldm_ipc::{AddRequest, Cookie, MediaFormat};
use serde_json::Value;
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
        .args(["--concurrent-fragments", "4"]);
    // Per yt-dlp process: with several site videos at once the total is higher.
    let settings = manager.settings();
    apply_proxy(&mut command, &settings.proxy);
    let limit = settings.speed_limit;
    if limit > 0 {
        command.args(["--limit-rate", &limit.to_string()]);
    }
    if let Some(selector) = request.format.as_deref().and_then(format_selector) {
        command.args(["--format", &selector]);
    }
    command
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

/// The proxy setting for yt-dlp. A manual proxy goes in the environment, not
/// in `--proxy`: the command line of a process is visible to other users,
/// and the address may hold a password.
fn apply_proxy(command: &mut Command, proxy: &ldm_ipc::ProxySettings) {
    match proxy.for_ytdlp() {
        None => {}
        Some(url) if url.is_empty() => {
            command.args(["--proxy", ""]);
        }
        Some(url) => {
            for name in ["http_proxy", "https_proxy", "all_proxy", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
                command.env(name, &url);
            }
            command.env_remove("no_proxy").env_remove("NO_PROXY");
        }
    }
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
    Video { name: Option<String>, size: Option<u64>, formats: Vec<MediaFormat> },
    /// A feed, channel, search or playlist: many videos.
    List { title: Option<String> },
}

const AUDIO: &str = "audio";

/// yt-dlp `--format` for a `MediaFormat::key`: the best video up to that
/// height plus the best audio, or the audio alone (M4A preferred, it plays
/// everywhere).
pub fn format_selector(key: &str) -> Option<String> {
    if key == AUDIO {
        return Some("ba[ext=m4a]/ba/b".into());
    }
    let height: u32 = key.parse().ok()?;
    Some(format!("bv*[height<={height}]+ba/b[height<={height}]/bv*+ba/b"))
}

fn as_size(value: &Value) -> Option<u64> {
    value.as_f64().filter(|v| v.is_finite() && *v > 0.0).map(|v| v as u64)
}

fn format_size(format: &Value) -> Option<u64> {
    as_size(&format["filesize"]).or_else(|| as_size(&format["filesize_approx"]))
}

fn codec(format: &Value, key: &str) -> String {
    format.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn has_video(format: &Value) -> bool {
    codec(format, "vcodec") != "none"
}

fn is_audio_only(format: &Value) -> bool {
    !has_video(format) && !matches!(codec(format, "acodec").as_str(), "" | "none")
}

fn height(format: &Value) -> Option<u64> {
    format.get("height").and_then(|h| h.as_u64()).filter(|h| *h > 0)
}

fn largest<'a>(formats: impl Iterator<Item = &'a Value>) -> Option<&'a Value> {
    formats.max_by_key(|f| format_size(f).unwrap_or(0))
}

/// Qualities for the "Novo download" window: one per video height, best
/// first, then audio only. Sizes add the audio track that yt-dlp merges in.
fn media_formats(json: &Value) -> Vec<MediaFormat> {
    let Some(formats) = json.get("formats").and_then(|f| f.as_array()) else { return Vec::new() };
    let best_audio = largest(formats.iter().filter(|f| is_audio_only(f)));
    let m4a_audio = largest(formats.iter().filter(|f| is_audio_only(f) && f["ext"] == "m4a")).or(best_audio);

    let mut heights: Vec<u64> = formats.iter().filter(|f| has_video(f)).filter_map(height).collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    let video_ext = json.get("ext").and_then(|e| e.as_str()).map(str::to_string);
    let mut result: Vec<MediaFormat> = heights
        .into_iter()
        .map(|h| {
            let at_height = || formats.iter().filter(move |f| has_video(f) && height(f) == Some(h));
            // Video-only streams get the best audio merged in; muxed ones already have it.
            let size = match largest(at_height().filter(|f| codec(f, "acodec") == "none")) {
                Some(video) => format_size(video).map(|v| v + best_audio.and_then(format_size).unwrap_or(0)),
                None => largest(at_height()).and_then(format_size),
            };
            MediaFormat { key: h.to_string(), label: format!("{h}p"), ext: video_ext.clone(), size }
        })
        .collect();
    if let Some(audio) = m4a_audio {
        result.push(MediaFormat {
            key: AUDIO.into(),
            label: "Somente áudio".into(),
            ext: audio.get("ext").and_then(|e| e.as_str()).map(str::to_string),
            size: format_size(audio),
        });
    }
    result
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
    PageProbe::Video { name, size, formats: media_formats(json) }
}

/// Asks yt-dlp what the URL is before the user confirms it.
pub async fn probe(id: u64, request: &AddRequest, proxy: &ldm_ipc::ProxySettings) -> Result<PageProbe, String> {
    let mut command = Command::new("yt-dlp");
    command.args(["-J", "--flat-playlist", "--no-playlist", "--no-warnings", "--no-colors"]);
    apply_proxy(&mut command, proxy);
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
            PageProbe::Video { name: Some("Meu vídeo: parte 1_2.webm".into()), size: Some(1500), formats: vec![] }
        );
    }

    #[test]
    fn single_video_size_fields() {
        let video = json!({"_type": "video", "title": "x", "ext": "mp4", "filesize_approx": 2048.0});
        assert_eq!(parse_probe(&video), PageProbe::Video { name: Some("x.mp4".into()), size: Some(2048), formats: vec![] });
        let unknown = json!({"title": "x"});
        assert_eq!(parse_probe(&unknown), PageProbe::Video { name: Some("x".into()), size: None, formats: vec![] });
    }

    #[test]
    fn qualities_of_a_youtube_video() {
        // Trimmed `formats` of a YouTube video: storyboard, audio, video-only and one muxed format.
        let video = json!({"title": "v", "ext": "webm", "formats": [
            {"format_id": "sb0", "ext": "mhtml", "vcodec": "none", "acodec": "none", "height": 90},
            {"format_id": "139", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.5", "filesize": 300},
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "filesize": 800},
            {"format_id": "251", "ext": "webm", "vcodec": "none", "acodec": "opus", "filesize": 1000},
            {"format_id": "18", "ext": "mp4", "vcodec": "avc1", "acodec": "mp4a.40.2", "height": 360, "filesize_approx": 5000},
            {"format_id": "136", "ext": "mp4", "vcodec": "avc1", "acodec": "none", "height": 720, "filesize": 20000},
            {"format_id": "247", "ext": "webm", "vcodec": "vp9", "acodec": "none", "height": 720, "filesize": 15000},
            {"format_id": "137", "ext": "mp4", "vcodec": "avc1", "acodec": "none", "height": 1080, "filesize": 40000}
        ]});
        let PageProbe::Video { formats, .. } = parse_probe(&video) else { panic!("a video") };
        let summary: Vec<(&str, &str, Option<&str>, Option<u64>)> =
            formats.iter().map(|f| (f.key.as_str(), f.label.as_str(), f.ext.as_deref(), f.size)).collect();
        assert_eq!(
            summary,
            [
                ("1080", "1080p", Some("webm"), Some(41000)),
                ("720", "720p", Some("webm"), Some(21000)),
                ("360", "360p", Some("webm"), Some(5000)),
                ("audio", "Somente áudio", Some("m4a"), Some(800)),
            ]
        );
    }

    #[test]
    fn format_selectors() {
        assert_eq!(format_selector("720").as_deref(), Some("bv*[height<=720]+ba/b[height<=720]/bv*+ba/b"));
        assert_eq!(format_selector("audio").as_deref(), Some("ba[ext=m4a]/ba/b"));
        assert_eq!(format_selector("; rm -rf"), None);
    }
}
