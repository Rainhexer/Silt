//! journald vacuum, old kernels, core dumps, trash.

use std::path::PathBuf;

use crate::config::Config;
use crate::distro::{binary_exists, run_capture, InitSystem, PackageManager, SystemProfile};

use super::{Category, CleanupAction, CleanupTarget, RiskTier};

pub fn detect(profile: &SystemProfile, config: &Config) -> Vec<CleanupTarget> {
    let mut targets = Vec::new();

    if profile.init_system == InitSystem::Systemd && binary_exists("journalctl") {
        let keep_days = config.cleanup.journalctl_keep_days;
        targets.push(CleanupTarget {
            id: "journal-vacuum".into(),
            label: format!("Journal logs older than {keep_days}d"),
            category: Category::SystemLogs,
            risk: RiskTier::Moderate,
            paths: vec![PathBuf::from("/var/log/journal")],
            size_bytes: journal_disk_usage(),
            action: CleanupAction::RunCommand {
                cmd: "journalctl".into(),
                args: vec![format!("--vacuum-time={keep_days}d")],
                needs_root: true,
            },
            description: format!(
                "Vacuum systemd journal entries older than {keep_days} days"
            ),
        });
    }

    if let Some(target) = old_kernels(profile) {
        targets.push(target);
    }

    // Core dumps.
    for (id, dir) in [
        ("coredumps-systemd", "/var/lib/systemd/coredump"),
        ("coredumps-crash", "/var/crash"),
    ] {
        let path = PathBuf::from(dir);
        if path.is_dir() && dir_has_entries(&path) {
            targets.push(CleanupTarget {
                id: id.into(),
                label: format!("Core dumps ({dir})"),
                category: Category::SystemLogs,
                risk: RiskTier::Moderate,
                paths: vec![path.clone()],
                size_bytes: None,
                action: CleanupAction::DeletePathContents(vec![path]),
                description: format!("Delete crash core dumps in {dir}"),
            });
        }
    }

    // Trash — user-level but grouped here in detection for simplicity;
    // categorized as Trash.
    if let Some(home) = dirs::home_dir() {
        let trash = home.join(".local/share/Trash");
        if trash.is_dir() && dir_has_entries(&trash.join("files")) {
            targets.push(CleanupTarget {
                id: "trash".into(),
                label: "Trash".into(),
                category: Category::Trash,
                risk: RiskTier::Moderate,
                paths: vec![trash.join("files"), trash.join("info")],
                size_bytes: None,
                action: CleanupAction::DeletePathContents(vec![
                    trash.join("files"),
                    trash.join("info"),
                ]),
                description: "Empty trash (~/.local/share/Trash)".into(),
            });
        }
    }

    targets
}

fn dir_has_entries(path: &PathBuf) -> bool {
    std::fs::read_dir(path)
        .map(|mut rd| rd.next().is_some())
        .unwrap_or(false)
}

/// Parse `journalctl --disk-usage`: "Archived and active journals take up 1.2G ..."
fn journal_disk_usage() -> Option<u64> {
    let out = run_capture("journalctl", &["--disk-usage"])?;
    for token in out.split_whitespace() {
        if let Some(bytes) = parse_journal_size(token) {
            return Some(bytes);
        }
    }
    None
}

fn parse_journal_size(token: &str) -> Option<u64> {
    let token = token.trim_end_matches('.');
    let split = token.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = token.split_at(split);
    let value: f64 = num.parse().ok()?;
    let multiplier: f64 = match unit {
        "B" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        "T" => 1024.0_f64.powi(4),
        _ => return None,
    };
    Some((value * multiplier) as u64)
}

/// Old kernels: only implemented for pacman-less distros via package manager
/// autoremove; on Arch, old kernels live in the package cache already. Debian
/// keeps old kernel packages installed — flag them.
fn old_kernels(profile: &SystemProfile) -> Option<CleanupTarget> {
    if profile.package_manager != PackageManager::Apt {
        return None;
    }
    let running = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    let running = running.trim();
    let out = run_capture("dpkg-query", &["-W", "-f=${Package}\n", "linux-image-*"])?;
    let old: Vec<String> = out
        .lines()
        .filter(|pkg| {
            pkg.starts_with("linux-image-")
                && !pkg.contains(running)
                && pkg.chars().any(|c| c.is_ascii_digit())
        })
        .map(String::from)
        .collect();
    if old.is_empty() {
        return None;
    }
    let mut args = vec!["purge".into(), "-y".into()];
    args.extend(old.iter().cloned());
    Some(CleanupTarget {
        id: "old-kernels".into(),
        label: format!("Old kernels ({})", old.len()),
        category: Category::SystemLogs,
        risk: RiskTier::Caution,
        paths: Vec::new(),
        size_bytes: None,
        action: CleanupAction::RunCommand {
            cmd: "apt-get".into(),
            args,
            needs_root: true,
        },
        description: format!(
            "Purge kernel packages not currently running: {}",
            old.join(", ")
        ),
    })
}
