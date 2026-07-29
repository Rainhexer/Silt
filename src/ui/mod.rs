//! TUI rendering. Tab layout: Overview / Clean / System / Log.

mod cache_tab;
pub mod farewell;
mod log_tab;
mod overview;
mod packages_tab;
mod sysinfo_tab;
pub mod theme;

use humansize::{format_size, BINARY};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::{App, StatusKind, Tab};
use crate::targets::RiskTier;
use theme::Theme;

/// Crate version, surfaced in the corner of the TUI and on the farewell screen.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn human(bytes: u64) -> String {
    format_size(bytes, BINARY)
}

pub fn risk_color(theme: &Theme, risk: RiskTier) -> ratatui::style::Color {
    match risk {
        RiskTier::Safe => theme.safe,
        RiskTier::Moderate => theme.warn,
        RiskTier::Caution => theme.danger,
    }
}

/// Risk badge: colored dot + tier name.
pub fn risk_badge(theme: &Theme, risk: RiskTier) -> Span<'static> {
    let (dot, name) = match risk {
        RiskTier::Safe => ("●", "safe    "),
        RiskTier::Moderate => ("●", "moderate"),
        RiskTier::Caution => ("▲", "caution "),
    };
    Span::styled(
        format!("{dot} {name}"),
        Style::default().fg(risk_color(theme, risk)),
    )
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Braille spinner frame, advancing every 100 ms.
pub fn spinner(app: &App) -> &'static str {
    let idx = (app.started.elapsed().as_millis() / 100) as usize % SPINNER.len();
    SPINNER[idx]
}

/// Overlay panels sit on top of a painted retro background; give them the same
/// bg so the `Clear` behind them doesn't punch a hole down to the terminal.
fn overlay_style(theme: &Theme) -> Style {
    match theme.bg {
        Some(bg) => Style::default().bg(bg),
        None => Style::default(),
    }
}

pub fn render(frame: &mut Frame, app: &App) {
    // Retro themes own the whole screen; the default dark/light themes let the
    // terminal's own background show through (bg = None, no-op fill).
    if let Some(bg) = app.theme.bg {
        frame.render_widget(Block::default().style(Style::default().bg(bg)), frame.area());
    }

    let [header, body, status_line, keys_line] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    render_tabs(frame, app, header);

    match app.tab {
        Tab::Overview => overview::render(frame, app, body),
        Tab::Cache => cache_tab::render(frame, app, body),
        Tab::Packages => packages_tab::render(frame, app, body),
        Tab::SysInfo => sysinfo_tab::render(frame, app, body),
        Tab::Log => log_tab::render(frame, app, body),
    }

    render_status(frame, app, status_line);
    render_keys(frame, app, keys_line);

    if app.confirm_pending {
        render_confirm_overlay(frame, app);
    }
    if app.pkg_confirm {
        render_pkg_confirm_overlay(frame, app);
    }
    if app.show_help {
        render_help_overlay(frame, app);
    }
}

/// A short block ramp tinted on the active theme's faint→accent gradient. At
/// rest it's a static decoration; while a background job runs a bright pulse
/// marches through it, advancing on the 100 ms render tick.
fn marching_ramp(app: &App) -> Vec<Span<'static>> {
    const WIDTH: usize = 10;
    // Triangular brightness wave; rotating its phase moves the pulse.
    const WAVE: [f64; WIDTH] = [0.12, 0.22, 0.38, 0.58, 0.8, 1.0, 0.8, 0.58, 0.38, 0.22];
    // Freeze the phase when idle so the ramp only moves during real work.
    let phase = if app.is_working() {
        (app.started.elapsed().as_millis() / 110) as usize
    } else {
        0
    };
    (0..WIDTH)
        .map(|i| {
            let level = WAVE[(i + phase) % WIDTH];
            let glyph = if level < 0.3 {
                '░'
            } else if level < 0.55 {
                '▒'
            } else if level < 0.82 {
                '▓'
            } else {
                '█'
            };
            Span::styled(glyph.to_string(), Style::default().fg(app.theme.bar_color(level)))
        })
        .collect()
}

fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let mut spans: Vec<Span> = vec![Span::styled(
        " ~silt ",
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    )];
    spans.extend(marching_ramp(app));
    spans.push(Span::raw("  "));

    for (i, tab) in Tab::ALL.iter().enumerate() {
        let label = format!(" {} {} ", i + 1, tab.title());
        if *tab == app.tab {
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(t.on_accent)
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(label, Style::default().fg(t.muted)));
        }
        spans.push(Span::raw(" "));
    }

    // Right-aligned theme badge: keeps the current palette named and the switch
    // key discoverable. All glyphs here are width-1, so char counts suffice.
    let name = theme::label(theme::ORDER[app.theme_idx]);
    let badge = [
        Span::styled("t", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {name} "), Style::default().fg(t.muted)),
    ];
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let badge_w: usize = badge.iter().map(|s| s.content.chars().count()).sum();
    if let Some(pad) = (area.width as usize).checked_sub(used + badge_w + 2) {
        spans.push(Span::styled(
            format!("{}│ ", " ".repeat(pad)),
            Style::default().fg(t.faint),
        ));
        spans.extend(badge);
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if app.status.is_empty() {
        return;
    }
    let (color, prefix) = match app.status_kind {
        StatusKind::Info => (t.muted, String::new()),
        StatusKind::Busy => (t.accent, format!("{} ", spinner(app))),
        StatusKind::Success => (t.safe, String::new()),
        StatusKind::Warn => (t.warn, String::new()),
        StatusKind::Error => (t.danger, String::new()),
    };
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{prefix}{}", app.status),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_keys(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let pairs: &[(&str, &str)] = if app.confirm_pending {
        &[("y", "clean"), ("n/Esc", "keep everything")]
    } else if app.pkg_confirm {
        &[("y", "uninstall"), ("n/Esc", "keep everything")]
    } else if app.pkg_filter_input {
        &[("type", "filter"), ("⏎", "keep filter"), ("Esc", "clear")]
    } else {
        match app.tab {
            Tab::Overview => &[
                ("j/k", "move"),
                ("l/⏎", "open dir"),
                ("h", "back"),
                ("r", "rescan"),
                ("Tab", "next tab"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Tab::Cache => &[
                ("j/k", "move"),
                ("Space", "mark"),
                ("a", "all safe"),
                ("A", "none"),
                ("⏎", "preview + clean"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Tab::Packages => &[
                ("j/k", "move"),
                ("Space", "mark"),
                ("s", "sort"),
                ("/", "filter"),
                ("d/⏎", "uninstall marked"),
                ("r", "reload"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Tab::Log => &[
                ("j/k", "scroll"),
                ("g/G", "top/bottom"),
                ("Tab", "next tab"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Tab::SysInfo => &[("Tab", "next tab"), ("?", "help"), ("q", "quit")],
        }
    };
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    for (i, (key, action)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(t.faint)));
        }
        spans.push(Span::styled(
            *key,
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {action}"), Style::default().fg(t.muted)));
    }

    // Right-aligned version stamp in the bottom corner. Dropped silently when
    // the keybind hints already fill the row. Width-1 glyphs, so chars suffice.
    let stamp = format!("v{VERSION} ");
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if let Some(pad) = (area.width as usize).checked_sub(used + stamp.chars().count() + 2) {
        spans.push(Span::raw(" ".repeat(pad + 2)));
        spans.push(Span::styled(stamp, Style::default().fg(t.faint)));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn titled_block<'a>(theme: &Theme, title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.faint))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
}

/// Fixed-width usage bar: `████████░░░░░░░░`.
pub fn usage_bar(fraction: f64, width: usize) -> String {
    let filled = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let mut bar = String::with_capacity(width * 3);
    for i in 0..width {
        bar.push(if i < filled { '█' } else { '░' });
    }
    bar
}

/// Centered rect of `width` x `height`, clamped to the frame.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn render_confirm_overlay(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let targets = app.selected_targets();
    let total: u64 = targets.iter().filter_map(|x| x.size_bytes).sum();
    let has_caution = targets.iter().any(|x| x.risk == RiskTier::Caution);
    let needs_sudo = targets.iter().any(|x| x.needs_sudo());

    let mut lines: Vec<Line> = vec![Line::default()];
    for tg in &targets {
        lines.push(Line::from(vec![
            Span::raw("  "),
            risk_badge(t, tg.risk),
            Span::styled(
                format!("  {}", tg.label),
                Style::default().fg(t.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {}",
                    tg.size_bytes.map(human).unwrap_or_else(|| "size unknown".into())
                ),
                Style::default().fg(t.muted),
            ),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Estimated reclaim: ", Style::default().fg(t.muted)),
        Span::styled(
            human(total),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
    ]));
    if needs_sudo {
        lines.push(Line::from(Span::styled(
            "  Needs sudo — the password prompt appears outside the TUI.",
            Style::default().fg(t.muted),
        )));
    }
    if has_caution {
        lines.push(Line::from(Span::styled(
            "  ▲ Includes Caution-tier targets that may hold real data.",
            Style::default().fg(t.danger).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "y",
            Style::default().fg(t.safe).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" clean it up      ", Style::default().fg(t.text)),
        Span::styled(
            "n",
            Style::default().fg(t.danger).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" keep everything", Style::default().fg(t.text)),
    ]));

    let width = 62;
    let height = lines.len() as u16 + 2;
    let area = centered(frame.area(), width, height);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .style(overlay_style(t))
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            " Confirm cleanup — nothing deleted yet ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_pkg_confirm_overlay(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let marked = app.marked_pkgs();
    let total: u64 = marked.iter().map(|p| p.size).sum();
    let needs_sudo = app.pkg_marked_needs_root();
    let purges_data = marked.iter().any(|p| !p.leftover_dirs().is_empty());

    let mut lines: Vec<Line> = vec![Line::default()];
    // Cap the listing so a huge selection doesn't overflow the screen.
    const SHOWN: usize = 12;
    for p in marked.iter().take(SHOWN) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  ● {:<8}", p.source.to_string()),
                Style::default().fg(t.muted),
            ),
            Span::styled(
                format!(" {}", p.name),
                Style::default().fg(t.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}", human(p.size)), Style::default().fg(t.muted)),
        ]));
    }
    if marked.len() > SHOWN {
        lines.push(Line::from(Span::styled(
            format!("  … and {} more", marked.len() - SHOWN),
            Style::default().fg(t.faint),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Frees about: ", Style::default().fg(t.muted)),
        Span::styled(
            human(total),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "  Caches and app data are removed too where possible.",
        Style::default().fg(t.muted),
    )));
    if purges_data {
        lines.push(Line::from(Span::styled(
            "  ▲ Leftover config/data folders in your home will be purged.",
            Style::default().fg(t.warn),
        )));
    }
    if needs_sudo {
        lines.push(Line::from(Span::styled(
            "  Needs sudo — the password prompt appears outside the TUI.",
            Style::default().fg(t.muted),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "y",
            Style::default().fg(t.danger).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" uninstall them      ", Style::default().fg(t.text)),
        Span::styled(
            "n",
            Style::default().fg(t.safe).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" keep everything", Style::default().fg(t.text)),
    ]));

    let width = 64;
    let height = lines.len() as u16 + 2;
    let area = centered(frame.area(), width, height);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .style(overlay_style(t))
        .border_style(Style::default().fg(t.danger))
        .title(Span::styled(
            format!(" Uninstall {} package(s) — nothing removed yet ", marked.len()),
            Style::default().fg(t.danger).add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_help_overlay(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let key = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(
                format!("  {k:<12}"),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(d.to_string(), Style::default().fg(t.text)),
        ])
    };
    let section = |s: &str| {
        Line::from(Span::styled(
            format!(" {s}"),
            Style::default().fg(t.dir).add_modifier(Modifier::BOLD),
        ))
    };
    let lines = vec![
        Line::default(),
        section("Everywhere"),
        key("Tab / 1-5", "switch tabs"),
        key("?", "this help"),
        key("q / Ctrl-c", "quit"),
        Line::default(),
        section("Overview (drill like ncdu)"),
        key("j k / ↑↓", "move"),
        key("l / ⏎ / →", "open directory (works mid-scan)"),
        key("h / ← / ⌫", "back up one level"),
        key("g / G", "jump to top / bottom"),
        key("r", "rescan this directory"),
        Line::default(),
        section("Clean"),
        key("Space", "mark / unmark target"),
        key("a / A", "mark all Safe / clear all"),
        key("⏎", "preview, then confirm"),
        Line::default(),
        section("Packages"),
        key("Space", "mark / unmark for uninstall"),
        key("s", "cycle sort (size / name / source)"),
        key("/", "filter by name"),
        key("d / ⏎", "uninstall marked (confirms first)"),
        key("r", "reload package list"),
        Line::default(),
        Line::default(),
        section("Look"),
        key("t / T", "next / previous color theme"),
        Line::default(),
        Line::from(Span::styled(
            "  Any key closes this.",
            Style::default().fg(t.muted),
        )),
    ];
    let area = centered(frame.area(), 56, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(overlay_style(t))
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            " Keys ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
