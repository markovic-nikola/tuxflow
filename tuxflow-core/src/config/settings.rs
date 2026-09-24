use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::keybindings::KeybindingsSettings;
use super::persist::{read_toml, write_toml};
use super::state::WindowSettings;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct AppSettings {
    pub appearance: AppearanceSettings,
    pub notifications: NotificationSettings,
    pub sidebar: SidebarSettings,
    pub tools: ToolSettings,
    pub keybindings: KeybindingsSettings,
    pub integrations: IntegrationSettings,
    /// Where the window geometry lived before it moved to the machine-local
    /// state file ([`super::state::LocalState`]). Read once to seed that
    /// file, never written back: this file is synced between machines, and
    /// a portrait monitor's geometry has no business on a laptop.
    #[serde(rename = "window", skip_serializing)]
    pub(crate) legacy_window: Option<WindowSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub theme: String,
    pub accent_color: String,
    /// Sidebar accent for local projects, and for remote (SSH) ones — the
    /// pair is what makes the two read apart at a glance. Palette names
    /// from `ui::accent` (unknown names fall back there).
    pub local_accent_color: String,
    pub remote_accent_color: String,
    pub font_family: String,
    pub font_size: u32,
    pub font_weight: u32,
    pub bold_font_weight: u32,
    pub line_height: f64,
    pub letter_spacing: f64,
    pub scrollback_lines: u32,
    pub terminal_theme: String,
    /// How the terminal pane shows that keys go elsewhere (the composer,
    /// a filter field, a modal, another window). A name from
    /// `FOCUS_INDICATOR_CHOICES`; unknown names fall back to the default.
    /// The cursor alone can't carry this: agent TUIs hide it.
    pub focus_indicator: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub on_crash: bool,
    pub on_auto_restart: bool,
    pub on_file_watch_restart: bool,
    pub on_process_finish: bool,
    pub on_agent_idle: bool,
    pub on_agent_idle_silence_fallback: bool,
    pub agent_idle_silence_seconds: u32,
    pub suppress_when_focused: bool,
    pub sound_enabled: bool,
    /// Freedesktop sound theme event ID, e.g. "complete", "bell",
    /// "message-new-instant", "dialog-information".
    pub sound_name: String,
    /// Per-agent idle sound overrides. `None` = fall back to `sound_name`.
    pub claude_sound_name: Option<String>,
    pub codex_sound_name: Option<String>,
    pub gemini_sound_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarSettings {
    #[serde(alias = "show_settings_footer")]
    pub single_project_expand: bool,
    pub auto_hide_sidebar: bool,
    pub show_keybind_hints: bool,
    /// Keep recently started projects at the top of the sidebar. Was
    /// briefly "running projects first" — the alias migrates that key.
    #[serde(alias = "running_first")]
    pub recent_first: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolSettings {
    pub default_editor: String,
    pub default_terminal: String,
    pub reuse_editor_window: bool,
    /// Show the local message composer under agent terminals (typing is
    /// local, the message is sent to the PTY in one write — avoids
    /// per-keystroke ssh lag on remote projects).
    pub agent_composer: bool,
    /// Bridge this machine's microphone to remote hosts, so agents running
    /// there can record voice input (Claude Code's hold-to-talk). While a
    /// remote project is open, the host can open the microphone — hence
    /// off by default. See `remote::mic`.
    pub remote_microphone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IntegrationSettings {
    pub mcp_enabled: bool,
}

/// Editor choices shown in Settings → Tools, as (command, label). Shared
/// by both shells so the dropdowns can't drift apart.
/// Terminal focus indicator: `(name, label)`, first entry is the default.
/// The names are what the widget's `FocusMarkStyle` offers.
pub const FOCUS_INDICATOR_CHOICES: &[(&str, &str)] = &[
    ("dim", "Dim the pane when unfocused"),
    ("top", "Accent line along the top edge"),
    ("left", "Accent line along the left edge"),
    ("ring", "Accent ring around the pane"),
    (
        "border",
        "Accent border, fading into the pane when unfocused",
    ),
    ("none", "None"),
];

pub const EDITOR_CHOICES: &[(&str, &str)] = &[
    ("xdg-open", "System Default (xdg-open)"),
    ("code", "VS Code (code)"),
    ("cursor", "Cursor (cursor)"),
    ("codium", "VSCodium (codium)"),
    ("zed", "Zed (zed)"),
    ("nvim", "Neovim (nvim)"),
    ("vim", "Vim (vim)"),
    ("hx", "Helix (hx)"),
    ("nano", "Nano (nano)"),
    ("emacs", "Emacs (emacs)"),
    ("kate", "Kate (kate)"),
    ("gedit", "GNOME Text Editor (gedit)"),
    ("sublime_text", "Sublime Text (sublime_text)"),
    ("idea", "IntelliJ IDEA (idea)"),
];

/// Terminal-app choices shown in Settings → Tools, as (command, label).
pub const TERMINAL_CHOICES: &[(&str, &str)] = &[
    ("xdg-open", "System Default (xdg-open)"),
    ("gnome-terminal", "GNOME Terminal (gnome-terminal)"),
    ("konsole", "Konsole (konsole)"),
    ("alacritty", "Alacritty (alacritty)"),
    ("kitty", "Kitty (kitty)"),
    ("ghostty", "Ghostty (ghostty)"),
    ("wezterm", "WezTerm (wezterm)"),
    ("foot", "Foot (foot)"),
    ("tilix", "Tilix (tilix)"),
    ("xfce4-terminal", "Xfce Terminal (xfce4-terminal)"),
    ("mate-terminal", "MATE Terminal (mate-terminal)"),
    ("terminator", "Terminator (terminator)"),
    ("st", "st (st)"),
    ("urxvt", "urxvt (urxvt)"),
    ("xterm", "xterm (xterm)"),
];

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            accent_color: "green".to_string(),
            local_accent_color: "green".to_string(),
            remote_accent_color: "yellow".to_string(),
            font_family: "Monospace".to_string(),
            font_size: 12,
            font_weight: 400,
            bold_font_weight: 700,
            line_height: 1.0,
            letter_spacing: 0.0,
            scrollback_lines: 10000,
            terminal_theme: "catppuccin-mocha".to_string(),
            focus_indicator: "dim".to_string(),
        }
    }
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            on_crash: true,
            on_auto_restart: true,
            on_file_watch_restart: false,
            on_process_finish: true,
            on_agent_idle: true,
            on_agent_idle_silence_fallback: false,
            agent_idle_silence_seconds: 20,
            suppress_when_focused: true,
            sound_enabled: false,
            sound_name: crate::util::sounds::DEFAULT_SOUND_ID.to_string(),
            claude_sound_name: None,
            codex_sound_name: None,
            gemini_sound_name: None,
        }
    }
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            single_project_expand: true,
            auto_hide_sidebar: false,
            show_keybind_hints: false,
            recent_first: false,
        }
    }
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            reuse_editor_window: true,
            default_editor: "xdg-open".to_string(),
            default_terminal: "xdg-open".to_string(),
            // Off by default — opt in via Settings → Tools → Agents.
            agent_composer: false,
            // Off by default: it exposes the microphone to the remote host.
            remote_microphone: false,
        }
    }
}

impl Default for IntegrationSettings {
    fn default() -> Self {
        Self { mcp_enabled: true }
    }
}

impl AppSettings {
    fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("tuxflow");
        config_dir.join("settings.toml")
    }

    pub fn load() -> Self {
        let Some(mut settings) = read_toml::<AppSettings>(&Self::config_path(), "settings") else {
            return Self::default();
        };
        if settings.migrate_keybindings() {
            settings.save();
        }
        settings
    }

    /// Migrate keybindings whose old default conflicts with common terminal app
    /// shortcuts (e.g. Ctrl+W in nano). Only rewrites bindings still set to the
    /// retired default — user customizations are preserved. Returns true if any
    /// changes were made.
    fn migrate_keybindings(&mut self) -> bool {
        let mut changed = false;
        if self.keybindings.close_process == "Ctrl+W" {
            self.keybindings.close_process = "Ctrl+Shift+W".into();
            changed = true;
        }
        changed
    }

    pub fn save(&self) {
        write_toml(self, &Self::config_path(), "settings");
    }
}
