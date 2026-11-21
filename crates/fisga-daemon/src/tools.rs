//! The programs Fisga hands work to: yt-dlp, ffmpeg and Deno for videos
//! (YouTube's players need a JavaScript runtime), and gallery-dl for the
//! posts and profiles of social and art sites in the browser's media
//! gallery. On Linux they come from the distro's packages. On Windows and
//! macOS they are fetched the first time they are needed, from each project's
//! own releases, checked against the sums published there, and kept in a
//! folder of Fisga's. One already on the PATH is left alone.
//!
//! Nothing is fetched until it is needed: the video programs for the first
//! video, gallery-dl for the first post or profile. yt-dlp and gallery-dl have
//! to follow the sites, so they are brought up to date then too, at most once
//! a day. They are separate programs Fisga starts and reads the output of;
//! none of their code is in Fisga.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use fisga_i18n::{tr, trf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Mutex;

/// How long a download may go without receiving anything.
const STALLED: Duration = Duration::from_secs(60);
/// How long the look for a new yt-dlp (or gallery-dl) may hold the work back.
const UPDATE_CHECK: Duration = Duration::from_secs(10);
const UPDATE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const NOTE_EVERY: Duration = Duration::from_millis(500);

/// One at a time: two downloads must not fetch the same program.
static BUSY: Mutex<()> = Mutex::const_new(());

/// Where a program's file comes from, and the SHA-256 it must have. (Each
/// system's plan uses some of this, and Linux's none.)
#[allow(dead_code)]
enum Source {
    /// A fixed address, its sum in a list published with the file, which names it.
    Listed { url: String, sums: String, name: String },
    /// A pinned release.
    Fixed { url: String, sum: &'static str },
    /// The newest release a forge's API describes (`…/releases/latest` on
    /// Codeberg or GitHub): its asset `name`, with the sum from the same
    /// release, in the asset `sums` (a list of sums) when there is one, else
    /// the `digest` GitHub keeps of each file. Both come from one answer, so a
    /// release published meanwhile cannot pair one's file with another's sum.
    Release { api: String, name: &'static str, sums: Option<&'static str> },
}

/// How a file becomes a program.
#[allow(dead_code)]
#[derive(Clone, Copy)]
enum Form {
    /// It is the program.
    Program,
    /// A zip with the program, and maybe the libraries next to it.
    Zip,
}

#[allow(dead_code)]
struct Tool {
    /// For the messages.
    name: &'static str,
    /// The file it ends up as, in the tools' folder.
    program: String,
    source: Source,
    form: Form,
}

/// What the programs are for: each kit is fetched when first needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kit {
    /// yt-dlp, Deno and ffmpeg.
    Videos,
    /// gallery-dl.
    Galleries,
}

#[cfg(any(windows, target_os = "macos"))]
const YT_DLP: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
#[cfg(any(windows, target_os = "macos"))]
const DENO: &str = "https://github.com/denoland/deno/releases/latest/download";

#[cfg(any(windows, target_os = "macos"))]
fn yt_dlp_tool(file: &str) -> Tool {
    Tool {
        name: "yt-dlp",
        program: program("yt-dlp"),
        source: Source::Listed { url: format!("{YT_DLP}/{file}"), sums: format!("{YT_DLP}/SHA2-256SUMS"), name: file.into() },
        form: Form::Program,
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn deno_tool(zip: &str) -> Tool {
    Tool {
        name: "Deno",
        program: program("deno"),
        source: Source::Listed { url: format!("{DENO}/{zip}"), sums: format!("{DENO}/{zip}.sha256sum"), name: zip.into() },
        form: Form::Zip,
    }
}

/// What videos need on this system, yt-dlp first.
#[cfg(windows)]
fn plan() -> Vec<Tool> {
    const FFMPEG: &str = "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest";
    const ZIP: &str = "ffmpeg-master-latest-win64-gpl-shared.zip";
    vec![
        yt_dlp_tool("yt-dlp.exe"),
        deno_tool("deno-x86_64-pc-windows-msvc.zip"),
        Tool {
            name: "ffmpeg",
            program: program("ffmpeg"),
            source: Source::Listed { url: format!("{FFMPEG}/{ZIP}"), sums: format!("{FFMPEG}/checksums.sha256"), name: ZIP.into() },
            form: Form::Zip,
        },
    ]
}

/// What videos need on this system, yt-dlp first. The ffmpeg builds yt-dlp's
/// project publishes have no macOS: these are static ones for Apple silicon.
#[cfg(target_os = "macos")]
fn plan() -> Vec<Tool> {
    const FFMPEG: &str = "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1";
    vec![
        yt_dlp_tool("yt-dlp_macos"),
        deno_tool("deno-aarch64-apple-darwin.zip"),
        Tool {
            name: "ffmpeg",
            program: program("ffmpeg"),
            source: Source::Fixed {
                url: format!("{FFMPEG}/ffmpeg-darwin-arm64"),
                sum: "a90e3db6a3fd35f6074b013f948b1aa45b31c6375489d39e572bea3f18336584",
            },
            form: Form::Program,
        },
        Tool {
            name: "ffprobe",
            program: program("ffprobe"),
            source: Source::Fixed {
                url: format!("{FFMPEG}/ffprobe-darwin-arm64"),
                sum: "bb2db6f5d8cef919da12fbf592119a987202a8c060a886f3cab091f9cab90b64",
            },
            form: Form::Program,
        },
    ]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn plan() -> Vec<Tool> {
    Vec::new()
}

/// gallery-dl, from its releases on Codeberg (the 64-bit Windows program,
/// with the release's list of sums).
#[cfg(windows)]
fn gallery_dl_plan() -> Vec<Tool> {
    vec![Tool {
        name: "gallery-dl",
        program: program("gallery-dl"),
        source: Source::Release {
            api: "https://codeberg.org/api/v1/repos/mikf/gallery-dl/releases/latest".into(),
            name: "gallery-dl.exe",
            sums: Some("SHA256SUMS"),
        },
        form: Form::Program,
    }]
}

/// gallery-dl's releases have no macOS program: these are the builds its
/// author publishes each day from the newest code, for Apple silicon.
#[cfg(target_os = "macos")]
fn gallery_dl_plan() -> Vec<Tool> {
    vec![Tool {
        name: "gallery-dl",
        program: program("gallery-dl"),
        source: Source::Release {
            api: "https://api.github.com/repos/gdl-org/builds/releases/latest".into(),
            name: "gallery-dl_macos",
            sums: None,
        },
        form: Form::Program,
    }]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn gallery_dl_plan() -> Vec<Tool> {
    Vec::new()
}

fn program(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Not the roaming folder on Windows: these are hundreds of megabytes.
fn folder() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("fisga").join("tools")
}

/// Fisga's copy of `name`, if there is one, or the system's.
fn own_or_system(name: &str) -> Command {
    let own = folder().join(program(name));
    match own.exists() {
        true => crate::shell::program(own),
        false => crate::shell::program(name),
    }
}

/// yt-dlp: Fisga's copy, if there is one, or the system's. Either finds the
/// tools' folder first on the PATH, for ffmpeg and Deno, and writes UTF-8:
/// on Windows it would use the system's code page, and a path with an accent
/// (Downloads\Vídeos) came out unreadable.
pub fn yt_dlp() -> Command {
    let dir = folder();
    let mut command = own_or_system("yt-dlp");
    command.args(["--encoding", "utf-8"]);
    if dir.exists() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        if let Ok(path) = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&path))) {
            command.env("PATH", path);
        }
    }
    command
}

/// gallery-dl: Fisga's copy, if there is one, or the system's.
pub fn gallery_dl() -> Command {
    own_or_system("gallery-dl")
}

fn on_path(program: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

/// Makes sure the kit's programs are there. `note` tells how it is going, for
/// the download waiting for them; it is only called when there is something to fetch.
pub async fn ensure(client: &reqwest::Client, kit: Kit, note: impl Fn(String) + Send + Sync) -> Result<(), String> {
    let tools = match kit {
        Kit::Videos => plan(),
        Kit::Galleries => gallery_dl_plan(),
    };
    ensure_in(client, &folder(), &tools, kit, &note).await
}

async fn ensure_in(
    client: &reqwest::Client,
    dir: &Path,
    tools: &[Tool],
    kit: Kit,
    note: &(impl Fn(String) + Send + Sync),
) -> Result<(), String> {
    if tools.is_empty() {
        return Ok(());
    }
    let _busy = BUSY.lock().await;
    let missing: Vec<&Tool> =
        tools.iter().filter(|tool| !dir.join(&tool.program).exists() && !on_path(&tool.program)).collect();
    for tool in missing {
        let say = |done, total| {
            note(match kit {
                Kit::Videos => trf!("Preparando os vídeos, só na primeira vez: {} ({})", tool.name, amount(done, total)),
                Kit::Galleries => trf!("Preparando as galerias, só na primeira vez: {} ({})", tool.name, amount(done, total)),
            })
        };
        install(client, dir, tool, &say).await.map_err(|e| match kit {
            Kit::Videos => trf!("não foi possível preparar os vídeos ({}): {}", tool.name, e),
            Kit::Galleries => trf!("não foi possível preparar as galerias ({}): {}", tool.name, e),
        })?;
        tracing::info!(tool = tool.name, "ferramenta instalada");
    }
    if let Some(tool) = tools.first() {
        update(client, dir, tool, note).await;
    }
    Ok(())
}

/// A new version of the tool (yt-dlp, gallery-dl), if the last look was over
/// a day ago. Failing is no reason to hold the work back: the copy here may
/// still do.
async fn update(client: &reqwest::Client, dir: &Path, tool: &Tool, note: &(impl Fn(String) + Send + Sync)) {
    let stamp = stamp_file(dir, tool);
    let age = std::fs::metadata(&stamp).and_then(|file| file.modified()).ok().and_then(|at| SystemTime::now().duration_since(at).ok());
    if !dir.join(&tool.program).exists() || age.is_some_and(|age| age < UPDATE_EVERY) {
        return;
    }
    let _ = std::fs::write(&stamp, "");
    let (url, latest) = match tokio::time::timeout(UPDATE_CHECK, tool.source.locate(client)).await {
        Ok(Ok(latest)) => latest,
        Ok(Err(e)) => return tracing::info!(tool = tool.name, error = %e, "não foi possível procurar uma versão nova"),
        Err(_) => return tracing::info!(tool = tool.name, "a procura por uma versão nova demorou demais"),
    };
    if std::fs::read_to_string(sum_file(dir, tool)).is_ok_and(|installed| installed == latest) {
        return;
    }
    let say = |done, total| note(trf!("Atualizando o {} ({})", tool.name, amount(done, total)));
    // A copy in use cannot be replaced on Windows: tomorrow, then.
    match install_from(client, dir, tool, &url, &latest, &say).await {
        Ok(()) => tracing::info!(tool = tool.name, "ferramenta atualizada"),
        Err(e) => tracing::warn!(tool = tool.name, error = %e, "não foi possível atualizar"),
    }
}

fn amount(done: u64, total: Option<u64>) -> String {
    let mb = |bytes: u64| bytes / 1_000_000;
    match total {
        Some(total) => trf!("{} de {} MB", mb(done), mb(total)),
        None => format!("{} MB", mb(done)),
    }
}

/// What the program installed was fetched as, to know when there is a new one.
fn sum_file(dir: &Path, tool: &Tool) -> PathBuf {
    dir.join(format!("{}.sha256", tool.program))
}

/// When the newest version was last looked for (its time of change).
fn stamp_file(dir: &Path, tool: &Tool) -> PathBuf {
    dir.join(format!("{}.checked", tool.program))
}

impl Source {
    /// The address to fetch and the SHA-256 the file must have.
    async fn locate(&self, client: &reqwest::Client) -> Result<(String, String), String> {
        match self {
            Source::Fixed { url, sum } => Ok((url.clone(), (*sum).to_string())),
            Source::Listed { url, sums, name } => {
                let sum = find_sum(&text(client, sums).await?, name).ok_or_else(|| trf!("{} não está na lista de somas", name))?;
                Ok((url.clone(), sum))
            }
            Source::Release { api, name, sums } => {
                let release: serde_json::Value =
                    serde_json::from_str(&text(client, api).await?).map_err(|e| trf!("resposta inválida de {}: {}", api, e))?;
                let address = |wanted: &str| {
                    let assets = release.get("assets").and_then(|assets| assets.as_array());
                    let asset = assets.into_iter().flatten().find(|asset| asset["name"] == wanted)?;
                    Some((asset, asset.get("browser_download_url")?.as_str()?.to_string()))
                };
                let missing = |wanted: &str| trf!("{} não está na versão mais nova", wanted);
                let (asset, url) = address(name).ok_or_else(|| missing(name))?;
                let sum = match sums {
                    Some(list) => {
                        let (_, list_url) = address(list).ok_or_else(|| missing(list))?;
                        find_sum(&text(client, &list_url).await?, name)
                    }
                    None => asset.get("digest").and_then(|digest| digest.as_str()).and_then(|digest| digest.strip_prefix("sha256:")).and_then(as_sum),
                };
                Ok((url, sum.ok_or_else(|| trf!("{} não está na lista de somas", name))?))
            }
        }
    }
}

/// The body of a page, as text. GitHub's API answers 60 requests an hour
/// without a login, per network address: machines that share one (the CI's)
/// run out. `$FISGA_GITHUB_TOKEN`, when set, goes with the requests to it,
/// and only to it; nothing else sends one.
async fn text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let mut request = client.get(url);
    if url.starts_with("https://api.github.com/") {
        if let Some(token) = std::env::var("FISGA_GITHUB_TOKEN").ok().filter(|token| !token.is_empty()) {
            request = request.bearer_auth(token);
        }
    }
    let response = request.send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    response.text().await.map_err(|e| e.to_string())
}

/// A SHA-256, written in hexadecimal: in lower case, or `None` when it is not one.
fn as_sum(word: &str) -> Option<String> {
    (word.len() == 64 && word.bytes().all(|b| b.is_ascii_hexdigit())).then(|| word.to_ascii_lowercase())
}

/// The SHA-256 of `name` in a list of sums: the usual `sum  name` lines, or a
/// file with one sum and no such line (Deno's on Windows).
pub(crate) fn find_sum(list: &str, name: &str) -> Option<String> {
    let listed = list.lines().find_map(|line| {
        let named = line.split_whitespace().any(|word| word.trim_start_matches('*') == name);
        line.split_whitespace().find_map(as_sum).filter(|_| named)
    });
    let mut sums = list.split_whitespace().filter_map(as_sum);
    let only = sums.next().filter(|_| sums.next().is_none());
    listed.or(only)
}

/// Fetches the newest file of the tool, checks it and puts it in place.
async fn install(client: &reqwest::Client, dir: &Path, tool: &Tool, say: &(impl Fn(u64, Option<u64>) + Send + Sync)) -> Result<(), String> {
    let (url, expected) = tool.source.locate(client).await?;
    install_from(client, dir, tool, &url, &expected, say).await
}

/// Fetches `url`, which must have the SHA-256 `expected`, as the tool.
async fn install_from(
    client: &reqwest::Client,
    dir: &Path,
    tool: &Tool,
    url: &str,
    expected: &str,
    say: &(impl Fn(u64, Option<u64>) + Send + Sync),
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{}.part", tool.program));
    let result = fetch(client, url, &file, say).await.and_then(|sum| match sum == expected {
        true => Ok(()),
        false => Err(trf!("a soma de {} não confere", url)),
    });
    let placed = match (result, tool.form) {
        (Err(e), _) => Err(e),
        (Ok(()), Form::Program) => place(&file, &dir.join(&tool.program)),
        (Ok(()), Form::Zip) => unzip(&file, dir, &tool.program).await,
    };
    let _ = std::fs::remove_file(&file);
    placed?;
    // Just fetched: the newest, no need to look again today.
    let _ = std::fs::write(stamp_file(dir, tool), "");
    std::fs::write(sum_file(dir, tool), expected).map_err(|e| e.to_string())
}

/// Downloads `url` to `file`; returns its SHA-256.
pub(crate) async fn fetch(client: &reqwest::Client, url: &str, file: &Path, say: &(impl Fn(u64, Option<u64>) + Send + Sync)) -> Result<String, String> {
    let mut response = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    let total = response.content_length();
    let mut out = tokio::fs::File::create(file).await.map_err(|e| e.to_string())?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let (mut done, mut said) = (0u64, Instant::now());
    say(0, total);
    loop {
        let chunk = tokio::time::timeout(STALLED, response.chunk()).await;
        let chunk = chunk.map_err(|_| tr!("a conexão parou").to_string())?.map_err(|e| e.to_string())?;
        let Some(chunk) = chunk else { break };
        digest.update(&chunk);
        out.write_all(&chunk).await.map_err(|e| e.to_string())?;
        done += chunk.len() as u64;
        if said.elapsed() >= NOTE_EVERY {
            said = Instant::now();
            say(done, total);
        }
    }
    out.flush().await.map_err(|e| e.to_string())?;
    Ok(digest.finish().as_ref().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Moves a file into place, able to run.
fn place(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(from, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    std::fs::rename(from, to).map_err(|e| e.to_string())
}

/// Takes the program out of a zip, with what is next to it there (ffmpeg's
/// libraries). The program goes last: its being there means all of it is.
async fn unzip(zip: &Path, dir: &Path, program: &str) -> Result<(), String> {
    let unpacked = dir.join(format!("{program}.unpacked"));
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let result = unzip_to(zip, &unpacked).await.and_then(|()| {
        let found = find_file(&unpacked, program).ok_or_else(|| trf!("{} não está no arquivo", program))?;
        let from = found.parent().unwrap_or(&unpacked);
        let mut names: Vec<_> = std::fs::read_dir(from)
            .map_err(|e| e.to_string())?
            .flatten()
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name())
            .filter(|name| *name != *program && !name.to_string_lossy().starts_with("ffplay"))
            .collect();
        names.push(program.into());
        names.iter().try_for_each(|name| place(&from.join(name), &dir.join(name)))
    });
    let _ = std::fs::remove_dir_all(&unpacked);
    result
}

/// Windows 10 and macOS come with a `tar` that reads zips.
async fn unzip_to(zip: &Path, dir: &Path) -> Result<(), String> {
    let tar = match std::env::var_os("SystemRoot") {
        Some(windows) => PathBuf::from(windows).join("System32").join("tar.exe"),
        None => PathBuf::from("/usr/bin/tar"),
    };
    let status = crate::shell::program(&tar).arg("-xf").arg(zip).arg("-C").arg(dir).status().await;
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(trf!("o tar falhou ({})", status)),
        Err(e) => Err(trf!("não foi possível iniciar o tar: {}", e)),
    }
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|entry| entry.path()).collect();
    entries.iter().find(|path| path.is_file() && path.file_name().is_some_and(|n| n == name)).cloned().or_else(|| {
        entries.iter().filter(|path| path.is_dir()).find_map(|path| find_file(path, name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    const SUM_OF_HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fisga-tools-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A site that answers each path with its page, "{base}" in the page
    /// replaced by the site's own address (404 for any other path); its address.
    async fn site(pages: &[(&'static str, String)]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let pages: Vec<(&'static str, String)> = pages.iter().map(|(path, body)| (*path, body.replace("{base}", &base))).collect();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0; 2048];
                let n = socket.read(&mut request).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or("").to_string();
                let path = line.split_whitespace().nth(1).unwrap_or("");
                let reply = match pages.iter().find(|(page, _)| *page == path) {
                    Some((_, body)) => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                };
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        base
    }

    /// Answers "/file" with "hello"; that file's URL.
    async fn server() -> String {
        format!("{}/file", site(&[("/file", "hello".into())]).await)
    }

    fn client() -> reqwest::Client {
        fisga_core::build_client_with(fisga_core::Proxy::Direct).unwrap()
    }

    fn tool(source: Source) -> Tool {
        Tool { name: "teste", program: "fisga-tools-teste".into(), source, form: Form::Program }
    }

    fn fixed(url: String, sum: &'static str) -> Tool {
        tool(Source::Fixed { url, sum })
    }

    #[test]
    fn sums_as_each_project_publishes_them() {
        let a = "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d";
        let b = "0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa";
        let list = format!("{a}  yt-dlp.exe\n{b}  yt-dlp_macos\n");
        assert_eq!(find_sum(&list, "yt-dlp.exe").as_deref(), Some(a));
        assert_eq!(find_sum(&list, "yt-dlp_macos").as_deref(), Some(b));
        assert_eq!(find_sum(&list, "yt-dlp"), None, "a name is matched whole");
        assert_eq!(find_sum(&format!("{a} *binary.zip\n"), "binary.zip").as_deref(), Some(a));
        // Deno on Windows: PowerShell's Get-FileHash.
        let upper = a.to_ascii_uppercase();
        let deno = format!("Algorithm : SHA256\nHash      : {upper}\nPath      : C:\\a\\deno.zip\n");
        assert_eq!(find_sum(&deno, "deno-x86_64-pc-windows-msvc.zip").as_deref(), Some(a));
        assert_eq!(find_sum("not a list", "x"), None);
        // gallery-dl's SHA256SUMS on Codeberg.
        let gallery = format!("{b}  gallery_dl-1.32.20.tar.gz\n{a}  gallery-dl.exe\n{b}  gallery-dl_x86.exe\n");
        assert_eq!(find_sum(&gallery, "gallery-dl.exe").as_deref(), Some(a));
    }

    #[tokio::test]
    async fn fetches_checks_and_keeps_the_program() {
        let dir = scratch("install");
        let notes = std::sync::Mutex::new(Vec::new());
        let tools = [fixed(server().await, SUM_OF_HELLO)];
        ensure_in(&client(), &dir, &tools, Kit::Videos, &|note: String| notes.lock().unwrap().push(note)).await.unwrap();
        assert_eq!(std::fs::read(dir.join(&tools[0].program)).unwrap(), b"hello");
        assert_eq!(std::fs::read_to_string(sum_file(&dir, &tools[0])).unwrap(), SUM_OF_HELLO);
        assert!(notes.lock().unwrap()[0].contains("primeira vez"));
        // There already: nothing to fetch, nothing to say.
        notes.lock().unwrap().clear();
        ensure_in(&client(), &dir, &tools, Kit::Videos, &|note: String| notes.lock().unwrap().push(note)).await.unwrap();
        assert!(notes.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_wrong_sum_leaves_nothing_behind() {
        let dir = scratch("sum");
        let tools = [fixed(server().await, "0000000000000000000000000000000000000000000000000000000000000000")];
        let error = ensure_in(&client(), &dir, &tools, Kit::Videos, &|_: String| {}).await.unwrap_err();
        assert!(error.contains("soma"), "{error}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A release as Codeberg's API describes it: the file and the list of sums are assets.
    fn codeberg_release(sums: &str) -> Vec<(&'static str, String)> {
        let assets = r#"{"tag_name": "v1.32.20", "assets": [
            {"name": "gallery-dl.bin", "browser_download_url": "{base}/v1.32.20/gallery-dl.bin"},
            {"name": "gallery-dl.exe", "browser_download_url": "{base}/v1.32.20/gallery-dl.exe"},
            {"name": "SHA256SUMS", "browser_download_url": "{base}/v1.32.20/SHA256SUMS"}]}"#;
        vec![
            ("/api/v1/repos/mikf/gallery-dl/releases/latest", assets.into()),
            ("/v1.32.20/gallery-dl.exe", "hello".into()),
            ("/v1.32.20/gallery-dl.bin", "world".into()),
            ("/v1.32.20/SHA256SUMS", sums.into()),
        ]
    }

    #[tokio::test]
    async fn gallery_dl_from_its_newest_release_on_codeberg() {
        let dir = scratch("codeberg");
        let other = "0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa";
        let base = site(&codeberg_release(&format!("{other}  gallery-dl.bin\n{SUM_OF_HELLO}  gallery-dl.exe\n"))).await;
        let source = Source::Release {
            api: format!("{base}/api/v1/repos/mikf/gallery-dl/releases/latest"),
            name: "gallery-dl.exe",
            sums: Some("SHA256SUMS"),
        };
        let (url, sum) = source.locate(&client()).await.unwrap();
        assert_eq!((url.as_str(), sum.as_str()), (format!("{base}/v1.32.20/gallery-dl.exe").as_str(), SUM_OF_HELLO));
        let notes = std::sync::Mutex::new(Vec::new());
        let tools = [tool(source)];
        ensure_in(&client(), &dir, &tools, Kit::Galleries, &|note: String| notes.lock().unwrap().push(note)).await.unwrap();
        assert_eq!(std::fs::read(dir.join(&tools[0].program)).unwrap(), b"hello");
        assert!(notes.lock().unwrap()[0].starts_with("Preparando as galerias, só na primeira vez: teste"), "{:?}", notes.lock().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_release_without_the_file_or_its_sum() {
        let base = site(&codeberg_release("")).await;
        let api = format!("{base}/api/v1/repos/mikf/gallery-dl/releases/latest");
        let missing = Source::Release { api: api.clone(), name: "gallery-dl_macos", sums: Some("SHA256SUMS") };
        assert!(missing.locate(&client()).await.unwrap_err().contains("gallery-dl_macos não está na versão mais nova"));
        let unlisted = Source::Release { api: api.clone(), name: "gallery-dl.exe", sums: Some("SHA256SUMS") };
        assert!(unlisted.locate(&client()).await.unwrap_err().contains("não está na lista de somas"));
        // Without a list, the asset's digest; this one has none.
        let undigested = Source::Release { api, name: "gallery-dl.exe", sums: None };
        assert!(undigested.locate(&client()).await.unwrap_err().contains("não está na lista de somas"));
        let dir = scratch("unreleased");
        let tools = [tool(Source::Release { api: format!("{base}/nada"), name: "x", sums: None })];
        let error = ensure_in(&client(), &dir, &tools, Kit::Galleries, &|_: String| {}).await.unwrap_err();
        assert!(error.starts_with("não foi possível preparar as galerias (teste)"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn gallery_dl_from_github_with_the_digest_github_keeps() {
        let release = format!(
            r#"{{"tag_name": "2026.11.21", "assets": [
                {{"name": "gallery-dl_linux", "digest": "sha256:{other}", "browser_download_url": "{{base}}/d/gallery-dl_linux"}},
                {{"name": "gallery-dl_macos", "digest": "sha256:{upper}", "browser_download_url": "{{base}}/d/gallery-dl_macos"}}]}}"#,
            other = "0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa",
            upper = SUM_OF_HELLO.to_ascii_uppercase(),
        );
        let base = site(&[("/repos/gdl-org/builds/releases/latest", release), ("/d/gallery-dl_macos", "hello".into())]).await;
        let dir = scratch("github");
        let tools = [tool(Source::Release {
            api: format!("{base}/repos/gdl-org/builds/releases/latest"),
            name: "gallery-dl_macos",
            sums: None,
        })];
        ensure_in(&client(), &dir, &tools, Kit::Galleries, &|_: String| {}).await.unwrap();
        assert_eq!(std::fs::read(dir.join(&tools[0].program)).unwrap(), b"hello");
        assert_eq!(std::fs::read_to_string(sum_file(&dir, &tools[0])).unwrap(), SUM_OF_HELLO);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_new_release_replaces_the_copy_once_a_day() {
        let dir = scratch("update");
        std::fs::create_dir_all(&dir).unwrap();
        let base = site(&codeberg_release(&format!("{SUM_OF_HELLO}  gallery-dl.exe\n"))).await;
        let tools = [tool(Source::Release {
            api: format!("{base}/api/v1/repos/mikf/gallery-dl/releases/latest"),
            name: "gallery-dl.exe",
            sums: Some("SHA256SUMS"),
        })];
        // An older copy, checked over a day ago (the stamp is missing).
        std::fs::write(dir.join(&tools[0].program), "old").unwrap();
        std::fs::write(sum_file(&dir, &tools[0]), "1111111111111111111111111111111111111111111111111111111111111111").unwrap();
        let notes = std::sync::Mutex::new(Vec::new());
        ensure_in(&client(), &dir, &tools, Kit::Galleries, &|note: String| notes.lock().unwrap().push(note)).await.unwrap();
        assert_eq!(std::fs::read(dir.join(&tools[0].program)).unwrap(), b"hello");
        assert!(notes.lock().unwrap()[0].starts_with("Atualizando o teste"), "{:?}", notes.lock().unwrap());
        // Looked at today: not again until tomorrow.
        std::fs::write(dir.join(&tools[0].program), "kept").unwrap();
        std::fs::write(sum_file(&dir, &tools[0]), "1111111111111111111111111111111111111111111111111111111111111111").unwrap();
        ensure_in(&client(), &dir, &tools, Kit::Galleries, &|_: String| {}).await.unwrap();
        assert_eq!(std::fs::read(dir.join(&tools[0].program)).unwrap(), b"kept");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn yt_dlp_writes_utf_8() {
        let command = yt_dlp();
        let args: Vec<_> = command.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["--encoding", "utf-8"]);
    }

    #[test]
    fn gallery_dl_is_fetched_where_there_is_no_package() {
        assert_eq!(gallery_dl_plan().len(), usize::from(cfg!(any(windows, target_os = "macos"))));
        let args: Vec<_> = gallery_dl().as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.is_empty(), "{args:?}");
    }

    #[test]
    fn finds_a_file_in_subfolders() {
        let dir = scratch("find");
        std::fs::create_dir_all(dir.join("ffmpeg-build/bin")).unwrap();
        std::fs::write(dir.join("ffmpeg-build/bin/ffmpeg"), "").unwrap();
        assert_eq!(find_file(&dir, "ffmpeg"), Some(dir.join("ffmpeg-build/bin/ffmpeg")));
        assert_eq!(find_file(&dir, "deno"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The real thing, from the real releases: their addresses, their lists of
    /// sums, the zips and whether the programs run here. Run by the CI on
    /// Windows and macOS (`cargo test -p fisga-daemon -- --ignored`).
    #[tokio::test]
    #[ignore = "fetches about 175 MB"]
    async fn the_real_tools_install_and_run() {
        let dir = scratch("real");
        for tool in plan().into_iter().chain(gallery_dl_plan()) {
            install(&client(), &dir, &tool, &|_: u64, _: Option<u64>| {}).await.unwrap_or_else(|e| panic!("{}: {e}", tool.name));
            let flag = if tool.name.starts_with("ff") { "-version" } else { "--version" };
            let output = crate::shell::program(dir.join(&tool.program)).arg(flag).output().await.unwrap();
            let text = String::from_utf8_lossy(&output.stdout);
            println!("{}: {}", tool.name, text.lines().next().unwrap_or(""));
            assert!(output.status.success(), "{}: {output:?}", tool.name);
        }
        // gallery-dl as the gallery runs it, and the line it prints for each
        // file (a direct link: nothing is asked of the network).
        if let Some(tool) = gallery_dl_plan().first() {
            let request = fisga_ipc::AddRequest {
                url: "https://example.org/fisga/foto%20de%20teste%C3%A9.JPG?x=1&y=2".into(),
                user_agent: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64)".into()),
                ..Default::default()
            };
            let none = fisga_ipc::ProxySettings { mode: fisga_ipc::ProxyMode::None, ..Default::default() };
            let mut command = crate::gallery::listing(crate::shell::program(dir.join(&tool.program)), &request, None, &none);
            let output = command.output().await.unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            println!("gallery-dl --print: {stdout}{}", String::from_utf8_lossy(&output.stderr));
            assert!(output.status.success(), "{output:?}");
            let found: Vec<_> = stdout.lines().filter_map(crate::gallery::parse_line).collect();
            assert_eq!(found.len(), 1, "{stdout}");
            assert_eq!(found[0].url, request.url);
            assert_eq!(found[0].name.as_deref(), Some("foto de testeé.jpg"));
        }
        if dir.exists() {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
