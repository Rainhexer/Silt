//! Clean tab: checklist of cleanup targets with sizes and risk tiers.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

use super::{human, risk_badge, risk_color, spinner, titled_block};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let [list_area, detail_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(7)]).areas(area);

    render_list(frame, app, list_area);
    render_detail(frame, app, detail_area);
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let selected_total: u64 = app
        .targets
        .iter()
        .filter(|tg| app.selected.contains(&tg.id))
        .filter_map(|tg| tg.size_bytes)
        .sum();
    let title = if app.selected.is_empty() {
        "Cleanup targets — nothing picked yet".to_string()
    } else {
        format!(
            "Cleanup targets — {} picked, ~{} to reclaim",
            app.selected.len(),
            human(selected_total)
        )
    };
    let block = titled_block(t, &title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.targets.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No cleanup targets detected on this system.",
                Style::default().fg(t.muted),
            )),
            inner,
        );
        return;
    }

    let visible = inner.height as usize;
    let offset = app.target_cursor.saturating_sub(visible.saturating_sub(1));

    let items: Vec<ListItem> = app
        .targets
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, tg)| {
            let checked = app.selected.contains(&tg.id);
            let cursor_here = i == app.target_cursor;

            let checkbox = if checked { "[■]" } else { "[ ]" };
            let size = tg
                .size_bytes
                .map(human)
                .unwrap_or_else(|| format!("{} sizing", spinner(app)));

            let label_style = if cursor_here {
                Style::default()
                    .fg(t.on_accent)
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD)
            } else if checked {
                Style::default().fg(t.text).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.muted)
            };
            let marker = if cursor_here { "▸" } else { " " };

            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{checkbox} "),
                    Style::default()
                        .fg(if checked { t.safe } else { t.faint })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:>10}  ", size),
                    Style::default().fg(if checked { t.text } else { t.muted }),
                ),
                risk_badge(t, tg.risk),
                Span::styled(format!("  {:<40} ", tg.label), label_style),
                Span::styled(format!(" {}", tg.category), Style::default().fg(t.faint)),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items), inner);
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let Some(tg) = app.targets.get(app.target_cursor) else {
        return;
    };
    let mut lines = vec![Line::from(vec![
        risk_badge(t, tg.risk),
        Span::styled(
            format!("  {}", tg.description),
            Style::default().fg(t.text),
        ),
    ])];
    // Show exactly what would run/be removed: transparency beats surprise.
    for preview in tg.dry_run_preview().into_iter().skip(1).take(3) {
        lines.push(Line::from(Span::styled(
            format!("  {}", preview.trim_start()),
            Style::default().fg(t.muted),
        )));
    }
    if tg.paths.len() > 3 {
        lines.push(Line::from(Span::styled(
            format!("  … and {} more paths", tg.paths.len() - 3),
            Style::default().fg(t.faint),
        )));
    }
    if tg.needs_sudo() {
        lines.push(Line::from(Span::styled(
            "  Runs via sudo — you'll be asked for your password outside the TUI.",
            Style::default().fg(risk_color(t, tg.risk)),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(titled_block(t, "What this does")),
        area,
    );
}
