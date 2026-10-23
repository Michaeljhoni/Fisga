//! Bridge between the GTK main loop and `ldmd`. The socket I/O runs on a small
//! tokio runtime in another thread; results come back as futures (awaited with
//! `glib::spawn_future_local`) and as a stream of `Event`s.

use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

use ldm_ipc::{Connection, DownloadInfo, PowerPlan, Request, Response, Settings};
use tokio::sync::mpsc;

pub enum Event {
    /// Full list, right after (re)connecting.
    Snapshot(Vec<DownloadInfo>),
    Update(DownloadInfo),
    Removed(u64),
    Settings(Settings),
    /// A suspend/power off was planned (or called off: `None`).
    Power(Option<PowerPlan>),
    Disconnected(String),
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("ldm-ipc")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// One request to the daemon.
pub async fn call(request: Request) -> anyhow::Result<Response> {
    background(async move { ldm_ipc::call(&request).await }).await?
}

/// Runs a future on the tokio runtime (socket I/O, the tray's D-Bus service)
/// and gives back its result.
pub async fn background<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> anyhow::Result<T> {
    Ok(runtime().spawn(future).await?)
}

/// Subscribes as the confirmer (the client that shows "Novo download"),
/// reconnecting (and starting `ldmd` if needed) when the connection drops.
pub fn subscribe() -> mpsc::UnboundedReceiver<Event> {
    let (tx, rx) = mpsc::unbounded_channel();
    runtime().spawn(async move {
        loop {
            let reason = match session(&tx).await {
                Ok(()) => "o ldmd encerrou a conexão".to_string(),
                Err(e) => e.to_string(),
            };
            if tx.send(Event::Disconnected(reason)).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    rx
}

async fn session(tx: &mpsc::UnboundedSender<Event>) -> anyhow::Result<()> {
    ldm_ipc::ensure_daemon().await?;
    let mut connection = Connection::open().await?;
    connection.send(&Request::Subscribe { confirmer: true }).await?;
    while let Some(message) = connection.recv().await? {
        let event = match message {
            Response::Downloads { items } => Event::Snapshot(items),
            Response::Update { item } => Event::Update(item),
            Response::Removed { id } => Event::Removed(id),
            Response::Settings { settings } => Event::Settings(settings),
            Response::Power { plan } => Event::Power(plan),
            _ => continue,
        };
        if tx.send(event).is_err() {
            return Ok(());
        }
    }
    Ok(())
}
