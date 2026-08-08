use std::cell::{Cell, RefCell};

use gtk4::gdk;
use libadwaita as adw;

struct AccentColor {
    name: &'static str,
    label: &'static str,
    bg: &'static str,
    fg: &'static str,
    /// Text-weight accent, used against dark surfaces.
    accent: &'static str,
    /// Same hue darkened for light surfaces. `accent` is tuned for dark mode
    /// and lands at 2-3:1 against libadwaita's light sidebar (#ebebeb) — these
    /// hit ~4.2:1 while keeping the hue recognisable. `bg`/`fg` need no
    /// variant: they are a filled button, not text, and already pair.
    accent_light: &'static str,
}

const ACCENT_COLORS: &[AccentColor] = &[
    AccentColor {
        name: "green",
        label: "Green (Default)",
        bg: "#2ea043",
        fg: "#ffffff",
        accent: "#3fb950",
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
        accent: "#c88800",
        accent_light: "#956500",
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

/// Remote-project accent (the logo yellow). Defined here rather than in
/// style.css so it can follow the colour scheme; style.css keeps the dark
/// value as the pre-`apply` fallback. On light surfaces #ffce5c measures
/// 1.24:1 — near-invisible — so light mode drops to a dark gold that still
/// reads as "remote" instead of turning into another green.
const REMOTE_ACCENT: &str = "#ffce5c";
const REMOTE_ACCENT_LIGHT: &str = "#9a6700";

thread_local! {
    static PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
    /// Last accent name passed to `apply`, replayed when the scheme flips.
    static CURRENT: RefCell<String> = const { RefCell::new(String::new()) };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

pub fn apply(name: &str) {
    CURRENT.with(|c| name.clone_into(&mut c.borrow_mut()));
    watch_color_scheme();
    render();
}

/// Re-render whenever the resolved scheme changes. libadwaita reports the
/// *resolved* value, so this covers the system flipping under
/// `ColorScheme::Default` as well as the user switching theme in settings.
/// Installed from `apply` so callers can't forget it.
fn watch_color_scheme() {
    if WATCHING.with(|w| w.replace(true)) {
        return;
    }
    adw::StyleManager::default().connect_dark_notify(|_| render());
}

/// The colour definitions for one accent under one scheme. Split out from
/// `render` so it can be tested without a display.
fn css_for(name: &str, dark: bool) -> String {
    let mut css = format!(
        "@define-color remote_accent {};\n",
        if dark {
            REMOTE_ACCENT
        } else {
            REMOTE_ACCENT_LIGHT
        }
    );

    if let Some(c) = ACCENT_COLORS.iter().find(|c| c.name == name)
        && !c.bg.is_empty()
    {
        let accent = if dark { c.accent } else { c.accent_light };
        css.push_str(&format!(
            "@define-color accent_bg_color {};\n\
             @define-color accent_fg_color {};\n\
             @define-color accent_color {};",
            c.bg, c.fg, accent,
        ));
    }
    css
}

fn render() {
    let dark = adw::StyleManager::default().is_dark();
    let css = CURRENT.with(|n| css_for(&n.borrow(), dark));

    PROVIDER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let provider = slot.get_or_insert_with(|| {
            let p = gtk4::CssProvider::new();
            gtk4::style_context_add_provider_for_display(
                &gdk::Display::default().expect("No display"),
                &p,
                800, // STYLE_PROVIDER_PRIORITY_USER, above APPLICATION (600)
            );
            p
        });
        provider.load_from_string(&css);
    });
}

pub fn color_choices() -> Vec<&'static str> {
    ACCENT_COLORS.iter().map(|c| c.label).collect()
}

pub fn color_index(name: &str) -> u32 {
    ACCENT_COLORS
        .iter()
        .position(|c| c.name == name)
        .unwrap_or(0) as u32
}

pub fn color_name(index: u32) -> &'static str {
    ACCENT_COLORS
        .get(index as usize)
        .map(|c| c.name)
        .unwrap_or("green")
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

    /// libadwaita's sidebar backgrounds, the surface these are read against.
    const LIGHT_SIDEBAR: &str = "#ebebeb";
    const DARK_SIDEBAR: &str = "#303030";
    /// Text-sized UI needs 4.5:1; allow a hair under for the derived hues.
    const MIN_CONTRAST: f64 = 4.0;

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

    #[test]
    fn remote_accent_is_readable_in_both_schemes() {
        assert!(contrast(REMOTE_ACCENT, DARK_SIDEBAR) >= MIN_CONTRAST);
        assert!(contrast(REMOTE_ACCENT_LIGHT, LIGHT_SIDEBAR) >= MIN_CONTRAST);
    }

    #[test]
    fn css_switches_with_the_scheme() {
        let dark = css_for("green", true);
        let light = css_for("green", false);
        assert!(dark.contains("@define-color accent_color #3fb950;"));
        assert!(light.contains("@define-color accent_color #1a7f37;"));
        assert!(dark.contains(&format!("remote_accent {REMOTE_ACCENT};")));
        assert!(light.contains(&format!("remote_accent {REMOTE_ACCENT_LIGHT};")));
        // The filled-button pair is scheme-independent by design.
        assert!(dark.contains("accent_bg_color #2ea043;"));
        assert!(light.contains("accent_bg_color #2ea043;"));
    }

    /// An unknown name still has to define remote_accent, or remote projects
    /// fall back to the stale dark yellow from style.css.
    #[test]
    fn unknown_accent_still_defines_remote() {
        let css = css_for("chartreuse", false);
        assert!(css.contains(&format!("remote_accent {REMOTE_ACCENT_LIGHT};")));
        assert!(!css.contains("accent_color"));
    }
}
