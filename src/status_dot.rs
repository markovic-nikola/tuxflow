//! The app's small canvas lights: the process row's status dot, and the
//! sync chip's spinner.
//!
//! The dot is a filled disc in the status colour, which shrinks to a core
//! and grows an orbiting arc while an agent is producing output — "Arc
//! sweep", picked from the twenty-candidate round (2026-08-27). It is a
//! canvas rather than the `text("●")` it replaces because nothing below
//! iced's canvas tier can rotate: container styles animate colour, radius,
//! size and shadow, and that is the whole list. The `canvas` feature is a
//! flag rather than a dependency — iced_graphics is already in the tree.
//! The spinner is the same arc without the core, speaking the same visual
//! language at the same cadence — GTK's `gtk4::Spinner` in this shell's
//! terms.

use std::f32::consts::TAU;

use iced::mouse;
use iced::widget::canvas::path::Arc;
use iced::widget::canvas::stroke::{LineCap, LineJoin};
use iced::widget::canvas::{Canvas, Frame, Geometry, Path, Program, Stroke};
use iced::{Color, Element, Point, Radians, Rectangle, Renderer, Theme};

use crate::theme;

/// Footprint of the light, animating or not — a row must not re-flow the
/// moment an agent starts working (GTK pins its dot and its DrawingArea to
/// a shared width for the same reason). Matches the advance of the `●` at
/// `size(10)` this replaces, so the labels beside it did not move.
pub const SIZE: f32 = 10.0;

/// The resting dot.
const REST_R: f32 = 3.2;
/// What it shrinks to while the arc orbits: small enough to read as the
/// arc's centre, big enough to keep carrying the status colour, which is
/// the one thing this widget owes the sidebar.
const CORE_R: f32 = 1.5;
/// The arc's own radius and weight are what little room [`SIZE`] leaves,
/// spent on the GAP rather than on ink: at 10 px the first cut (core 1.7,
/// arc 4.0, stroke 1.3) left 1.6 px between them and the pair read as one
/// blob turning, not a dot with something orbiting it.
const ARC_R: f32 = 3.95;
const ARC_W: f32 = 1.1;
/// Arc length — a little under a third of the circle. Longer starts to
/// read as a ring that is merely gapped rather than one that is turning.
const ARC_SPAN: f32 = TAU * 0.3;
/// Turns per sweep pass. MUST stay a whole number: the arc rides the card
/// sweep's phase rather than keeping a timer of its own, so a fractional
/// multiplier would teleport it every time that phase wraps 1 → 0. Two
/// turns over a 2.6 s pass is the ~1.3 s/turn the gallery previewed, and
/// at this radius the sweep's 20 fps moves the arc's tip about a pixel per
/// frame — there is no stepping to see at 8 px across.
const TURNS_PER_PASS: f32 = 2.0;

/// What the light is doing, beyond its colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Light {
    /// The plain disc.
    Rest,
    /// Producing output: the core with the arc orbiting at `phase`.
    Working(f32),
    /// Running but getting nowhere — an agent retrying its API: a pause
    /// glyph (picked over a dashed ring around the core, 2026-11-05 — the
    /// ring read as "stuck working" but less at a glance). Told apart by
    /// shape rather than hue on purpose: the colour is the
    /// project's accent, which is user-picked, so any fixed "waiting" hue
    /// collides with some accent (restarting amber against the remote
    /// gold already does), and the dot must keep saying where it lives.
    Stalled,
    /// Stopped short — an agent whose turn ended on an API error and
    /// waits for a prompt to go on: a warning triangle carrying a "!".
    /// The one light that changes hue as well: it is the row-level twin
    /// of the "needs you" badge, so the caller paints it in the attention
    /// colour (`theme::attention_for`), and the "!" takes the badge's ink.
    Failed,
}

struct StatusDot {
    color: Color,
    light: Light,
}

/// The pause glyph's bars: as tall as the resting disc is wide plus a
/// little, so it fills the same optical box; each bar a hair over the
/// arc's weight so the pair holds up at this size.
const PAUSE_H: f32 = 6.6;
const PAUSE_W: f32 = 1.8;
const PAUSE_GAP: f32 = 1.8;

/// How far past its box the light may paint: room for the warning
/// triangle's corners and their rounding.
const OVERDRAW: f32 = 2.5;

/// The warning triangle: a little over [`SIZE`], spilling a pixel past
/// the canvas on each side — at 10 px a triangle's "!" was a one-pixel
/// stem — while the box the row lays out stays 10 px, so the label beside
/// it does not move. Its corners are softened by a round-joined stroke of
/// the same ink; the "!" is a stem and a dot on its axis, low enough to
/// sit in its wide part. All measured from the canvas's top-left corner.
const WARN_TOP: f32 = -1.4;
const WARN_BOTTOM: f32 = 10.2;
const WARN_HALF: f32 = 6.2;
const WARN_ROUND: f32 = 1.4;
const BANG_W: f32 = 1.6;
const BANG_TOP: f32 = 2.4;
const BANG_STEM: f32 = 3.9;
const BANG_DOT_Y: f32 = 8.0;
const BANG_DOT_R: f32 = 0.9;

/// The status light for a process row.
pub fn status_dot<'a, Message: 'a>(color: Color, light: Light) -> Element<'a, Message> {
    Canvas::new(StatusDot { color, light })
        .width(SIZE)
        .height(SIZE)
        .into()
}

impl<Message> Program<Message> for StatusDot {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        // The clip reaches [`OVERDRAW`] past the box on every side (the
        // coordinates stay the box's own), so the warning triangle can be
        // bigger than the slot the row lays out.
        let mut frame = Frame::with_bounds(
            renderer,
            Rectangle::new(
                Point::new(-OVERDRAW, -OVERDRAW),
                iced::Size::new(
                    bounds.width + 2.0 * OVERDRAW,
                    bounds.height + 2.0 * OVERDRAW,
                ),
            ),
        );
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);

        match self.light {
            Light::Rest => frame.fill(&Path::circle(center, REST_R), self.color),
            Light::Stalled => {
                let top = center.y - PAUSE_H / 2.0;
                for x in [
                    center.x - PAUSE_GAP / 2.0 - PAUSE_W,
                    center.x + PAUSE_GAP / 2.0,
                ] {
                    frame.fill(
                        &Path::rounded_rectangle(
                            iced::Point::new(x, top),
                            iced::Size::new(PAUSE_W, PAUSE_H),
                            0.6.into(),
                        ),
                        self.color,
                    );
                }
            }
            Light::Failed => {
                let x = center.x;
                // Inset by half the rounding stroke, so stroke and fill
                // together fill the box.
                let inset = WARN_ROUND / 2.0;
                let triangle = Path::new(|builder| {
                    builder.move_to(Point::new(x, WARN_TOP + inset));
                    builder.line_to(Point::new(x + WARN_HALF - inset, WARN_BOTTOM - inset));
                    builder.line_to(Point::new(x - WARN_HALF + inset, WARN_BOTTOM - inset));
                    builder.close();
                });
                frame.fill(&triangle, self.color);
                frame.stroke(
                    &triangle,
                    Stroke::default()
                        .with_color(self.color)
                        .with_width(WARN_ROUND)
                        .with_line_join(LineJoin::Round),
                );
                let ink = theme::attention_ink(self.color);
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(x - BANG_W / 2.0, BANG_TOP),
                        iced::Size::new(BANG_W, BANG_STEM),
                        (BANG_W / 2.0).into(),
                    ),
                    ink,
                );
                frame.fill(&Path::circle(Point::new(x, BANG_DOT_Y), BANG_DOT_R), ink);
            }
            Light::Working(phase) => {
                frame.fill(&Path::circle(center, CORE_R), self.color);
                let start = phase * TAU * TURNS_PER_PASS;
                let arc = Path::new(|builder| {
                    builder.arc(Arc {
                        center,
                        radius: ARC_R,
                        start_angle: Radians(start),
                        end_angle: Radians(start + ARC_SPAN),
                    });
                });
                frame.stroke(
                    &arc,
                    Stroke::default()
                        .with_color(self.color)
                        .with_width(ARC_W)
                        .with_line_cap(LineCap::Round),
                );
            }
        }

        vec![frame.into_geometry()]
    }
}

/// Footprint of the sync chip's spinner. Sized under the 10.5 pt labels
/// beside it, so swapping the counters for the spinner never changes the
/// chip's height.
const SPINNER_SIZE: f32 = 11.0;
/// A touch more radius and ink than the dot's orbit: there is no core to
/// clear, and alone on the chip the arc carries the whole signal.
const SPINNER_R: f32 = 4.2;
const SPINNER_W: f32 = 1.4;
/// Same span and turn rate as the working arc above — one visual language
/// for "busy", whichever surface is saying it.
const SPINNER_SPAN: f32 = TAU * 0.3;
/// MUST stay whole, exactly as [`TURNS_PER_PASS`] must: the spinner rides
/// a looping phase that wraps 1 → 0, and a fractional multiplier would
/// teleport the arc at every seam.
const SPINNER_TURNS: f32 = 2.0;

struct Spinner {
    color: Color,
    phase: f32,
}

/// A bare orbiting arc — the sync chip's stand-in for its ↓↑ counters
/// while a sync runs. `phase` is a 0..1 loop (the app's `sync_spin`
/// chain).
pub fn spinner<'a, Message: 'a>(color: Color, phase: f32) -> Element<'a, Message> {
    Canvas::new(Spinner { color, phase })
        .width(SPINNER_SIZE)
        .height(SPINNER_SIZE)
        .into()
}

impl<Message> Program<Message> for Spinner {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let start = self.phase * TAU * SPINNER_TURNS;
        let arc = Path::new(|builder| {
            builder.arc(Arc {
                center: frame.center(),
                radius: SPINNER_R,
                start_angle: Radians(start),
                end_angle: Radians(start + SPINNER_SPAN),
            });
        });
        frame.stroke(
            &arc,
            Stroke::default()
                .with_color(self.color)
                .with_width(SPINNER_W)
                .with_line_cap(LineCap::Round),
        );
        vec![frame.into_geometry()]
    }
}
