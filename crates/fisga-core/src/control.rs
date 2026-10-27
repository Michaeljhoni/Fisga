//! The `*.fisga-part.json` control file that makes segmented downloads resumable.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One byte range of the file. `end` is inclusive; `done` counts bytes already
/// written starting at `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentState {
    pub start: u64,
    pub end: u64,
    pub done: u64,
}

impl SegmentState {
    pub fn len(&self) -> u64 {
        self.end + 1 - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn remaining(&self) -> u64 {
        self.len().saturating_sub(self.done)
    }

    /// Absolute offset of the next byte to download.
    pub fn pos(&self) -> u64 {
        self.start + self.done
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlFile {
    pub version: u32,
    pub url: String,
    pub total: u64,
    #[serde(default)]
    pub etag: Option<String>,
    pub segments: Vec<SegmentState>,
}

pub const VERSION: u32 = 1;

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(suffix);
    PathBuf::from(os)
}

/// `file.iso` → `file.iso.fisga-part` (data being downloaded).
pub fn part_path(final_path: &Path) -> PathBuf {
    with_suffix(final_path, ".fisga-part")
}

/// `file.iso` → `file.iso.fisga-part.json` (segment progress).
pub fn control_path(final_path: &Path) -> PathBuf {
    with_suffix(final_path, ".fisga-part.json")
}

/// A download paused before the rename left `file.iso.ldm-part` (and its
/// `.json`): they take the new names, so it resumes where it stopped.
pub fn adopt_legacy(final_path: &Path) {
    for (old, new) in [
        (with_suffix(final_path, ".ldm-part"), part_path(final_path)),
        (with_suffix(final_path, ".ldm-part.json"), control_path(final_path)),
    ] {
        if old.exists() && !new.exists() {
            let _ = std::fs::rename(old, new);
        }
    }
}

pub fn load(path: &Path) -> Option<ControlFile> {
    let data = std::fs::read(path).ok()?;
    let control: ControlFile = serde_json::from_slice(&data).ok()?;
    (control.version == VERSION).then_some(control)
}

/// Atomic and durable write (temp file synced, renamed, folder synced), so a
/// crash or a power cut never leaves a torn or vanished control file.
pub fn save(path: &Path, control: &ControlFile) -> io::Result<()> {
    use std::io::Write;
    let tmp = with_suffix(path, ".tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec(control)?)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)?;
    // The rename itself is only durable once the folder is (Linux).
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        let _ = std::fs::File::open(dir).and_then(|dir| dir.sync_all());
    }
    Ok(())
}
