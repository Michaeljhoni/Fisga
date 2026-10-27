//! `fisga-gui`: GTK4 interface of Fisga. Only a client of `fisgad` (see `fisga-ipc`);
//! all download logic stays in the daemon.
//!
//! - `fisga-gui`            opens the main window (download list).
//! - `fisga-gui --pending`  shows only the "Novo download" windows for downloads
//!   waiting for confirmation, and quits when they are answered. `fisgad` starts
//!   it this way when the browser sends a download and no GUI is open.
//! - `fisga-gui --background` runs without a window, in the tray (session
//!   autostart); the browser's downloads are confirmed from there.
//!
//! The app id is fixed (`io.fisga.Fisga`), so a second launch is forwarded to the
//! running instance through D-Bus, and compositors can match its windows.

mod app;
mod autostart;
mod batch;
mod cards;
mod confirm;
mod format;
mod grabber;
mod ipc;
mod item;
mod power;
mod prefs;
mod progress;
mod refresh;
mod speed;
mod theme;
mod tray;
mod verify;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.fisga.Fisga";

/// Started from an app launcher or by `fisgad`, stderr goes nowhere: keep GTK
/// warnings and our errors in `~/.local/state/fisga/fisga-gui.log` instead.
fn redirect_stderr_to_log() {
    use std::io::IsTerminal;
    use std::os::fd::AsRawFd;
    if std::io::stderr().is_terminal() {
        return;
    }
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/state")))
        .map(|state| state.join("fisga"));
    let Some(dir) = dir else { return };
    let file = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(dir.join("fisga-gui.log")));
    if let Ok(file) = file {
        // SAFETY: dup2 on valid descriptors; the file stays open for the process lifetime.
        unsafe { libc::dup2(file.as_raw_fd(), 2) };
        std::mem::forget(file);
        eprintln!("--- fisga-gui {} iniciado: {:?}", env!("CARGO_PKG_VERSION"), std::env::args().collect::<Vec<_>>());
    }
}

fn main() -> glib::ExitCode {
    fisga_ipc::migrate_legacy_dirs();
    redirect_stderr_to_log();
    autostart::migrate_legacy();
    let application = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let state = app::App::new(&application);

    let s = state.clone();
    application.connect_startup(move |_| {
        gtk::Window::set_default_icon_name(APP_ID);
        s.start()
    });
    let s = state.clone();
    application.connect_command_line(move |_, command_line| {
        let arguments = command_line.arguments();
        let has = |flag: &str| arguments.iter().skip(1).any(|arg| arg == flag);
        if has("--background") {
            s.run_in_background();
        } else if !has("--pending") {
            s.show_main_window();
        }
        glib::ExitCode::SUCCESS
    });
    application.run()
}
