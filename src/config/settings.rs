use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::keybindings::KeybindingsSettings;

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
    pub window: WindowSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub theme: String,
    pub accent_color: String,
    pub font_family: String,
    pub font_size: u32,
    pub font_weight: u32,
    pub bold_font_weight: u32,
    pub line_height: f64,
    pub letter_spacing: f64,
    pub scrollback_lines: u32,
    pub terminal_theme: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub on_crash: bool,
    pub on_auto_restart: bool,
    pub on_file_watch_restart: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarSettings {
    #[serde(alias = "show_settings_footer")]
    pub single_project_expand: bool,
    pub auto_hide_sidebar: bool,
    pub project_cpu_threshold: u32,
    pub project_mem_threshold: u32,
    pub process_cpu_threshold: u32,
    pub process_mem_threshold: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolSettings {
    pub default_editor: String,
    pub default_terminal: String,
    pub reuse_editor_window: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IntegrationSettings {
    pub mcp_enabled: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            accent_color: "green".to_string(),
            font_family: "Monospace".to_string(),
            font_size: 12,
            font_weight: 400,
            bold_font_weight: 700,
            line_height: 1.0,
            letter_spacing: 0.0,
            scrollback_lines: 10000,
            terminal_theme: "catppuccin-mocha".to_string(),
        }
    }
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            on_crash: true,
            on_auto_restart: true,
            on_file_watch_restart: false,
        }
    }
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            single_project_expand: true,
            auto_hide_sidebar: false,
            project_cpu_threshold: 0,
            project_mem_threshold: 0,
            process_cpu_threshold: 0,
            process_mem_threshold: 0,
        }
    }
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            reuse_editor_window: true,
            default_editor: "xdg-open".to_string(),
            default_terminal: "xdg-open".to_string(),
        }
    }
}

impl Default for IntegrationSettings {
    fn default() -> Self {
        Self { mcp_enabled: true }
    }
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 1200,
            height: 800,
            maximized: false,
        }
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
        let path = Self::config_path();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => match toml::from_str(&content) {
                    Ok(settings) => {
                        log::info!("Loaded settings from {}", path.display());
                        return settings;
                    }
                    Err(e) => log::warn!("Failed to parse settings: {e}"),
                },
                Err(e) => log::warn!("Failed to read settings file: {e}"),
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            log::error!("Failed to create config directory: {e}");
            return;
        }
        match toml::to_string_pretty(self) {
            Ok(content) => {
                if let Err(e) = fs::write(&path, content) {
                    log::error!("Failed to write settings: {e}");
                } else {
                    log::info!("Saved settings to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize settings: {e}"),
        }
    }
}
