use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use gtk4::gdk;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutAction {
    Copy,
    Paste,
    TerminalSearch,
    CommandPalette,
    AddNew,
    FilterProcesses,
    Settings,
    FocusSidebar,
    FocusTerminal,
    PrevProcess,
    NextProcess,
    FontIncrease,
    FontDecrease,
    QuickJump,
    NewTerminal,
    CloseProcess,
    PrevProject,
    NextProject,
    ClearOutput,
    ToggleProcess,
    RestartProcess,
    ToggleSidebar,
}

/// Returns (action, display_name, category) for all configurable actions.
pub fn action_metadata() -> Vec<(ShortcutAction, &'static str, &'static str)> {
    vec![
        (ShortcutAction::CommandPalette, "Command Palette", "General"),
        (ShortcutAction::AddNew, "Add Project or Process", "General"),
        (ShortcutAction::Settings, "Settings", "General"),
        (
            ShortcutAction::FilterProcesses,
            "Filter Processes",
            "General",
        ),
        (ShortcutAction::TerminalSearch, "Terminal Search", "General"),
        (ShortcutAction::Copy, "Copy", "General"),
        (ShortcutAction::Paste, "Paste", "General"),
        (ShortcutAction::FocusSidebar, "Focus Sidebar", "General"),
        (ShortcutAction::FocusTerminal, "Focus Terminal", "General"),
        (ShortcutAction::NewTerminal, "New Terminal", "General"),
        (
            ShortcutAction::CloseProcess,
            "Close Agent/Terminal",
            "General",
        ),
        (ShortcutAction::QuickJump, "Quick Jump", "Navigation"),
        (
            ShortcutAction::PrevProcess,
            "Previous Process",
            "Navigation",
        ),
        (ShortcutAction::NextProcess, "Next Process", "Navigation"),
        (
            ShortcutAction::PrevProject,
            "Previous Project",
            "Navigation",
        ),
        (ShortcutAction::NextProject, "Next Project", "Navigation"),
        (ShortcutAction::ClearOutput, "Clear Output", "General"),
        (
            ShortcutAction::ToggleProcess,
            "Start/Stop Process",
            "General",
        ),
        (ShortcutAction::RestartProcess, "Restart Process", "General"),
        (ShortcutAction::ToggleSidebar, "Toggle Sidebar", "General"),
        (
            ShortcutAction::FontIncrease,
            "Increase Font Size",
            "Terminal",
        ),
        (
            ShortcutAction::FontDecrease,
            "Decrease Font Size",
            "Terminal",
        ),
    ]
}

// ---------------------------------------------------------------------------
// Keybinding — runtime representation (gdk types for direct matching)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybinding {
    pub modifiers: gdk::ModifierType,
    pub key: gdk::Key,
}

const RELEVANT_MODIFIERS: gdk::ModifierType = gdk::ModifierType::from_bits_truncate(
    gdk::ModifierType::CONTROL_MASK.bits()
        | gdk::ModifierType::SHIFT_MASK.bits()
        | gdk::ModifierType::ALT_MASK.bits(),
);

fn normalize_modifiers(mods: gdk::ModifierType) -> gdk::ModifierType {
    mods & RELEVANT_MODIFIERS
}

impl Keybinding {
    pub fn matches(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
        let normalized = normalize_modifiers(modifiers);
        if self.modifiers != normalized {
            return false;
        }
        // For letter keys, GTK reports uppercase when Shift is held.
        // Compare case-insensitively for letters.
        let self_lower = self.key.to_lower();
        let other_lower = key.to_lower();
        self_lower == other_lower
    }
}

impl fmt::Display for Keybinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", keybinding_to_string(self))
    }
}

// ---------------------------------------------------------------------------
// String parsing / formatting
// ---------------------------------------------------------------------------

/// Parse a string like "Ctrl+Shift+C" into a Keybinding.
pub fn parse_keybinding(s: &str) -> Option<Keybinding> {
    let parts: Vec<&str> = s.split('+').collect();
    if parts.is_empty() {
        return None;
    }

    let mut modifiers = gdk::ModifierType::empty();
    let key_part = parts.last()?;

    for &part in &parts[..parts.len() - 1] {
        match part.trim() {
            "Ctrl" => modifiers |= gdk::ModifierType::CONTROL_MASK,
            "Shift" => modifiers |= gdk::ModifierType::SHIFT_MASK,
            "Alt" => modifiers |= gdk::ModifierType::ALT_MASK,
            _ => return None,
        }
    }

    let key = friendly_name_to_key(key_part.trim())?;
    Some(Keybinding { modifiers, key })
}

/// Convert a Keybinding back to a display string like "Ctrl+Shift+C".
pub fn keybinding_to_string(kb: &Keybinding) -> String {
    let mut parts = Vec::new();

    if kb.modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        parts.push("Ctrl");
    }
    if kb.modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
        parts.push("Shift");
    }
    if kb.modifiers.contains(gdk::ModifierType::ALT_MASK) {
        parts.push("Alt");
    }

    parts.push(key_to_friendly_name(&kb.key));
    parts.join("+")
}

/// Build a Keybinding from a raw GTK key event (used during key capture).
pub fn keybinding_from_event(key: gdk::Key, modifiers: gdk::ModifierType) -> Keybinding {
    // For letter keys with Shift, GTK reports the uppercase variant.
    // We store the uppercase key when Shift is part of the binding.
    let normalized_mods = normalize_modifiers(modifiers);
    Keybinding {
        modifiers: normalized_mods,
        key,
    }
}

pub fn is_modifier_key(key: &gdk::Key) -> bool {
    matches!(
        *key,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::ISO_Level3_Shift
    )
}

// Friendly name <-> gdk::Key mappings for special keys
fn key_to_friendly_name(key: &gdk::Key) -> &'static str {
    match *key {
        gdk::Key::comma => ",",
        gdk::Key::period => ".",
        gdk::Key::equal => "=",
        gdk::Key::minus => "-",
        gdk::Key::plus => "+",
        gdk::Key::slash => "/",
        gdk::Key::backslash => "\\",
        gdk::Key::bracketleft => "[",
        gdk::Key::bracketright => "]",
        gdk::Key::semicolon => ";",
        gdk::Key::apostrophe => "'",
        gdk::Key::grave => "`",
        gdk::Key::space => "Space",
        gdk::Key::Return => "Return",
        gdk::Key::Tab => "Tab",
        gdk::Key::BackSpace => "Backspace",
        gdk::Key::Delete => "Delete",
        gdk::Key::Home => "Home",
        gdk::Key::End => "End",
        gdk::Key::Page_Up => "PageUp",
        gdk::Key::Page_Down => "PageDown",
        gdk::Key::Left => "Left",
        gdk::Key::Right => "Right",
        gdk::Key::Up => "Up",
        gdk::Key::Down => "Down",
        gdk::Key::Escape => "Escape",
        gdk::Key::F1 => "F1",
        gdk::Key::F2 => "F2",
        gdk::Key::F3 => "F3",
        gdk::Key::F4 => "F4",
        gdk::Key::F5 => "F5",
        gdk::Key::F6 => "F6",
        gdk::Key::F7 => "F7",
        gdk::Key::F8 => "F8",
        gdk::Key::F9 => "F9",
        gdk::Key::F10 => "F10",
        gdk::Key::F11 => "F11",
        gdk::Key::F12 => "F12",
        // Letter and digit keys — use the gdk keyval name
        other => {
            // Leak a string for the static lifetime. This is fine since
            // we only have a small fixed set of keys in practice.
            let name = other.name().unwrap_or_default().to_string();
            if name.is_empty() {
                return "?";
            }
            // Capitalize first letter for display
            let display = if name.len() == 1 {
                name.to_uppercase()
            } else {
                name
            };
            Box::leak(display.into_boxed_str())
        }
    }
}

fn friendly_name_to_key(name: &str) -> Option<gdk::Key> {
    // Check special names first
    let key = match name {
        "," => gdk::Key::comma,
        "." => gdk::Key::period,
        "=" => gdk::Key::equal,
        "-" => gdk::Key::minus,
        "+" => gdk::Key::plus,
        "/" => gdk::Key::slash,
        "\\" => gdk::Key::backslash,
        "[" => gdk::Key::bracketleft,
        "]" => gdk::Key::bracketright,
        ";" => gdk::Key::semicolon,
        "'" => gdk::Key::apostrophe,
        "`" => gdk::Key::grave,
        "Space" => gdk::Key::space,
        "Return" => gdk::Key::Return,
        "Tab" => gdk::Key::Tab,
        "Backspace" => gdk::Key::BackSpace,
        "Delete" => gdk::Key::Delete,
        "Home" => gdk::Key::Home,
        "End" => gdk::Key::End,
        "PageUp" => gdk::Key::Page_Up,
        "PageDown" => gdk::Key::Page_Down,
        "Left" => gdk::Key::Left,
        "Right" => gdk::Key::Right,
        "Up" => gdk::Key::Up,
        "Down" => gdk::Key::Down,
        "Escape" => gdk::Key::Escape,
        "F1" => gdk::Key::F1,
        "F2" => gdk::Key::F2,
        "F3" => gdk::Key::F3,
        "F4" => gdk::Key::F4,
        "F5" => gdk::Key::F5,
        "F6" => gdk::Key::F6,
        "F7" => gdk::Key::F7,
        "F8" => gdk::Key::F8,
        "F9" => gdk::Key::F9,
        "F10" => gdk::Key::F10,
        "F11" => gdk::Key::F11,
        "F12" => gdk::Key::F12,
        _ => {
            // Try as a gdk key name (handles single letters, digits, etc.)
            let k = gdk::Key::from_name(name)?;
            if k == gdk::Key::VoidSymbol {
                // Try lowercase
                return gdk::Key::from_name(name.to_lowercase())
                    .filter(|k| *k != gdk::Key::VoidSymbol);
            }
            k
        }
    };
    Some(key)
}

// ---------------------------------------------------------------------------
// KeybindingsSettings — serde-compatible, persisted to TOML
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsSettings {
    pub copy: String,
    pub paste: String,
    pub terminal_search: String,
    pub command_palette: String,
    pub add_new: String,
    pub filter_processes: String,
    pub settings: String,
    pub focus_sidebar: String,
    pub focus_terminal: String,
    pub prev_process: String,
    pub next_process: String,
    pub font_increase: String,
    pub font_decrease: String,
    pub quick_jump: String,
    pub new_terminal: String,
    pub close_process: String,
    pub prev_project: String,
    pub next_project: String,
    pub clear_output: String,
    pub toggle_process: String,
    pub restart_process: String,
    pub toggle_sidebar: String,
}

impl Default for KeybindingsSettings {
    fn default() -> Self {
        Self {
            copy: "Ctrl+Shift+C".into(),
            paste: "Ctrl+Shift+V".into(),
            terminal_search: "Ctrl+Shift+F".into(),
            command_palette: "Ctrl+Shift+P".into(),
            add_new: "Ctrl+N".into(),
            filter_processes: "Ctrl+F".into(),
            settings: "Ctrl+,".into(),
            focus_sidebar: "Ctrl+Left".into(),
            focus_terminal: "Ctrl+Right".into(),
            prev_process: "Ctrl+Up".into(),
            next_process: "Ctrl+Down".into(),
            font_increase: "Ctrl+=".into(),
            font_decrease: "Ctrl+-".into(),
            quick_jump: "Ctrl+G".into(),
            new_terminal: "Ctrl+T".into(),
            close_process: "Ctrl+Shift+W".into(),
            prev_project: "Ctrl+Shift+Up".into(),
            next_project: "Ctrl+Shift+Down".into(),
            clear_output: "Ctrl+Alt+C".into(),
            toggle_process: "Ctrl+Alt+S".into(),
            restart_process: "Ctrl+Alt+R".into(),
            toggle_sidebar: "Ctrl+\\".into(),
        }
    }
}

impl KeybindingsSettings {
    fn get(&self, action: ShortcutAction) -> &str {
        match action {
            ShortcutAction::Copy => &self.copy,
            ShortcutAction::Paste => &self.paste,
            ShortcutAction::TerminalSearch => &self.terminal_search,
            ShortcutAction::CommandPalette => &self.command_palette,
            ShortcutAction::AddNew => &self.add_new,
            ShortcutAction::FilterProcesses => &self.filter_processes,
            ShortcutAction::Settings => &self.settings,
            ShortcutAction::FocusSidebar => &self.focus_sidebar,
            ShortcutAction::FocusTerminal => &self.focus_terminal,
            ShortcutAction::PrevProcess => &self.prev_process,
            ShortcutAction::NextProcess => &self.next_process,
            ShortcutAction::FontIncrease => &self.font_increase,
            ShortcutAction::FontDecrease => &self.font_decrease,
            ShortcutAction::QuickJump => &self.quick_jump,
            ShortcutAction::NewTerminal => &self.new_terminal,
            ShortcutAction::CloseProcess => &self.close_process,
            ShortcutAction::PrevProject => &self.prev_project,
            ShortcutAction::NextProject => &self.next_project,
            ShortcutAction::ClearOutput => &self.clear_output,
            ShortcutAction::ToggleProcess => &self.toggle_process,
            ShortcutAction::RestartProcess => &self.restart_process,
            ShortcutAction::ToggleSidebar => &self.toggle_sidebar,
        }
    }

    pub fn set(&mut self, action: ShortcutAction, value: String) {
        match action {
            ShortcutAction::Copy => self.copy = value,
            ShortcutAction::Paste => self.paste = value,
            ShortcutAction::TerminalSearch => self.terminal_search = value,
            ShortcutAction::CommandPalette => self.command_palette = value,
            ShortcutAction::AddNew => self.add_new = value,
            ShortcutAction::FilterProcesses => self.filter_processes = value,
            ShortcutAction::Settings => self.settings = value,
            ShortcutAction::FocusSidebar => self.focus_sidebar = value,
            ShortcutAction::FocusTerminal => self.focus_terminal = value,
            ShortcutAction::PrevProcess => self.prev_process = value,
            ShortcutAction::NextProcess => self.next_process = value,
            ShortcutAction::FontIncrease => self.font_increase = value,
            ShortcutAction::FontDecrease => self.font_decrease = value,
            ShortcutAction::QuickJump => self.quick_jump = value,
            ShortcutAction::NewTerminal => self.new_terminal = value,
            ShortcutAction::CloseProcess => self.close_process = value,
            ShortcutAction::PrevProject => self.prev_project = value,
            ShortcutAction::NextProject => self.next_project = value,
            ShortcutAction::ClearOutput => self.clear_output = value,
            ShortcutAction::ToggleProcess => self.toggle_process = value,
            ShortcutAction::RestartProcess => self.restart_process = value,
            ShortcutAction::ToggleSidebar => self.toggle_sidebar = value,
        }
    }
}

// ---------------------------------------------------------------------------
// KeybindingMap — runtime lookup
// ---------------------------------------------------------------------------

pub struct KeybindingMap {
    bindings: HashMap<ShortcutAction, Keybinding>,
    capturing: Rc<Cell<bool>>,
}

impl KeybindingMap {
    pub fn from_settings(settings: &KeybindingsSettings) -> Self {
        let defaults = KeybindingsSettings::default();
        let mut bindings = HashMap::new();

        for (action, _, _) in action_metadata() {
            let raw = settings.get(action);
            let kb = parse_keybinding(raw).unwrap_or_else(|| {
                log::warn!(
                    "Invalid keybinding '{}' for {:?}, using default",
                    raw,
                    action
                );
                parse_keybinding(defaults.get(action)).expect("default keybinding must be valid")
            });
            bindings.insert(action, kb);
        }

        Self {
            bindings,
            capturing: Rc::new(Cell::new(false)),
        }
    }

    pub fn set_capturing(&self, v: bool) {
        self.capturing.set(v);
    }

    pub fn is_capturing(&self) -> bool {
        self.capturing.get()
    }

    /// Find which action matches the given key event, if any.
    pub fn action_for(
        &self,
        key: gdk::Key,
        modifiers: gdk::ModifierType,
    ) -> Option<ShortcutAction> {
        for (&action, kb) in &self.bindings {
            if kb.matches(key, modifiers) {
                return Some(action);
            }
        }
        None
    }

    /// Get the display string for an action's current binding.
    pub fn display_string(&self, action: ShortcutAction) -> String {
        self.bindings
            .get(&action)
            .map(keybinding_to_string)
            .unwrap_or_default()
    }

    /// Check if a candidate binding conflicts with another action.
    /// Returns the conflicting action if found.
    pub fn find_conflict(
        &self,
        action: ShortcutAction,
        candidate: &Keybinding,
    ) -> Option<ShortcutAction> {
        for (&existing_action, kb) in &self.bindings {
            if existing_action == action {
                continue;
            }
            if kb.matches(candidate.key, candidate.modifiers) {
                return Some(existing_action);
            }
        }
        None
    }

    /// Update a single binding.
    pub fn update_binding(&mut self, action: ShortcutAction, kb: Keybinding) {
        self.bindings.insert(action, kb);
    }

    /// Get the display name for an action.
    pub fn action_display_name(action: ShortcutAction) -> &'static str {
        for (a, name, _) in action_metadata() {
            if a == action {
                return name;
            }
        }
        "Unknown"
    }
}
