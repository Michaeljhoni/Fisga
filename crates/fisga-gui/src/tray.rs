//! Icon in the system tray, through the StatusNotifierItem D-Bus protocol
//! (waybar, noctalia, KDE, GNOME with the AppIndicator extension...).
//!
//! The tray service runs on the tokio runtime of `ipc`; clicks come back to
//! the GTK main loop as `Action`s through a channel.

use ksni::menu::StandardItem;
use ksni::{MenuItem, TrayMethods};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Add,
    ResumeAll,
    PauseAll,
    Quit,
}

pub struct Tray {
    actions: mpsc::UnboundedSender<Action>,
    /// "2 baixando · 1,2 MB/s".
    summary: String,
}

pub type Handle = ksni::Handle<Tray>;

fn item(label: &str, icon: &str, action: Action) -> MenuItem<Tray> {
    StandardItem {
        label: label.into(),
        icon_name: icon.into(),
        activate: Box::new(move |tray: &mut Tray| {
            let _ = tray.actions.send(action);
        }),
        ..Default::default()
    }
    .into()
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        crate::APP_ID.into()
    }

    fn title(&self) -> String {
        "Fisga".into()
    }

    fn icon_name(&self) -> String {
        crate::APP_ID.into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "Fisga".into(), description: self.summary.clone(), ..Default::default() }
    }

    /// Left click: the main window.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.actions.send(Action::Open);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            // Not every tray shows tooltips (waybar does only on hover): the summary is here too.
            StandardItem { label: self.summary.clone(), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            item("Abrir o Fisga", "folder-download", Action::Open),
            item("Adicionar URL…", "list-add", Action::Add),
            MenuItem::Separator,
            item("Retomar todos", "media-playback-start", Action::ResumeAll),
            item("Pausar todos", "media-playback-pause", Action::PauseAll),
            MenuItem::Separator,
            item("Sair", "application-exit", Action::Quit),
        ]
    }

    /// The tray host went away (waybar restarted...): keep the service, the
    /// icon comes back when it returns.
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        eprintln!("fisga-gui: bandeja indisponível: {reason:?}");
        true
    }
}

/// Starts the tray icon. `wait_for_host`: succeed even when no tray host is
/// running yet (the session is still starting); the icon appears with it.
pub async fn spawn(
    actions: mpsc::UnboundedSender<Action>,
    summary: String,
    wait_for_host: bool,
) -> anyhow::Result<Handle> {
    let tray = Tray { actions, summary };
    let handle = crate::ipc::background(async move { tray.assume_sni_available(wait_for_host).spawn().await }).await??;
    Ok(handle)
}

pub async fn set_summary(handle: &Handle, summary: String) {
    let handle = handle.clone();
    let _ = crate::ipc::background(async move { handle.update(move |tray| tray.summary = summary).await }).await;
}

pub async fn shutdown(handle: Handle) {
    let _ = crate::ipc::background(async move { handle.shutdown().await }).await;
}
