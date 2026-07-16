//! Distro, package manager, init system, and subsystem detection.

use std::fmt;
use std::path::Path;
use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageManager {
    Pacman,
    Apt,
    Dnf,
    Zypper,
    Nix,
    Unknown,
}

impl fmt::Display for PackageManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            PackageManager::Pacman => "pacman",
            PackageManager::Apt => "apt",
            PackageManager::Dnf => "dnf",
            PackageManager::Zypper => "zypper",
            PackageManager::Nix => "nix",
            PackageManager::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InitSystem {
    Systemd,
    Other,
}

impl fmt::Display for InitSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InitSystem::Systemd => f.write_str("systemd"),
            InitSystem::Other => f.write_str("other"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemProfile {
    pub distro_id: String,
    pub distro_name: String,
    pub distro_version: Option<String>,
    pub kernel: String,
    pub package_manager: PackageManager,
    pub init_system: InitSystem,
    pub has_flatpak: bool,
    pub has_snap: bool,
    pub has_docker: bool,
    pub has_podman: bool,
    pub has_nix: bool,
    pub desktop_env: Option<String>,
    pub hostname: String,
}

impl SystemProfile {
    pub fn detect() -> Self {
        let os_release = parse_os_release(Path::new("/etc/os-release"));
        SystemProfile {
            distro_id: os_release.id,
            distro_name: os_release.name,
            distro_version: os_release.version,
            kernel: read_kernel(),
            package_manager: detect_package_manager(),
            init_system: detect_init_system(),
            has_flatpak: binary_exists("flatpak"),
            has_snap: binary_exists("snap"),
            has_docker: binary_exists("docker"),
            has_podman: binary_exists("podman"),
            has_nix: binary_exists("nix-store") || Path::new("/nix/store").is_dir(),
            desktop_env: detect_desktop_env(),
            hostname: std::fs::read_to_string("/etc/hostname")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".into()),
        }
    }
}

struct OsRelease {
    id: String,
    name: String,
    version: Option<String>,
}

fn parse_os_release(path: &Path) -> OsRelease {
    let mut id = String::from("unknown");
    let mut name = String::from("Unknown Linux");
    let mut version = None;

    if let Ok(contents) = std::fs::read_to_string(path) {
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').to_string();
            match key.trim() {
                "ID" => id = value,
                "PRETTY_NAME" => name = value,
                "NAME" if name == "Unknown Linux" => name = value,
                "VERSION_ID" => version = Some(value),
                _ => {}
            }
        }
    }

    OsRelease { id, name, version }
}

fn read_kernel() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

pub fn binary_exists(name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join(name).is_file())
}

fn detect_package_manager() -> PackageManager {
    // Order matters: some distros ship multiple (e.g. Arch containers with apt
    // installed for cross-builds). Prefer the native one via os-release hints
    // first, then fall back to binary presence.
    let candidates = [
        ("pacman", PackageManager::Pacman),
        ("apt", PackageManager::Apt),
        ("dnf", PackageManager::Dnf),
        ("zypper", PackageManager::Zypper),
        ("nix-env", PackageManager::Nix),
    ];
    for (bin, pm) in candidates {
        if binary_exists(bin) {
            return pm;
        }
    }
    PackageManager::Unknown
}

fn detect_init_system() -> InitSystem {
    if Path::new("/run/systemd/system").is_dir() {
        InitSystem::Systemd
    } else {
        InitSystem::Other
    }
}

fn detect_desktop_env() -> Option<String> {
    for var in ["XDG_CURRENT_DESKTOP", "DESKTOP_SESSION"] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Run a command and capture stdout as a string. Returns None on spawn
/// failure or non-zero exit.
pub fn run_capture(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd).args(args).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}
