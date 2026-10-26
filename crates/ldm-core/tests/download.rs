//! End-to-end tests against a tiny in-process HTTP/1.1 server with Range support.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ldm_core::{download, DownloadOptions, EngineEvent, Outcome, RequestSpec, Target};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct ServerOpts {
    ranges: bool,
    /// Pause after each 16 KiB write, to make downloads slow enough to cancel.
    delay: Duration,
    /// Answer this many requests with 503 before serving normally.
    fail_first: Arc<AtomicUsize>,
    /// The first range request of more than 512 KiB sends 64 KiB and then
    /// nothing more, never closing: a stalled connection.
    stall_once: Arc<AtomicBool>,
    /// More range requests at once than this get 503 with `Retry-After: 1`
    /// (0 = no limit); `busy` counts them.
    max_active: usize,
    active: Arc<AtomicUsize>,
    busy: Arc<AtomicUsize>,
    /// Range requests other than the probe's (`bytes=0-0`) get 404.
    missing_segments: bool,
}

struct TestServer {
    url: String,
    range_requests: Arc<AtomicUsize>,
}

fn test_data(len: usize) -> Arc<Vec<u8>> {
    Arc::new((0..len).map(|i| (i.wrapping_mul(31) ^ (i >> 9)) as u8).collect())
}

async fn serve(data: Arc<Vec<u8>>, opts: ServerOpts) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/files/test.bin", listener.local_addr().unwrap());
    let range_requests = Arc::new(AtomicUsize::new(0));
    let counter = range_requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else { return };
            let (data, opts, counter) = (data.clone(), opts.clone(), counter.clone());
            tokio::spawn(async move {
                let _ = handle(socket, data, opts, counter).await;
            });
        }
    });
    TestServer { url, range_requests }
}

async fn handle(
    mut socket: tokio::net::TcpStream,
    data: Arc<Vec<u8>>,
    opts: ServerOpts,
    counter: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut pending = Vec::new();
    loop {
        // Read one request head (keep-alive: several requests per connection).
        let head_end = loop {
            if let Some(pos) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            pending.extend_from_slice(&buf[..n]);
        };
        let head = String::from_utf8_lossy(&pending[..head_end]).to_string();
        pending.drain(..head_end);

        #[allow(deprecated)] // try_update needs a newer Rust than our MSRV (1.80)
        let failing = opts.fail_first.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
        if failing {
            socket
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                .await?;
            continue;
        }

        let range = head
            .lines()
            .find_map(|l| l.strip_prefix("range: ").or_else(|| l.strip_prefix("Range: ")))
            .and_then(|v| v.trim().strip_prefix("bytes="))
            .map(|v| {
                let (a, b) = v.split_once('-').unwrap();
                let start: usize = a.parse().unwrap();
                let end: usize = if b.is_empty() { data.len() - 1 } else { b.parse().unwrap() };
                (start, end.min(data.len() - 1))
            });

        if opts.missing_segments && range.is_some_and(|(_, end)| end > 0) {
            socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await?;
            continue;
        }
        let limited = opts.max_active > 0 && range.is_some();
        // Frees the slot however the answer ends: clients drop a split segment's connection mid-body.
        let _slot = limited.then(|| Slot(opts.active.clone()));
        if limited && opts.active.fetch_add(1, Ordering::SeqCst) >= opts.max_active {
            opts.busy.fetch_add(1, Ordering::SeqCst);
            socket
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1\r\nContent-Length: 0\r\n\r\n")
                .await?;
            continue;
        }

        let (status, start, end) = match range {
            Some((start, end)) if opts.ranges => {
                counter.fetch_add(1, Ordering::Relaxed);
                ("206 Partial Content", start, end)
            }
            _ => ("200 OK", 0, data.len() - 1),
        };
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"test.bin\"\r\n",
            end + 1 - start
        );
        if opts.ranges {
            response.push_str("Accept-Ranges: bytes\r\n");
            if status.starts_with("206") {
                response.push_str(&format!("Content-Range: bytes {start}-{end}/{}\r\n", data.len()));
            }
        }
        response.push_str("\r\n");
        socket.write_all(response.as_bytes()).await?;
        let stall = end + 1 - start > 512 * 1024 && opts.stall_once.swap(false, Ordering::SeqCst);
        for (i, chunk) in data[start..=end].chunks(16 * 1024).enumerate() {
            if stall && i == 4 {
                std::future::pending::<()>().await;
            }
            socket.write_all(chunk).await?;
            if !opts.delay.is_zero() {
                tokio::time::sleep(opts.delay).await;
            }
        }
    }
}

/// One answer in progress; leaving the scope (or the connection) ends it.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ldm-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn run(
    url: &str,
    target: Target,
    connections: usize,
    cancel: CancellationToken,
) -> (ldm_core::Result<Outcome>, Vec<EngineEvent>) {
    let client = ldm_core::build_client().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let opts = DownloadOptions { connections, connection_stagger: Duration::from_millis(20), ..Default::default() };
    let result = download(&client, &RequestSpec::new(url), target, &opts, tx, cancel).await;
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    (result, events)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn segmented_download_matches_source() {
    let data = test_data(6 * 1024 * 1024 + 123);
    let server = serve(data.clone(), ServerOpts { ranges: true, delay: Duration::from_millis(1), ..Default::default() }).await;
    let dir = temp_dir("segmented");

    let (result, events) =
        run(&server.url, Target::Dir { dir: dir.clone(), file_name: None }, 4, CancellationToken::new()).await;
    let Outcome::Completed(path) = result.unwrap() else { panic!("not completed") };

    assert_eq!(path, dir.join("test.bin"));
    assert_eq!(std::fs::read(&path).unwrap(), *data);
    assert!(!dir.join("test.bin.ldm-part").exists());
    assert!(!dir.join("test.bin.ldm-part.json").exists());
    assert!(matches!(events.first(), Some(EngineEvent::Started { resumable: true, .. })));
    // Probe + at least two segment requests means the file really was split.
    assert!(server.range_requests.load(Ordering::Relaxed) >= 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_without_ranges_uses_one_stream() {
    let data = test_data(300 * 1024);
    let server = serve(data.clone(), ServerOpts { ranges: false, ..Default::default() }).await;
    let dir = temp_dir("single");

    let (result, events) =
        run(&server.url, Target::Dir { dir: dir.clone(), file_name: None }, 8, CancellationToken::new()).await;
    let Outcome::Completed(path) = result.unwrap() else { panic!("not completed") };

    assert_eq!(std::fs::read(path).unwrap(), *data);
    assert!(matches!(events.first(), Some(EngineEvent::Started { resumable: false, .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_and_resume() {
    let data = test_data(8 * 1024 * 1024);
    let slow = serve(data.clone(), ServerOpts { ranges: true, delay: Duration::from_millis(8), ..Default::default() }).await;
    let dir = temp_dir("resume");

    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        trigger.cancel();
    });
    let (result, _) = run(&slow.url, Target::Dir { dir: dir.clone(), file_name: None }, 4, cancel).await;
    let Outcome::Paused(path) = result.unwrap() else { panic!("expected a pause") };
    assert!(dir.join("test.bin.ldm-part.json").exists());
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("test.bin.ldm-part.json")).unwrap()).unwrap();
    let done: u64 = saved["segments"].as_array().unwrap().iter().map(|s| s["done"].as_u64().unwrap()).sum();
    assert!(done > 0 && done < data.len() as u64, "partial progress was saved ({done} bytes)");

    // The resumed run also survives a temporary 503 on its first request.
    let fast = serve(
        data.clone(),
        ServerOpts { ranges: true, fail_first: Arc::new(AtomicUsize::new(1)), ..Default::default() },
    )
    .await;
    let (result, _) = run(&fast.url, Target::File(path.clone()), 4, CancellationToken::new()).await;
    let Outcome::Completed(done_path) = result.unwrap() else { panic!("not completed") };
    assert_eq!(done_path, path);
    assert_eq!(std::fs::read(&done_path).unwrap(), *data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_connection_is_taken_over() {
    let data = test_data(3 * 1024 * 1024);
    let opts = ServerOpts {
        ranges: true,
        delay: Duration::from_millis(1),
        stall_once: Arc::new(AtomicBool::new(true)),
        ..Default::default()
    };
    let server = serve(data.clone(), opts).await;
    let dir = temp_dir("stall");

    // The stalled connection would only time out after 30 s.
    let started = Instant::now();
    let (result, _) =
        run(&server.url, Target::Dir { dir: dir.clone(), file_name: None }, 4, CancellationToken::new()).await;
    let Outcome::Completed(path) = result.unwrap() else { panic!("not completed") };
    assert_eq!(std::fs::read(path).unwrap(), *data);
    assert!(started.elapsed() < Duration::from_secs(20), "took {:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_server_gets_fewer_connections() {
    let data = test_data(4 * 1024 * 1024);
    let busy = Arc::new(AtomicUsize::new(0));
    let opts = ServerOpts { ranges: true, delay: Duration::from_millis(1), max_active: 2, busy: busy.clone(), ..Default::default() };
    let server = serve(data.clone(), opts).await;
    let dir = temp_dir("busy");

    let (result, _) =
        run(&server.url, Target::Dir { dir: dir.clone(), file_name: None }, 8, CancellationToken::new()).await;
    let Outcome::Completed(path) = result.unwrap() else { panic!("not completed") };
    assert_eq!(std::fs::read(path).unwrap(), *data);
    // Each extra connection leaves at its first 503 instead of insisting.
    let refused = busy.load(Ordering::SeqCst);
    assert!(refused <= 10, "{refused} requests refused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_file_fails_without_retrying() {
    let data = test_data(2 * 1024 * 1024);
    let server = serve(data, ServerOpts { ranges: true, missing_segments: true, ..Default::default() }).await;
    let dir = temp_dir("missing");

    let started = Instant::now();
    let (result, _) =
        run(&server.url, Target::Dir { dir: dir.clone(), file_name: None }, 4, CancellationToken::new()).await;
    assert!(matches!(result, Err(ldm_core::Error::Status(s)) if s == 404), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
}
