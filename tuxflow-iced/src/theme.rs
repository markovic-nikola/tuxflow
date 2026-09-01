//! TuxFlow's design system on iced — direction C, "Soft depth"
//! (chosen from the four-directions review, 2026-08-26).
//!
//! Each project is a floating card on a darker ground; the ACTIVE card is
//! lit by a corner-to-corner accent gradient, and a card with something
//! RUNNING in it wears an accent border ring. Ports, counters and statuses
//! are pills; the composer is a rounded field with a filled accent send.
//! The location rule survives from the GTK system: gold = remote,
//! green = local, and a project's accent tints its own interactions.
//! Status colors (crashed red, restarting amber, stopped gray) stay
//! semantic and fixed. The local/remote accents and the terminal scheme
//! follow settings.toml (core's shared palette data); the compiled
//! constants are the shipped defaults.

use std::sync::RwLock;

use iced::gradient::Linear;
use iced::widget::{button, container, scrollable, text_editor, text_input};
use iced::{Background, Border, Color, Gradient, Radians, Shadow, Theme, Vector};
use tuxflow_core::config::palette;

// ── Surfaces ────────────────────────────────────────────────────────────
/// Sidebar ground the cards float on.
pub const BG_GROUND: Color = Color::from_rgb(0.071, 0.071, 0.090);
/// Project card.
pub const BG_CARD: Color = Color::from_rgb(0.090, 0.090, 0.114);
/// Window chrome: toolbar, status bar, composer bar.
pub const BG_CHROME: Color = Color::from_rgb(0.090, 0.090, 0.110);
/// Main-pane ground: full-pane views (settings, git, add forms) and the
/// placeholder states. A design surface of the dark shell — NOT the
/// terminal's background, which follows the user's terminal scheme via
/// `terminal_pane` (GTK's dialogs sit on the window background, never the
/// VTE palette).
pub const BG_PANE: Color = Color::from_rgb(0.118, 0.118, 0.180);
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

// ── Git (semantic, fixed — matches style.css so the shells agree) ───────
/// #73c991 — insertions, and commits waiting to be pushed. Same hue as
/// LOCAL_ACCENT by coincidence of the palette, not by reference: the
/// accent is user-settable and this must not move with it.
pub const GIT_ADDED: Color = Color::from_rgb(0.451, 0.788, 0.569);
/// #f14c4c — deletions.
pub const GIT_REMOVED: Color = Color::from_rgb(0.945, 0.298, 0.298);
/// #d29922 — commits waiting to be pulled. Amber reads as "incoming,
/// not yours yet" against the green of what you already have.
pub const GIT_BEHIND: Color = Color::from_rgb(0.824, 0.600, 0.133);
/// #dcdcaa — a modified file's badge in the changes list.
pub const GIT_MODIFIED: Color = Color::from_rgb(0.863, 0.863, 0.667);
/// #6c6c6c — an untracked file's badge: present, but not git's business yet.
pub const GIT_UNTRACKED: Color = Color::from_rgb(0.424, 0.424, 0.424);

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

pub fn pane(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_PANE)),
        ..Default::default()
    }
}

/// The pane behind the terminal, in the ACTIVE terminal scheme's own
/// background — looked up from core's shared table, never a constant.
/// The fork paints no default-background cells (view.rs batches only cells
/// whose background differs, "container already paints it"), so this
/// container IS the terminal's background: a hardcoded twin drifts the
/// moment the user picks another scheme, and any full-screen program that
/// paints its own background (BCE fills the grid, never the container)
/// gets framed in the stale color.
pub fn terminal_pane(scheme: &str) -> impl Fn(&Theme) -> container::Style {
    let (r, g, b) = palette::hex_rgb(palette::terminal_theme(scheme).background);
    let bg = Color::from_rgb(r, g, b);
    move |_: &Theme| container::Style {
        background: Some(Background::Color(bg)),
        ..Default::default()
    }
}

/// The tint behind one added or removed line of a diff, carried by the
/// row's container so it reaches the full width of the pane. A span's
/// `background` paints behind its glyphs only, which is what left every
/// band with a ragged right edge.
///
/// Removed sits lower than added on purpose: it is already the dimmer
/// half (see `DEL_ALPHA` in git_view), and red reads heavier than green
/// at equal alpha.
pub fn diff_band(color: Color) -> impl Fn(&Theme) -> container::Style {
    let a = if color == GIT_REMOVED { 0.11 } else { 0.13 };
    move |_| container::Style {
        background: Some(Background::Color(alpha(color, a))),
        ..Default::default()
    }
}

/// The angle every card gradient runs at: 135°, so offset 0 sits in the
/// top-left corner and offset 1 in the bottom-right one.
const CARD_DIAGONAL: Radians = Radians(2.356);

/// The ACTIVE card's static wash, as (offset, accent strength) along the
/// diagonal: lit at the top-left corner, fading out by the far one.
const CARD_WASH: [(f32, f32); 3] = [(0.0, 0.10), (0.55, 0.02), (1.0, 0.0)];

/// Peak accent strength of the working-agent band, on top of the wash.
/// A quarter of what it shipped at first (Nikola, off the calibration
/// bench and then a notch lower again): on the ACTIVE card the band lands
/// on a corner the wash has already lit, and 0.10 + 0.10 put a bright
/// field under the row labels — legible at 9:1, but it pulled the eye off
/// them, which is the whole complaint. The ring carries the motion now,
/// so the band only has to keep the surface from feeling static.
const SWEEP_PEAK: f32 = 0.025;
/// Half-width of the band as a fraction of the diagonal. Wide and soft —
/// this is a slow breath across a sidebar card, not a loading skeleton.
const SWEEP_HALF: f32 = 0.32;

/// Ring alpha with nothing sweeping — GTK's `.project-has-running` edge.
const RING_REST: f32 = 0.35;
/// …and the range it breathes through while an agent works. This is the
/// half of the signal that never crosses a row, which is why the band
/// could go as quiet as it did: the ring carries the motion, the band
/// only tints the surface it passes.
const RING_MIN: f32 = 0.22;
const RING_MAX: f32 = 0.60;

/// A project card. Two orthogonal signals, as in GTK, where they are two
/// unrelated CSS classes: an accent border RING says something inside is
/// running (`.project-has-running`, which lights the container's left
/// border to full accent and its title), and the corner-to-corner accent
/// gradient says this is the ACTIVE project owning the main pane
/// (`.project-active`, a background wash). GTK's ring is a 2px left edge;
/// iced borders are all-or-nothing per side, so it goes around the whole
/// card at a lower alpha for the same weight of ink.
///
/// `sweep` is the third: a 0..1 phase while an agent inside is producing
/// output. It drives BOTH a band of accent light travelling the same
/// diagonal and a breath in the ring, in step. Splitting the signal across
/// the two is what let the band drop to a whisper: a bright thing sliding
/// under a row label reads as interference however legible it measures, so
/// the carrying motion sits on the border, where there is no text.
///
/// The two halves are gated differently, and asymmetrically on purpose
/// (Nikola): the RING breathes on every card with a working agent, because
/// "something over there is busy" is exactly what a background card needs
/// to be able to say. The BAND rides the ACTIVE card only — it is a
/// modulation of the active wash, and on a background card, where there is
/// no wash under it, a lone band sliding across a dark card is the only
/// motion in an otherwise still sidebar, so it pulls the eye to the card
/// you are not looking at.
pub fn project_card(
    accent: Color,
    running: bool,
    active: bool,
    sweep: Option<f32>,
) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let background = match card_gradient(accent, active, sweep) {
            Some(gradient) => Background::Gradient(gradient),
            None => Background::Color(BG_CARD),
        };
        container::Style {
            background: Some(background),
            border: Border {
                color: if running {
                    alpha(accent, ring_alpha(sweep))
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

/// The ring's alpha at `sweep`: resting when nothing is working, else a
/// cosine breath in step with the band — dimmest as the band waits off the
/// top-left corner, fullest as it crosses the middle. Cosine rather than
/// the band's own triangle because the ring has no position to give it
/// away, only brightness, and a linear ramp reads as a flicker at the
/// turn. Periodic, so the phase wrapping 1 → 0 costs nothing; the one
/// seam is the pass where work ENDS, which leaves the ring at RING_MIN and
/// steps it back to rest — 0.13 alpha on a 1px border, once per finished
/// run, against a trough deep enough to read as breathing rather than
/// blinking. That trade was the point of dipping below rest at all.
fn ring_alpha(sweep: Option<f32>) -> f32 {
    match sweep {
        Some(phase) => {
            let breath = 0.5 - (phase * std::f32::consts::TAU).cos() / 2.0;
            RING_MIN + (RING_MAX - RING_MIN) * breath
        }
        None => RING_REST,
    }
}

/// Accent strength of the static wash at `t` along the diagonal.
fn wash_at(t: f32) -> f32 {
    let mut prev = CARD_WASH[0];
    for &stop in &CARD_WASH[1..] {
        if t <= stop.0 {
            let span = stop.0 - prev.0;
            let f = if span > 0.0 {
                ((t - prev.0) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return prev.1 + (stop.1 - prev.1) * f;
        }
        prev = stop;
    }
    prev.1
}

/// Where the band's peak sits at `phase`. It starts and ends a full
/// half-width OUTSIDE the card, so the pass loops into the next one with
/// nothing lit at the seam — no wrap-around jump to hide.
fn band_center(phase: f32) -> f32 {
    -SWEEP_HALF + phase * (1.0 + 2.0 * SWEEP_HALF)
}

/// Accent strength the band adds at `t`. A triangle rather than a cosine:
/// at these strengths over a near-black card the profiles are
/// indistinguishable, and a triangle needs three stops instead of five.
fn band_at(t: f32, center: f32) -> f32 {
    SWEEP_PEAK * (1.0 - (t - center).abs() / SWEEP_HALF).max(0.0)
}

/// The card's background gradient, or `None` when nothing lights it and a
/// flat fill will do. Only the active card is ever lit: the band is a
/// modulation of the wash, not a signal of its own (see [`project_card`]),
/// so a background card stays flat however busy it is and says so with its
/// ring instead.
fn card_gradient(accent: Color, active: bool, sweep: Option<f32>) -> Option<Gradient> {
    if !active {
        return None;
    }
    let center = sweep.map(band_center);
    // Stops go where the intensity changes slope: the wash's own
    // breakpoints, plus the band's leading edge, peak and trailing edge.
    // Six at most, comfortably inside iced's cap of eight.
    let mut offsets: Vec<f32> = CARD_WASH.iter().map(|(offset, _)| *offset).collect();
    if let Some(c) = center {
        offsets.extend(
            [c - SWEEP_HALF, c, c + SWEEP_HALF]
                .into_iter()
                .filter(|t| (0.0..=1.0).contains(t)),
        );
    }
    // Ascending, because `add_stop` writes at the sorted index WITHOUT
    // shifting what is already there: a stop added out of order silently
    // overwrites its neighbour.
    offsets.sort_by(|a, b| a.partial_cmp(b).expect("finite offsets"));
    offsets.dedup_by(|a, b| (*a - *b).abs() < 1e-3);

    let mut gradient = Linear::new(CARD_DIAGONAL);
    for t in offsets {
        let amount = wash_at(t) + center.map_or(0.0, |c| band_at(t, c));
        gradient = gradient.add_stop(t, mix(BG_CARD, accent, amount));
    }
    Some(Gradient::Linear(gradient))
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

/// The Ctrl+N keycap revealed on a sidebar row while Ctrl is held. Squared
/// off against [`pill`]'s full round on purpose: the port pill sits in the
/// same strip, and shape is what separates them at 9px where neither is
/// really readable as a word.
///
/// A real keycap's weighted bottom edge cannot be a border — iced border
/// widths are all-or-nothing across the four sides — so the lip is an
/// unblurred shadow offset a single pixel down, which renders as exactly
/// that edge and nothing else.
pub fn keycap(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(alpha(Color::WHITE, 0.055))),
        border: Border {
            color: alpha(Color::WHITE, 0.12),
            width: 1.0,
            radius: 3.0.into(),
        },
        shadow: Shadow {
            color: alpha(Color::BLACK, 0.5),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 0.0,
        },
        text_color: Some(TEXT_SECONDARY),
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

/// A pickable card — the add-project flow's Local/Remote fork.
///
/// Unlike `process_row`, this one is drawn at rest: it is the only thing on
/// the pane and has to read as a target rather than as a paragraph, so it
/// carries the settings card's surface plus an accent edge that lights on
/// hover.
pub fn choice_card(accent: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(match hovered {
                true => alpha(accent, 0.10),
                false => BG_CARD,
            })),
            text_color: TEXT,
            border: Border {
                radius: 10.0.into(),
                width: 1.0,
                color: alpha(accent, if hovered { 0.55 } else { 0.18 }),
            },
            ..Default::default()
        }
    }
}

/// Switch rows, in the accent rather than iced's default blue — the shells
/// share one accent and a stray blue is the only thing in the window wearing
/// a colour nobody chose.
pub fn toggler(
    accent: Color,
) -> impl Fn(&Theme, iced::widget::toggler::Status) -> iced::widget::toggler::Style {
    use iced::widget::toggler::{Status, default};
    move |theme, status| {
        let (on, hovered) = match status {
            Status::Active { is_toggled } => (is_toggled, false),
            Status::Hovered { is_toggled } => (is_toggled, true),
            Status::Disabled { .. } => (false, false),
        };
        // Geometry (radius, padding ratio) comes from the default so a
        // future iced can restyle the shape; only the colours are ours.
        iced::widget::toggler::Style {
            background: Background::Color(match (on, hovered) {
                (true, true) => alpha(accent, 0.85),
                (true, false) => accent,
                (false, true) => alpha(TEXT_SECONDARY, 0.45),
                (false, false) => alpha(TEXT_SECONDARY, 0.30),
            }),
            background_border_width: 0.0,
            background_border_color: Color::TRANSPARENT,
            // The knob stays readable on both sides: dark on the lit track,
            // light on the grey one.
            foreground: Background::Color(match on {
                true => ON_ACCENT,
                false => alpha(TEXT, 0.85),
            }),
            foreground_border_width: 0.0,
            foreground_border_color: Color::TRANSPARENT,
            ..default(theme, status)
        }
    }
}

/// Project card header strip: a soft accent tint on hover, and nothing
/// otherwise. GTK's `.project-active` also washes this row on the active
/// project, but there the card behind it is undecorated — here the active
/// card already wears the gradient, so a standing wash on top of it just
/// boxes the title inside its own card.
///
/// The tint lives on the container wrapping the WHOLE row — title and the
/// lifecycle glyphs beside it — rather than on the title button, which
/// stops short of the glyphs and reads as a box floating inside the card.
/// That also means it is driven by the row-level hover the sidebar already
/// tracks for the glyph reveal, not by the button's own status: pointing at
/// the glyph half has to light the strip too, and the button never sees it.
pub fn project_header(accent: Color, hovered: bool) -> impl Fn(&Theme) -> container::Style {
    let a = if hovered { 0.08 } else { 0.0 };
    move |_| container::Style {
        background: Some(Background::Color(alpha(accent, a))),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The header's title half: a hit target for expand/collapse only. The
/// wash it used to paint is [`project_header`]'s job now.
pub fn header_title(_: &Theme, _: button::Status) -> button::Style {
    flat(Color::TRANSPARENT, TEXT, 8.0)
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
        // A filled accent button reads as pressable, so an unpressable one
        // has to say otherwise — GTK dims `suggested-action` the same way
        // when `set_sensitive(false)`. Without this, Commit-with-no-message
        // looks armed and silently does nothing when clicked.
        button::Status::Disabled => flat(alpha(accent, 0.35), alpha(ON_ACCENT, 0.5), 99.0),
        _ => flat(accent, ON_ACCENT, 99.0),
    }
}

/// GTK header-bar icon button: flat until hovered, 6px corners, and a
/// persistent wash while toggled on (sidebar / filter), like Adwaita's
/// `.toggled` headerbar buttons.
pub fn toolbar_icon(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let wash = match (active, status) {
            (_, button::Status::Pressed) => 0.16,
            (true, button::Status::Hovered) => 0.15,
            (true, _) => 0.12,
            (false, button::Status::Hovered) => 0.08,
            (false, _) => 0.0,
        };
        flat(alpha(Color::WHITE, wash), TEXT, 6.0)
    }
}

/// Right-click menu card (the GTK sidebar popovers): field-toned, tight
/// radius, floating shadow.
pub fn menu_card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_FIELD)),
        border: Border {
            color: alpha(Color::WHITE, 0.09),
            width: 1.0,
            radius: 10.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..Default::default()
    }
}

/// One menu row: flat until hovered; destructive rows read red and wash
/// red (GTK's .destructive-menu-item).
pub fn menu_item(destructive: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let ink = match (destructive, hovered) {
            (true, _) => CRASHED,
            (false, true) => TEXT,
            (false, false) => TEXT_SECONDARY,
        };
        let bg = match (destructive, hovered) {
            (_, false) => Color::TRANSPARENT,
            (true, true) => alpha(CRASHED, 0.14),
            (false, true) => alpha(Color::WHITE, 0.07),
        };
        flat(bg, ink, 7.0)
    }
}

/// The confirmation card's destructive commit: filled red.
pub fn danger() -> impl Fn(&Theme, button::Status) -> button::Style {
    |_, status| match status {
        button::Status::Hovered => flat(
            Color {
                r: (CRASHED.r + 0.05).min(1.0),
                g: (CRASHED.g + 0.05).min(1.0),
                b: (CRASHED.b + 0.05).min(1.0),
                a: 1.0,
            },
            Color::WHITE,
            99.0,
        ),
        button::Status::Pressed => flat(alpha(CRASHED, 0.85), Color::WHITE, 99.0),
        _ => flat(CRASHED, Color::WHITE, 99.0),
    }
}

/// Tooltip bubble under the header buttons.
pub fn tooltip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CARD)),
        border: Border {
            color: alpha(Color::WHITE, 0.10),
            width: 1.0,
            radius: 6.0.into(),
        },
        text_color: Some(TEXT_SECONDARY),
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..Default::default()
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

/// Multi-line field (the commit message box). Same ink as `input`, but
/// squared off — a 99px radius on something 72px tall reads as a capsule,
/// not a text area.
pub fn editor(accent: Color) -> impl Fn(&Theme, text_editor::Status) -> text_editor::Style {
    move |_, status| {
        let focused = matches!(status, text_editor::Status::Focused { .. });
        text_editor::Style {
            background: Background::Color(BG_FIELD),
            border: Border {
                color: if focused {
                    alpha(accent, 0.55)
                } else {
                    alpha(Color::WHITE, 0.08)
                },
                width: 1.0,
                radius: 8.0.into(),
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stops(active: bool, sweep: Option<f32>) -> Vec<(f32, f32)> {
        let Some(Gradient::Linear(g)) = card_gradient(REMOTE_ACCENT, active, sweep) else {
            panic!("expected a gradient");
        };
        // Recover each stop's accent strength from the blend it landed on
        // (the red channel spans the widest between card and gold).
        g.stops
            .iter()
            .flatten()
            .map(|s| {
                (
                    s.offset,
                    (s.color.r - BG_CARD.r) / (REMOTE_ACCENT.r - BG_CARD.r),
                )
            })
            .collect()
    }

    /// The band's own contribution at each stop, with the static wash
    /// subtracted out. The band only ever paints on the active card, so
    /// isolating it means measuring against the wash rather than against a
    /// bare card.
    fn band_stops(phase: f32) -> Vec<(f32, f32)> {
        stops(true, Some(phase))
            .into_iter()
            .map(|(offset, amount)| (offset, amount - wash_at(offset)))
            .collect()
    }

    #[test]
    fn background_cards_stay_flat_however_busy() {
        // The band is a modulation of the active wash, not a signal of its
        // own: an unselected card is flat whether or not an agent in it is
        // working. Its half of the working signal is the ring.
        assert!(card_gradient(REMOTE_ACCENT, false, None).is_none());
        for i in 0..=8 {
            let phase = i as f32 / 8.0;
            assert!(
                card_gradient(REMOTE_ACCENT, false, Some(phase)).is_none(),
                "phase {phase} lit an unselected card"
            );
        }
    }

    #[test]
    fn stops_stay_ascending_and_within_the_cap() {
        // add_stop overwrites rather than shifts out of order, and drops
        // everything past the eighth — both silent, so pin them here.
        for i in 0..=40 {
            let phase = i as f32 / 40.0;
            let stops = stops(true, Some(phase));
            assert!(stops.len() <= 8, "phase {phase}: {} stops", stops.len());
            assert!(
                stops.windows(2).all(|w| w[0].0 < w[1].0),
                "phase {phase}: {stops:?}"
            );
        }
    }

    #[test]
    fn the_pass_loops_without_a_seam() {
        // Nothing of the band is on the card at either end of a pass, so
        // the phase can wrap 1 -> 0 with no jump to hide.
        for phase in [0.0, 1.0] {
            for (_, amount) in band_stops(phase) {
                assert!(amount.abs() < 1e-3, "phase {phase} lit the card: {amount}");
            }
        }
    }

    #[test]
    fn the_band_crosses_the_card() {
        // Mid-pass the peak is mid-card, and it travels monotonically.
        let peak = |phase: f32| {
            band_stops(phase)
                .into_iter()
                .max_by(|a, b| a.1.partial_cmp(&b.1).expect("finite"))
                .expect("a stop")
        };
        let (offset, amount) = peak(0.5);
        assert!((offset - 0.5).abs() < 1e-3, "peak at {offset}");
        assert!((amount - SWEEP_PEAK).abs() < 1e-3, "peak strength {amount}");
        assert!(peak(0.35).0 < offset && offset < peak(0.65).0);
    }

    #[test]
    fn the_ring_breathes_in_step_with_the_band() {
        assert!((ring_alpha(None) - RING_REST).abs() < 1e-6);
        // Dimmest where the band waits off-card, fullest mid-crossing.
        assert!((ring_alpha(Some(0.0)) - RING_MIN).abs() < 1e-6);
        assert!((ring_alpha(Some(0.5)) - RING_MAX).abs() < 1e-6);
        // Periodic: the phase wrapping 1 -> 0 must not step the border.
        assert!((ring_alpha(Some(1.0)) - ring_alpha(Some(0.0))).abs() < 1e-6);
        // Monotone up over the first half, so it reads as one breath.
        let mut prev = ring_alpha(Some(0.0));
        for i in 1..=25 {
            let a = ring_alpha(Some(i as f32 / 50.0));
            assert!(a > prev, "phase {i}/50 fell back to {a}");
            prev = a;
        }
    }

    #[test]
    fn the_ring_breathes_whether_or_not_the_card_is_selected() {
        // The asymmetry (Nikola): the background belongs to the active
        // card alone, but "an agent in here is working" has to read from
        // any card, so the ring's half of the signal ignores selection.
        let ring = |active| {
            project_card(REMOTE_ACCENT, true, active, Some(0.5))(&Theme::Dark)
                .border
                .color
                .a
        };
        assert!((ring(false) - RING_MAX).abs() < 1e-6);
        assert!((ring(false) - ring(true)).abs() < 1e-6);
    }

    #[test]
    fn the_sweep_adds_to_the_active_wash() {
        // Both signals stay readable at once: the active card keeps its
        // lit top-left corner while the band rides over it.
        let corner = |sweep| {
            stops(true, sweep)
                .into_iter()
                .find(|(offset, _)| *offset == 0.0)
                .expect("the 0.0 stop")
                .1
        };
        assert!((corner(None) - CARD_WASH[0].1).abs() < 1e-3);
        assert!(corner(Some(0.0)) > corner(None) - 1e-3);
        // The band's own pass over the corner is what brightens it.
        assert!(corner(Some(0.16)) > corner(None) + 0.02);
    }
}
