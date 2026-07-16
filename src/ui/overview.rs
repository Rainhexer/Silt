//! Overview tab: mounts summary + ncdu-style drill-down of disk usage.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::{App, ScanState};

use super::{human, spinner, titled_block, usage_bar};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let mounts_height = (app.mounts.len() as u16 + 2).min(8);
    // A marked-folders panel only appears once something is marked.
    let marks_height = if app.marked.is_empty() {
        0
    } else {
        (app.marked.len() as u16 + 2).min(9)
    };
    let [mounts_area, scan_area, marks_area] = Layout::vertical([
        Constraint::Length(mounts_height),
        Constraint::Min(0),
        Constraint::Length(marks_height),
    ])
    .areas(area);

    render_mounts(frame, app, mounts_area);
    render_scan(frame, app, scan_area);
    if !app.marked.is_empty() {
        render_marked(frame, app, marks_area);
    }
}

/// Panel listing every marked path with its size, plus a running total.
fn render_marked(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let visible = (area.height as usize).saturating_sub(2);
    let items: Vec<ListItem> = app
        .marked
        .iter()
        .take(visible)
        .map(|m| {
            ListItem::new(Line::from(vec![
                Span::styled("  ✓ ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("{:>10}  ", human(m.size)),
                    Style::default().fg(t.text),
                ),
                Span::styled(m.path.display().to_string(), Style::default().fg(t.muted)),
            ]))
        })
        .collect();
    let title = format!(
        "Marked to delete — {} · {}  (d wipes)",
        app.marked.len(),
        human(app.marked_total()),
    );
    frame.render_widget(
        List::new(items).block(
            titled_block(t, &title).border_style(Style::default().fg(t.danger)),
        ),
        area,
    );
}

fn render_mounts(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let items: Vec<ListItem> = app
        .mounts
        .iter()
        .map(|m| {
            // Remote mounts get a cloud badge; local ones a blank column so
            // everything stays aligned.
            let icon = if m.is_remote() { "☁ " } else { "  " };
            let mut spans = vec![
                Span::styled(
                    icon.to_string(),
                    Style::default().fg(t.dir).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<20}", m.mount_point.display().to_string()),
                    Style::default().fg(t.text).add_modifier(Modifier::BOLD),
                ),
            ];
            if m.total_is_synthetic() {
                // rclone reports a fake 1 PiB capacity when the remote has no
                // quota — a fill bar and "x of 1 PiB" would only mislead.
                // Show what actually lives on the remote instead.
                spans.push(Span::styled(
                    format!("{} stored in {}", human(m.used_bytes), m.kind.label()),
                    Style::default().fg(t.dir).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    "  (no size limit reported)".to_string(),
                    Style::default().fg(t.muted),
                ));
            } else {
                let frac = m.used_fraction();
                let color = if frac > 0.9 {
                    t.danger
                } else if frac > 0.75 {
                    t.warn
                } else {
                    t.safe
                };
                spans.push(Span::styled(usage_bar(frac, 20), Style::default().fg(color)));
                spans.push(Span::styled(
                    format!(" {:>3.0}%", frac * 100.0),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    format!("  {} of {}", human(m.used_bytes), human(m.total_bytes)),
                    Style::default().fg(t.muted),
                ));
                if m.is_remote() {
                    spans.push(Span::styled(
                        format!("  on {} server", m.kind.label()),
                        Style::default().fg(t.dir),
                    ));
                }
            }
            spans.push(Span::styled(format!("  {}", m.fs_type), Style::default().fg(t.faint)));
            ListItem::new(Line::from(spans))
        })
        .collect();

    frame.render_widget(List::new(items).block(titled_block(t, "Mounts")), area);
}

/// Breadcrumb from the drill-down stack: root, then each level's dir name.
fn breadcrumb(app: &App) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (i, level) in app.scan_stack.iter().enumerate() {
        if i == 0 {
            parts.push(level.root.display().to_string());
        } else if let Some(name) = level.root.file_name() {
            parts.push(name.to_string_lossy().into_owned());
        }
    }
    parts.join(" ▸ ")
}

fn render_scan(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let Some(level) = app.scan_stack.last() else {
        frame.render_widget(
            Paragraph::new("No scan started.").block(titled_block(t, "Disk usage")),
            area,
        );
        return;
    };

    let scanning = app.scan_state == ScanState::Running;
    let title = if scanning {
        format!(
            "{} — {} {} sifting… {} entries",
            breadcrumb(app),
            human(level.total_size),
            spinner(app),
            app.entries_visited,
        )
    } else if !level.complete {
        format!("{} — {} (partial, r rescans)", breadcrumb(app), human(level.total_size))
    } else {
        format!("{} — {}", breadcrumb(app), human(level.total_size))
    };

    let block = titled_block(t, &title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if level.entries.is_empty() {
        let msg = if scanning {
            format!("{} Sifting…", spinner(app))
        } else {
            "Nothing here but clean floor.".to_string()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(msg, Style::default().fg(t.muted))),
            inner,
        );
        return;
    }

    let max_size = level.entries.first().map(|e| e.size).unwrap_or(1).max(1);
    let total = level.total_size.max(1);
    let bar_width: usize = 24;
    let visible = inner.height as usize;
    // Keep cursor in view.
    let offset = level.cursor.saturating_sub(visible.saturating_sub(1));

    let items: Vec<ListItem> = level
        .entries
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, entry)| {
            let name = entry
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| entry.path.display().to_string());
            let frac = entry.size as f64 / max_size as f64;
            let pct = entry.size as f64 / total as f64 * 100.0;

            let selected = i == level.cursor;
            let name_style = if selected {
                Style::default()
                    .fg(t.on_accent)
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_dir {
                Style::default().fg(t.dir).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.text)
            };
            let marker = if selected { "▸" } else { " " };
            let marked = app.is_marked(&entry.path);
            let mark_glyph = if marked { "✓" } else { " " };
            let (name_txt, affordance) = if entry.remote {
                (format!(" {name}/ "), "☁")
            } else if entry.is_dir {
                (format!(" {name}/ "), "⏎")
            } else {
                (format!(" {name} "), " ")
            };

            // Remote mounts: size is what the cloud reports, not local disk,
            // so the % / bar columns (share of this folder) don't apply.
            let (pct_txt, bar_txt, bar_color) = if entry.remote {
                ("cloud ".to_string(), usage_bar(0.0, bar_width), t.faint)
            } else {
                (format!("{:>4.0}% ", pct), usage_bar(frac, bar_width), t.bar_color(frac))
            };

            let mut spans = vec![
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{mark_glyph} "),
                    Style::default().fg(t.danger).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:>10}  ", human(entry.size)),
                    Style::default().fg(if selected { t.text } else { t.muted }),
                ),
                Span::styled(pct_txt, Style::default().fg(if entry.remote { t.dir } else { t.faint })),
                Span::styled(bar_txt, Style::default().fg(bar_color)),
                Span::styled(name_txt, name_style),
                Span::styled(
                    affordance.to_string(),
                    Style::default().fg(if entry.remote {
                        t.dir
                    } else if selected && entry.is_dir {
                        t.accent
                    } else {
                        t.faint
                    }),
                ),
            ];
            if entry.remote {
                spans.push(Span::styled(
                    "  stored in cloud — not scanned".to_string(),
                    Style::default().fg(t.muted),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    frame.render_widget(List::new(items), inner);
}
