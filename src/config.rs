//! TOML config: `~/.config/silt/config.toml`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub scan: ScanConfig,
    pub cleanup: CleanupConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ScanConfig {
    pub default_root: String,
    pub exclude_paths: Vec<PathBuf>,
    pub follow_symlinks: bool,
    /// Walk into cloud/network mounts (rclone, sshfs, NFS, …). Off by
    /// default: sizing a FUSE cloud mount can download every file in it.
    pub include_remote_mounts: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        ScanConfig {
            default_root: "~".into(),
            exclude_paths: vec![
                PathBuf::from("/mnt"),
                PathBuf::from("/run/media"),
                PathBuf::from("/proc"),
                PathBuf::from("/sys"),
                PathBuf::from("/dev"),
            ],
            follow_symlinks: false,
            include_remote_mounts: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CleanupConfig {
    pub auto_confirm_safe_only: bool,
    pub journalctl_keep_days: u32,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        CleanupConfig {
            auto_confirm_safe_only: false,
            journalctl_keep_days: 14,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: String,
    pub bar_chart_style: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            theme: "dark".into(),
            bar_chart_style: "blocks".into(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let Some(path) = Self::path() else {
            return Ok(Config::default());
        };
        if !path.exists() {
            return Ok(Config::default());
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("parsing config {}", path.display()))?;
        Ok(config)
    }

    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("silt").join("config.toml"))
    }

    /// Resolve the configured default scan root, expanding a leading `~`.
    pub fn default_root(&self) -> PathBuf {
        expand_tilde(&self.scan.default_root)
    }

    /// Persist the chosen theme to `[ui] theme` in config.toml, leaving every
    /// other key, comment, and section the user wrote untouched. Creates the
    /// file (and its directory) if absent.
    pub fn save_theme(name: &str) -> Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let updated = upsert_ui_theme(&existing, name);
        std::fs::write(&path, updated)
            .with_context(|| format!("writing config {}", path.display()))?;
        Ok(())
    }
}

/// Set `theme = "<name>"` inside the `[ui]` table of a TOML document, editing
/// text in place: replace an existing key, else insert under an existing
/// `[ui]` header, else append a fresh `[ui]` table.
fn upsert_ui_theme(src: &str, name: &str) -> String {
    let line = format!("theme = \"{name}\"");
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    let mut in_ui = false;
    let mut ui_header: Option<usize> = None;

    for (i, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') {
            in_ui = trimmed == "[ui]";
            if in_ui {
                ui_header = Some(i);
            }
        } else if in_ui && trimmed.starts_with("theme") && trimmed[5..].trim_start().starts_with('=')
        {
            lines[i] = line;
            return join_lines(&lines, src);
        }
    }

    match ui_header {
        Some(h) => lines.insert(h + 1, line),
        None => {
            if !lines.is_empty() && !lines.last().map(|l| l.is_empty()).unwrap_or(true) {
                lines.push(String::new());
            }
            lines.push("[ui]".into());
            lines.push(line);
        }
    }
    join_lines(&lines, src)
}

/// Rejoin edited lines, keeping a trailing newline if the source had one (or was
/// empty, so a brand-new file ends cleanly).
fn join_lines(lines: &[String], src: &str) -> String {
    let mut out = lines.join("\n");
    if src.is_empty() || src.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}
