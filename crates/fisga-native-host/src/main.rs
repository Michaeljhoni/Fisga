//! Native messaging host. The browser starts it and exchanges messages framed as
//! a 32-bit native-endian length followed by UTF-8 JSON on stdin/stdout.
//! stdout carries only framed messages; diagnostics go to stderr (browser log).


/// The English of this crate's texts (see fisga-i18n).
static I18N: fisga_i18n::Table = fisga_i18n::Table::new(include_str!("../i18n/en.txt"));

use std::io::{self, Read, Write};

use fisga_ipc::{AddRequest, Request, Response};
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
    /// "Baixar todos os links com o Fisga": a page's links, to pick from in the app.
    #[serde(rename_all = "camelCase")]
    Links { page_url: String, links: Vec<String> },
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
            let daemon = matches!(fisga_ipc::call(&Request::Ping).await, Ok(Response::Pong { .. }));
            json!({ "type": "pong", "version": fisga_ipc::VERSION, "daemon": daemon })
        }
        // Not worth starting fisgad: the extension keeps what it read last.
        Incoming::Settings => match fisga_ipc::call(&Request::GetSettings).await {
            Ok(Response::Settings { settings }) => {
                let browser = settings.browser;
                json!({
                    "type": "settings",
                    "fileTypes": browser.file_types,
                    "forceKey": browser.force_key,
                    "excludedSites": browser.excluded_sites,
                })
            }
            Ok(other) => json!({ "type": "error", "message": format!("resposta inesperada: {other:?}") }),
            Err(e) => json!({ "type": "error", "message": e.to_string() }),
        },
        Incoming::Links { page_url, links } => {
            let result = async {
                fisga_ipc::ensure_daemon().await?;
                fisga_ipc::call(&Request::ShowLinks { page: page_url, links }).await
            }
            .await;
            match result {
                Ok(_) => json!({ "type": "shown" }),
                Err(e) => json!({ "type": "error", "message": e.to_string() }),
            }
        }
        Incoming::Download(request) => {
            let pending = request.confirm;
            let result = async {
                fisga_ipc::ensure_daemon().await?;
                fisga_ipc::call(&Request::Add(request)).await
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
    fisga_ipc::init_language();
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    while let Some(body) = read_message(&mut stdin)? {
        let reply = match parse(&body) {
            Ok(message) => handle(message).await,
            Err(e) => {
                eprintln!("fisga-native-host: mensagem inválida: {e}");
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
        let links = parse(br#"{"type":"links","pageUrl":"https://e.com/","links":["https://e.com/a.zip"]}"#).unwrap();
        assert!(matches!(links, Incoming::Links { page_url, links } if page_url == "https://e.com/" && links.len() == 1));
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

#[cfg(test)]
mod i18n_tests {
    /// Every text shown has its English, and every English its text.
    #[test]
    fn every_text_has_its_english() {
        let problems = fisga_i18n::check_crate(env!("CARGO_MANIFEST_DIR"), include_str!("../i18n/en.txt"));
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }
}
