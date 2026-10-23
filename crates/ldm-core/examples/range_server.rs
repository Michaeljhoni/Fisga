//! Local HTTP/1.1 test server with Range support, used by scripts/e2e.sh.
//!
//!   cargo run -p ldm-core --example range_server -- [port] [delay-ms-per-16KiB]
//!
//! `GET /files/<bytes>.bin` returns `<bytes>` of deterministic data (any query string is ignored).

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn byte_at(i: u64) -> u8 {
    (i.wrapping_mul(31) ^ (i >> 9)) as u8
}

async fn handle(mut socket: TcpStream, delay: Duration) -> std::io::Result<()> {
    let mut pending = Vec::new();
    loop {
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

        // The query is ignored: "?token=..." stands for another link to the same file.
        let path = head.split_whitespace().nth(1).unwrap_or("/");
        let path = path.split('?').next().unwrap_or(path);
        let size = path
            .strip_prefix("/files/")
            .and_then(|p| p.strip_suffix(".bin"))
            .and_then(|n| n.parse::<u64>().ok());
        let Some(size) = size.filter(|s| *s > 0) else {
            socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await?;
            continue;
        };
        let range = head
            .lines()
            .find_map(|l| {
                let (name, value) = l.split_once(':')?;
                name.eq_ignore_ascii_case("range").then(|| value.trim().to_string())
            })
            .and_then(|v| {
                let (a, b) = v.strip_prefix("bytes=")?.split_once('-')?;
                let start: u64 = a.parse().ok()?;
                let end: u64 = if b.is_empty() { size - 1 } else { b.parse().ok()? };
                Some((start, end.min(size - 1)))
            });
        let (start, end) = range.unwrap_or((0, size - 1));
        let mut response = format!(
            "HTTP/1.1 {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Type: application/octet-stream\r\n",
            if range.is_some() { "206 Partial Content" } else { "200 OK" },
            end + 1 - start
        );
        if range.is_some() {
            response.push_str(&format!("Content-Range: bytes {start}-{end}/{size}\r\n"));
        }
        response.push_str("\r\n");
        socket.write_all(response.as_bytes()).await?;

        let mut offset = start;
        let mut chunk = Vec::with_capacity(16 * 1024);
        while offset <= end {
            chunk.clear();
            let stop = (offset + 16 * 1024).min(end + 1);
            chunk.extend((offset..stop).map(byte_at));
            socket.write_all(&chunk).await?;
            offset = stop;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let delay = Duration::from_millis(args.next().and_then(|d| d.parse().ok()).unwrap_or(0));
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    println!("http://{}", listener.local_addr()?);
    loop {
        let (socket, _) = listener.accept().await?;
        tokio::spawn(async move {
            let _ = handle(socket, delay).await;
        });
    }
}
