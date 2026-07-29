//! Package manager cache + orphaned packages.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::distro::{run_capture, PackageManager, SystemProfile};

use super::{Category, CleanupAction, CleanupTarget, RiskTier};

pub fn detect(profile: &SystemProfile) -> Vec<CleanupTarget> {
    let mut targets = Vec::new();

    match profile.package_manager {
        PackageManager::Pacman => {
            let cache = PathBuf::from("/var/cache/pacman/pkg");
            let removable = pacman_removable_cache(&cache);
            // An empty removable list means nothing to reclaim, which is a
            // real answer — not the "unknown size" that empty `paths` means
            // for pure-command targets.
            let known_empty = removable.as_ref().is_some_and(|f| f.is_empty());
            targets.push(CleanupTarget {
                id: "pacman-cache".into(),
                label: "Pacman package cache".into(),
                category: Category::PackageManager,
                risk: RiskTier::Safe,
                // Only the files `-Sc` will really delete. Sizing the whole
                // directory counted the cached copies of *installed* packages
                // too, which `-Sc` keeps — that inflated the estimate by
                // gigabytes and made the finished cleanup look like it had
                // under-delivered.
                paths: removable.unwrap_or_else(|| vec![cache]),
                size_bytes: known_empty.then_some(0),
                action: CleanupAction::RunCommand {
                    // `pacman -Sc` leaves partial-download temp files
                    // (`download-*`, `*.part`) behind and errors on them
                    // ("could not open file ... Error reading fd 7"); sweep
                    // them explicitly so the cache is actually emptied.
                    // The sweep runs after pacman but must not decide the exit
                    // status — otherwise a genuine pacman failure is reported
                    // as success, and a stray rm error as a pacman failure.
                    cmd: "sh".into(),
                    args: vec![
                        "-c".into(),
                        "rc=0; pacman -Sc --noconfirm || rc=$?; \
                         rm -f /var/cache/pacman/pkg/download-* \
                         /var/cache/pacman/pkg/*.part 2>/dev/null; \
                         exit $rc"
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

/// The cache files `pacman -Sc` will actually remove: every package archive
/// whose exact name-version isn't currently installed, plus partial downloads.
/// None when the installed set can't be read, so the caller falls back to the
/// whole directory.
fn pacman_removable_cache(dir: &Path) -> Option<Vec<PathBuf>> {
    let query = run_capture("pacman", &["-Q"])?;
    let installed: HashSet<String> = query
        .lines()
        .filter_map(|l| {
            let (name, version) = l.trim().split_once(' ')?;
            Some(format!("{name}-{version}"))
        })
        .collect();
    if installed.is_empty() {
        return None;
    }

    let mut removable = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("download-") || name.ends_with(".part") {
            removable.push(entry.path());
            continue;
        }
        // Anything that isn't a package archive (db locks, stray files) is
        // left out — pacman won't touch it either.
        if let Some(name_version) = package_name_version(&name) {
            if !installed.contains(&name_version) {
                removable.push(entry.path());
            }
        }
    }
    Some(removable)
}

/// `foo-bar-1.2.3-1-x86_64.pkg.tar.zst` → `foo-bar-1.2.3-1`, matching the
/// `name version` pairs `pacman -Q` prints. Signature files resolve to the
/// package they sign. None when the name isn't a package archive.
fn package_name_version(file: &str) -> Option<String> {
    let base = file.strip_suffix(".sig").unwrap_or(file);
    let stem = base.split(".pkg.tar").next()?;
    if stem == base {
        return None;
    }
    // The last dash-separated component is the architecture.
    let (name_version, _arch) = stem.rsplit_once('-')?;
    Some(name_version.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_file_names_resolve_to_pacman_q_pairs() {
        assert_eq!(
            package_name_version("firefox-141.0-1-x86_64.pkg.tar.zst").as_deref(),
            Some("firefox-141.0-1")
        );
        // Names containing dashes, and an epoch in the version.
        assert_eq!(
            package_name_version("lib32-gcc-libs-15.1.1-1-x86_64.pkg.tar.zst").as_deref(),
            Some("lib32-gcc-libs-15.1.1-1")
        );
        assert_eq!(
            package_name_version("ffmpeg-2:7.1.1-3-x86_64.pkg.tar.zst").as_deref(),
            Some("ffmpeg-2:7.1.1-3")
        );
        // A signature tracks the package it signs.
        assert_eq!(
            package_name_version("firefox-141.0-1-x86_64.pkg.tar.zst.sig").as_deref(),
            Some("firefox-141.0-1")
        );
        // Older compression, and any-arch packages.
        assert_eq!(
            package_name_version("hicolor-icon-theme-0.18-1-any.pkg.tar.xz").as_deref(),
            Some("hicolor-icon-theme-0.18-1")
        );
        // Not package archives — pacman won't remove these, nor do we count them.
        assert_eq!(package_name_version("db.lck"), None);
        assert_eq!(package_name_version("download-12345"), None);
    }
}
