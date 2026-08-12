use std::cell::{Cell, RefCell};

use gtk4::gdk;
use libadwaita as adw;

use crate::config::settings::AppearanceSettings;

struct AccentColor {
    name: &'static str,
    label: &'static str,
    bg: &'static str,
    fg: &'static str,
    /// Text-weight accent, used against dark surfaces. Doubles as the
    /// sidebar hue for local/remote projects, which is why a few entries
    /// are tuned brighter than their `bg`: the sidebar reads them as text
    /// and thin borders, not as filled buttons.
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
/// `AppearanceSettings::default` carries the same names — it cannot reach
/// them from here, since `config` builds without `ui` (see src/lib.rs).
const FALLBACK_LOCAL: &str = "green";
const FALLBACK_REMOTE: &str = "yellow";

/// Status-dot hues that carry a fixed meaning rather than a chosen one,
/// as (name, dark, light). They still need the light twin: both ambers
/// measure ~2.2:1 on the light sidebar, under even the 3:1 that a dot has
/// to clear. Running/stopped/crashed aren't here — running follows the
/// project's accent, and the other two read in both schemes as they are.
const STATUS_COLORS: &[(&str, &str, &str)] = &[
    ("status_working", "#e0a030", "#b06a00"),
    ("status_restarting", "#cca700", "#8a6f00"),
];

/// The three accent choices in play: the app-wide accent plus the two
/// sidebar hues that tell local and remote projects apart.
#[derive(Clone)]
struct Accents {
    app: String,
    local: String,
    remote: String,
}

thread_local! {
    static PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
    /// Last accents passed to `apply`, replayed when the scheme flips.
    static CURRENT: RefCell<Accents> = const {
        RefCell::new(Accents {
            app: String::new(),
            local: String::new(),
            remote: String::new(),
        })
    };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

pub fn apply(appearance: &AppearanceSettings) {
    CURRENT.with(|c| {
        *c.borrow_mut() = Accents {
            app: appearance.accent_color.clone(),
            local: appearance.local_accent_color.clone(),
            remote: appearance.remote_accent_color.clone(),
        }
    });
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

/// A sidebar hue under one scheme. The sidebar reads these as text and as
/// alpha-blended tints, so it takes the text-weight `accent`, never `bg`.
/// An unknown name (hand-edited settings, a palette entry we dropped)
/// falls back to the shipped default rather than to nothing — the CSS
/// colour has to be defined or the whole rule is skipped by GTK.
fn sidebar_color(name: &str, fallback: &str, dark: bool) -> &'static str {
    let by_name = |n: &str| ACCENT_COLORS.iter().find(|c| c.name == n);
    let c = by_name(name)
        .or_else(|| by_name(fallback))
        .expect("fallback accent is in the palette");
    if dark { c.accent } else { c.accent_light }
}

/// The colour definitions for one set of accents under one scheme. Split
/// out from `render` so it can be tested without a display.
fn css_for(a: &Accents, dark: bool) -> String {
    let mut css = format!(
        "@define-color local_accent {};\n@define-color remote_accent {};\n",
        sidebar_color(&a.local, FALLBACK_LOCAL, dark),
        sidebar_color(&a.remote, FALLBACK_REMOTE, dark),
    );

    for (name, dark_hex, light_hex) in STATUS_COLORS {
        let hex = if dark { dark_hex } else { light_hex };
        css.push_str(&format!("@define-color {name} {hex};\n"));
    }

    if let Some(c) = ACCENT_COLORS.iter().find(|c| c.name == a.app)
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
    let css = CURRENT.with(|a| css_for(&a.borrow(), dark));

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

    fn accents(app: &str, local: &str, remote: &str) -> Accents {
        Accents {
            app: app.to_string(),
            local: local.to_string(),
            remote: remote.to_string(),
        }
    }

    #[test]
    fn css_switches_with_the_scheme() {
        let a = accents("green", "green", "yellow");
        let dark = css_for(&a, true);
        let light = css_for(&a, false);
        assert!(dark.contains("@define-color accent_color #73c991;"));
        assert!(light.contains("@define-color accent_color #1a7f37;"));
        assert!(dark.contains("remote_accent #ffce5c;"));
        assert!(light.contains("remote_accent #9a6700;"));
        assert!(dark.contains("status_working #e0a030;"));
        assert!(light.contains("status_working #b06a00;"));
        // The filled-button pair is scheme-independent by design.
        assert!(dark.contains("accent_bg_color #2ea043;"));
        assert!(light.contains("accent_bg_color #2ea043;"));
    }

    /// The shipped defaults are what the sidebar has always looked like:
    /// running green for local, logo gold for remote. A palette edit that
    /// moves either hue should have to say so here.
    #[test]
    fn defaults_keep_the_sidebar_identity_colors() {
        let d = AppearanceSettings::default();
        let css = css_for(
            &accents(
                &d.accent_color,
                &d.local_accent_color,
                &d.remote_accent_color,
            ),
            true,
        );
        assert!(css.contains("@define-color local_accent #73c991;"));
        assert!(css.contains("@define-color remote_accent #ffce5c;"));
    }

    /// The three choices are independent — picking a sidebar hue must not
    /// drag the app accent (or the other side) along with it.
    #[test]
    fn sidebar_accents_are_independent() {
        let css = css_for(&accents("blue", "purple", "red"), true);
        assert!(css.contains("@define-color local_accent #bd83d0;"));
        assert!(css.contains("@define-color remote_accent #ee7379;"));
        assert!(css.contains("@define-color accent_color #5d9de9;"));
    }

    /// An unknown name still has to define both sidebar colours, or the
    /// rules using them are dropped and the sidebar loses its accents.
    #[test]
    fn unknown_names_fall_back_to_the_defaults() {
        let css = css_for(&accents("chartreuse", "chartreuse", "chartreuse"), false);
        assert!(css.contains("@define-color local_accent #1a7f37;"));
        assert!(css.contains("@define-color remote_accent #9a6700;"));
        assert!(!css.contains("accent_color"));
    }
}
