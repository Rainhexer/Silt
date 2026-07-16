//! Unused Flatpak runtimes.

use crate::distro::{run_capture, SystemProfile};

use super::{Category, CleanupAction, CleanupTarget, RiskTier};

pub fn detect(profile: &SystemProfile) -> Vec<CleanupTarget> {
    if !profile.has_flatpak {
        return Vec::new();
    }

    // `flatpak uninstall --unused` with no unused refs prints nothing useful;
    // detect whether there's anything to do first.
    let unused = run_capture("flatpak", &["list", "--runtime", "--columns=ref"]);
    let has_runtimes = unused.map(|s| !s.trim().is_empty()).unwrap_or(false);
    if !has_runtimes {
        return Vec::new();
    }

    vec![CleanupTarget {
        id: "flatpak-unused".into(),
        label: "Flatpak: unused runtimes".into(),
        category: Category::PackageManager,
        risk: RiskTier::Safe,
        paths: Vec::new(),
        size_bytes: None,
        action: CleanupAction::RunCommand {
            cmd: "flatpak".into(),
            args: vec![
                "uninstall".into(),
                "--unused".into(),
                "--noninteractive".into(),
            ],
            needs_root: false,
        },
        description: "Uninstall Flatpak runtimes no installed app depends on".into(),
    }]
}
