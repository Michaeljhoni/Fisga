//! Launching `ldm-gui` for pending downloads ("Novo download" window).
//!
//! The GUI subscribes with `confirmer: true`; while one is connected it receives
//! the pending download as a normal `Update`. Otherwise the daemon starts
//! `ldm-gui --pending`, which shows only the confirmation window. If the GUI
//! cannot be started, or does not connect in time (no display, crash), the
//! download is queued anyway so the click in the browser is never lost.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ldm_ipc::State;

use crate::manager::Manager;
use crate::notify;

/// How long a freshly launched GUI has to connect before the download is
/// queued without confirmation.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A GUI launched this recently is assumed to be starting up: do not launch another.
const RELAUNCH_AFTER: Duration = Duration::from_secs(10);

pub struct Gui {
    confirmers: AtomicUsize,
    last_launch: Mutex<Option<Instant>>,
    /// `$LDM_GUI`, else `ldm-gui` next to `ldmd`. Empty = never launch.
    program: Option<PathBuf>,
    connect_timeout: Duration,
}

/// Keeps a confirmer counted while its subscription is open.
pub struct ConfirmerGuard(Arc<Gui>);

impl Drop for ConfirmerGuard {
    fn drop(&mut self) {
        self.0.confirmers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Gui {
    pub fn from_env() -> Arc<Self> {
        let program = match std::env::var_os("LDM_GUI") {
            Some(value) if value.is_empty() => None,
            Some(value) => Some(PathBuf::from(value)),
            None => Some(ldm_ipc::sibling_executable("ldm-gui")),
        };
        Self::new(program, CONNECT_TIMEOUT)
    }

    pub fn new(program: Option<PathBuf>, connect_timeout: Duration) -> Arc<Self> {
        Arc::new(Self { confirmers: AtomicUsize::new(0), last_launch: Mutex::new(None), program, connect_timeout })
    }

    pub fn attach(self: &Arc<Self>) -> ConfirmerGuard {
        self.confirmers.fetch_add(1, Ordering::SeqCst);
        // The launched GUI is up: if it quits, the next pending download launches it again.
        *self.last_launch.lock().unwrap() = None;
        ConfirmerGuard(self.clone())
    }

    pub fn has_confirmer(&self) -> bool {
        self.confirmers.load(Ordering::SeqCst) > 0
    }

    /// Makes sure someone will show the confirmation window for `id`.
    pub fn request_confirmation(self: &Arc<Self>, manager: &Arc<Manager>, id: u64) {
        if self.has_confirmer() {
            return;
        }
        let recently = self.last_launch.lock().unwrap().is_some_and(|t| t.elapsed() < RELAUNCH_AFTER);
        if !recently {
            if let Err(e) = self.launch() {
                tracing::warn!(error = %e, "não foi possível abrir o ldm-gui; o download começa sem confirmação");
                start_unconfirmed(manager, id);
                return;
            }
        }
        tokio::spawn(self.clone().fallback(manager.clone(), id));
    }

    fn launch(&self) -> anyhow::Result<()> {
        let program = self.program.as_ref().ok_or_else(|| anyhow::anyhow!("LDM_GUI vazio"))?;
        let mut command = tokio::process::Command::new(program);
        command
            .arg("--pending")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // Ctrl+C on a ldmd started from a terminal must not close the window.
            .process_group(0);
        let mut child = command.spawn()?;
        *self.last_launch.lock().unwrap() = Some(Instant::now());
        tracing::info!(program = %program.display(), "ldm-gui iniciado");
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        Ok(())
    }

    /// Queues the download if no GUI connected in time.
    async fn fallback(self: Arc<Self>, manager: Arc<Manager>, id: u64) {
        let deadline = Instant::now() + self.connect_timeout;
        while Instant::now() < deadline {
            if self.has_confirmer() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !self.has_confirmer() && manager.state(id) == Some(State::Pending) {
            tracing::warn!(id, "o ldm-gui não se conectou; o download começa sem confirmação");
            start_unconfirmed(&manager, id);
        }
    }
}

fn start_unconfirmed(manager: &Arc<Manager>, id: u64) {
    if manager.confirm(id, false, None, None).is_ok() {
        notify::send("Download adicionado", "A janela de confirmação não pôde ser aberta; o download foi para a fila.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldm_ipc::AddRequest;

    fn pending(manager: &Arc<Manager>) -> u64 {
        // Unreachable port: the queued job fails fast instead of downloading.
        let request = AddRequest { url: "http://127.0.0.1:9/a.iso".into(), confirm: true, ..Default::default() };
        manager.add(request).unwrap()
    }

    fn manager() -> Arc<Manager> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ldmd-gui-test-{}-{nanos}", std::process::id()));
        Manager::load(dir.join("downloads.json")).unwrap()
    }

    #[tokio::test]
    async fn missing_gui_starts_the_download_right_away() {
        let manager = manager();
        let gui = Gui::new(Some("/nonexistent/ldm-gui".into()), Duration::from_secs(60));
        let id = pending(&manager);
        gui.request_confirmation(&manager, id);
        assert_ne!(manager.state(id), Some(State::Pending));
    }

    #[tokio::test]
    async fn gui_that_never_connects_falls_back_to_the_queue() {
        let manager = manager();
        let gui = Gui::new(Some("true".into()), Duration::from_millis(200));
        let id = pending(&manager);
        gui.request_confirmation(&manager, id);
        assert_eq!(manager.state(id), Some(State::Pending));
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_ne!(manager.state(id), Some(State::Pending));
    }

    #[tokio::test]
    async fn connected_gui_keeps_the_download_pending() {
        let manager = manager();
        let gui = Gui::new(Some("/nonexistent/ldm-gui".into()), Duration::from_millis(200));
        let guard = gui.attach();
        let id = pending(&manager);
        gui.request_confirmation(&manager, id);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(manager.state(id), Some(State::Pending));
        drop(guard);
        assert!(!gui.has_confirmer());
    }

    #[tokio::test]
    async fn gui_that_connects_late_but_in_time_keeps_it_pending() {
        let manager = manager();
        let gui = Gui::new(Some("true".into()), Duration::from_millis(500));
        let id = pending(&manager);
        gui.request_confirmation(&manager, id);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _guard = gui.attach();
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(manager.state(id), Some(State::Pending));
    }
}
