//! Silt — terminal-native Linux storage cleaner.

mod app;
mod config;
mod distro;
mod packages;
mod scanner;
mod targets;
mod ui;

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Parser;
use serde_json::json;

use crate::app::App;
use crate::config::Config;
use crate::distro::SystemProfile;
use crate::targets::{build_registry, RiskTier};

#[derive(Parser, Debug)]
#[command(
    name = "silt",
    version,
    about = "Terminal-native Linux storage cleaner: scan, visualize, clean."
)]
struct Cli {
    /// Output scan results and target sizes as JSON (headless, no TUI).
    #[arg(long)]
    json: bool,

    /// Accepted for compatibility; headless cleanup always executes.
    #[arg(long)]
    yes: bool,

    /// Cleanup target id(s) to execute headlessly (repeatable).
    #[arg(long = "target", value_name = "ID")]
    targets: Vec<String>,

    /// With --yes and no explicit --target: run every Safe-tier target.
    #[arg(long)]
    all_safe: bool,

    /// Also allow Moderate-tier targets in headless mode.
    #[arg(long)]
    include_moderate: bool,

    /// List detected cleanup targets and exit.
    #[arg(long)]
    list_targets: bool,

    /// Print detected system profile and exit.
    #[arg(long)]
    profile: bool,

    /// Scan root for the Overview tab / --json scan (default: config or ~).
    #[arg(long, value_name = "PATH")]
    root: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    let profile = SystemProfile::detect();

    if cli.profile {
        println!("{}", serde_json::to_string_pretty(&profile)?);
        return Ok(());
    }

    if cli.list_targets {
        return list_targets(&profile, &config);
    }

    if cli.json {
        return json_report(&profile, &config, cli.root);
    }

    if cli.yes || cli.all_safe || !cli.targets.is_empty() {
        return headless_clean(&profile, &config, &cli);
    }

    // Interactive TUI.
    let mut config = config;
    if let Some(root) = cli.root {
        config.scan.default_root = root.display().to_string();
    }
    let terminal = &mut ratatui::init();
    let mut app = App::new(profile, config);
    let result = app.run(terminal);
    ratatui::restore();
    if result.is_ok() {
        ui::farewell::print(&app);
    }
    result
}

/// Persist headless session logs to `~/.local/share/silt/logs/`.
fn save_headless_log(lines: &[String]) {
    let Some(dir) = dirs::data_dir().map(|d| d.join("silt").join("logs")) else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("silt: failed to create log dir {}: {e}", dir.display());
        return;
    }
    // Compute a filename once, at the end of the run.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86400;
    let t = secs % 86400;
    let h = t / 3600;
    let m = (t % 3600) / 60;
    let s = t % 60;
    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    let day = doy - (153 * mp + 2) / 5 + 1;
    let stamp = format!("{:04}-{:02}-{:02}T{h:02}-{m:02}-{s:02}", year, month, day);
    let path = dir.join(format!("silt-{stamp}.log"));
    let content = lines.join("\n") + "\n";
    if let Err(e) = std::fs::write(&path, &content) {
        eprintln!("silt: failed to write session log {}: {e}", path.display());
    }
}

fn list_targets(profile: &SystemProfile, config: &Config) -> Result<()> {
    let mut registry = build_registry(profile, config);
    for t in &mut registry {
        t.ensure_sized();
    }
    println!("{:<28} {:<10} {:>12}  LABEL", "ID", "RISK", "SIZE");
    for t in &registry {
        println!(
            "{:<28} {:<10} {:>12}  {}",
            t.id,
            t.risk.to_string(),
            t.size_bytes.map(ui::human).unwrap_or_else(|| "?".into()),
            t.label
        );
    }
    Ok(())
}

fn json_report(profile: &SystemProfile, config: &Config, root: Option<PathBuf>) -> Result<()> {
    let mut registry = build_registry(profile, config);
    for t in &mut registry {
        t.ensure_sized();
    }

    let root = root.unwrap_or_else(|| config.default_root());
    let mounts = scanner::mounts::list_mounts();
    // Never walk remote (cloud/network) mounts: sizing a FUSE cloud mount
    // can force every file to download. Scanning the mount itself is allowed
    // as an explicit choice.
    let mut exclude = config.scan.exclude_paths.clone();
    if !config.scan.include_remote_mounts {
        exclude.extend(
            mounts
                .iter()
                .filter(|m| m.is_remote() && m.mount_point != root)
                .map(|m| m.mount_point.clone()),
        );
    }
    let handle = scanner::start_scan(root.clone(), exclude, config.scan.follow_symlinks);
    let mut entries = Vec::new();
    let mut total_size = 0u64;
    // Surface skipped remote mounts as entries (same as the TUI): size is
    // what the remote reports as used, and it doesn't count into the local
    // total.
    if !config.scan.include_remote_mounts {
        for m in mounts.iter().filter(|m| m.is_remote()) {
            if m.mount_point.parent() == Some(root.as_path()) {
                entries.push(json!({
                    "path": m.mount_point,
                    "size_bytes": m.used_bytes,
                    "is_dir": true,
                    "remote": true,
                }));
            }
        }
    }
    while let Ok(event) = handle.receiver.recv() {
        match event {
            scanner::ScanEvent::DirScanned { path, size, is_dir } => {
                entries.push(json!({
                    "path": path,
                    "size_bytes": size,
                    "is_dir": is_dir,
                    "remote": false,
                }));
            }
            scanner::ScanEvent::Done { total_size: t } => {
                total_size = t;
                break;
            }
            _ => {}
        }
    }

    let report = json!({
        "profile": profile,
        "mounts": mounts,
        "scan": {
            "root": root,
            "total_size_bytes": total_size,
            "entries": entries,
        },
        "targets": registry.iter().map(|t| json!({
            "id": t.id,
            "label": t.label,
            "category": t.category,
            "risk": t.risk,
            "size_bytes": t.size_bytes,
            "description": t.description,
            "paths": t.paths,
        })).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn headless_clean(profile: &SystemProfile, config: &Config, cli: &Cli) -> Result<()> {
    let mut registry = build_registry(profile, config);
    for t in &mut registry {
        t.ensure_sized();
    }

    let chosen: Vec<&targets::CleanupTarget> = if !cli.targets.is_empty() {
        let mut chosen = Vec::new();
        for id in &cli.targets {
            match registry.iter().find(|t| &t.id == id) {
                Some(t) => chosen.push(t),
                None => bail!(
                    "unknown target id '{id}' — run `silt --list-targets` to see available ids"
                ),
            }
        }
        chosen
    } else if cli.all_safe {
        registry
            .iter()
            .filter(|t| {
                t.risk == RiskTier::Safe
                    || (cli.include_moderate && t.risk == RiskTier::Moderate)
            })
            .collect()
    } else {
        bail!("headless mode needs --target=<id> (repeatable) or --all-safe");
    };

    // Guardrails: never run Caution targets headlessly without explicit
    // --target naming them; never run Moderate without --include-moderate.
    for t in &chosen {
        if t.risk == RiskTier::Caution && cli.targets.is_empty() {
            bail!("target '{}' is Caution tier; name it explicitly with --target", t.id);
        }
        if t.risk == RiskTier::Moderate && cli.targets.is_empty() && !cli.include_moderate {
            bail!(
                "target '{}' is Moderate tier; pass --include-moderate to allow",
                t.id
            );
        }
    }

    let total: u64 = chosen.iter().filter_map(|t| t.size_bytes).sum();
    println!("Selected {} target(s), ~{} reclaimable:", chosen.len(), ui::human(total));
    for t in &chosen {
        for line in t.plan_preview() {
            println!("  {line}");
        }
    }

    println!("\nExecuting…");
    // Keep the sudo timestamp alive: a large cleanup can outlast it.
    let _keepalive = targets::SudoKeepalive::start();
    let mut failures = 0usize;
    let mut log_lines: Vec<String> = Vec::new();
    log_lines.push(format!("Silt started — headless mode, {} target(s)", chosen.len()));
    for t in &chosen {
        println!(">> {}", t.label);
        log_lines.push(format!(">> {}", t.label));
        // Headless runs on a real terminal, so sudo may prompt directly.
        let outcome = t.execute(true);
        for l in &outcome.log {
            println!("   {l}");
            log_lines.push(format!("   {l}"));
        }
        for e in &outcome.errors {
            eprintln!("   ERROR: {e}");
            log_lines.push(format!("   ERROR: {e}"));
            failures += 1;
        }
    }
    let outcome = if failures > 0 {
        let msg = format!("{failures} operation(s) failed");
        log_lines.push(msg.clone());
        Err(anyhow::anyhow!("{msg} — see the log above"))
    } else {
        log_lines.push("Done.".into());
        Ok(())
    };
    save_headless_log(&log_lines);
    outcome
}
