//! Streams (HLS/DASH) and media pages (YouTube, Vimeo...) are handed to yt-dlp,
//! which keeps site extractors up to date and merges audio/video with ffmpeg.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use ldm_ipc::{AddRequest, Cookie};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::manager::{default_download_dir, JobResult, Manager};

const PROGRESS_TEMPLATE: &str = "download:LDMPROG %(progress.downloaded_bytes)s %(progress.total_bytes)s \
     %(progress.total_bytes_estimate)s %(progress.speed)s";

/// Netscape cookies.txt, readable only by the user, deleted after the run.
fn write_cookies_file(id: u64, cookies: &[Cookie]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = ldm_ipc::runtime_dir().join(format!("cookies-{id}.txt"));
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
        .args(["--progress-template", PROGRESS_TEMPLATE])
        .args(["--print", "before_dl:LDMNAME %(title)s"])
        .args(["--print", "after_move:LDMFILE %(filepath)s"])
        .args(["--concurrent-fragments", "4"])
        .arg("--paths")
        .arg(&dir)
        .args(["--output", &template]);
    if let Some(referrer) = &request.referrer {
        command.args(["--referer", referrer]);
    }
    if let Some(user_agent) = &request.user_agent {
        command.args(["--user-agent", user_agent]);
    }
    let cookies_file = if request.cookies.is_empty() {
        None
    } else {
        match write_cookies_file(id, &request.cookies) {
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
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);

    let result = run_child(manager, id, command, cancel).await;
    if let Some(path) = cookies_file {
        let _ = tokio::fs::remove_file(path).await;
    }
    result
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
        Ok(status) if status.success() => match final_path {
            Some(path) => JobResult::Completed(path),
            None => JobResult::Failed("o yt-dlp terminou sem informar o arquivo gerado".into()),
        },
        Ok(status) => {
            JobResult::Failed(last_error.unwrap_or_else(|| format!("o yt-dlp terminou com {status}")))
        }
        Err(e) => JobResult::Failed(e.to_string()),
    }
}
