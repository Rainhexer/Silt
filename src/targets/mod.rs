//! Cleanup target registry. Every deletable thing Silt knows about is a
//! `CleanupTarget` produced by one of the provider modules below. Nothing
//! outside this registry is ever deleted.

pub mod containers;
pub mod flatpak;
pub mod package_cache;
pub mod system_logs;
pub mod user_cache;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

    /// Human-readable preview of exactly what executing this target will do.
    pub fn plan_preview(&self) -> Vec<String> {
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

    /// True when executing this target would invoke sudo — either because it
    /// runs a root command, or because some path it deletes isn't ours. The
    /// latter matters: a single root-owned `~/.cache` subdirectory used to
    /// fail with EACCES mid-run, with no chance to authenticate.
    pub fn needs_sudo(&self) -> bool {
        if is_root() {
            return false;
        }
        match &self.action {
            CleanupAction::RunCommand { needs_root, .. } => *needs_root,
            CleanupAction::DeletePaths(paths) | CleanupAction::DeletePathContents(paths) => {
                paths.iter().any(|p| path_needs_root(p))
            }
        }
    }

    /// Execute for real.
    ///
    /// Every path is attempted: one failure never aborts the rest of the
    /// target, and the log is returned whether or not anything failed, so the
    /// underlying error is always visible in the Log tab.
    ///
    /// `interactive_sudo` controls how root escalation works: `true` lets sudo
    /// prompt on the terminal (headless mode, or systems that won't cache
    /// credentials); `false` uses `sudo -n`, which fails fast instead of
    /// blocking on a prompt the user can't see behind the TUI.
    pub fn execute(&self, interactive_sudo: bool) -> ExecOutcome {
        let mut out = ExecOutcome::default();
        match &self.action {
            CleanupAction::DeletePathContents(paths) => {
                for path in paths {
                    delete_contents(path, interactive_sudo, &mut out);
                }
            }
            CleanupAction::DeletePaths(paths) => {
                for path in paths {
                    if path.symlink_metadata().is_err() {
                        continue;
                    }
                    match remove_path(path, interactive_sudo) {
                        Removal::Removed => out.log.push(format!("removed {}", path.display())),
                        Removal::RemovedAsRoot => {
                            out.log.push(format!("removed {} (as root)", path.display()))
                        }
                        Removal::Failed(e) => {
                            out.errors.push(format!("{}: {e:#}", path.display()));
                            out.log.push(format!("! failed {}: {e:#}", path.display()));
                        }
                    }
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
                out.log.push(format!("$ {program} {}", full_args.join(" ")));
                match std::process::Command::new(&program).args(&full_args).output() {
                    Ok(output) => {
                        for line in String::from_utf8_lossy(&output.stdout).lines() {
                            out.log.push(line.to_string());
                        }
                        for line in String::from_utf8_lossy(&output.stderr).lines() {
                            out.log.push(format!("! {line}"));
                        }
                        if !output.status.success() {
                            out.errors.push(describe_command_failure(
                                &program,
                                &output,
                                interactive_sudo,
                            ));
                        }
                    }
                    Err(e) => {
                        out.errors.push(format!("running {program}: {e}"));
                        out.log.push(format!("! running {program}: {e}"));
                    }
                }
            }
        }
        out
    }
}

/// Result of executing one target: the full log (always), plus whatever went
/// wrong (empty when the target fully succeeded).
#[derive(Debug, Default)]
pub struct ExecOutcome {
    pub log: Vec<String>,
    pub errors: Vec<String>,
}

impl ExecOutcome {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Why a command that exited nonzero failed. Distinguishes sudo refusing to
/// authenticate from the command itself failing — conflating the two used to
/// report a working sudo as "credentials expired".
fn describe_command_failure(
    program: &str,
    output: &std::process::Output,
    interactive_sudo: bool,
) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if program == "sudo" && is_sudo_auth_failure(&stderr) {
        return if interactive_sudo {
            "sudo authentication failed".to_string()
        } else {
            "sudo credentials expired — confirm the cleanup again to re-authenticate".to_string()
        };
    }
    let detail: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    match detail.last() {
        Some(last) => format!("{program} exited with {}: {}", output.status, last.trim()),
        None => format!("{program} exited with {}", output.status),
    }
}

/// True when sudo's own stderr says it couldn't authenticate, as opposed to
/// the command under it failing.
fn is_sudo_auth_failure(stderr: &str) -> bool {
    const MARKERS: &[&str] = &[
        "a password is required",
        "a terminal is required",
        "no askpass program",
        "incorrect password",
        "you must have a tty",
        "is not in the sudoers file",
        "no tty present",
    ];
    MARKERS.iter().any(|m| stderr.contains(m))
}

enum Removal {
    Removed,
    RemovedAsRoot,
    Failed(anyhow::Error),
}

/// Remove a file or directory, escalating to `sudo rm -rf` when the direct
/// removal fails and the path is still there. Root-owned files inside an
/// otherwise-ours cache directory are the common case: `remove_dir_all` fails
/// partway with EACCES or "Directory not empty", and only root can finish.
fn remove_path(path: &Path, interactive_sudo: bool) -> Removal {
    let is_dir = path
        .symlink_metadata()
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false);
    let first = if is_dir {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match first {
        Ok(()) => Removal::Removed,
        Err(e) => {
            // Vanished under us (another process, or a partial remove that
            // finished the job) — nothing left to do.
            if path.symlink_metadata().is_err() {
                return Removal::Removed;
            }
            if is_root() {
                return Removal::Failed(anyhow::anyhow!("{e}"));
            }
            match sudo_rm(path, interactive_sudo) {
                Ok(()) => Removal::RemovedAsRoot,
                Err(se) => Removal::Failed(anyhow::anyhow!("{e}; sudo fallback: {se:#}")),
            }
        }
    }
}

/// Remove one hand-picked path (the Overview's mark-and-delete flow),
/// escalating to sudo when the direct removal can't finish. `try_direct` is
/// false when the path is already known to belong to another user, so the
/// doomed filesystem call is skipped.
pub fn remove_marked(path: &Path, try_direct: bool, interactive_sudo: bool) -> Result<()> {
    if path.symlink_metadata().is_err() {
        return Ok(());
    }
    if !try_direct {
        return sudo_rm(path, interactive_sudo);
    }
    match remove_path(path, interactive_sudo) {
        Removal::Removed | Removal::RemovedAsRoot => Ok(()),
        Removal::Failed(e) => Err(e),
    }
}

fn sudo_rm(path: &Path, interactive_sudo: bool) -> Result<()> {
    guard_recursive_delete(path)?;
    let mut cmd = std::process::Command::new("sudo");
    if !interactive_sudo {
        cmd.arg("-n");
    }
    let output = cmd
        .args(["rm", "-rf", "--"])
        .arg(path)
        .output()
        .with_context(|| format!("running sudo rm on {}", path.display()))?;
    if output.status.success() {
        return Ok(());
    }
    bail!("{}", describe_command_failure("sudo", &output, interactive_sudo));
}

/// Refuse to hand `rm -rf` a path that would take out the system or the whole
/// home directory, whatever a provider or a stale registry entry claims.
fn guard_recursive_delete(path: &Path) -> Result<()> {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if resolved.components().count() <= 1 {
        bail!("refusing to recursively delete {}", resolved.display());
    }
    if let Some(home) = dirs::home_dir() {
        if resolved == home {
            bail!("refusing to recursively delete the home directory");
        }
    }
    const FORBIDDEN: &[&str] = &[
        "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/opt", "/proc", "/root",
        "/run", "/sbin", "/srv", "/sys", "/usr", "/var",
    ];
    if FORBIDDEN.iter().any(|f| resolved == Path::new(f)) {
        bail!("refusing to recursively delete {}", resolved.display());
    }
    Ok(())
}

/// Empty a directory without removing it. Failures are recorded per entry so
/// one unreadable subdirectory can't strand the other forty-five.
fn delete_contents(dir: &Path, interactive_sudo: bool, out: &mut ExecOutcome) {
    if !dir.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            // Not even listable as this user — root owns it. Sudo can still
            // empty it in one shot.
            if is_root() {
                out.errors.push(format!("reading {}: {e}", dir.display()));
                out.log.push(format!("! reading {}: {e}", dir.display()));
                return;
            }
            match sudo_empty_dir(dir, interactive_sudo) {
                Ok(()) => out.log.push(format!("emptied {} (as root)", dir.display())),
                Err(se) => {
                    out.errors
                        .push(format!("reading {}: {e}; sudo fallback: {se:#}", dir.display()));
                    out.log.push(format!(
                        "! reading {}: {e}; sudo fallback: {se:#}",
                        dir.display()
                    ));
                }
            }
            return;
        }
    };
    let mut removed = 0u64;
    let mut as_root = 0u64;
    let mut failed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        match remove_path(&path, interactive_sudo) {
            Removal::Removed => removed += 1,
            Removal::RemovedAsRoot => {
                removed += 1;
                as_root += 1;
            }
            Removal::Failed(e) => {
                failed += 1;
                out.errors.push(format!("{}: {e:#}", path.display()));
                out.log.push(format!("! skipped {}: {e:#}", path.display()));
            }
        }
    }
    let mut line = format!("emptied {} ({removed} entries", dir.display());
    if as_root > 0 {
        line.push_str(&format!(", {as_root} as root"));
    }
    if failed > 0 {
        line.push_str(&format!(", {failed} failed"));
    }
    line.push(')');
    out.log.push(line);
}

/// `sudo find <dir> -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +` — empties a
/// directory the current user can't even read, keeping the directory itself.
fn sudo_empty_dir(dir: &Path, interactive_sudo: bool) -> Result<()> {
    guard_recursive_delete(dir)?;
    let mut cmd = std::process::Command::new("sudo");
    if !interactive_sudo {
        cmd.arg("-n");
    }
    let output = cmd
        .arg("find")
        .arg(dir)
        .args(["-mindepth", "1", "-maxdepth", "1", "-exec", "rm", "-rf", "--", "{}", "+"])
        .output()
        .with_context(|| format!("running sudo find on {}", dir.display()))?;
    if output.status.success() {
        return Ok(());
    }
    bail!("{}", describe_command_failure("sudo", &output, interactive_sudo));
}

/// Real UID of the current process, read from `/proc/self/status`.
fn own_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|u| u.parse().ok())
        })
        .unwrap_or(0)
}

/// True when a path is owned by another user, so removing it needs root.
/// (Callers already guard on `is_root()`; a missing path needs no sudo.)
pub fn path_needs_root(p: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(p) {
        Ok(m) => m.uid() != own_uid(),
        Err(_) => false,
    }
}

/// Refreshes the sudo timestamp in the background so a long cleanup can't
/// outlive the credentials it was authorized with. Stops on drop.
pub struct SudoKeepalive {
    stop: Arc<AtomicBool>,
}

impl SudoKeepalive {
    /// Starts the refresher. No-op (returns None) when already root.
    pub fn start() -> Option<Self> {
        if is_root() {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("silt-sudo-keepalive".into())
            .spawn(move || {
                // sudo's default timestamp_timeout is 15 minutes; refreshing
                // every 60s keeps it alive without hammering it.
                while !flag.load(Ordering::Relaxed) {
                    for _ in 0..60 {
                        if flag.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    let _ = std::process::Command::new("sudo").args(["-n", "-v"]).output();
                }
            })
            .ok()?;
        Some(Self { stop })
    }
}

impl Drop for SudoKeepalive {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("silt-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn target(action: CleanupAction) -> CleanupTarget {
        CleanupTarget {
            id: "t".into(),
            label: "t".into(),
            category: Category::UserCache,
            risk: RiskTier::Safe,
            paths: Vec::new(),
            size_bytes: None,
            action,
            description: "t".into(),
        }
    }

    #[test]
    fn delete_paths_removes_every_path() {
        let root = scratch("delete-paths");
        let dirs: Vec<PathBuf> = (0..5)
            .map(|i| {
                let d = root.join(format!("d{i}"));
                std::fs::create_dir_all(d.join("nested")).unwrap();
                std::fs::write(d.join("nested/f"), b"xxxx").unwrap();
                d
            })
            .collect();

        let out = target(CleanupAction::DeletePaths(dirs.clone())).execute(false);

        assert!(out.ok(), "unexpected errors: {:?}", out.errors);
        for d in &dirs {
            assert!(!d.exists(), "{} survived", d.display());
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The regression that made a 40 GiB selection reclaim 3.5 GiB: one
    /// undeletable path used to abort the whole target with `?`, stranding
    /// every path after it.
    #[test]
    fn one_failure_does_not_strand_the_rest() {
        let root = scratch("keep-going");
        // A directory we can't remove from: no write permission on the parent
        // means its child can't be unlinked.
        let locked = root.join("locked");
        std::fs::create_dir_all(locked.join("child")).unwrap();
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
        std::fs::set_permissions(&locked, perms).unwrap();

        let after: Vec<PathBuf> = (0..3)
            .map(|i| {
                let d = root.join(format!("after{i}"));
                std::fs::create_dir_all(&d).unwrap();
                d
            })
            .collect();

        let mut paths = vec![locked.join("child")];
        paths.extend(after.iter().cloned());
        target(CleanupAction::DeletePaths(paths)).execute(false);

        for d in &after {
            assert!(!d.exists(), "{} was stranded by an earlier failure", d.display());
        }

        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
        std::fs::set_permissions(&locked, perms).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn delete_path_contents_keeps_the_directory() {
        let root = scratch("contents");
        let cache = root.join("cache");
        std::fs::create_dir_all(cache.join("sub")).unwrap();
        std::fs::write(cache.join("file"), b"data").unwrap();
        std::fs::write(cache.join(".hidden"), b"data").unwrap();

        let out = target(CleanupAction::DeletePathContents(vec![cache.clone()])).execute(false);

        assert!(out.ok(), "unexpected errors: {:?}", out.errors);
        assert!(cache.is_dir(), "the directory itself must survive");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0, "dotfiles too");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A failing command must still surface its command line and stderr —
    /// they used to be dropped along with the error.
    #[test]
    fn failed_command_still_returns_its_log() {
        let out = target(CleanupAction::RunCommand {
            cmd: "sh".into(),
            args: vec!["-c".into(), "echo boom >&2; exit 3".into()],
            needs_root: false,
        })
        .execute(false);

        assert!(!out.ok());
        assert!(out.log.iter().any(|l| l.starts_with("$ sh")), "log: {:?}", out.log);
        assert!(out.log.iter().any(|l| l.contains("boom")), "log: {:?}", out.log);
        assert!(out.errors[0].contains("exit status: 3"), "{:?}", out.errors);
    }

    #[test]
    fn command_failure_is_not_blamed_on_sudo() {
        let output = std::process::Command::new("sh")
            .args(["-c", "echo 'error: could not lock database' >&2; exit 1"])
            .output()
            .unwrap();
        let msg = describe_command_failure("sudo", &output, false);
        assert!(msg.contains("could not lock database"), "{msg}");
        assert!(!msg.contains("credentials expired"), "{msg}");
    }

    #[test]
    fn sudo_auth_failure_is_reported_as_such() {
        let output = std::process::Command::new("sh")
            .args(["-c", "echo 'sudo: a password is required' >&2; exit 1"])
            .output()
            .unwrap();
        assert!(describe_command_failure("sudo", &output, false).contains("credentials expired"));
    }

    #[test]
    fn refuses_to_recursively_delete_critical_paths() {
        for p in ["/", "/usr", "/home", "/var", "/etc"] {
            assert!(
                guard_recursive_delete(Path::new(p)).is_err(),
                "{p} must be refused"
            );
        }
        if let Some(home) = dirs::home_dir() {
            assert!(guard_recursive_delete(&home).is_err());
            assert!(guard_recursive_delete(&home.join(".cache/foo")).is_ok());
        }
    }
}
