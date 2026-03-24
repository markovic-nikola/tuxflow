use std::sync::LazyLock;

use gtk4::gdk::RGBA;
use vte4::prelude::*;

struct TerminalTheme {
    name: &'static str,
    label: &'static str,
    foreground: RGBA,
    background: RGBA,
    cursor: RGBA,
    palette: [RGBA; 16],
}

fn hex(val: u32) -> RGBA {
    let r = ((val >> 16) & 0xFF) as f32 / 255.0;
    let g = ((val >> 8) & 0xFF) as f32 / 255.0;
    let b = (val & 0xFF) as f32 / 255.0;
    RGBA::new(r, g, b, 1.0)
}

static THEMES: LazyLock<Vec<TerminalTheme>> = LazyLock::new(|| {
    vec![
        // -- Dark themes --
        TerminalTheme {
            name: "catppuccin-mocha",
            label: "Catppuccin Mocha (Default)",
            foreground: hex(0xCDD6F4),
            background: hex(0x1E1E2E),
            cursor: hex(0xF5E0DC),
            palette: [
                hex(0x45475A), hex(0xF38BA8), hex(0xA6E3A1), hex(0xF9E2AF),
                hex(0x89B4FA), hex(0xF5C2E7), hex(0x94E2D5), hex(0xBAC2DE),
                hex(0x585B70), hex(0xF38BA8), hex(0xA6E3A1), hex(0xF9E2AF),
                hex(0x89B4FA), hex(0xF5C2E7), hex(0x94E2D5), hex(0xA6ADC8),
            ],
        },
        TerminalTheme {
            name: "dracula",
            label: "Dracula",
            foreground: hex(0xF8F8F2),
            background: hex(0x282A36),
            cursor: hex(0xF8F8F2),
            palette: [
                hex(0x21222C), hex(0xFF5555), hex(0x50FA7B), hex(0xF1FA8C),
                hex(0xBD93F9), hex(0xFF79C6), hex(0x8BE9FD), hex(0xF8F8F2),
                hex(0x6272A4), hex(0xFF6E6E), hex(0x69FF94), hex(0xFFFFA5),
                hex(0xD6ACFF), hex(0xFF92DF), hex(0xA4FFFF), hex(0xFFFFFF),
            ],
        },
        TerminalTheme {
            name: "nord",
            label: "Nord",
            foreground: hex(0xD8DEE9),
            background: hex(0x2E3440),
            cursor: hex(0xD8DEE9),
            palette: [
                hex(0x3B4252), hex(0xBF616A), hex(0xA3BE8C), hex(0xEBCB8B),
                hex(0x81A1C1), hex(0xB48EAD), hex(0x88C0D0), hex(0xE5E9F0),
                hex(0x4C566A), hex(0xBF616A), hex(0xA3BE8C), hex(0xEBCB8B),
                hex(0x81A1C1), hex(0xB48EAD), hex(0x8FBCBB), hex(0xECEFF4),
            ],
        },
        TerminalTheme {
            name: "gruvbox-dark",
            label: "Gruvbox Dark",
            foreground: hex(0xEBDBB2),
            background: hex(0x282828),
            cursor: hex(0xEBDBB2),
            palette: [
                hex(0x282828), hex(0xCC241D), hex(0x98971A), hex(0xD79921),
                hex(0x458588), hex(0xB16286), hex(0x689D6A), hex(0xA89984),
                hex(0x928374), hex(0xFB4934), hex(0xB8BB26), hex(0xFABD2F),
                hex(0x83A598), hex(0xD3869B), hex(0x8EC07C), hex(0xEBDBB2),
            ],
        },
        TerminalTheme {
            name: "one-dark",
            label: "One Dark",
            foreground: hex(0xABB2BF),
            background: hex(0x282C34),
            cursor: hex(0x528BFF),
            palette: [
                hex(0x282C34), hex(0xE06C75), hex(0x98C379), hex(0xE5C07B),
                hex(0x61AFEF), hex(0xC678DD), hex(0x56B6C2), hex(0xABB2BF),
                hex(0x545862), hex(0xE06C75), hex(0x98C379), hex(0xE5C07B),
                hex(0x61AFEF), hex(0xC678DD), hex(0x56B6C2), hex(0xBE5046),
            ],
        },
        TerminalTheme {
            name: "tokyo-night",
            label: "Tokyo Night",
            foreground: hex(0xC0CAF5),
            background: hex(0x1A1B26),
            cursor: hex(0xC0CAF5),
            palette: [
                hex(0x15161E), hex(0xF7768E), hex(0x9ECE6A), hex(0xE0AF68),
                hex(0x7AA2F7), hex(0xBB9AF7), hex(0x7DCFFF), hex(0xA9B1D6),
                hex(0x414868), hex(0xF7768E), hex(0x9ECE6A), hex(0xE0AF68),
                hex(0x7AA2F7), hex(0xBB9AF7), hex(0x7DCFFF), hex(0xC0CAF5),
            ],
        },
        TerminalTheme {
            name: "solarized-dark",
            label: "Solarized Dark",
            foreground: hex(0x839496),
            background: hex(0x002B36),
            cursor: hex(0x93A1A1),
            palette: [
                hex(0x073642), hex(0xDC322F), hex(0x859900), hex(0xB58900),
                hex(0x268BD2), hex(0xD33682), hex(0x2AA198), hex(0xEEE8D5),
                hex(0x002B36), hex(0xCB4B16), hex(0x586E75), hex(0x657B83),
                hex(0x839496), hex(0x6C71C4), hex(0x93A1A1), hex(0xFDF6E3),
            ],
        },
        // -- Light themes --
        TerminalTheme {
            name: "catppuccin-latte",
            label: "Catppuccin Latte",
            foreground: hex(0x4C4F69),
            background: hex(0xEFF1F5),
            cursor: hex(0xDC8A78),
            palette: [
                hex(0x5C5F77), hex(0xD20F39), hex(0x40A02B), hex(0xDF8E1D),
                hex(0x1E66F5), hex(0xEA76CB), hex(0x179299), hex(0xACB0BE),
                hex(0x6C6F85), hex(0xD20F39), hex(0x40A02B), hex(0xDF8E1D),
                hex(0x1E66F5), hex(0xEA76CB), hex(0x179299), hex(0x4C4F69),
            ],
        },
        TerminalTheme {
            name: "solarized-light",
            label: "Solarized Light",
            foreground: hex(0x657B83),
            background: hex(0xFDF6E3),
            cursor: hex(0x586E75),
            palette: [
                hex(0x073642), hex(0xDC322F), hex(0x859900), hex(0xB58900),
                hex(0x268BD2), hex(0xD33682), hex(0x2AA198), hex(0xEEE8D5),
                hex(0x002B36), hex(0xCB4B16), hex(0x586E75), hex(0x657B83),
                hex(0x839496), hex(0x6C71C4), hex(0x93A1A1), hex(0xFDF6E3),
            ],
        },
    ]
});

pub fn apply(terminal: &vte4::Terminal, name: &str) {
    let theme = THEMES.iter().find(|t| t.name == name).unwrap_or(&THEMES[0]);
    let palette_refs: Vec<&RGBA> = theme.palette.iter().collect();
    terminal.set_colors(
        Some(&theme.foreground),
        Some(&theme.background),
        &palette_refs,
    );
    terminal.set_color_cursor(Some(&theme.cursor));
    terminal.set_color_cursor_foreground(Some(&theme.background));
}

pub fn theme_choices() -> Vec<&'static str> {
    THEMES.iter().map(|t| t.label).collect()
}

pub fn theme_index(name: &str) -> u32 {
    THEMES
        .iter()
        .position(|t| t.name == name)
        .unwrap_or(0) as u32
}

pub fn theme_name(index: u32) -> &'static str {
    THEMES
        .get(index as usize)
        .map(|t| t.name)
        .unwrap_or("catppuccin-mocha")
}
