//! `ldm-gui`: GTK4 interface of LDM. Only a client of `ldmd` (see `ldm-ipc`);
//! all download logic stays in the daemon.
//!
//! - `ldm-gui`            opens the main window (download list).
//! - `ldm-gui --pending`  shows only the "Novo download" windows for downloads
//!   waiting for confirmation, and quits when they are answered. `ldmd` starts
//!   it this way when the browser sends a download and no GUI is open.
//! - `ldm-gui --background` runs without a window, in the tray (session
//!   autostart); the browser's downloads are confirmed from there.
//!
//! The app id is fixed (`io.ldm.Ldm`), so a second launch is forwarded to the
//! running instance through D-Bus, and compositors can match its windows.

mod app;
mod autostart;
mod batch;
mod confirm;
mod format;
mod grabber;
mod ipc;
mod item;
mod power;
mod prefs;
mod progress;
mod refresh;
mod tray;
mod verify;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.ldm.Ldm";

/// Started from an app launcher or by `ldmd`, stderr goes nowhere: keep GTK
/// warnings and our errors in `~/.local/state/ldm/ldm-gui.log` instead.
fn redirect_stderr_to_log() {
    use std::io::IsTerminal;
    use std::os::fd::AsRawFd;
    if std::io::stderr().is_terminal() {
        return;
    }
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/state")))
        .map(|state| state.join("ldm"));
    let Some(dir) = dir else { return };
    let file = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(dir.join("ldm-gui.log")));
    if let Ok(file) = file {
        // SAFETY: dup2 on valid descriptors; the file stays open for the process lifetime.
        unsafe { libc::dup2(file.as_raw_fd(), 2) };
        std::mem::forget(file);
        eprintln!("--- ldm-gui {} iniciado: {:?}", env!("CARGO_PKG_VERSION"), std::env::args().collect::<Vec<_>>());
    }
}

fn main() -> glib::ExitCode {
    redirect_stderr_to_log();
    let application = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let state = app::App::new(&application);

    let s = state.clone();
    application.connect_startup(move |_| s.start());
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
