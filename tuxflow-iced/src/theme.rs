//! TuxFlow's design system on iced — direction C, "Soft depth"
//! (chosen from the four-directions review, 2026-08-26).
//!
//! Each project is a floating card on a darker ground; the ACTIVE card is
//! lit by a corner-to-corner accent gradient. Ports, counters and statuses
//! are pills; the composer is a rounded field with a filled accent send.
//! The location rule survives from the GTK system: gold = remote,
//! green = local, and a project's accent tints its own interactions.
//! Status colors (crashed red, restarting amber, stopped gray) stay
//! semantic and fixed. The local/remote accents and the terminal scheme
//! follow settings.toml (core's shared palette data); the compiled
//! constants are the shipped defaults.

use std::sync::RwLock;

use iced::gradient::Linear;
use iced::widget::{button, container, scrollable, text_input};
use iced::{Background, Border, Color, Gradient, Radians, Shadow, Theme, Vector};
use tuxflow_core::config::palette;

// ── Surfaces ────────────────────────────────────────────────────────────
/// Sidebar ground the cards float on.
pub const BG_GROUND: Color = Color::from_rgb(0.071, 0.071, 0.090);
/// Project card.
pub const BG_CARD: Color = Color::from_rgb(0.090, 0.090, 0.114);
/// Window chrome: toolbar, status bar, composer bar.
pub const BG_CHROME: Color = Color::from_rgb(0.090, 0.090, 0.110);
/// Terminal surface — Catppuccin Mocha base, fixed.
pub const BG_TERMINAL: Color = Color::from_rgb(0.118, 0.118, 0.180);
/// Input field fill.
pub const BG_FIELD: Color = Color::from_rgb(0.114, 0.114, 0.141);
pub const HAIRLINE: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.05);

// ── Text ────────────────────────────────────────────────────────────────
pub const TEXT: Color = Color::from_rgb(0.925, 0.925, 0.945);
pub const TEXT_SECONDARY: Color = Color::from_rgb(0.651, 0.651, 0.690);
pub const DIM: Color = Color::from_rgb(0.561, 0.561, 0.604);

// ── Accents: where a project lives ──────────────────────────────────────
/// #73c991
pub const LOCAL_ACCENT: Color = Color::from_rgb(0.451, 0.788, 0.569);
/// #ffce5c — the logo gold.
pub const REMOTE_ACCENT: Color = Color::from_rgb(1.0, 0.808, 0.361);

// ── Status (semantic, fixed) ────────────────────────────────────────────
/// #f14c4c
pub const CRASHED: Color = Color::from_rgb(0.945, 0.298, 0.298);
/// #cca700
pub const RESTARTING: Color = Color::from_rgb(0.8, 0.655, 0.0);
/// Stopped dot on the card surface.
pub const STOPPED: Color = Color::from_rgb(0.290, 0.290, 0.322);

pub fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

/// The live local/remote accents, settable from settings. A process-wide
/// slot rather than a parameter because `accent_for` is called from every
/// view helper — threading two colors through ~40 call sites buys nothing.
/// Written once at boot and on a settings change, read on the view thread.
static ACCENTS: RwLock<(Color, Color)> = RwLock::new((LOCAL_ACCENT, REMOTE_ACCENT));

/// Resolve the two sidebar accents from their settings names (dark
/// variants — this shell's chrome is dark) and make them current.
pub fn set_accents(local_name: &str, remote_name: &str) {
    let local = palette::accent_by_name(local_name, palette::FALLBACK_LOCAL);
    let remote = palette::accent_by_name(remote_name, palette::FALLBACK_REMOTE);
    *ACCENTS.write().expect("accent slot") = (hex(local.accent), hex(remote.accent));
}

pub fn accent_for(remote: bool) -> Color {
    let (local_c, remote_c) = *ACCENTS.read().expect("accent slot");
    if remote { remote_c } else { local_c }
}

/// The lighter companion of each accent, for text on tinted surfaces —
/// the accent nudged toward white (matches the hand-tuned #ffe1a0 gold /
/// #b8e6c8 green pairs the defaults shipped with).
pub fn accent_soft(remote: bool) -> Color {
    let c = accent_for(remote);
    Color::from_rgb(
        c.r + (1.0 - c.r) * 0.5,
        c.g + (1.0 - c.g) * 0.5,
        c.b + (1.0 - c.b) * 0.5,
    )
}

pub fn hex(hex_str: &str) -> Color {
    let (r, g, b) = palette::hex_rgb(hex_str);
    Color::from_rgb(r, g, b)
}

/// Dark ink for text sitting ON a filled accent (the send button).
pub const ON_ACCENT: Color = Color::from_rgb(0.125, 0.102, 0.031);

pub fn bold() -> iced::Font {
    iced::Font {
        weight: iced::font::Weight::Bold,
        ..iced::Font::DEFAULT
    }
}

/// Flat card for settings groups — form_card without the floating shadow
/// (a settings page stacks several; shadows would stripe it).
pub fn settings_card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CARD)),
        border: Border {
            color: alpha(Color::WHITE, 0.06),
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}

/// A named terminal scheme from core's shared data, as iced_term colors.
pub fn terminal_palette(name: &str) -> iced_term::ColorPalette {
    let t = palette::terminal_theme(name);
    let p = |i: usize| t.palette[i].to_string();
    iced_term::ColorPalette {
        foreground: t.foreground.to_string(),
        background: t.background.to_string(),
        black: p(0),
        red: p(1),
        green: p(2),
        yellow: p(3),
        blue: p(4),
        magenta: p(5),
        cyan: p(6),
        white: p(7),
        bright_black: p(8),
        bright_red: p(9),
        bright_green: p(10),
        bright_yellow: p(11),
        bright_blue: p(12),
        bright_magenta: p(13),
        bright_cyan: p(14),
        bright_white: p(15),
        ..Default::default()
    }
}

// ── Containers ──────────────────────────────────────────────────────────

pub fn ground(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_GROUND)),
        ..Default::default()
    }
}

pub fn chrome(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CHROME)),
        ..Default::default()
    }
}

pub fn hairline(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(HAIRLINE)),
        ..Default::default()
    }
}

pub fn terminal_pane(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_TERMINAL)),
        ..Default::default()
    }
}

/// A project card. The ACTIVE one (owning the main pane) is lit by the
/// accent gradient running corner to corner and a tinted border.
pub fn project_card(accent: Color, active: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let background = if active {
            Background::Gradient(Gradient::Linear(
                Linear::new(Radians(2.356))
                    .add_stop(0.0, mix(BG_CARD, accent, 0.10))
                    .add_stop(0.55, mix(BG_CARD, accent, 0.02))
                    .add_stop(1.0, BG_CARD),
            ))
        } else {
            Background::Color(BG_CARD)
        };
        container::Style {
            background: Some(background),
            border: Border {
                color: if active {
                    alpha(accent, 0.22)
                } else {
                    alpha(Color::WHITE, 0.05)
                },
                width: 1.0,
                radius: 10.0.into(),
            },
            shadow: Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.25),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 8.0,
            },
            ..Default::default()
        }
    }
}

/// 26px rounded initials square.
pub fn icon_square(accent: Color, remote: bool) -> impl Fn(&Theme) -> container::Style {
    let bg = alpha(accent, if remote { 0.25 } else { 0.20 });
    let ink = accent_soft(remote);
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        text_color: Some(ink),
        ..Default::default()
    }
}

/// Neutral pill: counters, ports.
pub fn pill(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(alpha(Color::WHITE, 0.06))),
        border: Border {
            radius: 99.0.into(),
            ..Default::default()
        },
        text_color: Some(TEXT_SECONDARY),
        ..Default::default()
    }
}

/// Tinted status pill (toolbar): "● running" in the status's own color.
pub fn status_pill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(alpha(color, 0.15))),
        border: Border {
            radius: 99.0.into(),
            ..Default::default()
        },
        text_color: Some(Color {
            r: (color.r + 0.35).min(1.0),
            g: (color.g + 0.35).min(1.0),
            b: (color.b + 0.35).min(1.0),
            a: 1.0,
        }),
        ..Default::default()
    }
}

/// Overlay scrollbar with Adwaita manners: invisible until the pointer is
/// over the scrollable, then a thin floating scroller (no rail), a shade
/// stronger while grabbed. Pair with a narrow `scrollable::Scrollbar` at
/// the call site — this only paints, it doesn't size.
pub fn overlay_scrollbar(_: &Theme, status: scrollable::Status) -> scrollable::Style {
    let scroller = match status {
        scrollable::Status::Active { .. } => Color::TRANSPARENT,
        scrollable::Status::Hovered {
            is_vertical_scrollbar_hovered: true,
            ..
        }
        | scrollable::Status::Dragged { .. } => alpha(Color::WHITE, 0.45),
        scrollable::Status::Hovered { .. } => alpha(Color::WHITE, 0.22),
    };
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Background::Color(scroller),
            border: Border {
                radius: 99.0.into(),
                ..Default::default()
            },
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(BG_CARD),
            border: Border::default(),
            shadow: Shadow::default(),
            icon: TEXT_SECONDARY,
        },
    }
}

/// Centered form card.
pub fn form_card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CARD)),
        border: Border {
            color: alpha(Color::WHITE, 0.06),
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
            offset: Vector::new(0.0, 6.0),
            blur_radius: 24.0,
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

/// Process row inside a card: 8px radius, accent wash when selected.
pub fn process_row(
    accent: Color,
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let a = match (selected, hovered) {
            (true, true) => 0.18,
            (true, false) => 0.14,
            (false, true) => 0.08,
            (false, false) => 0.0,
        };
        let ink = if selected { TEXT } else { TEXT_SECONDARY };
        flat(alpha(accent, a), ink, 8.0)
    }
}

/// Project card header row: transparent, soft accent tint on hover.
pub fn project_row(accent: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        flat(alpha(accent, if hovered { 0.08 } else { 0.0 }), TEXT, 8.0)
    }
}

/// Neutral pill button: toolbar chips, "+ project", cancel.
pub fn pill_button(accent: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match status {
        button::Status::Hovered => flat(alpha(accent, 0.14), TEXT, 99.0),
        button::Status::Pressed => flat(alpha(accent, 0.22), TEXT, 99.0),
        button::Status::Disabled => flat(alpha(Color::WHITE, 0.03), DIM, 99.0),
        _ => flat(alpha(Color::WHITE, 0.05), TEXT_SECONDARY, 99.0),
    }
}

/// Pill button whose label carries intent color (stop reds on hover).
pub fn pill_intent(
    accent: Color,
    glyph: Color,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match status {
        button::Status::Hovered => flat(alpha(accent, 0.14), glyph, 99.0),
        button::Status::Pressed => flat(alpha(accent, 0.22), glyph, 99.0),
        _ => flat(alpha(Color::WHITE, 0.05), TEXT_SECONDARY, 99.0),
    }
}

/// The filled accent action: send / start / open.
pub fn primary(accent: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match status {
        button::Status::Hovered => flat(
            Color {
                r: (accent.r + 0.06).min(1.0),
                g: (accent.g + 0.06).min(1.0),
                b: (accent.b + 0.06).min(1.0),
                a: 1.0,
            },
            ON_ACCENT,
            99.0,
        ),
        button::Status::Pressed => flat(alpha(accent, 0.85), ON_ACCENT, 99.0),
        _ => flat(accent, ON_ACCENT, 99.0),
    }
}

/// Quiet close/utility glyph (the card ✕): nearly invisible until hover.
pub fn ghost(glyph_hover: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match status {
        button::Status::Hovered => flat(alpha(Color::WHITE, 0.06), glyph_hover, 99.0),
        button::Status::Pressed => flat(alpha(Color::WHITE, 0.10), glyph_hover, 99.0),
        _ => flat(Color::TRANSPARENT, DIM, 99.0),
    }
}

// ── Inputs ──────────────────────────────────────────────────────────────

/// Rounded field; border picks up the accent when focused.
pub fn input(accent: Color) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| {
        let focused = matches!(status, text_input::Status::Focused { .. });
        text_input::Style {
            background: Background::Color(BG_FIELD),
            border: Border {
                color: if focused {
                    alpha(accent, 0.55)
                } else {
                    alpha(Color::WHITE, 0.08)
                },
                width: 1.0,
                radius: 99.0.into(),
            },
            icon: TEXT_SECONDARY,
            placeholder: DIM,
            value: TEXT,
            selection: alpha(accent, 0.35),
        }
    }
}

/// Blend `base` toward `tint`.
fn mix(base: Color, tint: Color, t: f32) -> Color {
    Color {
        r: base.r + (tint.r - base.r) * t,
        g: base.g + (tint.g - base.g) * t,
        b: base.b + (tint.b - base.b) * t,
        a: 1.0,
    }
}
