//! Installed package inventory: system package manager, Flatpak, and Snap.
//! Listing is read-only; uninstalls run through the same confirm + sudo-gate
//! flow as every other deletion in Silt.

use std::fmt;
use std::path::PathBuf;
use std::process::Command;

use crate::distro::{PackageManager, SystemProfile};

/// Where a package came from, which decides how it gets removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkgSource {
    System(PackageManager),
    Flatpak,
    Snap,
}

impl fmt::Display for PkgSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PkgSource::System(pm) => write!(f, "{pm}"),
            PkgSource::Flatpak => f.write_str("flatpak"),
            PkgSource::Snap => f.write_str("snap"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Package {
    /// Unique key across sources: `"<source>:<uninstall_ref>"`.
    pub id: String,
    /// Human display name.
    pub name: String,
    /// What the uninstall command receives (app id for Flatpak, name otherwise).
    pub uninstall_ref: String,
    pub version: String,
    pub size: u64,
    pub description: String,
    pub source: PkgSource,
    /// Core system package — Silt refuses to mark these for removal.
    pub essential: bool,
}

impl Package {
    /// Uninstall command as (program, args, needs_root). None = source Silt
    /// can list but not remove (nix, unknown).
    pub fn uninstall_command(&self) -> Option<(String, Vec<String>, bool)> {
        let r = self.uninstall_ref.clone();
        let (cmd, args, root): (&str, Vec<String>, bool) = match self.source {
            PkgSource::System(pm) => match pm {
                PackageManager::Pacman => {
                    ("pacman", vec!["-Rns".into(), "--noconfirm".into(), r], true)
                }
                PackageManager::Apt => ("apt-get", vec!["purge".into(), "-y".into(), r], true),
                PackageManager::Dnf => ("dnf", vec!["remove".into(), "-y".into(), r], true),
                PackageManager::Zypper => (
                    "zypper",
                    vec!["--non-interactive".into(), "remove".into(), r],
                    true,
                ),
                PackageManager::Nix | PackageManager::Unknown => return None,
            },
            PkgSource::Flatpak => (
                "flatpak",
                vec![
                    "uninstall".into(),
                    "-y".into(),
                    "--noninteractive".into(),
                    "--delete-data".into(),
                    r,
                ],
                false,
            ),
            PkgSource::Snap => ("snap", vec!["remove".into(), "--purge".into(), r], true),
        };
        Some((cmd.to_string(), args, root))
    }

    /// True when removing this package will invoke sudo.
    pub fn needs_root(&self) -> bool {
        self.uninstall_command().map(|(_, _, r)| r).unwrap_or(false)
    }

    /// Leftover per-user data dirs to purge after a system-package uninstall.
    /// Exact-name matches only; Flatpak (`--delete-data`) and Snap (`--purge`)
    /// clean up after themselves.
    pub fn leftover_dirs(&self) -> Vec<PathBuf> {
        if !matches!(self.source, PkgSource::System(_)) {
            return Vec::new();
        }
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        let n = self.name.to_lowercase();
        [
            home.join(".cache").join(&n),
            home.join(".config").join(&n),
            home.join(".local/share").join(&n),
        ]
        .into_iter()
        .filter(|p| p.exists())
        .collect()
    }
}

/// Sort order for the Packages tab. `s` cycles through these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkgSort {
    Size,
    Name,
    Source,
}

impl PkgSort {
    pub fn next(self) -> PkgSort {
        match self {
            PkgSort::Size => PkgSort::Name,
            PkgSort::Name => PkgSort::Source,
            PkgSort::Source => PkgSort::Size,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PkgSort::Size => "size",
            PkgSort::Name => "name",
            PkgSort::Source => "source",
        }
    }
}

/// Packages Silt refuses to mark for removal: losing one bricks the system.
const ESSENTIAL: &[&str] = &[
    "base", "filesystem", "glibc", "gcc-libs", "systemd", "systemd-libs", "bash", "coreutils",
    "util-linux", "sudo", "pacman", "dpkg", "apt", "rpm", "dnf", "zypper", "grub", "grub2",
    "shim", "snapd", "flatpak", "dbus", "init", "libc6", "login", "passwd", "mount",
];

/// Arch kernel package names (a `linux-` prefix alone over-matches things
/// like linux-wallpaperengine, so match known kernels exactly).
const ARCH_KERNELS: &[&str] = &[
    "linux", "linux-lts", "linux-zen", "linux-hardened", "linux-rt", "linux-rt-lts",
    "linux-cachyos", "linux-cachyos-bore", "linux-cachyos-lts",
];

fn is_essential(name: &str) -> bool {
    ESSENTIAL.contains(&name)
        || ARCH_KERNELS.contains(&name)
        || name.starts_with("linux-image-") // debian kernels
        || name == "kernel"
        || name.starts_with("kernel-core") // rpm kernels
        || name.starts_with("kernel-default") // suse kernels
}

/// Run a command with a C locale (parsers depend on English field names and
/// `.` decimal separators). None on spawn failure or non-zero exit.
fn capture(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd).args(args).env("LC_ALL", "C").output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

/// Parse "12.34 MiB" / "962.6 MB" / "690 B" (binary and decimal units;
/// tolerates the non-breaking space g_format_size emits).
fn parse_human_size(s: &str) -> u64 {
    let s = s.replace('\u{00A0}', " ");
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: f64 = num.replace(',', ".").parse().unwrap_or(0.0);
    let mult: f64 = match unit.trim() {
        "" | "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => 1024.0,
        "MiB" => 1024f64.powi(2),
        "GiB" => 1024f64.powi(3),
        "TiB" => 1024f64.powi(4),
        _ => 1.0,
    };
    (num * mult) as u64
}

fn make(name: String, version: String, size: u64, desc: String, source: PkgSource) -> Package {
    let essential = matches!(source, PkgSource::System(_)) && is_essential(&name);
    Package {
        id: format!("{source}:{name}"),
        uninstall_ref: name.clone(),
        name,
        version,
        size,
        description: desc,
        source,
        essential,
    }
}

/// Enumerate every installed package Silt knows how to list. Returns the
/// packages plus human-readable warnings for sources it had to skip.
pub fn list_installed(profile: &SystemProfile) -> (Vec<Package>, Vec<String>) {
    let mut pkgs = Vec::new();
    let mut warnings = Vec::new();

    match profile.package_manager {
        PackageManager::Pacman => list_pacman(&mut pkgs, &mut warnings),
        PackageManager::Apt => list_dpkg(&mut pkgs, &mut warnings),
        PackageManager::Dnf | PackageManager::Zypper => {
            list_rpm(profile.package_manager, &mut pkgs, &mut warnings)
        }
        PackageManager::Nix => {
            warnings.push("nix packages aren't supported yet — skipped".into())
        }
        PackageManager::Unknown => {
            warnings.push("no supported system package manager detected".into())
        }
    }
    if profile.has_flatpak {
        list_flatpak(&mut pkgs, &mut warnings);
    }
    if profile.has_snap {
        list_snap(&mut pkgs, &mut warnings);
    }

    (pkgs, warnings)
}

fn list_pacman(pkgs: &mut Vec<Package>, warnings: &mut Vec<String>) {
    let Some(out) = capture("pacman", &["-Qi"]) else {
        warnings.push("pacman -Qi failed — system packages skipped".into());
        return;
    };
    let src = PkgSource::System(PackageManager::Pacman);
    let (mut name, mut version, mut desc) = (String::new(), String::new(), String::new());
    let mut size = 0u64;
    // Blocks are blank-line separated; a trailing "" flushes the last one.
    for line in out.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !name.is_empty() {
                pkgs.push(make(
                    std::mem::take(&mut name),
                    std::mem::take(&mut version),
                    size,
                    std::mem::take(&mut desc),
                    src,
                ));
                size = 0;
            }
            continue;
        }
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let val = val.trim();
        match key.trim() {
            "Name" => name = val.into(),
            "Version" => version = val.into(),
            "Installed Size" => size = parse_human_size(val),
            "Description" => desc = val.into(),
            _ => {}
        }
    }
}

fn list_dpkg(pkgs: &mut Vec<Package>, warnings: &mut Vec<String>) {
    let fmt = "${db:Status-Status}\t${Package}\t${Version}\t${Installed-Size}\t${binary:Summary}\n";
    let Some(out) = capture("dpkg-query", &["-W", &format!("-f={fmt}")]) else {
        warnings.push("dpkg-query failed — system packages skipped".into());
        return;
    };
    let src = PkgSource::System(PackageManager::Apt);
    for line in out.lines() {
        let mut f = line.splitn(5, '\t');
        let (Some(status), Some(name), Some(version), Some(kib)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        if status != "installed" {
            continue;
        }
        let size = kib.trim().parse::<u64>().unwrap_or(0) * 1024;
        let desc = f.next().unwrap_or("").to_string();
        pkgs.push(make(name.into(), version.into(), size, desc, src));
    }
}

fn list_rpm(pm: PackageManager, pkgs: &mut Vec<Package>, warnings: &mut Vec<String>) {
    let fmt = "%{NAME}\t%{VERSION}-%{RELEASE}\t%{SIZE}\t%{SUMMARY}\n";
    let Some(out) = capture("rpm", &["-qa", "--queryformat", fmt]) else {
        warnings.push("rpm -qa failed — system packages skipped".into());
        return;
    };
    let src = PkgSource::System(pm);
    for line in out.lines() {
        let mut f = line.splitn(4, '\t');
        let (Some(name), Some(version), Some(bytes)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let size = bytes.trim().parse().unwrap_or(0);
        let desc = f.next().unwrap_or("").to_string();
        pkgs.push(make(name.into(), version.into(), size, desc, src));
    }
}

fn list_flatpak(pkgs: &mut Vec<Package>, warnings: &mut Vec<String>) {
    // Tab-separated when stdout isn't a tty. Includes runtimes: they're often
    // the biggest flatpak disk cost and users should see them.
    let Some(out) = capture("flatpak", &["list", "--columns=application,name,version,size"])
    else {
        warnings.push("flatpak list failed — flatpaks skipped".into());
        return;
    };
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 4 {
            continue;
        }
        let (app_id, name, version, size) = (f[0].trim(), f[1].trim(), f[2].trim(), f[3]);
        if app_id.is_empty() {
            continue;
        }
        let display = if name.is_empty() { app_id } else { name };
        pkgs.push(Package {
            id: format!("flatpak:{app_id}"),
            name: display.to_string(),
            uninstall_ref: app_id.to_string(),
            version: version.to_string(),
            size: parse_human_size(size),
            description: app_id.to_string(),
            source: PkgSource::Flatpak,
            essential: false,
        });
    }
}

fn list_snap(pkgs: &mut Vec<Package>, warnings: &mut Vec<String>) {
    let Some(out) = capture("snap", &["list"]) else {
        warnings.push("snap list failed — snaps skipped".into());
        return;
    };
    for line in out.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 {
            continue;
        }
        let (name, version, rev) = (cols[0], cols[1], cols[2]);
        // `snap list` prints no size; the mounted .snap file is the on-disk
        // footprint of the installed revision.
        let size = std::fs::metadata(format!("/var/lib/snapd/snaps/{name}_{rev}.snap"))
            .map(|m| m.len())
            .unwrap_or(0);
        let publisher = cols.get(4).copied().unwrap_or("");
        pkgs.push(Package {
            id: format!("snap:{name}"),
            name: name.to_string(),
            uninstall_ref: name.to_string(),
            version: version.to_string(),
            size,
            description: if publisher.is_empty() {
                String::new()
            } else {
                format!("published by {publisher}")
            },
            source: PkgSource::Snap,
            essential: name == "snapd" || name.starts_with("core"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes_parse() {
        assert_eq!(parse_human_size("690 B"), 690);
        assert_eq!(parse_human_size("6.48 MiB"), 6794772);
        assert_eq!(parse_human_size("1 GiB"), 1073741824);
        // flatpak emits a non-breaking space and decimal units
        assert_eq!(parse_human_size("319.1\u{00A0}MB"), 319100000);
        assert_eq!(parse_human_size("2,5 GB"), 2500000000);
        assert_eq!(parse_human_size(""), 0);
    }

    #[test]
    fn essential_guard() {
        assert!(is_essential("glibc"));
        assert!(is_essential("linux"));
        assert!(is_essential("linux-lts"));
        assert!(is_essential("kernel-core"));
        assert!(is_essential("linux-image-6.1.0-18-amd64"));
        assert!(!is_essential("linux-firmware"));
        assert!(!is_essential("linux-wallpaperengine-git-debug"));
        assert!(!is_essential("htop"));
    }

    /// Live smoke test: inventory the running system without panicking.
    #[test]
    fn list_installed_smoke() {
        let profile = crate::distro::SystemProfile::detect();
        let (pkgs, warnings) = list_installed(&profile);
        for w in &warnings {
            eprintln!("warning: {w}");
        }
        eprintln!("found {} packages", pkgs.len());
        for p in &pkgs {
            assert!(!p.id.is_empty());
            assert!(!p.name.is_empty());
        }
    }
}
