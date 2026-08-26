//! Shared color data for both shells: the accent palette (app accent plus
//! the local/remote sidebar hues), the fixed-meaning status hues, and the
//! terminal color schemes. Everything is a hex string — the GTK shell
//! parses into `gdk::RGBA`/CSS, the iced shell into `iced::Color` or
//! `iced_term::ColorPalette` — so this file is the single authoritative
//! representation and a palette edit lands in both shells at once.
//!
//! The contrast tests live here too: the numbers are properties of the
//! data, not of either toolkit.

pub struct AccentColor {
    pub name: &'static str,
    pub label: &'static str,
    pub bg: &'static str,
    pub fg: &'static str,
    /// Text-weight accent, used against dark surfaces. Doubles as the
    /// sidebar hue for local/remote projects, which is why a few entries
    /// are tuned brighter than their `bg`: the sidebar reads them as text
    /// and thin borders, not as filled buttons.
    pub accent: &'static str,
    /// Same hue darkened for light surfaces. `accent` is tuned for dark mode
    /// and lands at 2-3:1 against libadwaita's light sidebar (#ebebeb) — these
    /// hit ~4.2:1 while keeping the hue recognisable. `bg`/`fg` need no
    /// variant: they are a filled button, not text, and already pair.
    pub accent_light: &'static str,
}

pub const ACCENT_COLORS: &[AccentColor] = &[
    AccentColor {
        name: "green",
        label: "Green",
        bg: "#2ea043",
        fg: "#ffffff",
        // The sidebar's running-green since day one, and 6.7:1 on the dark
        // sidebar where a button-weight #3fb950 only manages 4.2:1.
        accent: "#73c991",
        accent_light: "#1a7f37",
    },
    AccentColor {
        name: "blue",
        label: "Blue",
        bg: "#3584e4",
        fg: "#ffffff",
        accent: "#5d9de9",
        accent_light: "#1c6dcf",
    },
    AccentColor {
        name: "purple",
        label: "Purple",
        bg: "#9141ac",
        fg: "#ffffff",
        accent: "#bd83d0",
        accent_light: "#9141ac",
    },
    AccentColor {
        name: "teal",
        label: "Teal",
        bg: "#2190a4",
        fg: "#ffffff",
        accent: "#27a8c0",
        accent_light: "#1b7888",
    },
    AccentColor {
        name: "orange",
        label: "Orange",
        bg: "#e66100",
        fg: "#ffffff",
        accent: "#ff6c00",
        accent_light: "#b84e00",
    },
    AccentColor {
        name: "red",
        label: "Red",
        bg: "#e01b24",
        fg: "#ffffff",
        accent: "#ee7379",
        accent_light: "#db1a23",
    },
    AccentColor {
        name: "pink",
        label: "Pink",
        bg: "#d56199",
        fg: "#ffffff",
        accent: "#db79a9",
        accent_light: "#c5347a",
    },
    AccentColor {
        name: "yellow",
        label: "Yellow",
        bg: "#c88800",
        fg: "#ffffff",
        // TuxFlow's logo gold — the default remote-project accent. On light
        // surfaces it measures 1.24:1, hence the very different companion.
        accent: "#ffce5c",
        accent_light: "#9a6700",
    },
    AccentColor {
        name: "slate",
        label: "Slate",
        bg: "#6e8898",
        fg: "#ffffff",
        accent: "#869ca9",
        accent_light: "#5b7280",
    },
];

/// Palette names used when a settings file omits or misspells a choice.
/// `AppearanceSettings::default` carries the same names.
pub const FALLBACK_LOCAL: &str = "green";
pub const FALLBACK_REMOTE: &str = "yellow";

/// Status-dot hues that carry a fixed meaning rather than a chosen one,
/// as (name, dark, light). They still need the light twin: both ambers
/// measure ~2.2:1 on the light sidebar, under even the 3:1 that a dot has
/// to clear. Running/stopped/crashed aren't here — running follows the
/// project's accent, and the other two read in both schemes as they are.
pub const STATUS_COLORS: &[(&str, &str, &str)] = &[
    ("status_working", "#e0a030", "#b06a00"),
    ("status_restarting", "#cca700", "#8a6f00"),
];

/// An accent by name, falling back to `fallback` for unknown names
/// (hand-edited settings, a palette entry we dropped) rather than to
/// nothing — a consumer needs *some* color or its rules drop entirely.
pub fn accent_by_name(name: &str, fallback: &str) -> &'static AccentColor {
    let by_name = |n: &str| ACCENT_COLORS.iter().find(|c| c.name == n);
    by_name(name)
        .or_else(|| by_name(fallback))
        .expect("fallback accent is in the palette")
}

pub fn accent_choices() -> Vec<&'static str> {
    ACCENT_COLORS.iter().map(|c| c.label).collect()
}

pub fn accent_index(name: &str) -> u32 {
    ACCENT_COLORS
        .iter()
        .position(|c| c.name == name)
        .unwrap_or(0) as u32
}

pub fn accent_name(index: u32) -> &'static str {
    ACCENT_COLORS
        .get(index as usize)
        .map(|c| c.name)
        .unwrap_or(FALLBACK_LOCAL)
}

// ── Terminal color schemes ──────────────────────────────────────────────

pub struct TerminalTheme {
    pub name: &'static str,
    pub label: &'static str,
    pub foreground: &'static str,
    pub background: &'static str,
    pub cursor: &'static str,
    /// The 16 ANSI colors, normal 0-7 then bright 8-15.
    pub palette: [&'static str; 16],
}

pub const TERMINAL_THEMES: &[TerminalTheme] = &[
    // -- Dark themes --
    TerminalTheme {
        name: "catppuccin-mocha",
        label: "Catppuccin Mocha (Default)",
        foreground: "#CDD6F4",
        background: "#1E1E2E",
        cursor: "#F5E0DC",
        palette: [
            "#45475A", "#F38BA8", "#A6E3A1", "#F9E2AF", "#89B4FA", "#F5C2E7", "#94E2D5", "#BAC2DE",
            "#585B70", "#F38BA8", "#A6E3A1", "#F9E2AF", "#89B4FA", "#F5C2E7", "#94E2D5", "#A6ADC8",
        ],
    },
    TerminalTheme {
        name: "dracula",
        label: "Dracula",
        foreground: "#F8F8F2",
        background: "#282A36",
        cursor: "#F8F8F2",
        palette: [
            "#21222C", "#FF5555", "#50FA7B", "#F1FA8C", "#BD93F9", "#FF79C6", "#8BE9FD", "#F8F8F2",
            "#6272A4", "#FF6E6E", "#69FF94", "#FFFFA5", "#D6ACFF", "#FF92DF", "#A4FFFF", "#FFFFFF",
        ],
    },
    TerminalTheme {
        name: "nord",
        label: "Nord",
        foreground: "#D8DEE9",
        background: "#2E3440",
        cursor: "#D8DEE9",
        palette: [
            "#3B4252", "#BF616A", "#A3BE8C", "#EBCB8B", "#81A1C1", "#B48EAD", "#88C0D0", "#E5E9F0",
            "#4C566A", "#BF616A", "#A3BE8C", "#EBCB8B", "#81A1C1", "#B48EAD", "#8FBCBB", "#ECEFF4",
        ],
    },
    TerminalTheme {
        name: "gruvbox-dark",
        label: "Gruvbox Dark",
        foreground: "#EBDBB2",
        background: "#282828",
        cursor: "#EBDBB2",
        palette: [
            "#282828", "#CC241D", "#98971A", "#D79921", "#458588", "#B16286", "#689D6A", "#A89984",
            "#928374", "#FB4934", "#B8BB26", "#FABD2F", "#83A598", "#D3869B", "#8EC07C", "#EBDBB2",
        ],
    },
    TerminalTheme {
        name: "one-dark",
        label: "One Dark",
        foreground: "#ABB2BF",
        background: "#282C34",
        cursor: "#528BFF",
        palette: [
            "#282C34", "#E06C75", "#98C379", "#E5C07B", "#61AFEF", "#C678DD", "#56B6C2", "#ABB2BF",
            "#545862", "#E06C75", "#98C379", "#E5C07B", "#61AFEF", "#C678DD", "#56B6C2", "#BE5046",
        ],
    },
    TerminalTheme {
        name: "tokyo-night",
        label: "Tokyo Night",
        foreground: "#C0CAF5",
        background: "#1A1B26",
        cursor: "#C0CAF5",
        palette: [
            "#15161E", "#F7768E", "#9ECE6A", "#E0AF68", "#7AA2F7", "#BB9AF7", "#7DCFFF", "#A9B1D6",
            "#414868", "#F7768E", "#9ECE6A", "#E0AF68", "#7AA2F7", "#BB9AF7", "#7DCFFF", "#C0CAF5",
        ],
    },
    TerminalTheme {
        name: "solarized-dark",
        label: "Solarized Dark",
        foreground: "#839496",
        background: "#002B36",
        cursor: "#93A1A1",
        palette: [
            "#073642", "#DC322F", "#859900", "#B58900", "#268BD2", "#D33682", "#2AA198", "#EEE8D5",
            "#002B36", "#CB4B16", "#586E75", "#657B83", "#839496", "#6C71C4", "#93A1A1", "#FDF6E3",
        ],
    },
    // -- Light themes --
    TerminalTheme {
        name: "catppuccin-latte",
        label: "Catppuccin Latte",
        foreground: "#4C4F69",
        background: "#EFF1F5",
        cursor: "#DC8A78",
        palette: [
            "#5C5F77", "#D20F39", "#40A02B", "#DF8E1D", "#1E66F5", "#EA76CB", "#179299", "#ACB0BE",
            "#6C6F85", "#D20F39", "#40A02B", "#DF8E1D", "#1E66F5", "#EA76CB", "#179299", "#4C4F69",
        ],
    },
    TerminalTheme {
        name: "solarized-light",
        label: "Solarized Light",
        foreground: "#657B83",
        background: "#FDF6E3",
        cursor: "#586E75",
        palette: [
            "#073642", "#DC322F", "#859900", "#B58900", "#268BD2", "#D33682", "#2AA198", "#EEE8D5",
            "#002B36", "#CB4B16", "#586E75", "#657B83", "#839496", "#6C71C4", "#93A1A1", "#FDF6E3",
        ],
    },
];

pub fn terminal_theme(name: &str) -> &'static TerminalTheme {
    TERMINAL_THEMES
        .iter()
        .find(|t| t.name == name)
        .unwrap_or(&TERMINAL_THEMES[0])
}

pub fn theme_choices() -> Vec<&'static str> {
    TERMINAL_THEMES.iter().map(|t| t.label).collect()
}

pub fn theme_index(name: &str) -> u32 {
    TERMINAL_THEMES
        .iter()
        .position(|t| t.name == name)
        .unwrap_or(0) as u32
}

pub fn theme_name(index: u32) -> &'static str {
    TERMINAL_THEMES
        .get(index as usize)
        .map(|t| t.name)
        .unwrap_or("catppuccin-mocha")
}

/// Parse "#rrggbb" into 0.0-1.0 channels. Data in this file is compile-time
/// constant and well-formed; unknown input gets black rather than a panic.
pub fn hex_rgb(hex: &str) -> (f32, f32, f32) {
    let h = hex.trim_start_matches('#');
    let chan = |i: usize| {
        u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f32 / 255.0
    };
    (chan(0), chan(2), chan(4))
}

pub fn is_dark_theme(name: &str) -> bool {
    let (r, g, b) = hex_rgb(terminal_theme(name).background);
    // Luminance approximation
    (0.299 * r + 0.587 * g + 0.114 * b) < 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance / WCAG contrast, so the palette's readability is
    /// asserted rather than eyeballed.
    fn luminance(hex: &str) -> f64 {
        let h = hex.trim_start_matches('#');
        let chan = |i: usize| {
            let v = u8::from_str_radix(&h[i..i + 2], 16).expect("hex pair") as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * chan(0) + 0.7152 * chan(2) + 0.0722 * chan(4)
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        let (hi, lo) = if x > y { (x, y) } else { (y, x) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// The surface these are read against. `.sidebar` is
    /// `alpha(@window_bg_color, 0.97)`, which screenshots measure as
    /// #fafafa light / #222226 dark — both further from the foreground
    /// than the values below, so asserting against these keeps a margin.
    const LIGHT_SIDEBAR: &str = "#ebebeb";
    const DARK_SIDEBAR: &str = "#303030";
    /// Text-sized UI needs 4.5:1; allow a hair under for the derived hues.
    const MIN_CONTRAST: f64 = 4.0;
    /// A status dot is a graphic, not text — WCAG asks 3:1 of it.
    const MIN_DOT_CONTRAST: f64 = 3.0;

    #[test]
    fn every_accent_is_readable_in_both_schemes() {
        for c in ACCENT_COLORS {
            let dark = contrast(c.accent, DARK_SIDEBAR);
            let light = contrast(c.accent_light, LIGHT_SIDEBAR);
            assert!(
                dark >= MIN_CONTRAST,
                "{} dark accent {} is {dark:.2}:1 on {DARK_SIDEBAR}",
                c.name,
                c.accent
            );
            assert!(
                light >= MIN_CONTRAST,
                "{} light accent {} is {light:.2}:1 on {LIGHT_SIDEBAR}",
                c.name,
                c.accent_light
            );
        }
    }

    /// The fixed status hues have no picker to escape a bad scheme with,
    /// so they carry the same burden of proof as the palette.
    #[test]
    fn status_dots_are_visible_in_both_schemes() {
        for (name, dark_hex, light_hex) in STATUS_COLORS {
            let dark = contrast(dark_hex, DARK_SIDEBAR);
            let light = contrast(light_hex, LIGHT_SIDEBAR);
            assert!(
                dark >= MIN_DOT_CONTRAST,
                "{name} dark {dark_hex} is {dark:.2}:1 on {DARK_SIDEBAR}"
            );
            assert!(
                light >= MIN_DOT_CONTRAST,
                "{name} light {light_hex} is {light:.2}:1 on {LIGHT_SIDEBAR}"
            );
        }
    }

    #[test]
    fn theme_lookup_falls_back_to_default() {
        assert_eq!(terminal_theme("no-such-theme").name, "catppuccin-mocha");
        assert!(is_dark_theme("catppuccin-mocha"));
        assert!(!is_dark_theme("solarized-light"));
    }

    #[test]
    fn hex_parses() {
        assert_eq!(hex_rgb("#ffffff"), (1.0, 1.0, 1.0));
        assert_eq!(hex_rgb("#000000"), (0.0, 0.0, 0.0));
    }
}
