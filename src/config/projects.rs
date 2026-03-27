use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::schema::ProcessConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedProjects {
    #[serde(default)]
    pub directories: Vec<String>,
    #[serde(default)]
    pub icons: HashMap<String, String>,
    #[serde(default)]
    pub names: HashMap<String, String>,
    #[serde(default)]
    pub process_order: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub expanded: HashMap<String, bool>,
    #[serde(default)]
    pub deleted_processes: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub custom_commands: HashMap<String, Vec<ProcessConfig>>,
}

impl SavedProjects {
    fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("tuxflow");
        config_dir.join("projects.toml")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => match toml::from_str(&content) {
                    Ok(saved) => {
                        log::info!("Loaded saved projects from {}", path.display());
                        return saved;
                    }
                    Err(e) => log::warn!("Failed to parse saved projects: {e}"),
                },
                Err(e) => log::warn!("Failed to read saved projects: {e}"),
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
                    log::error!("Failed to write saved projects: {e}");
                } else {
                    log::info!("Saved projects list to {}", path.display());
                }
            }
            Err(e) => log::error!("Failed to serialize saved projects: {e}"),
        }
    }

    pub fn add(&mut self, dir: &str) {
        if !self.directories.iter().any(|d| d == dir) {
            self.directories.push(dir.to_string());
            self.save();
        }
    }

    pub fn remove(&mut self, dir: &str) {
        self.directories.retain(|d| d != dir);
        self.icons.remove(dir);
        self.names.remove(dir);
        self.save();
    }

    pub fn set_icon(&mut self, dir: &str, icon_path: Option<String>) {
        match icon_path {
            Some(path) => {
                self.icons.insert(dir.to_string(), path);
            }
            None => {
                self.icons.remove(dir);
            }
        }
        self.save();
    }

    pub fn get_icon(&self, dir: &str) -> Option<&String> {
        self.icons.get(dir)
    }

    pub fn set_name(&mut self, dir: &str, name: &str) {
        self.names.insert(dir.to_string(), name.to_string());
        self.save();
    }

    pub fn get_name(&self, dir: &str) -> Option<&String> {
        self.names.get(dir)
    }

    pub fn reorder_to_match(&mut self, new_order: &[String]) {
        self.directories = new_order.to_vec();
        self.save();
    }

    pub fn set_process_order(&mut self, dir: &str, order: Vec<String>) {
        self.process_order.insert(dir.to_string(), order);
        self.save();
    }

    pub fn get_process_order(&self, dir: &str) -> Option<&Vec<String>> {
        self.process_order.get(dir)
    }

    pub fn set_expanded(&mut self, dir: &str, expanded: bool) {
        self.expanded.insert(dir.to_string(), expanded);
        self.save();
    }

    pub fn is_expanded(&self, dir: &str) -> Option<bool> {
        self.expanded.get(dir).copied()
    }

    pub fn add_deleted_process(&mut self, dir: &str, process_name: &str) {
        let list = self.deleted_processes.entry(dir.to_string()).or_default();
        if !list.iter().any(|n| n == process_name) {
            list.push(process_name.to_string());
            self.save();
        }
    }

    pub fn has_deleted_processes(&self, dir: &str) -> bool {
        self.deleted_processes
            .get(dir)
            .is_some_and(|list| !list.is_empty())
    }

    pub fn has_deleted_processes_matching(&self, dir: &str, prefix: &str) -> bool {
        self.deleted_processes
            .get(dir)
            .is_some_and(|list| list.iter().any(|n| n.starts_with(prefix)))
    }

    pub fn is_process_deleted(&self, dir: &str, process_name: &str) -> bool {
        self.deleted_processes
            .get(dir)
            .map(|list| list.iter().any(|n| n == process_name))
            .unwrap_or(false)
    }

    pub fn add_custom_command(&mut self, dir: &str, config: ProcessConfig) {
        let list = self.custom_commands.entry(dir.to_string()).or_default();
        // Replace if same name exists, otherwise append
        if let Some(existing) = list.iter_mut().find(|c| c.name == config.name) {
            *existing = config;
        } else {
            list.push(config);
        }
        self.save();
    }

    pub fn get_custom_commands(&self, dir: &str) -> Option<&Vec<ProcessConfig>> {
        self.custom_commands.get(dir)
    }

    pub fn set_display_name(&mut self, dir: &str, process_name: &str, display_name: &str) {
        if let Some(list) = self.custom_commands.get_mut(dir)
            && let Some(cmd) = list.iter_mut().find(|c| c.name == process_name)
        {
            cmd.display_name = Some(display_name.to_string());
            self.save();
        }
    }

    pub fn remove_custom_command(&mut self, dir: &str, process_name: &str) {
        if let Some(list) = self.custom_commands.get_mut(dir) {
            list.retain(|c| c.name != process_name);
            self.save();
        }
    }
}
