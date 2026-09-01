//! App-level keyboard shortcuts, honoring the user's keybinding strings
//! from the shared settings.toml (the settings window edits them; this
//! module parses and matches them).
//!
//! Every chord the app handles is also RESERVED in each terminal via the
//! fork's `BindingAction::Passthrough`: the stock bindings map the whole
//! Ctrl+Shift alphabet (and Ctrl+arrows &c.) to terminal input, so
//! without the reservation a chord would be typed into the shell and the
//! app would never see it.

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use tuxflow_core::config::keybindings::KeybindingsSettings;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppAction {
    TerminalSearch,
    CommandPalette,
    Settings,
    PrevProcess,
    NextProcess,
    PrevProject,
    NextProject,
    NewTerminal,
    CloseProcess,
    FontIncrease,
    FontDecrease,
    MoveProcessUp,
    MoveProcessDown,
    ToggleSidebar,
    FilterSidebar,
    /// Ctrl+1..9 — the GTK app's fixed process switcher.
    SelectProcessN(u8),
    /// Alt+1..9 — the GTK app's fixed project switcher.
    SelectProjectN(u8),
}

#[derive(Clone, Debug, PartialEq)]
enum ChordKey {
    Char(String),
    Named(Named),
}

#[derive(Clone, Debug)]
struct Chord {
    modifiers: Modifiers,
    key: ChordKey,
}

pub struct AppKeys {
    bindings: Vec<(Chord, AppAction)>,
}

impl AppKeys {
    pub fn from_settings(kb: &KeybindingsSettings) -> Self {
        let sources: [(&String, AppAction); 13] = [
            (&kb.terminal_search, AppAction::TerminalSearch),
            (&kb.toggle_sidebar, AppAction::ToggleSidebar),
            (&kb.filter_processes, AppAction::FilterSidebar),
            (&kb.command_palette, AppAction::CommandPalette),
            (&kb.settings, AppAction::Settings),
            (&kb.prev_process, AppAction::PrevProcess),
            (&kb.next_process, AppAction::NextProcess),
            (&kb.prev_project, AppAction::PrevProject),
            (&kb.next_project, AppAction::NextProject),
            (&kb.new_terminal, AppAction::NewTerminal),
            (&kb.close_process, AppAction::CloseProcess),
            (&kb.font_increase, AppAction::FontIncrease),
            (&kb.font_decrease, AppAction::FontDecrease),
        ];
        let mut bindings = Vec::new();
        for (raw, action) in sources {
            match parse(raw) {
                Some(chord) => bindings.push((chord, action)),
                None => log::warn!("unparseable keybinding {raw:?} for {action:?}"),
            }
        }
        // Built-ins with no settings key (yet): process reorder — the
        // keyboard stand-in for the GTK sidebar's drag-and-drop.
        for (raw, action) in [
            ("Alt+Shift+Up", AppAction::MoveProcessUp),
            ("Alt+Shift+Down", AppAction::MoveProcessDown),
        ] {
            if let Some(chord) = parse(raw) {
                bindings.push((chord, action));
            }
        }
        // The GTK app's fixed switchers: Ctrl+N processes, Alt+N projects.
        for n in 1..=9u8 {
            let digit = ChordKey::Char(n.to_string());
            bindings.push((
                Chord {
                    modifiers: Modifiers::CTRL,
                    key: digit.clone(),
                },
                AppAction::SelectProcessN(n),
            ));
            bindings.push((
                Chord {
                    modifiers: Modifiers::ALT,
                    key: digit,
                },
                AppAction::SelectProjectN(n),
            ));
        }
        Self { bindings }
    }

    pub fn action_for(&self, key: &Key, modifiers: Modifiers) -> Option<AppAction> {
        let mods = relevant(modifiers);
        self.bindings
            .iter()
            .find(|(chord, _)| chord.modifiers == mods && chord_matches(&chord.key, key))
            .map(|(_, action)| *action)
    }

    /// Passthrough reservations to install on every terminal at spawn.
    pub fn reservations(
        &self,
    ) -> Vec<(
        iced_term::bindings::Binding<iced_term::bindings::InputKind>,
        iced_term::bindings::BindingAction,
    )> {
        self.bindings
            .iter()
            .map(|(chord, _)| {
                let target = match &chord.key {
                    ChordKey::Char(c) => iced_term::bindings::InputKind::Char(c.clone()),
                    ChordKey::Named(n) => iced_term::bindings::InputKind::KeyCode(*n),
                };
                (
                    iced_term::bindings::Binding {
                        target,
                        modifiers: chord.modifiers,
                        terminal_mode_include: iced_term::TermMode::empty(),
                        terminal_mode_exclude: iced_term::TermMode::empty(),
                    },
                    iced_term::bindings::BindingAction::Passthrough,
                )
            })
            .collect()
    }
}

/// Keep only the modifier bits chords are defined over.
fn relevant(m: Modifiers) -> Modifiers {
    let mut out = Modifiers::empty();
    if m.control() {
        out |= Modifiers::CTRL;
    }
    if m.shift() {
        out |= Modifiers::SHIFT;
    }
    if m.alt() {
        out |= Modifiers::ALT;
    }
    out
}

fn chord_matches(chord: &ChordKey, key: &Key) -> bool {
    match (chord, key.as_ref()) {
        (ChordKey::Char(c), Key::Character(k)) => k.eq_ignore_ascii_case(c),
        (ChordKey::Named(n), Key::Named(k)) => *n == k,
        _ => false,
    }
}

/// The inverse of `parse`, for the settings capture UI: a pressed key +
/// modifiers as the settings string form, or `None` for what a chord
/// can't express (modifier-only presses, media keys &c.). The output must
/// round-trip through `parse` — that's what the unit test pins.
pub fn chord_string(key: &Key, modifiers: Modifiers) -> Option<String> {
    let key_part = match key.as_ref() {
        Key::Named(n) => match n {
            Named::ArrowUp => "Up".to_string(),
            Named::ArrowDown => "Down".to_string(),
            Named::ArrowLeft => "Left".to_string(),
            Named::ArrowRight => "Right".to_string(),
            Named::Space => "Space".to_string(),
            Named::Enter => "Return".to_string(),
            Named::Tab => "Tab".to_string(),
            Named::Backspace => "Backspace".to_string(),
            Named::Delete => "Delete".to_string(),
            Named::Home => "Home".to_string(),
            Named::End => "End".to_string(),
            Named::PageUp => "PageUp".to_string(),
            Named::PageDown => "PageDown".to_string(),
            Named::F1 => "F1".to_string(),
            Named::F2 => "F2".to_string(),
            Named::F3 => "F3".to_string(),
            Named::F4 => "F4".to_string(),
            Named::F5 => "F5".to_string(),
            Named::F6 => "F6".to_string(),
            Named::F7 => "F7".to_string(),
            Named::F8 => "F8".to_string(),
            Named::F9 => "F9".to_string(),
            Named::F10 => "F10".to_string(),
            Named::F11 => "F11".to_string(),
            Named::F12 => "F12".to_string(),
            _ => return None,
        },
        Key::Character(c) => {
            let mut chars = c.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                return None;
            };
            match ch {
                // '+' is the separator, so as a KEY it has to carry a name —
                // "Ctrl++" splits into an empty part and can never re-parse.
                // GTK writes/reads the same name.
                '+' => String::from("Plus"),
                // ASCII-only uppercasing: `to_uppercase` on ß expands to
                // "SS", which parses to a two-char chord nothing can press.
                ch if ch.is_ascii() => ch.to_ascii_uppercase().to_string(),
                ch => ch.to_string(),
            }
        }
        Key::Unidentified => return None,
    };
    // Same order GTK's `keybinding_to_string` writes — the shells share the
    // settings file, and the conflict check compares these as strings.
    let mut out = String::new();
    if modifiers.control() {
        out.push_str("Ctrl+");
    }
    if modifiers.shift() {
        out.push_str("Shift+");
    }
    if modifiers.alt() {
        out.push_str("Alt+");
    }
    // A chord needs a real modifier or a function key — a bare letter would
    // shadow typing everywhere. Only F1–F12 pass (len > 1): the letter F
    // itself must not slip through this gate, or capturing it installs a
    // modifier-less Passthrough that eats every `f` in every terminal.
    if out.is_empty() && !(key_part.len() > 1 && key_part.starts_with('F')) {
        return None;
    }
    out.push_str(&key_part);
    Some(out)
}

/// Parse the settings string form ("Ctrl+Shift+F", "Ctrl+=", "Ctrl+Up").
fn parse(raw: &str) -> Option<Chord> {
    let parts: Vec<&str> = raw.split('+').map(str::trim).collect();
    let (key_part, mod_parts) = parts.split_last()?;
    let mut modifiers = Modifiers::empty();
    for part in mod_parts {
        match *part {
            "Ctrl" => modifiers |= Modifiers::CTRL,
            "Shift" => modifiers |= Modifiers::SHIFT,
            "Alt" => modifiers |= Modifiers::ALT,
            _ => return None,
        }
    }
    let key = match *key_part {
        "Up" => ChordKey::Named(Named::ArrowUp),
        "Down" => ChordKey::Named(Named::ArrowDown),
        "Left" => ChordKey::Named(Named::ArrowLeft),
        "Right" => ChordKey::Named(Named::ArrowRight),
        "Space" => ChordKey::Named(Named::Space),
        "Return" => ChordKey::Named(Named::Enter),
        "Tab" => ChordKey::Named(Named::Tab),
        "Backspace" => ChordKey::Named(Named::Backspace),
        "Delete" => ChordKey::Named(Named::Delete),
        "Home" => ChordKey::Named(Named::Home),
        "End" => ChordKey::Named(Named::End),
        "PageUp" => ChordKey::Named(Named::PageUp),
        "PageDown" => ChordKey::Named(Named::PageDown),
        "Escape" => ChordKey::Named(Named::Escape),
        "F1" => ChordKey::Named(Named::F1),
        "F2" => ChordKey::Named(Named::F2),
        "F3" => ChordKey::Named(Named::F3),
        "F4" => ChordKey::Named(Named::F4),
        "F5" => ChordKey::Named(Named::F5),
        "F6" => ChordKey::Named(Named::F6),
        "F7" => ChordKey::Named(Named::F7),
        "F8" => ChordKey::Named(Named::F8),
        "F9" => ChordKey::Named(Named::F9),
        "F10" => ChordKey::Named(Named::F10),
        "F11" => ChordKey::Named(Named::F11),
        "F12" => ChordKey::Named(Named::F12),
        // The named form of the separator character (see `chord_string`).
        "Plus" => ChordKey::Char("+".to_string()),
        other if !other.is_empty() => ChordKey::Char(other.to_lowercase()),
        _ => return None,
    };
    Some(Chord { modifiers, key })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::Key as LogicalKey;

    fn keys() -> AppKeys {
        AppKeys::from_settings(&KeybindingsSettings::default())
    }

    #[test]
    fn default_chords_parse() {
        // All shipped defaults must parse — a silent drop means a
        // shortcut the GTK app honors and this shell ignores.
        // 13 settings-backed + 2 reorder built-ins + 18 digit switchers.
        assert_eq!(keys().bindings.len(), 33);
    }

    #[test]
    fn digit_switchers_match() {
        let keys = keys();
        let key: LogicalKey = LogicalKey::Character("3".into());
        assert_eq!(
            keys.action_for(&key, Modifiers::CTRL),
            Some(AppAction::SelectProcessN(3))
        );
        assert_eq!(
            keys.action_for(&key, Modifiers::ALT),
            Some(AppAction::SelectProjectN(3))
        );
    }

    #[test]
    fn chord_string_round_trips_through_parse() {
        let cases: [(LogicalKey, Modifiers, &str); 6] = [
            (
                LogicalKey::Character("f".into()),
                Modifiers::CTRL | Modifiers::SHIFT,
                "Ctrl+Shift+F",
            ),
            (
                LogicalKey::Named(Named::ArrowUp),
                Modifiers::CTRL,
                "Ctrl+Up",
            ),
            (LogicalKey::Character(",".into()), Modifiers::CTRL, "Ctrl+,"),
            // Three-modifier chords use GTK's Ctrl,Shift,Alt order — the
            // shells share the file and the conflict check is a string
            // compare, so a second spelling of one chord is a drift.
            (
                LogicalKey::Character("c".into()),
                Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT,
                "Ctrl+Shift+Alt+C",
            ),
            // The separator as a key needs its named form to survive parse.
            (
                LogicalKey::Character("+".into()),
                Modifiers::CTRL,
                "Ctrl+Plus",
            ),
            // Non-ASCII keys pass through unexpanded (ß must not become SS).
            (
                LogicalKey::Character("\u{df}".into()),
                Modifiers::CTRL,
                "Ctrl+\u{df}",
            ),
        ];
        for (key, mods, want) in cases {
            let s = chord_string(&key, mods).expect("expressible chord");
            assert_eq!(s, want);
            let chord = parse(&s).expect("round-trips");
            assert_eq!(chord.modifiers, mods);
            assert!(chord_matches(&chord.key, &key));
        }
        // Inexpressible: bare letters and lone modifiers. The letter F is
        // the trap — the F1-F12 exemption must not admit it, or capturing
        // it installs a modifier-less binding that eats every typed `f`.
        assert_eq!(
            chord_string(&LogicalKey::Character("x".into()), Modifiers::empty()),
            None
        );
        assert_eq!(
            chord_string(&LogicalKey::Character("f".into()), Modifiers::empty()),
            None
        );
        assert!(chord_string(&LogicalKey::Named(Named::F5), Modifiers::empty()).is_some());
    }

    #[test]
    fn matches_character_chord_case_insensitively() {
        let keys = keys();
        let key: LogicalKey = LogicalKey::Character("F".into());
        assert_eq!(
            keys.action_for(&key, Modifiers::CTRL | Modifiers::SHIFT),
            Some(AppAction::TerminalSearch)
        );
        // A dropped modifier lands on the *other* binding sharing the key
        // (Ctrl+F filters the sidebar, GTK parity), never on this one.
        assert_eq!(
            keys.action_for(&key, Modifiers::CTRL),
            Some(AppAction::FilterSidebar)
        );
        assert_eq!(keys.action_for(&key, Modifiers::SHIFT), None);
    }

    #[test]
    fn matches_named_chord() {
        let keys = keys();
        let key: LogicalKey = LogicalKey::Named(Named::ArrowUp);
        assert_eq!(
            keys.action_for(&key, Modifiers::CTRL),
            Some(AppAction::PrevProcess)
        );
        assert_eq!(
            keys.action_for(&key, Modifiers::CTRL | Modifiers::SHIFT),
            Some(AppAction::PrevProject)
        );
    }

    #[test]
    fn reservations_cover_every_binding() {
        let keys = keys();
        let reservations = keys.reservations();
        assert_eq!(reservations.len(), keys.bindings.len());
        assert!(
            reservations.iter().all(|(_, action)| matches!(
                action,
                iced_term::bindings::BindingAction::Passthrough
            ))
        );
    }
}
