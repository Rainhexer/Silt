//! Docker/Podman dangling images, stopped containers, unused volumes.

use crate::distro::{run_capture, SystemProfile};

use super::{Category, CleanupAction, CleanupTarget, RiskTier};

pub fn detect(profile: &SystemProfile) -> Vec<CleanupTarget> {
    let mut targets = Vec::new();
    if profile.has_docker {
        targets.extend(engine_targets("docker"));
    }
    if profile.has_podman {
        targets.extend(engine_targets("podman"));
    }
    targets
}

fn engine_targets(engine: &str) -> Vec<CleanupTarget> {
    let mut targets = Vec::new();

    // `system df` gives reclaimable estimates; skip targets entirely if the
    // daemon isn't reachable (docker installed but not running).
    let Some(df) = run_capture(
        engine,
        &["system", "df", "--format", "{{.Type}}\t{{.Reclaimable}}"],
    ) else {
        return targets;
    };

    let mut images_reclaimable = None;
    let mut containers_reclaimable = None;
    let mut volumes_reclaimable = None;
    let mut build_cache_reclaimable = None;
    for line in df.lines() {
        let mut parts = line.split('\t');
        let (Some(kind), Some(reclaim)) = (parts.next(), parts.next()) else {
            continue;
        };
        let bytes = parse_docker_size(reclaim);
        match kind.trim() {
            "Images" => images_reclaimable = bytes,
            "Containers" => containers_reclaimable = bytes,
            "Local Volumes" => volumes_reclaimable = bytes,
            "Build Cache" => build_cache_reclaimable = bytes,
            _ => {}
        }
    }

    targets.push(CleanupTarget {
        id: format!("{engine}-dangling-images"),
        label: format!("{engine}: dangling images"),
        category: Category::Containers,
        risk: RiskTier::Safe,
        paths: Vec::new(),
        size_bytes: images_reclaimable,
        action: CleanupAction::RunCommand {
            cmd: engine.into(),
            args: vec!["image".into(), "prune".into(), "-f".into()],
            needs_root: false,
        },
        description: format!("{engine} image prune -f (untagged/dangling images only)"),
    });

    targets.push(CleanupTarget {
        id: format!("{engine}-stopped-containers"),
        label: format!("{engine}: stopped containers"),
        category: Category::Containers,
        risk: RiskTier::Moderate,
        paths: Vec::new(),
        size_bytes: containers_reclaimable,
        action: CleanupAction::RunCommand {
            cmd: engine.into(),
            args: vec!["container".into(), "prune".into(), "-f".into()],
            needs_root: false,
        },
        description: format!("{engine} container prune -f (removes all stopped containers)"),
    });

    if build_cache_reclaimable.unwrap_or(0) > 0 {
        targets.push(CleanupTarget {
            id: format!("{engine}-build-cache"),
            label: format!("{engine}: build cache"),
            category: Category::Containers,
            risk: RiskTier::Safe,
            paths: Vec::new(),
            size_bytes: build_cache_reclaimable,
            action: CleanupAction::RunCommand {
                cmd: engine.into(),
                args: vec!["builder".into(), "prune".into(), "-f".into()],
                needs_root: false,
            },
            description: format!("{engine} builder prune -f (buildkit layer cache)"),
        });
    }

    // Volumes can hold real data (databases!). Caution tier, never
    // bulk-selected; listed per spec with explicit confirmation.
    targets.push(CleanupTarget {
        id: format!("{engine}-unused-volumes"),
        label: format!("{engine}: unused volumes (DANGER: may hold data)"),
        category: Category::Containers,
        risk: RiskTier::Caution,
        paths: Vec::new(),
        size_bytes: volumes_reclaimable,
        action: CleanupAction::RunCommand {
            cmd: engine.into(),
            args: vec!["volume".into(), "prune".into(), "-f".into()],
            needs_root: false,
        },
        description: format!(
            "{engine} volume prune -f — deletes ALL volumes not attached to a container, \
             including database data. Verify with `{engine} volume ls` first."
        ),
    });

    targets
}

/// Parse docker size strings like "1.5GB", "300MB (55%)", "0B".
fn parse_docker_size(s: &str) -> Option<u64> {
    let s = s.split_whitespace().next()?;
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = s.split_at(split);
    let value: f64 = num.parse().ok()?;
    let multiplier: f64 = match unit {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((value * multiplier) as u64)
}
