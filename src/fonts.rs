//! Installed font families for the Settings font pickers, and the UI
//! font's application.
//!
//! The lists come from the font database iced already loaded (fontdb via
//! cosmic-text), so what a picker offers is exactly what the renderer
//! can resolve by name.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use iced::advanced::graphics::text::cosmic_text::fontdb;
use iced::advanced::graphics::text::font_system;

/// The saved value that means "whatever the system calls monospace".
pub const SYSTEM_MONOSPACE: &str = "Monospace";

/// What a picker chooses a font for. The two differ in the saved value
/// that means "the system's own" and in which families they list.
#[derive(Debug, Clone, Copy)]
pub struct Role {
    system: &'static str,
    /// List fixed-width families only; otherwise all, proportional first.
    fixed_width_only: bool,
}

/// `appearance.font_family`.
pub const TERMINAL: Role = Role {
    system: SYSTEM_MONOSPACE,
    fixed_width_only: true,
};

/// `appearance.ui_font_family`; empty is the system's sans-serif.
pub const UI: Role = Role {
    system: "",
    fixed_width_only: false,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The generic family, resolved by fontconfig.
    System,
    Installed,
    /// Saved in settings.toml but absent here. The file is synced between
    /// machines, so the name stays selectable and is never rewritten — the
    /// other machine still has the font.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontChoice {
    /// What gets saved and handed to the renderer.
    pub family: String,
    pub kind: Kind,
}

impl fmt::Display for FontChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            Kind::System if self.family.is_empty() => f.write_str("System default"),
            Kind::System => write!(f, "{} (system default)", self.family),
            Kind::Installed => f.write_str(&self.family),
            Kind::Missing => write!(f, "{} (not installed)", self.family),
        }
    }
}

/// A saved family as its picker sees it: empty means the role's system
/// family, as it always has for the terminal.
pub fn saved_family(role: Role, saved: &str) -> &str {
    match saved.trim() {
        "" => role.system,
        family => family,
    }
}

/// What the pickers need to know about one installed family.
#[derive(Debug, Clone)]
struct Family {
    name: String,
    /// The database's fixed-pitch flag, on any of its faces.
    monospaced: bool,
    /// Has the whole basic Latin alphabet, i.e. can draw this app.
    latin: bool,
}

/// Every installed family. Read once: the database is filled at startup
/// and never rescanned, so a font installed later shows after a restart.
fn installed() -> &'static [Family] {
    static INSTALLED: OnceLock<Vec<Family>> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let mut system = font_system().write().expect("font system lock");
        let db = system.raw().db();
        let mut families: BTreeMap<&str, Family> = BTreeMap::new();
        for face in db.faces() {
            let Some((name, _)) = face.families.first() else {
                continue;
            };
            families
                .entry(name)
                .and_modify(|family| family.monospaced |= face.monospaced)
                // One face speaks for the family's alphabet: weights and
                // styles share a character set, and each check maps a file.
                .or_insert_with(|| Family {
                    name: name.clone(),
                    monospaced: face.monospaced,
                    latin: db.with_face_data(face.id, covers_latin).unwrap_or(false),
                });
        }
        families.into_values().collect()
    })
}

fn covers_latin(data: &[u8], index: u32) -> bool {
    ttf_parser::Face::parse(data, index).is_ok_and(|face| {
        ('a'..='z')
            .chain('A'..='Z')
            .all(|letter| face.glyph_index(letter).is_some())
    })
}

/// The families `role`'s picker offers, `saved` included even when this
/// machine lacks it.
pub fn choices(role: Role, saved: &str) -> Vec<FontChoice> {
    arrange(installed().iter().cloned(), role, saved)
}

/// Make `saved` the font of every piece of UI text, live.
///
/// The UI draws in iced's default font, which is the GENERIC sans-serif
/// family, and iced fixes that default when the window is created. What
/// can change at runtime is the family the generic name resolves to, so
/// this repoints it in the font database. Nothing in the app names the UI
/// family, which is also why `theme::bold()` follows without being told.
///
/// A family this machine lacks (settings.toml is synced) resolves to the
/// system's own sans-serif, not to whatever the fallback chain finds.
pub fn apply_ui_family(saved: &str) {
    static SYSTEM_SANS: OnceLock<String> = OnceLock::new();
    let mut system = font_system().write().expect("font system lock");
    let db = system.raw().db();
    let original = SYSTEM_SANS
        .get_or_init(|| db.family_name(&fontdb::Family::SansSerif).to_string())
        .as_str();
    let saved = saved_family(UI, saved);
    // The database's own spelling: a query by name is exact.
    let family = db
        .faces()
        .filter_map(|face| face.families.first())
        .find(|(name, _)| !saved.is_empty() && name.eq_ignore_ascii_case(saved))
        .map_or_else(|| original.to_string(), |(name, _)| name.clone());
    if db.family_name(&fontdb::Family::SansSerif) == family {
        return;
    }
    // `db_mut` drops cosmic-text's match cache with it.
    system.raw().db_mut().set_sans_serif_family(family);
    // Text already on screen keeps its shaping until the font system's
    // version moves, and loading a font is the only thing that moves it.
    // An empty source adds no face (fontdb logs one warning for it).
    system.load_font(Cow::Owned(Vec::new()));
}

/// Order: the system family, a missing saved family, then the installed
/// ones alphabetically. Only families that can draw Latin text are
/// offered — most of an ordinary install is per-script faces (some 190
/// Noto families here), symbols and math, none of which this UI can be
/// read in. The terminal narrows that to fixed-width families; the UI
/// lists proportional ones ahead of fixed-width ones.
///
/// "Fixed-width" is `fixed_width`'s call, not the database's flag alone,
/// which misses real terminal fonts (Noto Sans Mono, Latin Modern Mono).
/// A saved family is listed whatever it is, so a font chosen by
/// hand-editing settings.toml never shows as an empty picker.
fn arrange(
    installed: impl IntoIterator<Item = Family>,
    role: Role,
    saved: &str,
) -> Vec<FontChoice> {
    // Keyed by lowercase name: alphabetical regardless of case.
    let mut families: BTreeMap<String, Family> = installed
        .into_iter()
        // Dot-prefixed families are fontconfig's hidden system fonts.
        .filter(|family| !family.name.is_empty() && !family.name.starts_with('.'))
        .map(|family| (family.name.to_lowercase(), family))
        .collect();

    let saved = saved_family(role, saved);
    let mut choices = vec![FontChoice {
        family: role.system.to_string(),
        kind: Kind::System,
    }];
    // The generic name is the first row whatever case it was saved in, and
    // a real family of that name would be unreachable behind it anyway.
    families.remove(&role.system.to_lowercase());
    let saved_key = saved.to_lowercase();
    if !saved.eq_ignore_ascii_case(role.system) && !families.contains_key(&saved_key) {
        choices.push(FontChoice {
            family: saved.to_string(),
            kind: Kind::Missing,
        });
    }
    let (mut fixed, mut other) = (Vec::new(), Vec::new());
    for (key, family) in families {
        let is_fixed = fixed_width(&family.name, family.monospaced);
        let offered = family.latin && (is_fixed || !role.fixed_width_only);
        if offered || key == saved_key {
            if is_fixed { &mut fixed } else { &mut other }.push(family.name);
        }
    }
    // A terminal's `other` is at most the saved family.
    choices.extend(other.into_iter().chain(fixed).map(|family| FontChoice {
        family,
        kind: Kind::Installed,
    }));
    choices
}

/// Whether a family belongs in a terminal's list: the face's fixed-pitch
/// flag, corrected by the name in both directions. Nerd Fonts' plain
/// variants are terminal fonts whose wide icons cost them the flag, the
/// "Propo"/"Prop" variants of mono families are proportional by design,
/// and emoji fonts are flagged without being text fonts at all.
fn fixed_width(family: &str, flagged: bool) -> bool {
    let name = family.to_lowercase();
    let named = name.contains("mono") || name.contains("nerd font");
    (flagged || named) && !name.contains("prop") && !name.contains("emoji")
}

/// The row for the saved family. Matching ignores case so a hand-edited
/// `jetbrains mono` shows as the installed font it names, not as missing.
pub fn selected<'a>(choices: &'a [FontChoice], role: Role, saved: &str) -> Option<&'a FontChoice> {
    let saved = saved_family(role, saved);
    choices
        .iter()
        .find(|choice| choice.family.eq_ignore_ascii_case(saved))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn faces(list: &[(&str, bool)]) -> Vec<Family> {
        list.iter()
            .map(|(name, monospaced)| Family {
                name: name.to_string(),
                monospaced: *monospaced,
                latin: true,
            })
            .collect()
    }

    fn names(choices: &[FontChoice]) -> Vec<String> {
        choices.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_terminal_list_is_fixed_width_families_only() {
        let choices = arrange(
            faces(&[
                ("Ubuntu", false),
                ("Noto Sans Mono", false),
                ("DejaVu Sans Mono", true),
                ("DejaVu Sans Mono", true),
                ("Arial", false),
                ("Courier", true),
                (".Hidden UI", false),
            ]),
            TERMINAL,
            "",
        );
        assert_eq!(
            names(&choices),
            [
                "Monospace (system default)",
                "Courier",
                "DejaVu Sans Mono",
                "Noto Sans Mono",
            ]
        );
    }

    #[test]
    fn families_without_the_latin_alphabet_are_left_out_unless_saved() {
        let mut installed = faces(&[("Noto Sans Bengali", false), ("Symbols Mono", true)]);
        for family in &mut installed {
            family.latin = false;
        }
        installed.extend(faces(&[("Inter", false), ("Hack", true)]));

        let ui = arrange(installed.clone(), UI, "");
        assert_eq!(names(&ui), ["System default", "Inter", "Hack"]);
        let terminal = arrange(installed.clone(), TERMINAL, "");
        assert_eq!(names(&terminal), ["Monospace (system default)", "Hack"]);

        let ui = arrange(installed, UI, "Noto Sans Bengali");
        assert_eq!(
            names(&ui),
            ["System default", "Inter", "Noto Sans Bengali", "Hack"]
        );
    }

    #[test]
    fn a_saved_proportional_font_stays_in_the_terminal_list() {
        let choices = arrange(
            faces(&[("Ubuntu", false), ("Arial", false), ("Courier", true)]),
            TERMINAL,
            "ubuntu",
        );
        assert_eq!(
            names(&choices),
            ["Monospace (system default)", "Ubuntu", "Courier"]
        );
        assert_eq!(
            selected(&choices, TERMINAL, "ubuntu").unwrap().family,
            "Ubuntu"
        );
    }

    #[test]
    fn a_saved_font_missing_here_stays_selectable_under_its_own_name() {
        let choices = arrange(
            faces(&[("Courier", true)]),
            TERMINAL,
            "MesloLGL Nerd Font Mono",
        );
        assert_eq!(
            names(&choices),
            [
                "Monospace (system default)",
                "MesloLGL Nerd Font Mono (not installed)",
                "Courier",
            ]
        );
        let picked = selected(&choices, TERMINAL, "MesloLGL Nerd Font Mono").unwrap();
        assert_eq!(picked.kind, Kind::Missing);
        assert_eq!(picked.family, "MesloLGL Nerd Font Mono");
    }

    #[test]
    fn selection_ignores_case_and_empty_means_system() {
        let choices = arrange(
            faces(&[("JetBrains Mono", true)]),
            TERMINAL,
            "jetbrains mono",
        );
        assert_eq!(
            choices.len(),
            2,
            "an installed font is not listed as missing"
        );
        assert_eq!(
            selected(&choices, TERMINAL, "jetbrains mono")
                .unwrap()
                .family,
            "JetBrains Mono"
        );
        assert_eq!(selected(&choices, TERMINAL, "").unwrap().kind, Kind::System);
        assert_eq!(
            selected(&choices, TERMINAL, "monospace").unwrap().kind,
            Kind::System
        );
    }

    #[test]
    fn the_ui_list_leads_with_proportional_families_and_an_unnamed_system_row() {
        let choices = arrange(
            faces(&[("Ubuntu", false), ("Courier", true), ("Arial", false)]),
            UI,
            " ",
        );
        assert_eq!(
            names(&choices),
            ["System default", "Arial", "Ubuntu", "Courier"]
        );
        let picked = selected(&choices, UI, "").unwrap();
        assert_eq!((picked.kind, picked.family.as_str()), (Kind::System, ""));

        let choices = arrange(faces(&[("Arial", false)]), UI, "Inter");
        assert_eq!(names(&choices)[1], "Inter (not installed)");
    }

    #[test]
    fn the_name_corrects_the_fixed_pitch_flag() {
        assert!(fixed_width("Courier", true));
        assert!(fixed_width("Noto Sans Mono", false));
        assert!(fixed_width("MesloLGS Nerd Font", false));
        assert!(!fixed_width("JetBrainsMono Nerd Font Propo", false));
        assert!(!fixed_width("Latin Modern Mono Prop", true));
        assert!(!fixed_width("Noto Color Emoji", true));
        assert!(!fixed_width("Ubuntu", false));
    }
}
