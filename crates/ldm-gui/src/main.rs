//! `ldm-gui`: GTK4 interface of LDM. Only a client of `ldmd` (see `ldm-ipc`);
//! all download logic stays in the daemon.
//!
//! - `ldm-gui`            opens the main window (download list).
//! - `ldm-gui --pending`  shows only the "Novo download" windows for downloads
//!   waiting for confirmation, and quits when they are answered. `ldmd` starts
//!   it this way when the browser sends a download and no GUI is open.
//!
//! The app id is fixed (`io.ldm.Ldm`), so a second launch is forwarded to the
//! running instance through D-Bus, and compositors can match its windows.

mod app;
mod confirm;
mod format;
mod ipc;
mod item;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.ldm.Ldm";

fn main() -> glib::ExitCode {
    let application = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let state = app::App::new(&application);

    let s = state.clone();
    application.connect_startup(move |_| s.start());
    let s = state.clone();
    application.connect_command_line(move |_, command_line| {
        let pending_only = command_line.arguments().iter().skip(1).any(|arg| arg == "--pending");
        if !pending_only {
            s.show_main_window();
        }
        glib::ExitCode::SUCCESS
    });
    application.run()
}
