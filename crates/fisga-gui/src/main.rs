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

// A window program on Windows: no console window behind it.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

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
/// warnings and our errors in `~/.local/state/fisga/fisga-gui.log` instead
/// (`%LOCALAPPDATA%\fisga\log\fisga-gui.log` on Windows).
fn redirect_stderr_to_log() {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        return;
    }
    #[cfg(unix)]
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/state")))
        .map(|state| state.join("fisga"));
    #[cfg(windows)]
    let dir = std::env::var_os("LOCALAPPDATA").map(|dir| std::path::PathBuf::from(dir).join("fisga").join("log"));
    let Some(dir) = dir else { return };
    let path = dir.join("fisga-gui.log");
    let file = std::fs::create_dir_all(&dir).and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(&path));
    let Ok(file) = file else { return };
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: dup2 on valid descriptors; the file stays open for the process lifetime.
        unsafe { libc::dup2(file.as_raw_fd(), 2) };
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[link(name = "kernel32")]
        extern "system" {
            fn SetStdHandle(std_handle: u32, handle: *mut std::ffi::c_void) -> i32;
        }
        const STD_ERROR_HANDLE: u32 = -12i32 as u32;
        // Rust's stderr (our messages) asks Windows for the handle each time;
        // GTK's goes through the C runtime's descriptor 2.
        // SAFETY: a valid handle, open for the process lifetime; a valid path.
        unsafe {
            SetStdHandle(STD_ERROR_HANDLE, file.as_raw_handle());
            if let Ok(c_path) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) {
                let fd = libc::open(c_path.as_ptr(), libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT, 0o644);
                if fd >= 0 {
                    libc::dup2(fd, 2);
                }
            }
        }
    }
    std::mem::forget(file);
    eprintln!("--- fisga-gui {} iniciado: {:?}", env!("CARGO_PKG_VERSION"), std::env::args().collect::<Vec<_>>());
}

/// Inside `Fisga.app` (macOS), GTK's data (icons, settings schemas, image
/// loaders) is in the bundle's `Resources`, which GTK does not look for by
/// itself; the daemon starts this binary directly, so it is set here.
///
/// gdk-pixbuf wants the loaders' absolute paths, which depend on where the app
/// was put: the bundle has `loaders.cache.in` with `@RESOURCES@` in their place.
#[cfg(target_os = "macos")]
fn use_bundle_resources() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(resources) = exe.parent().and_then(|macos| macos.parent()).map(|contents| contents.join("Resources")) else {
        return;
    };
    let share = resources.join("share");
    if !share.is_dir() {
        return;
    }
    std::env::set_var("XDG_DATA_DIRS", &share);
    std::env::set_var("GSETTINGS_SCHEMA_DIR", share.join("glib-2.0").join("schemas"));
    let template = resources.join("lib/gdk-pixbuf-2.0/2.10.0/loaders.cache.in");
    let cache = std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join("Library/Caches/Fisga"));
    if let (Ok(template), Some(cache)) = (std::fs::read_to_string(template), cache) {
        let file = cache.join("loaders.cache");
        let written = std::fs::create_dir_all(&cache)
            .and_then(|_| std::fs::write(&file, template.replace("@RESOURCES@", &resources.to_string_lossy())));
        if written.is_ok() {
            std::env::set_var("GDK_PIXBUF_MODULE_FILE", file);
        }
    }
    // A .dmg runs nothing when the app is installed: the browsers learn where
    // the native host is here (each start, as the app may have moved).
    let cli = fisga_ipc::sibling_executable("fisga");
    std::thread::spawn(move || {
        let _ = std::process::Command::new(cli)
            .arg("install-hosts")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

fn main() -> glib::ExitCode {
    #[cfg(target_os = "macos")]
    use_bundle_resources();
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
