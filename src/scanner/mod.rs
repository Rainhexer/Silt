//! Scan orchestration: spawns the walker thread and streams events to the UI.

pub mod mounts;
pub mod walker;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum ScanEvent {
    /// A direct child of the scan root finished sizing.
    DirScanned { path: PathBuf, size: u64, is_dir: bool },
    /// Coarse progress: number of entries visited so far.
    Progress { entries_visited: u64 },
    /// Scan finished; total size of the root.
    Done { total_size: u64 },
    /// Non-fatal error (permission denied etc.), reported and skipped.
    Warning(String),
}

#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
    /// Entry is a remote (cloud/network) mount point. Its `size` is what the
    /// remote reports as used, not local disk usage, and it is never walked —
    /// walking a FUSE cloud mount can download every file.
    pub remote: bool,
}

/// Handle to a running scan. Dropping does not stop the scan; call `cancel`.
pub struct ScanHandle {
    pub receiver: Receiver<ScanEvent>,
    cancel: Arc<AtomicBool>,
}

impl ScanHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Start scanning `root` on a background thread. Sizes each direct child of
/// `root` (recursively for directories) and streams `ScanEvent`s.
pub fn start_scan(root: PathBuf, exclude: Vec<PathBuf>, follow_symlinks: bool) -> ScanHandle {
    let (tx, rx): (Sender<ScanEvent>, Receiver<ScanEvent>) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_flag = Arc::clone(&cancel);

    std::thread::Builder::new()
        .name("silt-scanner".into())
        .spawn(move || {
            walker::scan_root(&root, &exclude, follow_symlinks, &tx, &cancel_flag);
        })
        .expect("failed to spawn scanner thread");

    ScanHandle { receiver: rx, cancel }
}
