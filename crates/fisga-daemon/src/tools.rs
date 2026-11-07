//! The programs videos need: yt-dlp, ffmpeg and Deno (YouTube's players need
//! a JavaScript runtime). On Linux they come from the distro's packages. On
//! Windows and macOS they are fetched the first time a video needs them, from
//! each project's own releases, checked against the sums published there, and
//! kept in a folder of Fisga's. One already on the PATH is left alone.
//!
//! Nothing is fetched until a video is asked for. yt-dlp has to follow the
//! sites, so it is brought up to date then too, at most once a day.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use fisga_i18n::{tr, trf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Mutex;

/// How long a download may go without receiving anything.
const STALLED: Duration = Duration::from_secs(60);
/// How long the look for a new yt-dlp may hold a video back.
const UPDATE_CHECK: Duration = Duration::from_secs(10);
const UPDATE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const NOTE_EVERY: Duration = Duration::from_millis(500);

/// One at a time: two videos must not fetch the same program.
static BUSY: Mutex<()> = Mutex::const_new(());

/// Where a file's SHA-256 comes from. (Each system's plan uses some of this,
/// and Linux's none.)
#[allow(dead_code)]
enum Sum {
    /// A list published with the file, which names it.
    Listed { url: String, name: String },
    /// A pinned release.
    Fixed(&'static str),
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
    url: String,
    sum: Sum,
    form: Form,
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
        url: format!("{YT_DLP}/{file}"),
        sum: Sum::Listed { url: format!("{YT_DLP}/SHA2-256SUMS"), name: file.into() },
        form: Form::Program,
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn deno_tool(zip: &str) -> Tool {
    Tool {
        name: "Deno",
        program: program("deno"),
        url: format!("{DENO}/{zip}"),
        sum: Sum::Listed { url: format!("{DENO}/{zip}.sha256sum"), name: zip.into() },
        form: Form::Zip,
    }
}

/// What this system needs, yt-dlp first.
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
            url: format!("{FFMPEG}/{ZIP}"),
            sum: Sum::Listed { url: format!("{FFMPEG}/checksums.sha256"), name: ZIP.into() },
            form: Form::Zip,
        },
    ]
}

/// What this system needs, yt-dlp first. The ffmpeg builds yt-dlp's project
/// publishes have no macOS: these are static ones for Apple silicon.
#[cfg(target_os = "macos")]
fn plan() -> Vec<Tool> {
    const FFMPEG: &str = "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1";
    vec![
        yt_dlp_tool("yt-dlp_macos"),
        deno_tool("deno-aarch64-apple-darwin.zip"),
        Tool {
            name: "ffmpeg",
            program: program("ffmpeg"),
            url: format!("{FFMPEG}/ffmpeg-darwin-arm64"),
            sum: Sum::Fixed("a90e3db6a3fd35f6074b013f948b1aa45b31c6375489d39e572bea3f18336584"),
            form: Form::Program,
        },
        Tool {
            name: "ffprobe",
            program: program("ffprobe"),
            url: format!("{FFMPEG}/ffprobe-darwin-arm64"),
            sum: Sum::Fixed("bb2db6f5d8cef919da12fbf592119a987202a8c060a886f3cab091f9cab90b64"),
            form: Form::Program,
        },
    ]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn plan() -> Vec<Tool> {
    Vec::new()
}

fn program(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Not the roaming folder on Windows: these are hundreds of megabytes.
fn folder() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("fisga").join("tools")
}

/// yt-dlp: Fisga's copy, if there is one, or the system's. Either finds the
/// tools' folder first on the PATH, for ffmpeg and Deno, and writes UTF-8:
/// on Windows it would use the system's code page, and a path with an accent
/// (Downloads\Vídeos) came out unreadable.
pub fn yt_dlp() -> Command {
    let dir = folder();
    let own = dir.join(program("yt-dlp"));
    let mut command = match own.exists() {
        true => crate::shell::program(own),
        false => crate::shell::program("yt-dlp"),
    };
    command.args(["--encoding", "utf-8"]);
    if dir.exists() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        if let Ok(path) = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&path))) {
            command.env("PATH", path);
        }
    }
    command
}

fn on_path(program: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

/// Makes sure the programs are there. `note` tells how it is going, for the
/// download waiting for them; it is only called when there is something to fetch.
pub async fn ensure(client: &reqwest::Client, note: impl Fn(String) + Send + Sync) -> Result<(), String> {
    ensure_in(client, &folder(), &plan(), &note).await
}

async fn ensure_in(client: &reqwest::Client, dir: &Path, tools: &[Tool], note: &(impl Fn(String) + Send + Sync)) -> Result<(), String> {
    if tools.is_empty() {
        return Ok(());
    }
    let _busy = BUSY.lock().await;
    let missing: Vec<&Tool> =
        tools.iter().filter(|tool| !dir.join(&tool.program).exists() && !on_path(&tool.program)).collect();
    for tool in missing {
        let say = |done, total| note(trf!("Preparando os vídeos, só na primeira vez: {} ({})", tool.name, amount(done, total)));
        install(client, dir, tool, &say).await.map_err(|e| trf!("não foi possível preparar os vídeos ({}): {}", tool.name, e))?;
        tracing::info!(tool = tool.name, "ferramenta de vídeo instalada");
    }
    if let Some(tool) = tools.first() {
        update(client, dir, tool, note).await;
    }
    Ok(())
}

/// A new yt-dlp, if the last look was over a day ago. Failing is no reason to
/// hold the video back: the copy here may still do.
async fn update(client: &reqwest::Client, dir: &Path, tool: &Tool, note: &(impl Fn(String) + Send + Sync)) {
    let stamp = dir.join("checked");
    let age = std::fs::metadata(&stamp).and_then(|file| file.modified()).ok().and_then(|at| SystemTime::now().duration_since(at).ok());
    if !dir.join(&tool.program).exists() || age.is_some_and(|age| age < UPDATE_EVERY) {
        return;
    }
    let _ = std::fs::write(&stamp, "");
    let latest = match tokio::time::timeout(UPDATE_CHECK, tool.sum.expected(client)).await {
        Ok(Ok(latest)) => latest,
        Ok(Err(e)) => return tracing::info!(error = %e, "não foi possível procurar um yt-dlp novo"),
        Err(_) => return tracing::info!("a procura por um yt-dlp novo demorou demais"),
    };
    if std::fs::read_to_string(sum_file(dir, tool)).is_ok_and(|installed| installed == latest) {
        return;
    }
    let say = |done, total| note(trf!("Atualizando o {} ({})", tool.name, amount(done, total)));
    // A copy in use cannot be replaced on Windows: tomorrow, then.
    match install(client, dir, tool, &say).await {
        Ok(()) => tracing::info!(tool = tool.name, "ferramenta de vídeo atualizada"),
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

impl Sum {
    async fn expected(&self, client: &reqwest::Client) -> Result<String, String> {
        match self {
            Sum::Fixed(sum) => Ok((*sum).to_string()),
            Sum::Listed { url, name } => {
                let response = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
                let list = response.text().await.map_err(|e| e.to_string())?;
                find_sum(&list, name).ok_or_else(|| trf!("{} não está na lista de somas", name))
            }
        }
    }
}

/// The SHA-256 of `name` in a list of sums: the usual `sum  name` lines, or a
/// file with one sum and no such line (Deno's on Windows).
pub(crate) fn find_sum(list: &str, name: &str) -> Option<String> {
    let is_sum = |word: &&str| word.len() == 64 && word.bytes().all(|b| b.is_ascii_hexdigit());
    let listed = list.lines().find_map(|line| {
        let named = line.split_whitespace().any(|word| word.trim_start_matches('*') == name);
        line.split_whitespace().find(is_sum).filter(|_| named)
    });
    let mut sums = list.split_whitespace().filter(is_sum);
    let only = sums.next().filter(|_| sums.next().is_none());
    listed.or(only).map(str::to_ascii_lowercase)
}

/// Fetches the tool, checks it and puts it in place.
async fn install(client: &reqwest::Client, dir: &Path, tool: &Tool, say: &(impl Fn(u64, Option<u64>) + Send + Sync)) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let expected = tool.sum.expected(client).await?;
    let file = dir.join(format!("{}.part", tool.program));
    let result = fetch(client, &tool.url, &file, say).await.and_then(|sum| match sum == expected {
        true => Ok(()),
        false => Err(trf!("a soma de {} não confere", tool.url)),
    });
    let placed = match (result, tool.form) {
        (Err(e), _) => Err(e),
        (Ok(()), Form::Program) => place(&file, &dir.join(&tool.program)),
        (Ok(()), Form::Zip) => unzip(&file, dir, &tool.program).await,
    };
    let _ = std::fs::remove_file(&file);
    placed?;
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

    /// Answers every request with "hello"; the URL of a file on it.
    async fn server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0; 1024];
                let _ = socket.read(&mut request).await;
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello").await;
            }
        });
        format!("http://{address}/file")
    }

    fn client() -> reqwest::Client {
        fisga_core::build_client_with(fisga_core::Proxy::Direct).unwrap()
    }

    fn tool(url: String, sum: &'static str) -> Tool {
        Tool { name: "teste", program: "fisga-tools-teste".into(), url, sum: Sum::Fixed(sum), form: Form::Program }
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
    }

    #[tokio::test]
    async fn fetches_checks_and_keeps_the_program() {
        let dir = scratch("install");
        let notes = std::sync::Mutex::new(Vec::new());
        let tools = [tool(server().await, SUM_OF_HELLO)];
        ensure_in(&client(), &dir, &tools, &|note: String| notes.lock().unwrap().push(note)).await.unwrap();
        assert_eq!(std::fs::read(dir.join(&tools[0].program)).unwrap(), b"hello");
        assert_eq!(std::fs::read_to_string(sum_file(&dir, &tools[0])).unwrap(), SUM_OF_HELLO);
        assert!(notes.lock().unwrap()[0].contains("primeira vez"));
        // There already: nothing to fetch, nothing to say.
        notes.lock().unwrap().clear();
        ensure_in(&client(), &dir, &tools, &|note: String| notes.lock().unwrap().push(note)).await.unwrap();
        assert!(notes.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_wrong_sum_leaves_nothing_behind() {
        let dir = scratch("sum");
        let tools = [tool(server().await, "0000000000000000000000000000000000000000000000000000000000000000")];
        let error = ensure_in(&client(), &dir, &tools, &|_: String| {}).await.unwrap_err();
        assert!(error.contains("soma"), "{error}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn yt_dlp_writes_utf_8() {
        let command = yt_dlp();
        let args: Vec<_> = command.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["--encoding", "utf-8"]);
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
    #[ignore = "fetches about 150 MB"]
    async fn the_real_tools_install_and_run() {
        let dir = scratch("real");
        for tool in plan() {
            install(&client(), &dir, &tool, &|_: u64, _: Option<u64>| {}).await.unwrap_or_else(|e| panic!("{}: {e}", tool.name));
            let flag = if tool.name.starts_with("ff") { "-version" } else { "--version" };
            let output = crate::shell::program(dir.join(&tool.program)).arg(flag).output().await.unwrap();
            let text = String::from_utf8_lossy(&output.stdout);
            println!("{}: {}", tool.name, text.lines().next().unwrap_or(""));
            assert!(output.status.success(), "{}: {output:?}", tool.name);
        }
        if dir.exists() {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
