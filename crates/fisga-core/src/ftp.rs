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
use url::Url;

use crate::engine::{finalize, resolve_target, SpeedMeter};
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
        let host = url.host_str().ok_or_else(|| Error::Other("endereço FTP sem servidor".into()))?;
        let decode = |text: &str| percent_decode_str(text).decode_utf8_lossy().into_owned();
        let path = decode(url.path().strip_prefix('/').unwrap_or(url.path()));
        if path.is_empty() || path.ends_with('/') {
            return Err(Error::Other("o endereço FTP é de uma pasta, não de um arquivo".into()));
        }
        if path.contains(['\r', '\n']) {
            return Err(Error::Other("endereço FTP inválido".into()));
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
            530 => "o servidor recusou o login (usuário ou senha)".to_string(),
            550 => "o arquivo não existe no servidor, ou o usuário não pode lê-lo".to_string(),
            421 => "o servidor FTP encerrou a conexão (ocupado ou muitas conexões)".to_string(),
            _ => format!("{} {}", self.code, self.text),
        };
        Error::Other(format!("FTP, {what}: {reason}"))
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
            Err(_) => return Err(Error::Other(format!("o servidor FTP {} não respondeu", address.host))),
        };
        let peer = stream.peer_addr()?;
        let (reader, writer) = stream.into_split();
        let mut control = Control { reader: BufReader::new(reader), writer, peer };
        let greeting = control.reply().await?;
        if greeting.code != 220 {
            return Err(greeting.error("ao conectar"));
        }
        Ok(control)
    }

    /// Reads one reply, multi-line ones (`213-…` up to `213 …`) whole.
    async fn reply(&mut self) -> Result<Reply> {
        let mut first: Option<u16> = None;
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(REPLY_TIMEOUT, self.reader.read_line(&mut line)).await;
            let read = read.map_err(|_| Error::Other("o servidor FTP parou de responder".into()))??;
            if read == 0 {
                return Err(Error::Other("o servidor FTP fechou a conexão".into()));
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
                (None, None) => return Err(Error::Other(format!("resposta FTP inválida: {line}"))),
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
            return Err(reply.error("no login"));
        }
        let binary = self.command("TYPE I").await?;
        if binary.code != 200 {
            return Err(binary.error("no modo binário"));
        }
        Ok(())
    }

    /// The file's size, when the server says it (`SIZE` is an extension).
    async fn size(&mut self, path: &str) -> Result<Option<u64>> {
        let reply = self.command(&format!("SIZE {path}")).await?;
        match reply.code {
            213 => Ok(reply.text.trim().parse().ok()),
            550 => Err(reply.error("ao procurar o arquivo")),
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
                    return Err(reply.error("no modo passivo"));
                }
                passive_port(&reply.text)
            }
        };
        let port = port.ok_or_else(|| Error::Other("FTP: o servidor não informou a porta de dados".into()))?;
        let address = SocketAddr::new(self.peer.ip(), port);
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
            Ok(stream) => Ok(stream?),
            Err(_) => Err(Error::Other("FTP: a conexão de dados não abriu".into())),
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

    let final_path = resolve_target(target, &address.name);
    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part = control::part_path(&final_path);
    // What an earlier attempt left, unless it is already more than the file.
    let kept = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
    let mut offset = if total.is_some_and(|total| kept <= total) { kept } else { 0 };
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
        return Err(retrieve.error("ao pedir o arquivo"));
    }

    let mut file = tokio::fs::OpenOptions::new().create(true).write(true).truncate(offset == 0).open(&part).await?;
    if offset > 0 {
        file.set_len(offset).await?;
        tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(offset)).await?;
    }
    let mut downloaded = offset;
    let mut meter = SpeedMeter::new(downloaded);
    let mut last_report = Instant::now();
    let mut buffer = vec![0u8; CHUNK];
    let paused = loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => break true,
            read = tokio::time::timeout(opts.idle_timeout, data.read(&mut buffer)) => read,
        };
        let n = match read {
            Err(_) => return Err(Error::Other("tempo esgotado esperando dados do servidor FTP".into())),
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
    };
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    drop(data);
    if paused {
        // What arrived stays for `REST` next time.
        tokio::spawn(control.quit());
        return Ok(Outcome::Paused(final_path));
    }
    let done = control.reply().await?;
    control.quit().await;
    if !matches!(done.code, 226 | 250) {
        return Err(done.error("ao terminar"));
    }
    if let Some(total) = total.filter(|total| downloaded != *total) {
        return Err(Error::Other(format!("conexão FTP encerrada em {downloaded} de {total} bytes")));
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
    /// it knows EPSV (else only PASV). Returns its address and the commands seen.
    async fn server(content: Vec<u8>, epsv: bool) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
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
                            socket.write_all(&content[rest..]).await.unwrap();
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
            let (base, seen) = server(content(), epsv).await;
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

    #[tokio::test]
    async fn resumes_where_it_stopped() {
        let (base, seen) = server(content(), true).await;
        let dir = scratch("resume");
        std::fs::write(dir.join("a.bin.fisga-part"), &content()[..100_000]).unwrap();
        let Outcome::Completed(path) = fetch(&format!("{base}/pub/a.bin"), &dir).await.unwrap() else { panic!() };
        assert_eq!(std::fs::read(&path).unwrap(), content());
        assert!(seen.lock().unwrap().contains(&"REST 100000".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn says_what_went_wrong() {
        let (base, _) = server(content(), true).await;
        let dir = scratch("errors");
        let missing = fetch(&format!("{base}/pub/outro.bin"), &dir).await.unwrap_err().to_string();
        assert!(missing.contains("não existe"), "{missing}");
        let host = base.trim_start_matches("ftp://");
        let refused = fetch(&format!("ftp://ana:x@{host}/pub/a.bin"), &dir).await.unwrap_err().to_string();
        assert!(refused.contains("login"), "{refused}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
