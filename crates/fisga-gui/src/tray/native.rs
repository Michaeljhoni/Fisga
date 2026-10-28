//! Icon in the notification area of Windows and the menu bar of macOS,
//! through `tray-icon`. It lives on the GTK main thread, whose main loop also
//! runs the system's (window messages on Windows, the NSApplication loop on
//! macOS); its clicks are picked up from `tray-icon`'s channels by a timer.

use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk_pixbuf, gio, glib};
use tokio::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::Action;

const ICON: &[u8] = include_bytes!("../../data/icons/32/io.fisga.Fisga.png");

const MENU: &[Option<(&str, Action)>] = &[
    Some(("Abrir o Fisga", Action::Open)),
    Some(("Adicionar URL…", Action::Add)),
    None,
    Some(("Retomar todos", Action::ResumeAll)),
    Some(("Pausar todos", Action::PauseAll)),
    None,
    Some(("Sair", Action::Quit)),
];

pub struct Tray {
    icon: TrayIcon,
    /// "2 baixando · 1,2 MB/s", greyed out at the top of the menu.
    summary: MenuItem,
}

/// Dropping the last one removes the icon.
pub type Handle = Rc<Tray>;

/// The PNG as the RGBA rows `tray-icon` takes.
fn icon() -> anyhow::Result<tray_icon::Icon> {
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_static(ICON));
    let pixbuf = gdk_pixbuf::Pixbuf::from_stream(&stream, gio::Cancellable::NONE)?;
    anyhow::ensure!(pixbuf.n_channels() == 4 && pixbuf.bits_per_sample() == 8, "ícone sem canal alfa");
    let (width, height, stride) = (pixbuf.width() as usize, pixbuf.height() as usize, pixbuf.rowstride() as usize);
    let pixels = pixbuf.read_pixel_bytes();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        rgba.extend_from_slice(&pixels[row * stride..row * stride + width * 4]);
    }
    Ok(tray_icon::Icon::from_rgba(rgba, width as u32, height as u32)?)
}

/// `_wait_for_host`: the system's tray is always there.
pub async fn spawn(actions: mpsc::UnboundedSender<Action>, summary: String, _wait_for_host: bool) -> anyhow::Result<Handle> {
    let menu = Menu::new();
    let summary_item = MenuItem::new(&summary, false, None);
    menu.append(&summary_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    let mut ids: Vec<(MenuId, Action)> = Vec::new();
    for entry in MENU {
        match entry {
            Some((label, action)) => {
                let item = MenuItem::new(label, true, None);
                ids.push((item.id().clone(), *action));
                menu.append(&item)?;
            }
            None => menu.append(&PredefinedMenuItem::separator())?,
        }
    }
    let icon = TrayIconBuilder::new()
        .with_tooltip(tooltip(&summary))
        .with_icon(icon()?)
        .with_menu(Box::new(menu))
        // Left click opens the window, as on Linux; the menu is on the right button.
        .with_menu_on_left_click(cfg!(target_os = "macos"))
        .build()?;
    let tray = Rc::new(Tray { icon, summary: summary_item });

    let weak = Rc::downgrade(&tray);
    glib::timeout_add_local(Duration::from_millis(150), move || {
        if weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some((_, action)) = ids.iter().find(|(id, _)| *id == event.id) {
                let _ = actions.send(*action);
            }
        }
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                let _ = actions.send(Action::Open);
            }
        }
        glib::ControlFlow::Continue
    });
    Ok(tray)
}

fn tooltip(summary: &str) -> String {
    match summary.is_empty() {
        true => "Fisga".into(),
        false => format!("Fisga\n{summary}"),
    }
}

pub async fn set_summary(handle: &Handle, summary: String) {
    handle.summary.set_text(&summary);
    let _ = handle.icon.set_tooltip(Some(tooltip(&summary)));
}

pub async fn shutdown(handle: Handle) {
    drop(handle);
}
