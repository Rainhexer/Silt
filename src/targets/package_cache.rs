//! Package manager cache + orphaned packages.

use std::path::PathBuf;

use crate::distro::{run_capture, PackageManager, SystemProfile};

use super::{Category, CleanupAction, CleanupTarget, RiskTier};

pub fn detect(profile: &SystemProfile) -> Vec<CleanupTarget> {
    let mut targets = Vec::new();

    match profile.package_manager {
        PackageManager::Pacman => {
            targets.push(CleanupTarget {
                id: "pacman-cache".into(),
                label: "Pacman package cache".into(),
                category: Category::PackageManager,
                risk: RiskTier::Safe,
                paths: vec![PathBuf::from("/var/cache/pacman/pkg")],
                size_bytes: None,
                action: CleanupAction::RunCommand {
                    // `pacman -Sc` leaves partial-download temp files
                    // (`download-*`, `*.part`) behind and errors on them
                    // ("could not open file ... Error reading fd 7"); sweep
                    // them explicitly so the cache is actually emptied.
                    cmd: "sh".into(),
                    args: vec![
                        "-c".into(),
                        "pacman -Sc --noconfirm; rm -f \
                         /var/cache/pacman/pkg/download-* \
                         /var/cache/pacman/pkg/*.part"
                            .into(),
                    ],
                    needs_root: true,
                },
                description: "Remove cached packages not installed (pacman -Sc)".into(),
            });
            if let Some(orphans) = pacman_orphans() {
                if !orphans.is_empty() {
                    let mut args = vec!["-Rns".into(), "--noconfirm".into()];
                    args.extend(orphans.iter().cloned());
                    targets.push(CleanupTarget {
                        id: "pacman-orphans".into(),
                        label: format!("Orphaned packages ({})", orphans.len()),
                        category: Category::PackageManager,
                        risk: RiskTier::Moderate,
                        paths: Vec::new(),
                        size_bytes: None,
                        action: CleanupAction::RunCommand {
                            cmd: "pacman".into(),
                            args,
                            needs_root: true,
                        },
                        description: format!(
                            "Remove {} orphaned packages: {}",
                            orphans.len(),
                            orphans.join(", ")
                        ),
                    });
                }
            }
        }
        PackageManager::Apt => {
            targets.push(CleanupTarget {
                id: "apt-cache".into(),
                label: "APT package cache".into(),
                category: Category::PackageManager,
                risk: RiskTier::Safe,
                paths: vec![PathBuf::from("/var/cache/apt/archives")],
                size_bytes: None,
                action: CleanupAction::RunCommand {
                    cmd: "apt-get".into(),
                    args: vec!["clean".into()],
                    needs_root: true,
                },
                description: "Clear downloaded .deb archives (apt-get clean)".into(),
            });
            targets.push(CleanupTarget {
                id: "apt-autoremove".into(),
                label: "Unneeded packages (autoremove)".into(),
                category: Category::PackageManager,
                risk: RiskTier::Moderate,
                paths: Vec::new(),
                size_bytes: None,
                action: CleanupAction::RunCommand {
                    cmd: "apt-get".into(),
                    args: vec!["autoremove".into(), "-y".into()],
                    needs_root: true,
                },
                description: "Remove automatically installed packages no longer needed".into(),
            });
        }
        PackageManager::Dnf => {
            targets.push(CleanupTarget {
                id: "dnf-cache".into(),
                label: "DNF package cache".into(),
                category: Category::PackageManager,
                risk: RiskTier::Safe,
                paths: vec![PathBuf::from("/var/cache/dnf")],
                size_bytes: None,
                action: CleanupAction::RunCommand {
                    cmd: "dnf".into(),
                    args: vec!["clean".into(), "all".into()],
                    needs_root: true,
                },
                description: "Clear DNF caches (dnf clean all)".into(),
            });
            targets.push(CleanupTarget {
                id: "dnf-autoremove".into(),
                label: "Unneeded packages (autoremove)".into(),
                category: Category::PackageManager,
                risk: RiskTier::Moderate,
                paths: Vec::new(),
                size_bytes: None,
                action: CleanupAction::RunCommand {
                    cmd: "dnf".into(),
                    args: vec!["autoremove".into(), "-y".into()],
                    needs_root: true,
                },
                description: "Remove packages installed as dependencies no longer needed".into(),
            });
        }
        PackageManager::Zypper => {
            targets.push(CleanupTarget {
                id: "zypper-cache".into(),
                label: "Zypper package cache".into(),
                category: Category::PackageManager,
                risk: RiskTier::Safe,
                paths: vec![PathBuf::from("/var/cache/zypp/packages")],
                size_bytes: None,
                action: CleanupAction::RunCommand {
                    cmd: "zypper".into(),
                    args: vec!["clean".into(), "--all".into()],
                    needs_root: true,
                },
                description: "Clear zypper caches (zypper clean --all)".into(),
            });
        }
        PackageManager::Nix => {
            targets.push(CleanupTarget {
                id: "nix-gc".into(),
                label: "Nix store garbage".into(),
                category: Category::PackageManager,
                risk: RiskTier::Moderate,
                paths: Vec::new(),
                size_bytes: None,
                action: CleanupAction::RunCommand {
                    cmd: "nix-collect-garbage".into(),
                    args: vec!["-d".into()],
                    needs_root: false,
                },
                description: "Garbage-collect unreferenced Nix store paths".into(),
            });
        }
        PackageManager::Unknown => {}
    }

    targets
}

/// `pacman -Qdtq` — orphaned packages, one per line. None if pacman missing
/// or exit code nonzero with no output (pacman exits 1 when no orphans).
fn pacman_orphans() -> Option<Vec<String>> {
    let output = std::process::Command::new("pacman")
        .args(["-Qdtq"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    Some(
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
    )
}

/// Best-effort reclaimable estimate for command-based caches where we can't
/// walk a path (unused for now, kept for --json enrichment).
#[allow(dead_code)]
fn apt_autoremove_estimate() -> Option<u64> {
    let out = run_capture("apt-get", &["autoremove", "--dry-run"])?;
    // Look for "After this operation, X B of disk space will be freed."
    for line in out.lines() {
        if line.contains("disk space will be freed") {
            return parse_apt_size(line);
        }
    }
    None
}

#[allow(dead_code)]
fn parse_apt_size(line: &str) -> Option<u64> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    for window in tokens.windows(2) {
        if let Ok(value) = window[0].replace(',', ".").parse::<f64>() {
            let multiplier = match window[1] {
                "B" => 1.0,
                "kB" => 1e3,
                "MB" => 1e6,
                "GB" => 1e9,
                _ => continue,
            };
            return Some((value * multiplier) as u64);
        }
    }
    None
}
