//! Downloads over FTP (RFC 959): one connection, in binary mode, passive
//! (EPSV, else PASV), resumed where it stopped with `REST`. The login comes
//! from the address (`ftp://user:password@host/path`), else the anonymous one.
//! No FTPS, and no proxy: HTTP and SOCKS proxies are for HTTP.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use percent_encoding::percent_decode_str;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use fisga_i18n::{tr, trf};
use url::Url;

use crate::control::{ControlFile, SegmentState};
use crate::engine::{finalize, resolve_target, SpeedMeter, SAVE_EVERY};
use crate::probe::ProbeResult;
use crate::{control, filename, DownloadOptions, EngineEvent, Error, Outcome, Progress, RequestSpec, Result, Target};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a reply from the server may take.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_PORT: u16 = 21;
const CHUNK: usize = 64 * 1024;

/// What the address asks for.
struct Address {
    host: String,
    port: u16,
    user: String,
    password: String,
    /// The file, as the server is asked for it: relative to the login's
    /// folder, or absolute when the address starts with `%2F`.
    path: String,
    name: String,
}

impl Address {
    fn parse(url: &Url) -> Result<Address> {
        let host = url.host_str().ok_or_else(|| Error::Other(tr!("endereço FTP sem servidor").into()))?;
        let decode = |text: &str| percent_decode_str(text).decode_utf8_lossy().into_owned();
        let path = decode(url.path().strip_prefix('/').unwrap_or(url.path()));
        if path.is_empty() || path.ends_with('/') {
            return Err(Error::Other(tr!("o endereço FTP é de uma pasta, não de um arquivo").into()));
        }
        if path.contains(['\r', '\n']) {
            return Err(Error::Other(tr!("endereço FTP inválido").into()));
        }
        let (user, password) = match url.username() {
            "" => ("anonymous".to_string(), "fisga@".to_string()),
            user => (decode(user), decode(url.password().unwrap_or_default())),
        };
        let name = filename::from_url(url).unwrap_or_else(|| "download".to_string());
        Ok(Address {
            host: host.trim_start_matches('[').trim_end_matches(']').to_string(),
            port: url.port().unwrap_or(DEFAULT_PORT),
            user,
            password,
            path,
            name: filename::sanitize(&name),
        })
    }
}

/// A reply: its code and the text of its last line.
#[derive(Debug, PartialEq, Eq)]
struct Reply {
    code: u16,
    text: String,
}

impl Reply {
    fn error(&self, what: &str) -> Error {
        let reason = match self.code {
            530 => tr!("o servidor recusou o login (usuário ou senha)").to_string(),
            550 => tr!("o arquivo não existe no servidor, ou o usuário não pode lê-lo").to_string(),
            421 => tr!("o servidor FTP encerrou a conexão (ocupado ou muitas conexões)").to_string(),
            _ => format!("{} {}", self.code, self.text),
        };
        Error::Other(trf!("FTP, {}: {}", what, reason))
    }
}

/// The control connection.
struct Control {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    peer: SocketAddr,
}

impl Control {
    async fn connect(address: &Address) -> Result<Control> {
        let target = (address.host.as_str(), address.port);
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target)).await {
            Ok(stream) => stream?,
            Err(_) => return Err(Error::Other(trf!("o servidor FTP {} não respondeu", address.host))),
        };
        let peer = stream.peer_addr()?;
        let (reader, writer) = stream.into_split();
        let mut control = Control { reader: BufReader::new(reader), writer, peer };
        let greeting = control.reply().await?;
        if greeting.code != 220 {
            return Err(greeting.error(tr!("ao conectar")));
        }
        Ok(control)
    }

    /// Reads one reply, multi-line ones (`213-…` up to `213 …`) whole.
    async fn reply(&mut self) -> Result<Reply> {
        let mut first: Option<u16> = None;
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(REPLY_TIMEOUT, self.reader.read_line(&mut line)).await;
            let read = read.map_err(|_| Error::Other(tr!("o servidor FTP parou de responder").into()))??;
            if read == 0 {
                return Err(Error::Other(tr!("o servidor FTP fechou a conexão").into()));
            }
            let line = line.trim_end_matches(['\r', '\n']);
            let code = line.get(..3).and_then(|code| code.parse::<u16>().ok());
            let last = line.as_bytes().get(3) != Some(&b'-');
            match (first, code) {
                (None, Some(code)) if last => return Ok(Reply { code, text: line[3..].trim().to_string() }),
                (None, Some(code)) => first = Some(code),
                (Some(first), Some(code)) if code == first && last => {
                    return Ok(Reply { code, text: line[3..].trim().to_string() });
                }
                (None, None) => return Err(Error::Other(trf!("resposta FTP inválida: {}", line))),
                _ => {}
            }
        }
    }

    async fn command(&mut self, command: &str) -> Result<Reply> {
        self.writer.write_all(format!("{command}\r\n").as_bytes()).await?;
        self.reply().await
    }

    async fn login(&mut self, address: &Address) -> Result<()> {
        let reply = self.command(&format!("USER {}", address.user)).await?;
        let reply = match reply.code {
            331 | 332 => self.command(&format!("PASS {}", address.password)).await?,
            _ => reply,
        };
        if !matches!(reply.code, 230 | 202) {
            return Err(reply.error(tr!("no login")));
        }
        let binary = self.command("TYPE I").await?;
        if binary.code != 200 {
            return Err(binary.error(tr!("no modo binário")));
        }
        Ok(())
    }

    /// The file's size, when the server says it (`SIZE` is an extension).
    async fn size(&mut self, path: &str) -> Result<Option<u64>> {
        let reply = self.command(&format!("SIZE {path}")).await?;
        match reply.code {
            213 => Ok(reply.text.trim().parse().ok()),
            550 => Err(reply.error(tr!("ao procurar o arquivo"))),
            _ => Ok(None),
        }
    }

    /// A data connection in passive mode, at the control connection's host:
    /// a PASV address inside the server's network (behind NAT) is ignored.
    async fn passive(&mut self) -> Result<TcpStream> {
        let reply = self.command("EPSV").await?;
        let port = match reply.code {
            229 => extended_port(&reply.text),
            _ => {
                let reply = self.command("PASV").await?;
                if reply.code != 227 {
                    return Err(reply.error(tr!("no modo passivo")));
                }
                passive_port(&reply.text)
            }
        };
        let port = port.ok_or_else(|| Error::Other(tr!("FTP: o servidor não informou a porta de dados").into()))?;
        let address = SocketAddr::new(self.peer.ip(), port);
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
            Ok(stream) => Ok(stream?),
            Err(_) => Err(Error::Other(tr!("FTP: a conexão de dados não abriu").into())),
        }
    }

    async fn quit(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.command("QUIT")).await;
    }
}

/// `Entering Extended Passive Mode (|||6446|)` → 6446.
fn extended_port(text: &str) -> Option<u16> {
    let inside = text.split_once('(')?.1.split_once(')')?.0;
    let delimiter = inside.chars().next()?;
    inside.split(delimiter).nth(3)?.parse().ok()
}

/// `Entering Passive Mode (192,168,1,2,25,46)` → 25 × 256 + 46.
fn passive_port(text: &str) -> Option<u16> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let numbers: Vec<u16> = text[start..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .take(6)
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    match numbers.as_slice() {
        [_, _, _, _, high, low] if *high < 256 && *low < 256 => Some(high * 256 + low),
        _ => None,
    }
}

/// Size and name, for the "Novo download" window. FTP servers resume
/// (`REST`): the download is resumable when the size is known.
pub async fn probe(spec: &RequestSpec) -> Result<ProbeResult> {
    let url = Url::parse(&spec.url)?;
    let address = Address::parse(&url)?;
    let mut control = Control::connect(&address).await?;
    control.login(&address).await?;
    let total = control.size(&address.path).await?;
    control.quit().await;
    Ok(ProbeResult { final_url: url, total, ranges: total.is_some(), file_name: address.name, content_type: None, etag: None })
}

/// Where an earlier attempt stopped. A control file (this engine's, or the
/// HTTP engine's, whose connections write all over a preallocated part)
/// says how far the part is complete from its start; a part without one is
/// an older attempt of this engine's, written in order. Never past the file.
fn resume_offset(final_path: &std::path::Path, total: Option<u64>) -> u64 {
    let Some(total) = total else { return 0 };
    let kept = std::fs::metadata(control::part_path(final_path)).map_or(0, |m| m.len());
    let control_file = control::control_path(final_path);
    let done = match control::load(&control_file) {
        Some(saved) if saved.total == total => control::done_prefix(&saved.segments),
        // Another file's, or unreadable.
        Some(_) => 0,
        None if control_file.exists() => 0,
        None => kept,
    };
    if kept > total {
        0
    } else {
        done.min(kept)
    }
}

/// Records that the part holds `done` bytes in order from its start, so this
/// engine and the HTTP one can continue it. Without a size there is nothing
/// to continue: an old control file goes away, never to claim more than the
/// part holds.
async fn save_progress(final_path: &std::path::Path, url: &str, total: Option<u64>, done: u64) {
    let path = control::control_path(final_path);
    let record = total.filter(|total| *total > 0).map(|total| ControlFile {
        version: control::VERSION,
        url: url.to_string(),
        total,
        etag: None,
        segments: vec![SegmentState { start: 0, end: total - 1, done }],
    });
    let saved = tokio::task::spawn_blocking(move || match record {
        Some(record) => control::save(&path, &record),
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    })
    .await;
    if !matches!(saved, Ok(Ok(()))) {
        tracing::warn!(path = %final_path.display(), "não foi possível salvar o arquivo de controle");
    }
}

pub async fn download(
    spec: &RequestSpec,
    target: Target,
    opts: &DownloadOptions,
    events: mpsc::UnboundedSender<EngineEvent>,
    cancel: CancellationToken,
) -> Result<Outcome> {
    let url = Url::parse(&spec.url)?;
    let address = Address::parse(&url)?;
    let connect = async {
        let mut control = Control::connect(&address).await?;
        control.login(&address).await?;
        let total = control.size(&address.path).await?;
        Ok::<_, Error>((control, total))
    };
    let (mut control, total) = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::Cancelled),
        connected = connect => connected?,
    };

    let final_path = resolve_target(target, &address.name, &spec.url, opts.paths.as_ref())?;
    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part = control::part_path(&final_path);
    let mut offset = resume_offset(&final_path, total);
    let _ = events.send(EngineEvent::Started { path: final_path.clone(), total, resumable: total.is_some() });
    if total == Some(0) {
        control.quit().await;
        tokio::fs::write(&final_path, b"").await?;
        let _ = tokio::fs::remove_file(&part).await;
        return Ok(Outcome::Completed(final_path));
    }

    let mut data = control.passive().await?;
    if offset > 0 && control.command(&format!("REST {offset}")).await?.code != 350 {
        // The server cannot resume: from the start.
        offset = 0;
    }
    let retrieve = control.command(&format!("RETR {}", address.path)).await?;
    if !matches!(retrieve.code, 125 | 150) {
        return Err(retrieve.error(tr!("ao pedir o arquivo")));
    }

    // Before the part is cut at `offset`: from now on it holds only that much.
    save_progress(&final_path, &spec.url, total, offset).await;
    let mut file = tokio::fs::OpenOptions::new().create(true).write(true).truncate(offset == 0).open(&part).await?;
    if offset > 0 {
        file.set_len(offset).await?;
        tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(offset)).await?;
    }
    let mut downloaded = offset;
    let mut meter = SpeedMeter::new(downloaded);
    let mut last_report = Instant::now();
    let mut last_save = Instant::now();
    let mut buffer = vec![0u8; CHUNK];
    let paused = loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => break true,
            read = tokio::time::timeout(opts.idle_timeout, data.read(&mut buffer)) => read,
        };
        let n = match read {
            Err(_) => return Err(Error::Other(tr!("tempo esgotado esperando dados do servidor FTP").into())),
            Ok(read) => read?,
        };
        if n == 0 {
            break false;
        }
        file.write_all(&buffer[..n]).await?;
        downloaded += n as u64;
        for limiter in &opts.limiters {
            limiter.consume(n).await;
        }
        if last_report.elapsed() >= Duration::from_millis(500) {
            last_report = Instant::now();
            let speed = meter.update(downloaded);
            let progress = Progress { downloaded, total, speed, active_connections: 1, segments: Vec::new() };
            let _ = events.send(EngineEvent::Progress(progress));
        }
        if last_save.elapsed() >= SAVE_EVERY {
            last_save = Instant::now();
            // On disk first: the record never claims bytes a power cut lost.
            file.flush().await?;
            file.sync_data().await?;
            save_progress(&final_path, &spec.url, total, downloaded).await;
        }
    };
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    drop(data);
    save_progress(&final_path, &spec.url, total, downloaded).await;
    if paused {
        // What arrived stays for `REST` next time.
        tokio::spawn(control.quit());
        return Ok(Outcome::Paused(final_path));
    }
    let done = control.reply().await?;
    control.quit().await;
    if !matches!(done.code, 226 | 250) {
        return Err(done.error(tr!("ao terminar")));
    }
    if let Some(total) = total.filter(|total| downloaded != *total) {
        return Err(Error::Other(trf!("conexão FTP encerrada em {} de {} bytes", downloaded, total)));
    }
    let progress = Progress { downloaded, total: Some(downloaded), speed: 0.0, active_connections: 0, segments: Vec::new() };
    let _ = events.send(EngineEvent::Progress(progress));
    Ok(Outcome::Completed(finalize(&part, final_path).await?))
}

/// `ftp:` addresses.
pub fn is_ftp(url: &str) -> bool {
    url.get(..4).is_some_and(|scheme| scheme.eq_ignore_ascii_case("ftp:"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::net::TcpListener;

    #[test]
    fn addresses() {
        let at = |text: &str| Address::parse(&Url::parse(text).unwrap());
        let anonymous = at("ftp://ftp.exemplo.com/pub/linux/distro%20nova.iso").unwrap();
        assert_eq!((anonymous.host.as_str(), anonymous.port), ("ftp.exemplo.com", 21));
        assert_eq!((anonymous.user.as_str(), anonymous.path.as_str()), ("anonymous", "pub/linux/distro nova.iso"));
        assert_eq!(anonymous.name, "distro nova.iso");
        let login = at("ftp://ana:s%40nha@10.0.0.2:2121/%2Fhome/ana/a.zip").unwrap();
        assert_eq!((login.user.as_str(), login.password.as_str(), login.port), ("ana", "s@nha", 2121));
        assert_eq!(login.path, "/home/ana/a.zip", "%2F: from the root");
        assert!(at("ftp://ftp.exemplo.com/pub/").is_err(), "a folder");
        assert!(at("ftp://ftp.exemplo.com/a%0D%0ADELE%20b").is_err(), "no commands smuggled in the path");
        assert!(is_ftp("FTP://x/a") && !is_ftp("https://x/a"));
    }

    #[test]
    fn passive_replies() {
        assert_eq!(extended_port("Entering Extended Passive Mode (|||6446|)"), Some(6446));
        assert_eq!(passive_port("Entering Passive Mode (192,168,1,2,25,46)."), Some(25 * 256 + 46));
        assert_eq!(passive_port("Entering Passive Mode 192,168,1,2,25,46"), Some(6446));
        assert_eq!(passive_port("nada"), None);
    }

    /// A tiny FTP server for one file, serving each session in turn. `epsv`:
    /// it knows EPSV (else only PASV); `cut`: the first transfer stops after
    /// that many bytes (0: none). Returns its address and the commands seen.
    async fn server(content: Vec<u8>, epsv: bool, cut: usize) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let mut cut = cut;
            while let Ok((socket, _)) = listener.accept().await {
                let (reader, mut writer) = socket.into_split();
                let mut lines = BufReader::new(reader).lines();
                writer.write_all(b"220-Bem-vindo\r\n220 pronto\r\n").await.unwrap();
                let mut data: Option<TcpListener> = None;
                let mut rest = 0usize;
                while let Ok(Some(line)) = lines.next_line().await {
                    log.lock().unwrap().push(line.clone());
                    let (command, argument) = line.split_once(' ').unwrap_or((&line, ""));
                    let reply = match command {
                        "USER" if argument == "anonymous" => "331 senha\r\n".to_string(),
                        "USER" => "530 nao\r\n".to_string(),
                        "PASS" => "230 entrou\r\n".to_string(),
                        "TYPE" => "200 binario\r\n".to_string(),
                        "SIZE" if argument == "pub/a.bin" => format!("213 {}\r\n", content.len()),
                        "SIZE" => "550 nao existe\r\n".to_string(),
                        "EPSV" | "PASV" => {
                            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                            let port = listener.local_addr().unwrap().port();
                            data = Some(listener);
                            match (command, epsv) {
                                ("EPSV", true) => format!("229 Entering Extended Passive Mode (|||{port}|)\r\n"),
                                ("EPSV", false) => "500 nao conheco\r\n".to_string(),
                                _ => format!("227 Entering Passive Mode (10,9,8,7,{},{})\r\n", port / 256, port % 256),
                            }
                        }
                        "REST" => {
                            rest = argument.parse().unwrap();
                            "350 ok\r\n".to_string()
                        }
                        "RETR" => {
                            writer.write_all(b"150 abrindo\r\n").await.unwrap();
                            let (mut socket, _) = data.take().unwrap().accept().await.unwrap();
                            let end = match std::mem::take(&mut cut) {
                                0 => content.len(),
                                cut => (rest + cut).min(content.len()),
                            };
                            socket.write_all(&content[rest..end]).await.unwrap();
                            drop(socket);
                            rest = 0;
                            "226 enviado\r\n".to_string()
                        }
                        "QUIT" => "221 tchau\r\n".to_string(),
                        _ => "502 nao\r\n".to_string(),
                    };
                    if writer.write_all(reply.as_bytes()).await.is_err() || command == "QUIT" {
                        break;
                    }
                }
            }
        });
        (format!("ftp://{address}"), seen)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fisga-ftp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn content() -> Vec<u8> {
        (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }

    async fn fetch(url: &str, dir: &std::path::Path) -> Result<Outcome> {
        let (events, _) = mpsc::unbounded_channel();
        let target = Target::Dir { dir: dir.to_path_buf(), file_name: None };
        download(&RequestSpec::new(url), target, &DownloadOptions::default(), events, CancellationToken::new()).await
    }

    #[tokio::test]
    async fn downloads_a_file() {
        for epsv in [true, false] {
            let (base, seen) = server(content(), epsv, 0).await;
            let probe = probe(&RequestSpec::new(format!("{base}/pub/a.bin"))).await.unwrap();
            assert_eq!((probe.total, probe.ranges, probe.file_name.as_str()), (Some(300_000), true, "a.bin"));
            let dir = scratch(&format!("whole-{epsv}"));
            let Outcome::Completed(path) = fetch(&format!("{base}/pub/a.bin"), &dir).await.unwrap() else { panic!() };
            assert_eq!(std::fs::read(&path).unwrap(), content());
            assert!(!control::part_path(&path).exists());
            let seen = seen.lock().unwrap().clone();
            assert!(seen.contains(&"TYPE I".to_string()) && seen.contains(&"RETR pub/a.bin".to_string()), "{seen:?}");
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    /// What an earlier attempt at `url` left in `dir`: `done` bytes of `a.bin`.
    fn earlier_attempt(dir: &std::path::Path, url: &str, done: usize) {
        let path = dir.join("a.bin");
        std::fs::write(control::part_path(&path), &content()[..done]).unwrap();
        let segments = vec![control::SegmentState { start: 0, end: 299_999, done: done as u64 }];
        let saved = control::ControlFile { version: control::VERSION, url: url.into(), total: 300_000, etag: None, segments };
        control::save(&control::control_path(&path), &saved).unwrap();
    }

    #[tokio::test]
    async fn resumes_where_it_stopped() {
        let (base, seen) = server(content(), true, 0).await;
        let dir = scratch("resume");
        let url = format!("{base}/pub/a.bin");
        earlier_attempt(&dir, &url, 100_000);
        let Outcome::Completed(path) = fetch(&url, &dir).await.unwrap() else { panic!() };
        assert_eq!(path, dir.join("a.bin"));
        assert_eq!(std::fs::read(&path).unwrap(), content());
        assert!(seen.lock().unwrap().contains(&"REST 100000".to_string()));
        assert!(!control::control_path(&path).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_transfer_cut_short_continues_where_it_stopped() {
        let (base, seen) = server(content(), true, 120_000).await;
        let dir = scratch("cut");
        let url = format!("{base}/pub/a.bin");
        let error = fetch(&url, &dir).await.unwrap_err().to_string();
        assert!(error.contains("120000"), "{error}");
        let saved = control::load(&control::control_path(&dir.join("a.bin"))).unwrap();
        assert_eq!(control::done_prefix(&saved.segments), 120_000);
        let Outcome::Completed(path) = fetch(&url, &dir).await.unwrap() else { panic!() };
        assert_eq!(std::fs::read(&path).unwrap(), content());
        assert!(seen.lock().unwrap().contains(&"REST 120000".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn continues_an_http_download_from_its_complete_start() {
        let (base, seen) = server(content(), true, 0).await;
        let dir = scratch("from-http");
        let url = format!("{base}/pub/a.bin");
        let path = dir.join("a.bin");
        // The HTTP engine's part: as big as the file, with holes of zeros.
        let mut part = content();
        part[100_000..200_000].fill(0);
        part[250_000..].fill(0);
        std::fs::write(control::part_path(&path), &part).unwrap();
        let segments = vec![
            SegmentState { start: 0, end: 99_999, done: 100_000 },
            SegmentState { start: 100_000, end: 199_999, done: 0 },
            SegmentState { start: 200_000, end: 299_999, done: 50_000 },
        ];
        let saved = ControlFile { version: control::VERSION, url: url.clone(), total: 300_000, etag: None, segments };
        control::save(&control::control_path(&path), &saved).unwrap();
        let Outcome::Completed(done) = fetch(&url, &dir).await.unwrap() else { panic!() };
        assert_eq!(done, path);
        assert!(std::fs::read(&path).unwrap() == content(), "holes of zeros left in the file");
        assert!(seen.lock().unwrap().contains(&"REST 100000".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn where_an_earlier_attempt_stopped() {
        let dir = scratch("offset");
        let path = dir.join("a.bin");
        assert_eq!(resume_offset(&path, Some(300_000)), 0, "nothing yet");
        std::fs::write(control::part_path(&path), &content()[..1000]).unwrap();
        assert_eq!(resume_offset(&path, Some(300_000)), 1000, "an older attempt of this engine's");
        assert_eq!(resume_offset(&path, Some(500)), 0, "more than the file");
        assert_eq!(resume_offset(&path, None), 0);
        std::fs::write(control::control_path(&path), b"{").unwrap();
        assert_eq!(resume_offset(&path, Some(300_000)), 0, "a control file that cannot be read");
        earlier_attempt(&dir, "ftp://x/a.bin", 600);
        assert_eq!(resume_offset(&path, Some(300_000)), 600);
        assert_eq!(resume_offset(&path, Some(299_999)), 0, "another file's");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_partial_file_of_another_address_is_left_alone() {
        let (base, seen) = server(content(), true, 0).await;
        let dir = scratch("other");
        earlier_attempt(&dir, "ftp://outro.exemplo.com/a.bin", 100_000);
        let Outcome::Completed(path) = fetch(&format!("{base}/pub/a.bin"), &dir).await.unwrap() else { panic!() };
        assert_eq!(path, dir.join("a (1).bin"));
        assert_eq!(std::fs::read(&path).unwrap(), content());
        assert_eq!(std::fs::read(dir.join("a.bin.fisga-part")).unwrap(), &content()[..100_000]);
        assert!(!seen.lock().unwrap().iter().any(|command| command.starts_with("REST")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn says_what_went_wrong() {
        let (base, _) = server(content(), true, 0).await;
        let dir = scratch("errors");
        let missing = fetch(&format!("{base}/pub/outro.bin"), &dir).await.unwrap_err().to_string();
        assert!(missing.contains("não existe"), "{missing}");
        let host = base.trim_start_matches("ftp://");
        let refused = fetch(&format!("ftp://ana:x@{host}/pub/a.bin"), &dir).await.unwrap_err().to_string();
        assert!(refused.contains("login"), "{refused}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
