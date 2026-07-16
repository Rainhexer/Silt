//! Cleanup target registry. Every deletable thing Silt knows about is a
//! `CleanupTarget` produced by one of the provider modules below. Nothing
//! outside this registry is ever deleted.

pub mod containers;
pub mod flatpak;
pub mod package_cache;
pub mod system_logs;
pub mod user_cache;

use std::fmt;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::config::Config;
use crate::distro::SystemProfile;
use crate::scanner::walker::path_size;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskTier {
    /// Regenerable data (package caches, thumbnails). Bulk-selectable.
    Safe,
    /// Probably fine (old logs, orphan packages). Bulk-selectable with flag.
    Moderate,
    /// Could contain real data (Docker volumes). Per-item confirm only.
    Caution,
}

impl fmt::Display for RiskTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskTier::Safe => f.write_str("safe"),
            RiskTier::Moderate => f.write_str("moderate"),
            RiskTier::Caution => f.write_str("caution"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    PackageManager,
    SystemLogs,
    UserCache,
    Containers,
    Trash,
    /// Arbitrary folders the user hand-picked in the Overview's mark-and-
    /// delete flow — not one of the curated registry targets.
    Marked,
    /// Packages uninstalled from the Packages tab.
    Packages,
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Category::PackageManager => "Package manager",
            Category::SystemLogs => "System logs",
            Category::UserCache => "User cache",
            Category::Containers => "Containers",
            Category::Trash => "Trash",
            Category::Marked => "Marked folders",
            Category::Packages => "Packages",
        };
        f.write_str(s)
    }
}

/// What executing a target actually does.
#[derive(Debug, Clone)]
pub enum CleanupAction {
    /// Recursively delete the contents of these paths (the paths themselves
    /// are kept if they are directories, so `~/.cache/foo` stays as an empty
    /// dir rather than vanishing).
    DeletePathContents(Vec<PathBuf>),
    /// Delete these exact paths (files or whole directories).
    DeletePaths(Vec<PathBuf>),
    /// Run an external command (package manager, journalctl, docker...).
    /// `needs_root` triggers a `sudo` prefix when not running as root.
    RunCommand {
        cmd: String,
        args: Vec<String>,
        needs_root: bool,
    },
}

#[derive(Debug, Clone)]
pub struct CleanupTarget {
    pub id: String,
    pub label: String,
    pub category: Category,
    pub risk: RiskTier,
    /// Paths involved, for display and sizing. May be empty for pure-command
    /// targets whose size comes from `size_bytes` directly.
    pub paths: Vec<PathBuf>,
    /// Reclaimable size. None = not yet sized or unknowable.
    pub size_bytes: Option<u64>,
    pub action: CleanupAction,
    /// One-line human description of what will happen.
    pub description: String,
}

impl CleanupTarget {
    /// Compute size from paths if not already set by the provider.
    pub fn ensure_sized(&mut self) {
        if self.size_bytes.is_some() {
            return;
        }
        if self.paths.is_empty() {
            return;
        }
        let total: u64 = self.paths.iter().map(|p| path_size(p)).sum();
        self.size_bytes = Some(total);
    }

    /// Human-readable dry-run preview lines.
    pub fn dry_run_preview(&self) -> Vec<String> {
        let mut lines = vec![self.description.clone()];
        match &self.action {
            CleanupAction::DeletePathContents(paths) => {
                for p in paths {
                    lines.push(format!("  rm -r {}/* (keep dir)", p.display()));
                }
            }
            CleanupAction::DeletePaths(paths) => {
                for p in paths {
                    lines.push(format!("  rm -r {}", p.display()));
                }
            }
            CleanupAction::RunCommand { cmd, args, needs_root } => {
                let prefix = if *needs_root && !is_root() { "sudo " } else { "" };
                lines.push(format!("  {prefix}{cmd} {}", args.join(" ")));
            }
        }
        lines
    }

    /// True when executing this target would invoke sudo.
    pub fn needs_sudo(&self) -> bool {
        matches!(
            &self.action,
            CleanupAction::RunCommand { needs_root: true, .. }
        ) && !is_root()
    }

    /// Execute for real. Returns human-readable log lines.
    ///
    /// `interactive_sudo` controls how root commands escalate: `true` lets
    /// sudo prompt on the terminal (headless mode); `false` uses `sudo -n`,
    /// which fails fast instead of blocking on a prompt the user can't see
    /// (TUI mode — credentials must already be cached via `sudo -v`).
    pub fn execute(&self, interactive_sudo: bool) -> Result<Vec<String>> {
        let mut log = Vec::new();
        match &self.action {
            CleanupAction::DeletePathContents(paths) => {
                for path in paths {
                    delete_contents(path, &mut log)?;
                }
            }
            CleanupAction::DeletePaths(paths) => {
                for path in paths {
                    if !path.exists() {
                        continue;
                    }
                    if path.is_dir() {
                        std::fs::remove_dir_all(path)
                            .with_context(|| format!("removing {}", path.display()))?;
                    } else {
                        std::fs::remove_file(path)
                            .with_context(|| format!("removing {}", path.display()))?;
                    }
                    log.push(format!("removed {}", path.display()));
                }
            }
            CleanupAction::RunCommand { cmd, args, needs_root } => {
                let (program, full_args) = if *needs_root && !is_root() {
                    let mut a = if interactive_sudo {
                        vec![cmd.clone()]
                    } else {
                        vec!["-n".to_string(), cmd.clone()]
                    };
                    a.extend(args.iter().cloned());
                    ("sudo".to_string(), a)
                } else {
                    (cmd.clone(), args.clone())
                };
                log.push(format!("$ {program} {}", full_args.join(" ")));
                let output = std::process::Command::new(&program)
                    .args(&full_args)
                    .output()
                    .with_context(|| format!("running {program}"))?;
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    log.push(line.to_string());
                }
                for line in String::from_utf8_lossy(&output.stderr).lines() {
                    log.push(format!("! {line}"));
                }
                if !output.status.success() {
                    if program == "sudo" && !interactive_sudo {
                        bail!(
                            "sudo refused to run without a prompt (credentials \
                             expired?) — confirm the cleanup again to re-authenticate"
                        );
                    }
                    bail!("{program} exited with {}", output.status);
                }
            }
        }
        Ok(log)
    }
}

fn delete_contents(dir: &PathBuf, log: &mut Vec<String>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?;
    let mut removed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let result = if path.is_dir() && !entry.file_type().map(|t| t.is_symlink()).unwrap_or(false)
        {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => removed += 1,
            Err(e) => log.push(format!("! skipped {}: {e}", path.display())),
        }
    }
    log.push(format!("emptied {} ({removed} entries)", dir.display()));
    Ok(())
}

pub fn is_root() -> bool {
    // Avoid a libc dependency for one call.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).map(|u| u == "0"))
        })
        .unwrap_or(false)
}

/// Build the full target registry for this system. Sizing is done lazily by
/// the caller (it can be slow) — providers set `size_bytes` only when cheap.
pub fn build_registry(profile: &SystemProfile, config: &Config) -> Vec<CleanupTarget> {
    let mut targets = Vec::new();
    targets.extend(package_cache::detect(profile));
    targets.extend(system_logs::detect(profile, config));
    targets.extend(user_cache::detect());
    targets.extend(containers::detect(profile));
    targets.extend(flatpak::detect(profile));
    targets
}
