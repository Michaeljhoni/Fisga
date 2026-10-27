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
#[allow(clippy::large_enum_variant)] // one short-lived value per message
enum Incoming {
    Ping,
    /// The file types and the force key of the "Navegador" preferences.
    Settings,
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
        // Not worth starting ldmd: the extension keeps what it read last.
        Incoming::Settings => match ldm_ipc::call(&Request::GetSettings).await {
            Ok(Response::Settings { settings }) => {
                json!({ "type": "settings", "fileTypes": settings.browser.file_types, "forceKey": settings.browser.force_key })
            }
            Ok(other) => json!({ "type": "error", "message": format!("resposta inesperada: {other:?}") }),
            Err(e) => json!({ "type": "error", "message": e.to_string() }),
        },
        Incoming::Download(request) => {
            let pending = request.confirm;
            let result = async {
                ldm_ipc::ensure_daemon().await?;
                ldm_ipc::call(&Request::Add(request)).await
            }
            .await;
            match result {
                Ok(Response::Added { id }) => json!({ "type": "added", "id": id, "pending": pending }),
                Ok(other) => json!({ "type": "error", "message": format!("resposta inesperada: {other:?}") }),
                Err(e) => json!({ "type": "error", "message": e.to_string() }),
            }
        }
    }
}

/// Downloads from the browser open the "Novo download" window unless the
/// extension sends `"confirm": false` (its "começar sem confirmação" option).
fn parse(body: &[u8]) -> serde_json::Result<Incoming> {
    let mut value: Value = serde_json::from_slice(body)?;
    if let Some(object) = value.as_object_mut() {
        if object.get("type").and_then(Value::as_str) == Some("download") {
            object.entry("confirm").or_insert(Value::Bool(true));
        }
    }
    serde_json::from_value(value)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    while let Some(body) = read_message(&mut stdin)? {
        let reply = match parse(&body) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn download(body: &str) -> AddRequest {
        match parse(body.as_bytes()).unwrap() {
            Incoming::Download(request) => request,
            _ => panic!("expected a download"),
        }
    }

    #[test]
    fn browser_downloads_ask_for_confirmation_by_default() {
        let request = download(r#"{"type":"download","url":"https://e.com/a.mp4","kind":"direct","title":"A"}"#);
        assert!(request.confirm);
        assert_eq!(request.title.as_deref(), Some("A"));
    }

    #[test]
    fn extension_can_skip_confirmation() {
        let request = download(r#"{"type":"download","url":"https://e.com/a.mp4","confirm":false}"#);
        assert!(!request.confirm);
    }

    #[test]
    fn ping_and_invalid_messages() {
        assert!(matches!(parse(br#"{"type":"ping"}"#).unwrap(), Incoming::Ping));
        assert!(matches!(parse(br#"{"type":"settings"}"#).unwrap(), Incoming::Settings));
        assert!(parse(br#"{"type":"nope"}"#).is_err());
        assert!(parse(b"[1,2]").is_err());
    }

    #[test]
    fn framing_roundtrip() {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &json!({"type": "pong"})).unwrap();
        let body = read_message(&mut buffer.as_slice()).unwrap().unwrap();
        assert_eq!(body, br#"{"type":"pong"}"#);
        assert!(read_message(&mut &b""[..]).unwrap().is_none());
    }
}
