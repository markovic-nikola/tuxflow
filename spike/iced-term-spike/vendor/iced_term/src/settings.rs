use std::{collections::HashMap, path::PathBuf};

use crate::ColorPalette;
use iced::Font;

#[cfg(target_os = "windows")]
const DEFAULT_SHELL: &str = "wsl.exe";

#[cfg(not(target_os = "windows"))]
const DEFAULT_SHELL: &str = "/bin/bash";

#[derive(Default, Clone)]
pub struct Settings {
    pub font: FontSettings,
    pub theme: ThemeSettings,
    pub backend: BackendSettings,
}

#[derive(Debug, Clone)]
pub struct BackendSettings {
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub working_directory: Option<PathBuf>,
    /// Scrollback length in lines (`term::Config::scrolling_history`).
    pub scrolling_history: usize,
    /// Characters that terminate a double-click (semantic) selection.
    pub semantic_escape_chars: String,
    /// Enable the kitty keyboard protocol (a capability VTE lacks).
    pub kitty_keyboard: bool,
    /// OSC 52 policy. The default (`OnlyCopy`) is what agent workflows
    /// need: programs may set the clipboard, never read it.
    pub osc52: alacritty_terminal::term::Osc52,
}

impl Default for BackendSettings {
    fn default() -> Self {
        // Terminal knobs inherit alacritty's own defaults — one source of
        // truth instead of copied constants.
        let config = alacritty_terminal::term::Config::default();
        Self {
            program: DEFAULT_SHELL.to_string(),
            args: vec![],
            env: HashMap::new(),
            working_directory: None,
            scrolling_history: config.scrolling_history,
            semantic_escape_chars: config.semantic_escape_chars,
            kitty_keyboard: config.kitty_keyboard,
            osc52: config.osc52,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FontSettings {
    pub size: f32,
    pub scale_factor: f32,
    pub font_type: Font,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            size: 14.0,
            scale_factor: 1.3,
            font_type: Font::MONOSPACE,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct ThemeSettings {
    pub color_pallete: Box<ColorPalette>,
}

impl ThemeSettings {
    pub fn new(color_pallete: Box<ColorPalette>) -> Self {
        Self { color_pallete }
    }
}
