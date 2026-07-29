//! Log tab: cleanup plans and execution log.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;
use crate::ui::theme::Theme;

use super::{spinner, titled_block};

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let title = if app.cleanup_running {
        format!("Log {} cleanup running…", spinner(app))
    } else {
        format!("Log — {} lines", app.log.len())
    };
    let block = titled_block(t, &title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible = inner.height as usize;
    // Show a window ending at (or scrolled to) the newest lines.
    let start = app
        .log_scroll
        .min(app.log.len().saturating_sub(1))
        .saturating_sub(visible.saturating_sub(1));

    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(start)
        .take(visible)
        .map(|l| style_line(t, l))
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

fn style_line<'a>(t: &Theme, line: &'a str) -> Line<'a> {
    let style = if line.starts_with("ERROR") || line.contains("ERROR:") {
        Style::default().fg(t.danger).add_modifier(Modifier::BOLD)
    } else if line.starts_with("WARNING") {
        Style::default().fg(t.warn).add_modifier(Modifier::BOLD)
    } else if line.starts_with("===") || line.starts_with("---") {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else if line.starts_with(">>") {
        Style::default().fg(t.text).add_modifier(Modifier::BOLD)
    } else if line.starts_with('✦') {
        Style::default().fg(t.safe).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.muted)
    };
    Line::from(Span::styled(line, style))
}
