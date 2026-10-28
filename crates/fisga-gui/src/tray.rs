//! Icon in the system tray. Linux: the StatusNotifierItem D-Bus protocol
//! (`sni`). Windows and macOS: the system's own notification area, through
//! `tray-icon` (`native`). Both send clicks back as `Action`s through a
//! channel, and both are driven with the same functions.

#[cfg(not(any(windows, target_os = "macos")))]
mod sni;
#[cfg(not(any(windows, target_os = "macos")))]
pub use sni::{set_summary, shutdown, spawn, Handle};

#[cfg(any(windows, target_os = "macos"))]
mod native;
#[cfg(any(windows, target_os = "macos"))]
pub use native::{set_summary, shutdown, spawn, Handle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Add,
    ResumeAll,
    PauseAll,
    Quit,
}
