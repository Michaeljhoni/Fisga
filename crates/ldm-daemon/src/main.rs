//! `ldmd`: owns the download list and runs the downloads. Clients (CLI, GUI,
//! browser host) talk to it over a Unix socket; see `ldm-ipc`.

mod clipboard;
mod gui;
mod manager;
mod notify;
mod ytdlp;

use std::io::IsTerminal;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use anyhow::Context;
use ldm_ipc::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};
use tokio_util::sync::CancellationToken;

use crate::gui::{Gui, Unanswered};
use crate::manager::{Answer, Manager};

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("LDM_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if std::io::stderr().is_terminal() {
        builder.init();
        return;
    }
    let log_dir = dirs::state_dir().unwrap_or_else(std::env::temp_dir).join("ldm");
    let file = std::fs::create_dir_all(&log_dir)
        .and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(log_dir.join("ldmd.log")));
    match file {
        Ok(file) => builder.with_ansi(false).with_writer(std::sync::Mutex::new(file)).init(),
        Err(_) => builder.init(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let runtime_dir = ldm_ipc::runtime_dir();
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700))?;
    let socket = ldm_ipc::socket_path();
    if UnixStream::connect(&socket).await.is_ok() {
        tracing::info!("ldmd já está rodando");
        return Ok(());
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).with_context(|| format!("bind {}", socket.display()))?;

    let store = dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("ldm").join("downloads.json");
    let settings = dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("ldm").join("settings.json");
    let manager = Manager::load(store, settings)?;
    let gui = Gui::from_env();
    let shutdown = CancellationToken::new();
    tracing::info!(socket = %socket.display(), "ldmd {} pronto", ldm_ipc::VERSION);

    tokio::spawn(manager.clone().autosave(shutdown.clone()));
    tokio::spawn(scheduler(manager.clone(), gui.clone(), shutdown.clone()));
    clipboard::spawn(manager.clone(), gui.clone());
    manager.schedule();

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(serve_client(manager.clone(), gui.clone(), stream, shutdown.clone()));
                }
                Err(e) => tracing::warn!(error = %e, "accept falhou"),
            },
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
            _ = shutdown.cancelled() => break,
        }
    }

    tracing::info!("encerrando");
    shutdown.cancel();
    manager.shutdown().await;
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

/// Checks the scheduler's clock. A planned suspend/power off must be seen by
/// someone who can cancel it: without a GUI connected, one is launched.
async fn scheduler(manager: Arc<Manager>, gui: Arc<Gui>, shutdown: CancellationToken) {
    let mut ticker = tokio::time::interval(manager::tick_interval());
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = ticker.tick() => {
                let planned = manager.power_plan().is_some();
                manager.scheduler_tick(ldm_ipc::LocalTime::now());
                if !planned && manager.power_plan().is_some() {
                    gui.show_window();
                }
            }
        }
    }
}

async fn write(writer:&mut tokio::net::unix::OwnedWriteHalf, response: &Response) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(response)?;
    line.push(b'\n');
    writer.write_all(&line).await
}

async fn serve_client(manager: Arc<Manager>, gui: Arc<Gui>, stream: UnixStream, shutdown: CancellationToken) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(e) => {
                let _ = write(&mut writer, &Response::Error { message: format!("pedido inválido: {e}") }).await;
                continue;
            }
        };
        let response = match request {
            Request::Ping => Response::Pong { version: ldm_ipc::VERSION.to_string() },
            Request::Add(add) => {
                let confirm = add.confirm;
                match manager.add(add) {
                    Ok(id) => {
                        if confirm {
                            gui.request_confirmation(&manager, id, Unanswered::Queue);
                        }
                        Response::Added { id }
                    }
                    Err(e) => Response::Error { message: e.to_string() },
                }
            }
            Request::Confirm { id, now, file_name, output_dir, format, conflict, queue, checksum } => {
                result(manager.confirm(id, Answer { now, file_name, output_dir, format, conflict, queue, checksum }))
            }
            Request::SetQueue { id, queue } => result(manager.set_queue(id, queue)),
            Request::Reorder { id, direction } => result(manager.reorder(id, direction)),
            Request::RefreshUrl { id, url } => result(manager.refresh_url(id, url).await),
            Request::AwaitUrl { id, waiting } => result(manager.await_url(id, waiting)),
            Request::SetSpeedLimit { id, limit } => result(manager.set_speed_limit(id, limit)),
            Request::Verify { id, checksum } => result(manager.verify(id, checksum)),
            Request::Grab { url, depth } => match manager.grab(&url, depth).await {
                Ok((links, pages)) => Response::Grabbed { links, pages },
                Err(e) => Response::Error { message: format!("{e:#}") },
            },
            Request::AddBatch { urls, output_dir, queue, start } => match manager.add_batch(urls, output_dir, queue, start) {
                Ok(ids) => Response::AddedBatch { ids },
                Err(e) => Response::Error { message: format!("{e:#}") },
            },
            Request::List => Response::Downloads { items: manager.list() },
            Request::Pause { id } => result(manager.pause(id)),
            Request::Resume { id } => result(manager.resume(id)),
            Request::Remove { id, delete_file } => match manager.remove(id, delete_file).await {
                Ok(()) => Response::Removed { id },
                Err(e) => Response::Error { message: e.to_string() },
            },
            Request::GetSettings => Response::Settings { settings: manager.settings() },
            Request::SetSettings { settings } => result(manager.set_settings(settings)),
            Request::CancelPower => result(manager.cancel_power()),
            Request::GetLogins => Response::Logins { logins: manager.logins() },
            Request::SetLogins { logins } => result(manager.set_logins(logins)),
            Request::Shutdown => {
                shutdown.cancel();
                Response::Ok
            }
            Request::Subscribe { confirmer } => {
                let _confirmer = confirmer.then(|| gui.attach());
                // Subscribe before taking the snapshot so no update is lost.
                let mut updates = manager.subscribe();
                // A planned power off first: `ldm-gui --pending` quits after the
                // list arrives unless a window (this countdown) is already open.
                if let Some(plan) = manager.power_plan() {
                    if write(&mut writer, &Response::Power { plan: Some(plan) }).await.is_err() {
                        return;
                    }
                }
                if write(&mut writer, &Response::Downloads { items: manager.list() }).await.is_err() {
                    return;
                }
                if write(&mut writer, &Response::Settings { settings: manager.settings() }).await.is_err() {
                    return;
                }
                loop {
                    tokio::select! {
                        update = updates.recv() => match update {
                            Ok(update) => if write(&mut writer, &update).await.is_err() { return },
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                let items = manager.list();
                                if write(&mut writer, &Response::Downloads { items }).await.is_err() { return }
                            }
                            Err(_) => return,
                        },
                        line = lines.next_line() => if !matches!(line, Ok(Some(_))) { return },
                        _ = shutdown.cancelled() => return,
                    }
                }
            }
        };
        if write(&mut writer, &response).await.is_err() {
            return;
        }
    }
}

fn result(r: anyhow::Result<()>) -> Response {
    match r {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error { message: e.to_string() },
    }
}
