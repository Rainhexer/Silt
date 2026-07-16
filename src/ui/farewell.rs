//! The parting shot: printed to the real terminal once the TUI has restored
//! and control is about to return to the shell. Sediment/river motif to
//! match the rest of Silt's identity — what got "washed away" this session.

use std::collections::HashMap;
use std::time::Duration;

use crossterm::style::{Color, Stylize};

use crate::app::App;
use crate::targets::Category;
use crate::ui::human;

const CATEGORY_ORDER: [Category; 7] = [
    Category::PackageManager,
    Category::Packages,
    Category::UserCache,
    Category::SystemLogs,
    Category::Containers,
    Category::Trash,
    Category::Marked,
];

fn rgb(c: ratatui::style::Color) -> Color {
    match c {
        ratatui::style::Color::Rgb(r, g, b) => Color::Rgb { r, g, b },
        _ => Color::Reset,
    }
}

/// Prints the closing screen. Called after `ratatui::restore()`, so this is
/// plain scrollback output — no alternate screen, no raw mode.
pub fn print(app: &App) {
    let t = &app.theme;
    let accent = rgb(t.accent);
    let dir = rgb(t.dir);
    let muted = rgb(t.muted);
    let faint = rgb(t.faint);
    let text = rgb(t.text);

    let mut by_category: HashMap<Category, u64> = HashMap::new();
    for (cat, _, bytes) in &app.session_freed {
        *by_category.entry(*cat).or_insert(0) += bytes;
    }
    let max_cat = by_category.values().copied().max().unwrap_or(0).max(1);

    println!();
    println!("     {}", "▁▂▃▄▅▆▇█▇▆▅▄▃▂▁▂▃▄▅▆▇█▇▆▅▄▃▂▁".with(faint));
    println!(
        "     {}{}{}",
        "░▒▓ ".with(accent),
        "s i l t".with(accent).bold(),
        " ▓▒░  — the sediment has settled".with(muted)
    );
    println!("     {}", "▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔".with(faint));
    println!();

    if app.session_freed.is_empty() {
        println!("     {}", "no sediment washed away this time.".with(muted));
        println!(
            "     {}",
            "riverbed's just as you found it.".with(faint)
        );
    } else {
        println!(
            "     {}   {}",
            "reclaimed".with(muted),
            human(app.session_reclaimed).with(accent).bold()
        );
        println!(
            "     {}   {}",
            "targets cleared".with(muted),
            app.session_freed.len().to_string().with(text)
        );
        println!();
        for cat in CATEGORY_ORDER {
            let bytes = by_category.get(&cat).copied().unwrap_or(0);
            if bytes == 0 {
                continue;
            }
            let frac = bytes as f64 / max_cat as f64;
            let name = format!("{:<16}", cat.to_string());
            println!(
                "     {} {}  {}",
                name.with(text),
                flow_bar(frac, 20).with(dir),
                human(bytes).with(muted)
            );
        }
    }

    println!();
    println!(
        "     {}",
        format!("~ {} well spent ~", fmt_duration(app.started.elapsed())).with(faint)
    );
    println!();
    println!("     {}", "goodbye.".with(text).bold());
    println!();
}

/// A "current" bar: flowing water where sediment washed clear, still water
/// where nothing moved.
fn flow_bar(fraction: f64, width: usize) -> String {
    let filled = (fraction.clamp(0.0, 1.0) * width as f64).round() as usize;
    let mut s = String::with_capacity(width);
    for i in 0..width {
        s.push(if i < filled { '≈' } else { '·' });
    }
    s
}

fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}
