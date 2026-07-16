//! System tab: distro / kernel / package manager / subsystems.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;

use super::titled_block;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let p = &app.profile;

    let yes_no = |b: bool| {
        if b {
            Span::styled("● yes", Style::default().fg(t.safe))
        } else {
            Span::styled("○ no", Style::default().fg(t.faint))
        }
    };
    let row = |key: &str, value: Span<'static>| {
        Line::from(vec![
            Span::styled(
                format!("{key:<18}"),
                Style::default().fg(t.dir).add_modifier(Modifier::BOLD),
            ),
            value,
        ])
    };
    let text = |s: String| Span::styled(s, Style::default().fg(t.text));

    let lines = vec![
        row("Hostname", text(p.hostname.clone())),
        row("Distro", text(format!("{} (id: {})", p.distro_name, p.distro_id))),
        row(
            "Version",
            text(p.distro_version.clone().unwrap_or_else(|| "rolling".into())),
        ),
        row("Kernel", text(p.kernel.clone())),
        row("Package manager", text(p.package_manager.to_string())),
        row("Init system", text(p.init_system.to_string())),
        row(
            "Desktop",
            text(p.desktop_env.clone().unwrap_or_else(|| "none (headless?)".into())),
        ),
        Line::default(),
        row("Flatpak", yes_no(p.has_flatpak)),
        row("Snap", yes_no(p.has_snap)),
        row("Docker", yes_no(p.has_docker)),
        row("Podman", yes_no(p.has_podman)),
        row("Nix", yes_no(p.has_nix)),
        Line::default(),
        row("Running as root", yes_no(crate::targets::is_root())),
        Line::from(Span::styled(
            "Root-needing targets (package cache, journal) prompt for sudo per run.",
            Style::default().fg(t.muted),
        )),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(titled_block(t, "System profile")),
        area,
    );
}
