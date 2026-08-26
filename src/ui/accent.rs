use std::cell::{Cell, RefCell};

use gtk4::gdk;
use libadwaita as adw;

use crate::config::settings::AppearanceSettings;
// The palette data (and its contrast tests) lives in core so both shells
// share one authoritative set of hues; this module is the GTK half —
// turning names into `@define-color` CSS at USER priority and re-rendering
// when the scheme flips.
use tuxflow_core::config::palette::{
    ACCENT_COLORS, FALLBACK_LOCAL, FALLBACK_REMOTE, STATUS_COLORS,
};

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
    let c = tuxflow_core::config::palette::accent_by_name(name, fallback);
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

pub use tuxflow_core::config::palette::{
    accent_choices as color_choices, accent_index as color_index, accent_name as color_name,
};

#[cfg(test)]
mod tests {
    use super::*;

    // The contrast assertions (accents and status dots against both
    // sidebar schemes) moved to core with the data they test —
    // tuxflow-core/src/config/palette.rs. What stays here is the CSS half.

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
