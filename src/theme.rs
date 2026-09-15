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

// ── Scheme palette ──────────────────────────────────────────────────────
/// Every color that changes between the dark and light shells: surfaces,
/// text, and the semantic hues whose dark-tuned values fall below contrast
/// on white (core's `palette.rs` carries the same rule for the ambers).
/// `light` says which one is current, for the few places that need to
/// know rather than a color (the iced base theme, the shipped shadows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub light: bool,
    /// Sidebar ground the cards float on.
    pub ground: Color,
    /// Project card.
    pub card: Color,
    /// Window chrome: toolbar, status bar, composer bar.
    pub chrome: Color,
    /// Main-pane ground: full-pane views (settings, git, add forms) and
    /// the placeholder states. A design surface of the shell — NOT the
    /// terminal's background, which follows the user's terminal scheme
    /// via `terminal_pane` (GTK's dialogs sit on the window background,
    /// never the VTE palette).
    pub pane: Color,
    /// Input field fill.
    pub field: Color,
    pub hairline: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub dim: Color,
    /// Stopped dot on the card surface.
    pub stopped: Color,
    /// The ink every translucent wash is mixed from — hover, pills,
    /// keycaps, thin borders: white over the dark shell, black over the
    /// light one, where a white wash on a white card is simply invisible.
    pub wash: Color,
    /// #cca700 / its dark twin — the restart-pending amber.
    pub restarting: Color,
    /// The status bar's update chip — the GTK stylesheet's `.update-label`.
    /// Semantic like the git colours: not the accent, which the user sets.
    pub update_chip: Color,
    /// Insertions, and commits waiting to be pushed. Same hue as the
    /// default local accent by coincidence of the palette, not by
    /// reference: the accent is user-settable and this must not move
    /// with it.
    pub git_added: Color,
    /// Deletions.
    pub git_removed: Color,
    /// Commits waiting to be pulled. Amber reads as "incoming, not yours
    /// yet" against the green of what you already have.
    pub git_behind: Color,
    /// A modified file's badge in the changes list.
    pub git_modified: Color,
    /// An untracked file's badge: present, but not git's business yet.
    pub git_untracked: Color,
    /// Multiplier on every drop shadow's alpha. The dark shell's shadows
    /// are tuned against near-black surfaces, where a 0.25 black barely
    /// registers; the same value on a white card reads as a heavy grey
    /// halo (Nikola, on the light card), so the light palette scales
    /// them down rather than carrying a second set of shadow constants.
    pub shadow_scale: f32,
}

/// The shipped dark shell — direction C's values, matching the GTK stylesheet
/// where the two shells share a hue.
pub const DARK: Palette = Palette {
    light: false,
    ground: Color::from_rgb(0.071, 0.071, 0.090),
    card: Color::from_rgb(0.090, 0.090, 0.114),
    chrome: Color::from_rgb(0.090, 0.090, 0.110),
    pane: Color::from_rgb(0.118, 0.118, 0.180),
    field: Color::from_rgb(0.114, 0.114, 0.141),
    hairline: Color::from_rgba(1.0, 1.0, 1.0, 0.05),
    text: Color::from_rgb(0.925, 0.925, 0.945),
    text_secondary: Color::from_rgb(0.651, 0.651, 0.690),
    dim: Color::from_rgb(0.561, 0.561, 0.604),
    stopped: Color::from_rgb(0.290, 0.290, 0.322),
    wash: Color::WHITE,
    restarting: Color::from_rgb(0.8, 0.655, 0.0),
    update_chip: Color::from_rgb(0.800, 0.655, 0.0),
    git_added: Color::from_rgb(0.451, 0.788, 0.569),
    git_removed: Color::from_rgb(0.945, 0.298, 0.298),
    git_behind: Color::from_rgb(0.824, 0.600, 0.133),
    git_modified: Color::from_rgb(0.863, 0.863, 0.667),
    git_untracked: Color::from_rgb(0.424, 0.424, 0.424),
    shadow_scale: 1.0,
};

/// The light shell: libadwaita's light surfaces (window #fafafa, cards
/// white, header #ebebeb, text #2e3436), the same structure as DARK. The
/// semantic hues are darkened twins — every dark value measured under
/// 3:1 on white (#73c991 green 1.9:1, #dcdcaa 1.4:1, the ambers ~2.2:1).
pub const LIGHT: Palette = Palette {
    light: true,
    ground: Color::from_rgb(0.941, 0.941, 0.945),
    card: Color::WHITE,
    chrome: Color::from_rgb(0.922, 0.922, 0.922),
    pane: Color::from_rgb(0.980, 0.980, 0.980),
    field: Color::from_rgb(0.965, 0.965, 0.970),
    hairline: Color::from_rgba(0.0, 0.0, 0.0, 0.10),
    text: Color::from_rgb(0.180, 0.204, 0.212),
    text_secondary: Color::from_rgb(0.369, 0.361, 0.392),
    dim: Color::from_rgb(0.545, 0.541, 0.561),
    stopped: Color::from_rgb(0.663, 0.659, 0.678),
    wash: Color::BLACK,
    restarting: Color::from_rgb(0.541, 0.435, 0.0),
    update_chip: Color::from_rgb(0.541, 0.435, 0.0),
    git_added: Color::from_rgb(0.184, 0.541, 0.333),
    git_removed: Color::from_rgb(0.776, 0.157, 0.157),
    git_behind: Color::from_rgb(0.604, 0.404, 0.0),
    git_modified: Color::from_rgb(0.478, 0.478, 0.102),
    git_untracked: Color::from_rgb(0.424, 0.424, 0.424),
    shadow_scale: 0.3,
};

/// A drop shadow's colour: black at `dark_alpha`, scaled by the scheme.
pub fn shadow(dark_alpha: f32) -> Color {
    alpha(Color::BLACK, dark_alpha * pal().shadow_scale)
}

/// The current palette. A process-wide slot like ACCENTS below: `pal()`
/// is read from every view helper, and threading a palette through every
/// call site buys nothing. Written at boot and on a scheme change.
static PALETTE: RwLock<Palette> = RwLock::new(DARK);

pub fn pal() -> Palette {
    *PALETTE.read().expect("palette slot")
}

// ── Accents: where a project lives ──────────────────────────────────────
/// #73c991
pub const LOCAL_ACCENT: Color = Color::from_rgb(0.451, 0.788, 0.569);
/// #ffce5c — the logo gold.
pub const REMOTE_ACCENT: Color = Color::from_rgb(1.0, 0.808, 0.361);

// ── Status (semantic, fixed) ────────────────────────────────────────────
/// #f14c4c — reads in both schemes as it is (core's STATUS_COLORS
/// carries no twin for it either).
pub const CRASHED: Color = Color::from_rgb(0.945, 0.298, 0.298);

pub fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

/// The live local/remote accents, settable from settings. A process-wide
/// slot rather than a parameter because `accent_for` is called from every
/// view helper — threading two colors through ~40 call sites buys nothing.
/// Written once at boot and on a settings change, read on the view thread.
static ACCENTS: RwLock<(Color, Color)> = RwLock::new((LOCAL_ACCENT, REMOTE_ACCENT));

/// The accent NAMES, kept so a scheme flip can re-resolve them: the same
/// name is a different hex on each scheme (core's `accent` vs
/// `accent_light`, the sidebar reads these as text and thin borders).
static ACCENT_NAMES: RwLock<(String, String)> = RwLock::new((String::new(), String::new()));

/// Resolve the two sidebar accents from their settings names, in the
/// current scheme's variant, and make them current.
pub fn set_accents(local_name: &str, remote_name: &str) {
    *ACCENT_NAMES.write().expect("accent names") =
        (local_name.to_string(), remote_name.to_string());
    let local = palette::accent_by_name(local_name, palette::FALLBACK_LOCAL);
    let remote = palette::accent_by_name(remote_name, palette::FALLBACK_REMOTE);
    let light = pal().light;
    let pick = |c: &palette::AccentColor| hex(if light { c.accent_light } else { c.accent });
    *ACCENTS.write().expect("accent slot") = (pick(local), pick(remote));
}

/// Switch the shell between its dark and light palettes. Re-resolves the
/// accents, since their light twins are part of the scheme.
pub fn set_scheme(light: bool) {
    *PALETTE.write().expect("palette slot") = if light { LIGHT } else { DARK };
    let (local, remote) = ACCENT_NAMES.read().expect("accent names").clone();
    set_accents(&local, &remote);
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
        background: Some(Background::Color(pal().card)),
        border: Border {
            color: alpha(pal().wash, 0.06),
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
        background: Some(Background::Color(pal().ground)),
        ..Default::default()
    }
}

pub fn chrome(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(pal().chrome)),
        ..Default::default()
    }
}

pub fn hairline(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(pal().hairline)),
        ..Default::default()
    }
}

pub fn pane(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(pal().pane)),
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
/// How much the terminal pane darkens while it is not where keys go, as a
/// drop in CIE lightness (L*, 0–100). A drop rather than a wash alpha
/// because the same alpha is not the same dim: 18 % black costs a
/// Mocha pane ~3.5 L* and a Latte pane ~15 — measured on the light-scheme
/// bench (2026-09-14), where it read as a grey slab. 3.5 is the Mocha
/// value Nikola approved off the first bench, so dark schemes are
/// unchanged by construction.
pub const UNFOCUSED_DIM_LSTAR: f32 = 3.5;
/// Alpha of the accent for the line/ring focus indicators.
const FOCUS_LINE_ALPHA: f32 = 0.8;
const FOCUS_RING_ALPHA: f32 = 0.45;

/// Alpha of a black wash that lowers `bg` by `UNFOCUSED_DIM_LSTAR`. A black
/// wash at alpha a scales sRGB by (1 − a), i.e. linear luminance by
/// (1 − a)^2.2, so the alpha that lands on the target luminance is a
/// closed form. Floors are only there for the degenerate ends (a pure
/// black background cannot get darker).
pub fn dim_alpha_for(bg: Color) -> f32 {
    fn lin(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    fn lstar(y: f32) -> f32 {
        if y <= 0.008856 {
            903.3 * y
        } else {
            116.0 * y.cbrt() - 16.0
        }
    }
    fn luminance(l: f32) -> f32 {
        if l <= 8.0 {
            l / 903.3
        } else {
            ((l + 16.0) / 116.0).powi(3)
        }
    }
    let y = 0.2126 * lin(bg.r) + 0.7152 * lin(bg.g) + 0.0722 * lin(bg.b);
    if y <= 0.0005 {
        return 0.0;
    }
    let target = luminance((lstar(y) - UNFOCUSED_DIM_LSTAR).max(0.0));
    let scale = (target / y).clamp(0.0, 1.0).powf(1.0 / 2.2);
    (1.0 - scale).clamp(0.0, 0.5)
}

/// The terminal's focus indicator, painted by the widget from the same
/// flag that shapes its cursor. `name` is the `focus_indicator` setting
/// (see core's `FOCUS_INDICATOR_CHOICES`); an unknown name takes the
/// default rather than no mark, so a hand-edited file can't silently
/// switch the indicator off. `scheme` is the terminal theme, which the
/// dim's strength follows.
pub fn focus_mark(name: &str, accent: Color, scheme: &str) -> Option<iced_term::FocusMark> {
    use iced_term::{FocusMark, FocusMarkStyle};
    let (style, color) = match name {
        "none" => return None,
        "top" => (
            FocusMarkStyle::TopLine(1.0),
            alpha(accent, FOCUS_LINE_ALPHA),
        ),
        "left" => (
            FocusMarkStyle::LeftLine(2.0),
            alpha(accent, FOCUS_LINE_ALPHA),
        ),
        "ring" => (FocusMarkStyle::Ring(1.0), alpha(accent, FOCUS_RING_ALPHA)),
        _ => (
            FocusMarkStyle::Dim,
            alpha(Color::BLACK, dim_alpha_for(terminal_background(scheme))),
        ),
    };
    Some(FocusMark { color, style })
}

/// The terminal scheme's background as a colour.
fn terminal_background(scheme: &str) -> Color {
    let (r, g, b) = palette::hex_rgb(palette::terminal_theme(scheme).background);
    Color::from_rgb(r, g, b)
}

pub fn terminal_pane(scheme: &str) -> impl Fn(&Theme) -> container::Style {
    let bg = terminal_background(scheme);
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
    let a = if color == pal().git_removed {
        0.11
    } else {
        0.13
    };
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
            None => Background::Color(pal().card),
        };
        container::Style {
            background: Some(background),
            border: Border {
                color: if running {
                    alpha(accent, ring_alpha(sweep))
                } else {
                    alpha(pal().wash, 0.05)
                },
                width: 1.0,
                radius: 10.0.into(),
            },
            shadow: Shadow {
                color: shadow(0.25),
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
        gradient = gradient.add_stop(t, mix(pal().card, accent, amount));
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
        background: Some(Background::Color(alpha(pal().wash, 0.06))),
        border: Border {
            radius: 99.0.into(),
            ..Default::default()
        },
        text_color: Some(pal().text_secondary),
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
        background: Some(Background::Color(alpha(pal().wash, 0.055))),
        border: Border {
            color: alpha(pal().wash, 0.12),
            width: 1.0,
            radius: 3.0.into(),
        },
        shadow: Shadow {
            color: shadow(0.5),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 0.0,
        },
        text_color: Some(pal().text_secondary),
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
        | scrollable::Status::Dragged { .. } => alpha(pal().wash, 0.45),
        scrollable::Status::Hovered { .. } => alpha(pal().wash, 0.22),
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
            background: Background::Color(pal().card),
            border: Border::default(),
            shadow: Shadow::default(),
            icon: pal().text_secondary,
        },
    }
}

/// Centered form card.
pub fn form_card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(pal().card)),
        border: Border {
            color: alpha(pal().wash, 0.06),
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: Shadow {
            color: shadow(0.35),
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
        let ink = if selected {
            pal().text
        } else {
            pal().text_secondary
        };
        flat(alpha(accent, a), ink, 8.0)
    }
}

/// How much of a dragged row is left behind in the sidebar while its
/// ghost travels — the GTK stylesheet's `.dragging { opacity: 0.35 }`. Applied to
/// the row's ink and dot from the view; a button style can't fade its
/// content.
pub const LIFTED_ALPHA: f32 = 0.35;

/// The 2px rule marking a drop slot, GTK's `.drop-target-above/-below`
/// border in `@accent_bg_color`. Drawn as a layer of its own because iced
/// borders are all-or-nothing across the four sides; transparent when
/// idle, since the layer is always in the tree.
pub fn drop_line(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(color)),
        ..Default::default()
    }
}

/// The lifted row following the pointer — GTK's WidgetPaintable drag
/// icon, done as a floating card in the row's own colours.
pub fn drag_ghost(accent: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(pal().field)),
        border: Border {
            color: alpha(accent, 0.55),
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: shadow(0.45),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 14.0,
        },
        ..Default::default()
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
                false => pal().card,
            })),
            text_color: pal().text,
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
                (false, true) => alpha(pal().text_secondary, 0.45),
                (false, false) => alpha(pal().text_secondary, 0.30),
            }),
            background_border_width: 0.0,
            background_border_color: Color::TRANSPARENT,
            // The knob stays readable on both sides: dark on the lit track,
            // light on the grey one.
            foreground: Background::Color(match on {
                true => ON_ACCENT,
                false => alpha(pal().text, 0.85),
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
    flat(Color::TRANSPARENT, pal().text, 8.0)
}

/// Neutral pill button: toolbar chips, "+ project", cancel.
pub fn pill_button(accent: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match status {
        button::Status::Hovered => flat(alpha(accent, 0.14), pal().text, 99.0),
        button::Status::Pressed => flat(alpha(accent, 0.22), pal().text, 99.0),
        button::Status::Disabled => flat(alpha(pal().wash, 0.03), pal().dim, 99.0),
        _ => flat(alpha(pal().wash, 0.05), pal().text_secondary, 99.0),
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
        _ => flat(alpha(pal().wash, 0.05), pal().text_secondary, 99.0),
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
        flat(alpha(pal().wash, wash), pal().text, 6.0)
    }
}

/// Right-click menu card (the GTK sidebar popovers): field-toned, tight
/// radius, floating shadow.
pub fn menu_card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(pal().field)),
        border: Border {
            color: alpha(pal().wash, 0.09),
            width: 1.0,
            radius: 10.0.into(),
        },
        shadow: Shadow {
            color: shadow(0.4),
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
            (false, true) => pal().text,
            (false, false) => pal().text_secondary,
        };
        let bg = match (destructive, hovered) {
            (_, false) => Color::TRANSPARENT,
            (true, true) => alpha(CRASHED, 0.14),
            (false, true) => alpha(pal().wash, 0.07),
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
        background: Some(Background::Color(pal().card)),
        border: Border {
            color: alpha(pal().wash, 0.10),
            width: 1.0,
            radius: 6.0.into(),
        },
        text_color: Some(pal().text_secondary),
        shadow: Shadow {
            color: shadow(0.35),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..Default::default()
    }
}

/// Quiet close/utility glyph (the card ✕): nearly invisible until hover.
pub fn ghost(glyph_hover: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| match status {
        button::Status::Hovered => flat(alpha(pal().wash, 0.06), glyph_hover, 99.0),
        button::Status::Pressed => flat(alpha(pal().wash, 0.10), glyph_hover, 99.0),
        _ => flat(Color::TRANSPARENT, pal().dim, 99.0),
    }
}

// ── Inputs ──────────────────────────────────────────────────────────────

/// Rounded field; border picks up the accent when focused.
pub fn input(accent: Color) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| {
        let focused = matches!(status, text_input::Status::Focused { .. });
        text_input::Style {
            background: Background::Color(pal().field),
            border: Border {
                color: if focused {
                    alpha(accent, 0.55)
                } else {
                    alpha(pal().wash, 0.08)
                },
                width: 1.0,
                radius: 99.0.into(),
            },
            icon: pal().text_secondary,
            placeholder: pal().dim,
            value: pal().text,
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
            background: Background::Color(pal().field),
            border: Border {
                color: if focused {
                    alpha(accent, 0.55)
                } else {
                    alpha(pal().wash, 0.08)
                },
                width: 1.0,
                radius: 8.0.into(),
            },
            placeholder: pal().dim,
            value: pal().text,
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

    /// The dim is a constant drop in perceived lightness, not a constant
    /// alpha: the wash that reads as a hint on a dark scheme reads as a
    /// slab on a light one.
    #[test]
    fn dim_alpha_follows_the_scheme_background() {
        let mocha = dim_alpha_for(terminal_background("catppuccin-mocha"));
        let latte = dim_alpha_for(terminal_background("catppuccin-latte"));
        assert!((0.15..=0.21).contains(&mocha), "mocha {mocha}");
        assert!((0.03..=0.06).contains(&latte), "latte {latte}");
        assert_eq!(dim_alpha_for(Color::BLACK), 0.0);
        for t in palette::TERMINAL_THEMES {
            let a = dim_alpha_for(terminal_background(t.name));
            assert!((0.0..=0.3).contains(&a), "{}: {a}", t.name);
        }
    }

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
                    (s.color.r - pal().card.r) / (REMOTE_ACCENT.r - pal().card.r),
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
