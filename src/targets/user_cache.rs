//! ~/.cache breakdown: browser caches, thumbnails, and language toolchain
//! build caches, each sized individually.

use std::path::PathBuf;

use super::{Category, CleanupAction, CleanupTarget, RiskTier};

/// Known ~/.cache subdirectories worth calling out by name. Everything is
/// Safe tier — caches regenerate.
const KNOWN_CACHES: &[(&str, &str)] = &[
    ("mozilla", "Firefox cache"),
    ("chromium", "Chromium cache"),
    ("google-chrome", "Chrome cache"),
    ("BraveSoftware", "Brave cache"),
    ("thumbnails", "Thumbnail cache"),
    ("pip", "pip cache"),
    ("yarn", "Yarn cache"),
    ("pnpm", "pnpm store cache"),
    ("go-build", "Go build cache"),
    ("mesa_shader_cache", "Mesa shader cache"),
    ("nvidia", "NVIDIA shader cache"),
    ("fontconfig", "Fontconfig cache"),
];

pub fn detect() -> Vec<CleanupTarget> {
    let mut targets = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return targets;
    };
    let cache_root = home.join(".cache");
    if !cache_root.is_dir() {
        return targets;
    }

    for (subdir, label) in KNOWN_CACHES {
        let path = cache_root.join(subdir);
        if path.is_dir() {
            targets.push(cache_target(
                format!("cache-{}", subdir.to_lowercase()),
                (*label).to_string(),
                path,
            ));
        }
    }

    // npm keeps its cache in ~/.npm (_cacache), not ~/.cache.
    let npm = home.join(".npm/_cacache");
    if npm.is_dir() {
        targets.push(cache_target("cache-npm".into(), "npm cache".into(), npm));
    }

    // cargo: registry cache + src are regenerable; leave build target dirs
    // alone (they belong to projects, not a global cache).
    let cargo = home.join(".cargo/registry");
    if cargo.is_dir() {
        targets.push(CleanupTarget {
            id: "cache-cargo-registry".into(),
            label: "Cargo registry cache".into(),
            category: Category::UserCache,
            risk: RiskTier::Safe,
            paths: vec![cargo.join("cache"), cargo.join("src")],
            size_bytes: None,
            action: CleanupAction::DeletePathContents(vec![
                cargo.join("cache"),
                cargo.join("src"),
            ]),
            description: "Delete downloaded crate archives and unpacked sources".into(),
        });
    }

    // Remainder of ~/.cache not covered above, as one catch-all target.
    let known: Vec<PathBuf> = KNOWN_CACHES
        .iter()
        .map(|(s, _)| cache_root.join(s))
        .collect();
    if let Ok(entries) = std::fs::read_dir(&cache_root) {
        let others: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !known.contains(p))
            .collect();
        if !others.is_empty() {
            targets.push(CleanupTarget {
                id: "cache-other".into(),
                label: format!("Other ~/.cache dirs ({})", others.len()),
                category: Category::UserCache,
                risk: RiskTier::Moderate,
                paths: others.clone(),
                size_bytes: None,
                action: CleanupAction::DeletePaths(others),
                description: "Delete remaining ~/.cache subdirectories (apps recreate on demand)"
                    .into(),
            });
        }
    }

    targets
}

fn cache_target(id: String, label: String, path: PathBuf) -> CleanupTarget {
    CleanupTarget {
        id,
        label,
        category: Category::UserCache,
        risk: RiskTier::Safe,
        description: format!("Delete contents of {}", path.display()),
        action: CleanupAction::DeletePathContents(vec![path.clone()]),
        paths: vec![path],
        size_bytes: None,
    }
}
