//! Silt palettes. The default `dark`/`light` themes keep the house rule — the
//! terminal's own background is the canvas, styled with foreground color and
//! weight only. The retro themes are a deliberate departure: a 16-bit machine's
//! identity *is* its screen color, so those paint a full-screen `bg`. Amber
//! carries identity, teal/light marks directories, risk colors stay reserved
//! for risk.

use ratatui::style::Color;

/// Theme cycle order for the `t` / `T` switch. First entry is the default.
pub const ORDER: [&str; 8] = [
    "dark", "light", "c64", "amber", "green", "cga", "spectrum", "amiga",
];

/// Human label shown in the header badge and status line on a switch.
pub fn label(name: &str) -> &'static str {
    match name {
        "light" => "Daylight",
        "c64" => "Commodore 64",
        "amber" => "Amber CRT",
        "green" => "Green Phosphor",
        "cga" => "IBM CGA",
        "spectrum" => "ZX Spectrum",
        "amiga" => "Amiga Workbench",
        _ => "Silt Dark",
    }
}

/// Index of `name` in [`ORDER`], falling back to 0 (dark) for anything unknown.
pub fn index_of(name: &str) -> usize {
    ORDER.iter().position(|n| *n == name).unwrap_or(0)
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Identity color: active tab, selection background, primary keys.
    pub accent: Color,
    /// Dimmer accent for bars and secondary emphasis.
    pub accent_dim: Color,
    /// Text drawn on top of an accent-colored selection block.
    pub on_accent: Color,
    /// Directories and drill-down affordances.
    pub dir: Color,
    /// Primary text.
    pub text: Color,
    /// Secondary text: sizes, categories.
    pub muted: Color,
    /// Dimmest legible tone: separators, hints, empty bar track.
    pub faint: Color,
    pub safe: Color,
    pub warn: Color,
    pub danger: Color,
    /// Full-screen background. `None` = the terminal is the canvas (dark/light);
    /// `Some` = a 16-bit machine's signature screen color, painted edge to edge.
    pub bg: Option<Color>,
}

impl Theme {
    pub fn from_name(name: &str) -> Theme {
        match name {
            "light" => Theme::light(),
            "c64" => Theme::c64(),
            "amber" => Theme::amber(),
            "green" => Theme::green(),
            "cga" => Theme::cga(),
            "spectrum" => Theme::spectrum(),
            "amiga" => Theme::amiga(),
            _ => Theme::dark(),
        }
    }

    pub fn dark() -> Theme {
        Theme {
            accent: Color::Rgb(230, 179, 102),
            accent_dim: Color::Rgb(176, 134, 72),
            on_accent: Color::Rgb(28, 24, 18),
            dir: Color::Rgb(102, 204, 187),
            text: Color::Rgb(224, 218, 206),
            muted: Color::Rgb(158, 152, 140),
            faint: Color::Rgb(108, 104, 96),
            safe: Color::Rgb(137, 201, 119),
            warn: Color::Rgb(232, 196, 104),
            danger: Color::Rgb(233, 109, 99),
            bg: None,
        }
    }

    pub fn light() -> Theme {
        Theme {
            accent: Color::Rgb(158, 106, 18),
            accent_dim: Color::Rgb(186, 140, 60),
            on_accent: Color::Rgb(255, 250, 240),
            dir: Color::Rgb(16, 122, 108),
            text: Color::Rgb(56, 50, 42),
            muted: Color::Rgb(122, 114, 102),
            faint: Color::Rgb(168, 160, 148),
            safe: Color::Rgb(52, 130, 40),
            warn: Color::Rgb(158, 120, 10),
            danger: Color::Rgb(178, 52, 44),
            bg: None,
        }
    }

    /// Commodore 64: light-blue-on-dark-blue screen, chrome yellow for identity,
    /// cyan for directories. Text lifted brighter than the real machine's for
    /// legibility on a modern display.
    pub fn c64() -> Theme {
        Theme {
            accent: Color::Rgb(191, 206, 114),
            accent_dim: Color::Rgb(150, 162, 90),
            on_accent: Color::Rgb(40, 32, 120),
            dir: Color::Rgb(140, 214, 220),
            text: Color::Rgb(174, 164, 240),
            muted: Color::Rgb(140, 130, 205),
            faint: Color::Rgb(96, 85, 170),
            safe: Color::Rgb(148, 224, 137),
            warn: Color::Rgb(214, 214, 120),
            danger: Color::Rgb(214, 128, 118),
            bg: Some(Color::Rgb(48, 38, 130)),
        }
    }

    /// Amber monochrome CRT. One phosphor, so directories read as brighter
    /// amber; risk tiers get the smallest hue nudge needed to stay separable.
    pub fn amber() -> Theme {
        Theme {
            accent: Color::Rgb(255, 183, 77),
            accent_dim: Color::Rgb(200, 140, 50),
            on_accent: Color::Rgb(26, 16, 0),
            dir: Color::Rgb(255, 214, 148),
            text: Color::Rgb(240, 172, 66),
            muted: Color::Rgb(182, 126, 44),
            faint: Color::Rgb(112, 76, 22),
            safe: Color::Rgb(196, 208, 96),
            warn: Color::Rgb(255, 200, 80),
            danger: Color::Rgb(255, 124, 72),
            bg: Some(Color::Rgb(24, 15, 2)),
        }
    }

    /// P1 green phosphor CRT. Same monochrome discipline as amber.
    pub fn green() -> Theme {
        Theme {
            accent: Color::Rgb(122, 255, 142),
            accent_dim: Color::Rgb(72, 182, 92),
            on_accent: Color::Rgb(0, 22, 6),
            dir: Color::Rgb(178, 255, 190),
            text: Color::Rgb(104, 226, 124),
            muted: Color::Rgb(74, 162, 94),
            faint: Color::Rgb(46, 104, 60),
            safe: Color::Rgb(150, 255, 160),
            warn: Color::Rgb(208, 255, 118),
            danger: Color::Rgb(255, 148, 118),
            bg: Some(Color::Rgb(0, 18, 6)),
        }
    }

    /// IBM CGA high-intensity mode 1: cyan + magenta + white on black. Magenta
    /// is identity; cyan is directories.
    pub fn cga() -> Theme {
        Theme {
            accent: Color::Rgb(255, 90, 255),
            accent_dim: Color::Rgb(198, 70, 198),
            on_accent: Color::Rgb(12, 4, 16),
            dir: Color::Rgb(90, 240, 250),
            text: Color::Rgb(236, 236, 244),
            muted: Color::Rgb(122, 196, 214),
            faint: Color::Rgb(96, 104, 138),
            safe: Color::Rgb(96, 240, 180),
            warn: Color::Rgb(250, 246, 120),
            danger: Color::Rgb(255, 118, 150),
            bg: Some(Color::Rgb(8, 4, 14)),
        }
    }

    /// ZX Spectrum, BRIGHT set on black paper. Yellow identity, cyan dirs.
    pub fn spectrum() -> Theme {
        Theme {
            accent: Color::Rgb(255, 238, 88),
            accent_dim: Color::Rgb(198, 184, 60),
            on_accent: Color::Rgb(12, 10, 18),
            dir: Color::Rgb(84, 236, 250),
            text: Color::Rgb(234, 236, 246),
            muted: Color::Rgb(152, 162, 214),
            faint: Color::Rgb(92, 100, 146),
            safe: Color::Rgb(96, 232, 118),
            warn: Color::Rgb(255, 220, 82),
            danger: Color::Rgb(255, 104, 116),
            bg: Some(Color::Rgb(12, 10, 20)),
        }
    }

    /// Amiga Workbench 1.x: orange on that unmistakable blue desktop, white text.
    pub fn amiga() -> Theme {
        Theme {
            accent: Color::Rgb(255, 138, 0),
            accent_dim: Color::Rgb(202, 108, 0),
            on_accent: Color::Rgb(0, 34, 82),
            dir: Color::Rgb(150, 214, 255),
            text: Color::Rgb(238, 244, 255),
            muted: Color::Rgb(168, 190, 228),
            faint: Color::Rgb(88, 122, 176),
            safe: Color::Rgb(122, 222, 142),
            warn: Color::Rgb(255, 192, 74),
            danger: Color::Rgb(255, 122, 112),
            bg: Some(Color::Rgb(0, 66, 150)),
        }
    }

    /// Amber ramp for usage bars: blend faint → accent by `t` (0..=1).
    pub fn bar_color(&self, t: f64) -> Color {
        let t = t.clamp(0.0, 1.0);
        let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (self.faint, self.accent) else {
            return self.accent_dim;
        };
        let lerp = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
        Color::Rgb(lerp(r1, r2), lerp(g1, g2), lerp(b1, b2))
    }
}
