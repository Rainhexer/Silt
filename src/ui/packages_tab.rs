//! Packages tab: every installed package (system PM, Flatpak, Snap) with
//! sizes, sortable and filterable, mark-and-uninstall like the Clean tab.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::packages::{Package, PkgSource};

use super::{human, spinner, titled_block, usage_bar};

const BAR_WIDTH: usize = 12;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let [summary_area, list_area, detail_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(6),
    ])
    .areas(area);

    render_summary(frame, app, summary_area);
    render_list(frame, app, list_area);
    render_detail(frame, app, detail_area);
}

fn source_color(app: &App, source: PkgSource) -> ratatui::style::Color {
    let t = &app.theme;
    match source {
        PkgSource::System(_) => t.accent_dim,
        PkgSource::Flatpak => t.dir,
        PkgSource::Snap => t.warn,
    }
}

/// One line: per-source package counts and total footprint.
fn render_summary(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if app.packages.is_empty() {
        return;
    }
    // (source label, count, bytes) in first-seen order.
    let mut groups: Vec<(PkgSource, usize, u64)> = Vec::new();
    for p in &app.packages {
        match groups.iter_mut().find(|(s, _, _)| *s == p.source) {
            Some((_, n, b)) => {
                *n += 1;
                *b += p.size;
            }
            None => groups.push((p.source, 1, p.size)),
        }
    }
    let total: u64 = groups.iter().map(|(_, _, b)| b).sum();
    let mut spans: Vec<Span> = vec![
        Span::raw(" "),
        Span::styled(
            format!("{} packages · {}", app.packages.len(), human(total)),
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ),
    ];
    for (src, n, bytes) in &groups {
        spans.push(Span::styled("   ", Style::default()));
        spans.push(Span::styled(
            format!("● {src} "),
            Style::default().fg(source_color(app, *src)),
        ));
        spans.push(Span::styled(
            format!("{n} ({})", human(*bytes)),
            Style::default().fg(t.muted),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let marked_total: u64 = app.marked_pkgs().iter().map(|p| p.size).sum();
    let mut title = format!(
        "Installed — {} shown · sort: {}",
        app.pkg_view.len(),
        app.pkg_sort.label()
    );
    if !app.pkg_filter.is_empty() || app.pkg_filter_input {
        title.push_str(&format!(" · filter: {}", app.pkg_filter));
        if app.pkg_filter_input {
            title.push('▏');
        }
    }
    if !app.pkg_marked.is_empty() {
        title.push_str(&format!(
            " · {} marked (~{})",
            app.pkg_marked.len(),
            human(marked_total)
        ));
    }
    let block = titled_block(t, &title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.pkg_loading {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{} Taking inventory of installed packages…", spinner(app)),
                Style::default().fg(t.accent),
            )),
            inner,
        );
        return;
    }
    if app.pkg_view.is_empty() {
        let msg = if app.packages.is_empty() {
            "No packages found. r reloads."
        } else {
            "Nothing matches the filter. Esc clears it."
        };
        frame.render_widget(
            Paragraph::new(Span::styled(msg, Style::default().fg(t.muted))),
            inner,
        );
        return;
    }

    // Bars scale against the largest visible package.
    let max_size = app
        .pkg_view
        .iter()
        .map(|&i| app.packages[i].size)
        .max()
        .unwrap_or(1)
        .max(1);

    let visible = inner.height as usize;
    let offset = app.pkg_cursor.saturating_sub(visible.saturating_sub(1));

    let items: Vec<ListItem> = app
        .pkg_view
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(row, &idx)| {
            let p = &app.packages[idx];
            let cursor_here = row == app.pkg_cursor;
            let checked = app.pkg_marked.contains(&p.id);

            let checkbox = if checked { "[■]" } else { "[ ]" };
            let frac = p.size as f64 / max_size as f64;
            let bar = usage_bar(frac, BAR_WIDTH);

            let name_style = if cursor_here {
                Style::default()
                    .fg(t.on_accent)
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD)
            } else if checked {
                Style::default().fg(t.danger).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.text)
            };
            let marker = if cursor_here { "▸" } else { " " };

            let mut spans = vec![
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{checkbox} "),
                    Style::default()
                        .fg(if checked { t.danger } else { t.faint })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:>10} ", human(p.size)), Style::default().fg(t.muted)),
                Span::styled(bar, Style::default().fg(t.bar_color(frac))),
                Span::styled(
                    format!(" {:<8}", p.source.to_string()),
                    Style::default().fg(source_color(app, p.source)),
                ),
                Span::styled(format!(" {:<32}", p.name), name_style),
                Span::styled(format!(" {}", p.version), Style::default().fg(t.faint)),
            ];
            if p.essential {
                spans.push(Span::styled(
                    "  ▲ core",
                    Style::default().fg(t.danger).add_modifier(Modifier::BOLD),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    frame.render_widget(List::new(items), inner);
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = titled_block(t, "What uninstalling does");
    let Some(p) = app.pkg_at_cursor() else {
        frame.render_widget(block, area);
        return;
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("{} {}", p.name, p.version),
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  — {}", if p.description.is_empty() { "no description" } else { &p.description }),
            Style::default().fg(t.muted),
        ),
    ])];
    lines.push(preview_line(p, t));
    let leftovers = p.leftover_dirs();
    if !leftovers.is_empty() {
        let list = leftovers
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(Line::from(Span::styled(
            format!("  also purges leftover data: {list}"),
            Style::default().fg(t.warn),
        )));
    }
    if p.essential {
        lines.push(Line::from(Span::styled(
            "  ▲ Core system package — Silt won't mark this for removal.",
            Style::default().fg(t.danger).add_modifier(Modifier::BOLD),
        )));
    } else if p.needs_root() {
        lines.push(Line::from(Span::styled(
            "  Runs via sudo — you'll be asked for your password outside the TUI.",
            Style::default().fg(t.muted),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(block),
        area,
    );
}

/// The exact command an uninstall would run, or why one isn't available.
pub fn preview_line<'a>(p: &Package, t: &crate::ui::theme::Theme) -> Line<'a> {
    match p.uninstall_command() {
        Some((cmd, args, root)) => {
            let prefix = if root && !crate::targets::is_root() { "sudo " } else { "" };
            Line::from(Span::styled(
                format!("  $ {prefix}{cmd} {}", args.join(" ")),
                Style::default().fg(t.muted),
            ))
        }
        None => Line::from(Span::styled(
            "  Silt can't uninstall packages from this source yet.",
            Style::default().fg(t.warn),
        )),
    }
}
