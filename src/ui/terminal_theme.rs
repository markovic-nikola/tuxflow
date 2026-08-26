//! GTK half of the terminal color schemes: the palette data lives in core
//! (shared with the iced shell); this module converts it to `gdk::RGBA`
//! and feeds VTE.

use gtk4::gdk::RGBA;
use tuxflow_core::config::palette::{self, terminal_theme};
use vte4::prelude::*;

pub use tuxflow_core::config::palette::{is_dark_theme, theme_choices, theme_index, theme_name};

fn rgba(hex: &str) -> RGBA {
    let (r, g, b) = palette::hex_rgb(hex);
    RGBA::new(r, g, b, 1.0)
}

pub fn apply(terminal: &vte4::Terminal, name: &str) {
    let theme = terminal_theme(name);
    let palette: Vec<RGBA> = theme.palette.iter().map(|h| rgba(h)).collect();
    let palette_refs: Vec<&RGBA> = palette.iter().collect();
    terminal.set_colors(
        Some(&rgba(theme.foreground)),
        Some(&rgba(theme.background)),
        &palette_refs,
    );
    terminal.set_color_cursor(Some(&rgba(theme.cursor)));
    terminal.set_color_cursor_foreground(Some(&rgba(theme.background)));
}

/// The named theme's background as a CSS color string (same fallback as
/// `apply`). Used to blend chrome around the terminal — e.g. the composer
/// bar — into the terminal area.
pub fn background_css(name: &str) -> String {
    terminal_theme(name).background.to_string()
}
