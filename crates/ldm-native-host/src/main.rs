//! Native messaging host. The browser starts it and exchanges messages framed as
//! a 32-bit native-endian length followed by UTF-8 JSON on stdin/stdout.
//! stdout carries only framed messages; diagnostics go to stderr (browser log).

use std::io::{self, Read, Write};

use ldm_ipc::{AddRequest, Request, Response};
use serde::Deserialize;
use serde_json::{json, Value};

/// Chrome limits messages sent to the host to 4 GiB; anything this large is a bug.
const MAX_MESSAGE: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum Incoming {
    Ping,
    Download(AddRequest),
}

fn read_message(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_ne_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "mensagem grande demais"));
    }
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    Ok(Some(body))
}

fn write_message(output: &mut impl Write, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value)?;
    output.write_all(&(body.len() as u32).to_ne_bytes())?;
    output.write_all(&body)?;
    output.flush()
}

async fn handle(message: Incoming) -> Value {
    match message {
        Incoming::Ping => {
            let daemon = matches!(ldm_ipc::call(&Request::Ping).await, Ok(Response::Pong { .. }));
            json!({ "type": "pong", "version": ldm_ipc::VERSION, "daemon": daemon })
        }
        Incoming::Download(request) => {
            let result = async {
                ldm_ipc::ensure_daemon().await?;
                ldm_ipc::call(&Request::Add(request)).await
            }
            .await;
            match result {
                Ok(Response::Added { id }) => json!({ "type": "added", "id": id }),
                Ok(other) => json!({ "type": "error", "message": format!("resposta inesperada: {other:?}") }),
                Err(e) => json!({ "type": "error", "message": e.to_string() }),
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    while let Some(body) = read_message(&mut stdin)? {
        let reply = match serde_json::from_slice::<Incoming>(&body) {
            Ok(message) => handle(message).await,
            Err(e) => {
                eprintln!("ldm-native-host: mensagem inválida: {e}");
                json!({ "type": "error", "message": format!("mensagem inválida: {e}") })
            }
        };
        write_message(&mut stdout, &reply)?;
    }
    Ok(())
}
