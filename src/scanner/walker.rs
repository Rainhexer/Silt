//! Recursive size aggregation using walkdir.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use walkdir::WalkDir;

use super::ScanEvent;

/// Report progress every N entries so the channel isn't flooded.
const PROGRESS_INTERVAL: u64 = 2048;

/// Scan the direct children of `root`, sizing each one (recursively for
/// directories), and emit events on `tx`. Honors `cancel`.
pub fn scan_root(
    root: &Path,
    exclude: &[PathBuf],
    follow_symlinks: bool,
    tx: &Sender<ScanEvent>,
    cancel: &AtomicBool,
) {
    let mut total: u64 = 0;
    let mut visited: u64 = 0;

    let children = match std::fs::read_dir(root) {
        Ok(rd) => rd,
        Err(e) => {
            let _ = tx.send(ScanEvent::Warning(format!("{}: {e}", root.display())));
            let _ = tx.send(ScanEvent::Done { total_size: 0 });
            return;
        }
    };

    for child in children.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let path = child.path();
        if is_excluded(&path, exclude) {
            continue;
        }

        let file_type = match child.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_symlink() && !follow_symlinks {
            // Count the link itself, don't follow.
            let size = child.metadata().map(|m| m.len()).unwrap_or(0);
            total += size;
            visited += 1;
            let _ = tx.send(ScanEvent::DirScanned { path, size, is_dir: false });
            continue;
        }

        if file_type.is_dir() {
            let size = dir_size(&path, exclude, follow_symlinks, tx, cancel, &mut visited);
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            total += size;
            let _ = tx.send(ScanEvent::DirScanned { path, size, is_dir: true });
        } else {
            let size = child.metadata().map(|m| m.len()).unwrap_or(0);
            total += size;
            visited += 1;
            let _ = tx.send(ScanEvent::DirScanned { path, size, is_dir: false });
        }
    }

    let _ = tx.send(ScanEvent::Done { total_size: total });
}

fn is_excluded(path: &Path, exclude: &[PathBuf]) -> bool {
    exclude.iter().any(|ex| path == ex || path.starts_with(ex))
}

/// Recursive size of a directory. Emits periodic progress and warnings for
/// unreadable subtrees; never aborts on error.
fn dir_size(
    dir: &Path,
    exclude: &[PathBuf],
    follow_symlinks: bool,
    tx: &Sender<ScanEvent>,
    cancel: &AtomicBool,
    visited: &mut u64,
) -> u64 {
    let mut size: u64 = 0;
    let walker = WalkDir::new(dir)
        .follow_links(follow_symlinks)
        .same_file_system(true)
        .into_iter()
        .filter_entry(|e| !is_excluded(e.path(), exclude));

    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            return size;
        }
        match entry {
            Ok(e) => {
                if e.file_type().is_file() {
                    size += e.metadata().map(|m| m.len()).unwrap_or(0);
                }
                *visited += 1;
                if (*visited).is_multiple_of(PROGRESS_INTERVAL) {
                    let _ = tx.send(ScanEvent::Progress { entries_visited: *visited });
                }
            }
            Err(e) => {
                if let Some(path) = e.path() {
                    let _ = tx.send(ScanEvent::Warning(format!("{}: {e}", path.display())));
                }
            }
        }
    }
    size
}

/// One-shot blocking size of a path (file or directory). Used by target
/// sizing where streaming isn't needed.
pub fn path_size(path: &Path) -> u64 {
    if path.is_file() {
        return path.metadata().map(|m| m.len()).unwrap_or(0);
    }
    if !path.is_dir() {
        return 0;
    }
    WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
}
