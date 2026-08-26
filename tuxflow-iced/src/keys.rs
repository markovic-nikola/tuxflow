//! App-level keyboard shortcuts, honoring the user's keybinding strings
//! from the shared settings.toml (read-only — that file belongs to both
//! shells and this one never writes it).
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
    PrevProcess,
    NextProcess,
    PrevProject,
    NextProject,
    NewTerminal,
    CloseProcess,
    FontIncrease,
    FontDecrease,
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
        let sources: [(&String, AppAction); 10] = [
            (&kb.terminal_search, AppAction::TerminalSearch),
            (&kb.command_palette, AppAction::CommandPalette),
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
        // All ten shipped defaults must parse — a silent drop means a
        // shortcut the GTK app honors and this shell ignores.
        assert_eq!(keys().bindings.len(), 10);
    }

    #[test]
    fn matches_character_chord_case_insensitively() {
        let keys = keys();
        let key: LogicalKey = LogicalKey::Character("F".into());
        assert_eq!(
            keys.action_for(&key, Modifiers::CTRL | Modifiers::SHIFT),
            Some(AppAction::TerminalSearch)
        );
        // Extra modifier bits (caps lock &c.) are ignored, missing ones fail.
        assert_eq!(keys.action_for(&key, Modifiers::CTRL), None);
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
