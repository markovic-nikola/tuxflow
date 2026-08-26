//! TuxFlow's design system on iced — ported from the approved GTK
//! stylesheet (data/style.css + ui/accent.rs), not reinvented.
//!
//! The identity: **the accent is location**. Local projects are green,
//! remote projects are logo gold, and a project's accent tints every
//! interaction it owns — the 2px rail, row hovers, the selected row, the
//! running dot, the running project's name. There are no gray hovers
//! anywhere. Status colors (crashed red, restarting amber, stopped gray)
//! are semantic and fixed.

use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Theme};

// ── Surfaces ────────────────────────────────────────────────────────────
/// Window chrome: toolbar, status bar, composer bar.
pub const BG_CHROME: Color = Color::from_rgb(0.137, 0.137, 0.149);
/// Sidebar sits a step darker than the chrome.
pub const BG_SIDEBAR: Color = Color::from_rgb(0.106, 0.106, 0.118);
/// Terminal surface — Catppuccin Mocha base, fixed.
pub const BG_TERMINAL: Color = Color::from_rgb(0.118, 0.118, 0.180);
pub const BORDER: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.12);

// ── Text ────────────────────────────────────────────────────────────────
pub const TEXT: Color = Color::from_rgb(0.91, 0.91, 0.925);
pub const TEXT_SECONDARY: Color = Color::from_rgb(0.66, 0.66, 0.69);
pub const DIM: Color = Color::from_rgb(0.42, 0.42, 0.455);

// ── Accents: where a project lives (style.css @define-color) ───────────
/// #73c991
pub const LOCAL_ACCENT: Color = Color::from_rgb(0.451, 0.788, 0.569);
/// #ffce5c — the logo gold.
pub const REMOTE_ACCENT: Color = Color::from_rgb(1.0, 0.808, 0.361);

// ── Status (semantic, fixed) ────────────────────────────────────────────
/// #f14c4c
pub const CRASHED: Color = Color::from_rgb(0.945, 0.298, 0.298);
/// #cca700
pub const RESTARTING: Color = Color::from_rgb(0.8, 0.655, 0.0);
/// #6c6c6c
pub const STOPPED: Color = Color::from_rgb(0.424, 0.424, 0.424);

pub fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

pub fn accent_for(remote: bool) -> Color {
    if remote { REMOTE_ACCENT } else { LOCAL_ACCENT }
}

/// Catppuccin Mocha — the GTK default terminal scheme, field for field.
pub fn terminal_palette() -> iced_term::ColorPalette {
    iced_term::ColorPalette {
        foreground: String::from("#CDD6F4"),
        background: String::from("#1E1E2E"),
        black: String::from("#45475A"),
        red: String::from("#F38BA8"),
        green: String::from("#A6E3A1"),
        yellow: String::from("#F9E2AF"),
        blue: String::from("#89B4FA"),
        magenta: String::from("#F5C2E7"),
        cyan: String::from("#94E2D5"),
        white: String::from("#BAC2DE"),
        bright_black: String::from("#585B70"),
        bright_red: String::from("#F38BA8"),
        bright_green: String::from("#A6E3A1"),
        bright_yellow: String::from("#F9E2AF"),
        bright_blue: String::from("#89B4FA"),
        bright_magenta: String::from("#F5C2E7"),
        bright_cyan: String::from("#94E2D5"),
        bright_white: String::from("#A6ADC8"),
        ..Default::default()
    }
}

// ── Containers ──────────────────────────────────────────────────────────

pub fn sidebar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_SIDEBAR)),
        ..Default::default()
    }
}

/// Toolbar / status bar / composer bar: chrome surface with a hairline on
/// the given edge (style.css draws them with 1px alpha(@borders,.3)).
pub fn chrome(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CHROME)),
        ..Default::default()
    }
}

pub fn hairline(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(alpha(BORDER, 0.3))),
        ..Default::default()
    }
}

pub fn terminal_pane(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_TERMINAL)),
        ..Default::default()
    }
}

/// The project's 2px left rail (.project-container border-left):
/// transparent when collapsed+idle, a whisper when expanded, full accent
/// while something inside runs. Remote idles at 0.28 — location shows
/// even when nothing runs.
pub fn rail(accent: Color, running: bool, expanded: bool, remote: bool) -> container::Style {
    let color = if running {
        alpha(accent, if remote { 0.75 } else { 1.0 })
    } else if remote {
        alpha(accent, 0.28)
    } else if expanded {
        alpha(accent, 0.2)
    } else {
        Color::TRANSPARENT
    };
    container::Style {
        background: Some(Background::Color(color)),
        ..Default::default()
    }
}

/// 24px rounded initials square (.project-icon).
pub fn icon_square(accent: Color, remote: bool) -> impl Fn(&Theme) -> container::Style {
    let bg = alpha(accent, if remote { 0.32 } else { 0.2 });
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        border: Border {
            radius: 5.0.into(),
            ..Default::default()
        },
        text_color: Some(TEXT),
        ..Default::default()
    }
}

/// Centered form card (.project-detail-card).
pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(alpha(BORDER, 0.08))),
        border: Border {
            color: alpha(BORDER, 0.15),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

// ── Buttons ─────────────────────────────────────────────────────────────

fn flat(bg: Color, text_color: Color, radius: f32) -> button::Style {
    button::Style {
        background: Some(Background::Color(bg)),
        text_color,
        border: Border {
            radius: radius.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Process row (.process-row): 6px radius, accent-alpha hover 0.08,
/// selected 0.15, selected+hover 0.20.
pub fn process_row(
    accent: Color,
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let a = match (selected, hovered) {
            (true, true) => 0.20,
            (true, false) => 0.15,
            (false, true) => 0.08,
            (false, false) => 0.0,
        };
        flat(alpha(accent, a), TEXT, 6.0)
    }
}

/// Project header row (.project-row): full-width, accent hover 0.10/0.12.
pub fn project_row(
    accent: Color,
    remote: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let a = match (hovered, remote) {
            (true, true) => 0.12,
            (true, false) => 0.10,
            _ => 0.0,
        };
        flat(alpha(accent, a), TEXT, 6.0)
    }
}

/// Quiet chip (.status-chip): transparent at rest, soft accent tint on
/// hover, a bit stronger pressed. `glyph` colors the label; pass a status
/// color for play/stop-style intent (btn-play/btn-stop hovers).
pub fn chip(accent: Color, glyph: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match status {
        button::Status::Hovered => flat(alpha(accent, 0.12), glyph, 6.0),
        button::Status::Pressed => flat(alpha(accent, 0.2), glyph, 6.0),
        button::Status::Disabled => flat(Color::TRANSPARENT, alpha(glyph, 0.4), 6.0),
        _ => flat(Color::TRANSPARENT, TEXT_SECONDARY, 6.0),
    }
}

/// The one filled button per view (send / add / open): accent-tinted rest,
/// stronger hover (composer send chip language).
pub fn primary_chip(accent: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match status {
        button::Status::Hovered => flat(alpha(accent, 0.28), TEXT, 6.0),
        button::Status::Pressed => flat(alpha(accent, 0.38), TEXT, 6.0),
        _ => flat(alpha(accent, 0.12), TEXT, 6.0),
    }
}

// ── Inputs ──────────────────────────────────────────────────────────────

/// Composer / form field (.composer-field): quiet filled field, 8px
/// radius, 1px border that picks up the accent when focused.
pub fn input(accent: Color) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| {
        let focused = matches!(status, text_input::Status::Focused { .. });
        text_input::Style {
            background: Background::Color(alpha(BORDER, 0.12)),
            border: Border {
                color: if focused {
                    alpha(accent, 0.6)
                } else {
                    alpha(BORDER, 0.45)
                },
                width: 1.0,
                radius: 8.0.into(),
            },
            icon: TEXT_SECONDARY,
            placeholder: DIM,
            value: TEXT,
            selection: alpha(accent, 0.35),
        }
    }
}
