use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::schema::ProcessConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedProjects {
    #[serde(default)]
    pub directories: Vec<String>,
    #[serde(default)]
    pub icons: BTreeMap<String, String>,
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    #[serde(default)]
    pub process_order: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub expanded: BTreeMap<String, bool>,
    #[serde(default)]
    pub deleted_processes: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub custom_commands: BTreeMap<String, Vec<ProcessConfig>>,
    /// Unix seconds of the last user-visible activity (a process starting)
    /// per project. Drives the sidebar's "recently used first" sort.
    #[serde(default)]
    pub last_used: BTreeMap<String, u64>,
    /// The file every mutation writes back to, stamped by [`Self::load_from`].
    /// `None` — which is what `default()` gives — means **nowhere**; see
    /// [`Self::save`] for why that is the safe default rather than the real
    /// config path.
    ///
    /// Skipped by serde: it is plumbing, not data. It also sits last on
    /// purpose — TOML puts every plain value before the first table, so a
    /// serialized path down here would fail to encode rather than quietly
    /// appear in the user's file.
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl SavedProjects {
    fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("tuxflow");
        config_dir.join("projects.toml")
    }

    pub fn load() -> Self {
        Self::load_from(Self::config_path())
    }

    /// Load from an explicit file, and bind every later mutation to it.
    ///
    /// This is the seam `load()` is built on, and the ONLY way a test should
    /// obtain a `SavedProjects` it intends to mutate: the setters all persist
    /// (see [`Self::save`]), so one built any other way either writes nothing
    /// or — before this existed — wrote the developer's real workspace. The
    /// file need not exist yet; a missing one loads as empty, exactly as the
    /// app's first run does.
    pub fn load_from(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut saved = Self::read(&path);
        saved.path = Some(path);
        saved
    }

    /// Parse the file, or an empty set if it is absent, unreadable or
    /// malformed. Leaves `path` unset — [`Self::load_from`] stamps it.
    fn read(path: &Path) -> Self {
        if path.exists() {
            match fs::read_to_string(path) {
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

    /// Persist to the file this was loaded from. Called by every setter, so
    /// callers rarely need it directly.
    ///
    /// A `SavedProjects` with no file behind it writes **nothing**. That is
    /// the entire point of the `Option`: `default()` is what tests reach for,
    /// and defaulting it to the real config path meant a test calling any
    /// setter replaced a 33-project workspace with its own empty struct —
    /// which happened twice. Making the accidental case inert costs nothing,
    /// because the app always arrives through `load()`.
    pub fn save(&self) {
        let Some(path) = self.path.as_deref() else {
            log::error!(
                "SavedProjects::save() on an unbound instance — ignored. \
                 Use load() in the app, or load_from(tmp) in tests."
            );
            return;
        };
        self.write_to(path);
    }

    fn write_to(&self, path: &Path) {
        if let Some(parent) = path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            log::error!("Failed to create config directory: {e}");
            return;
        }
        match toml::to_string_pretty(self) {
            Ok(content) => {
                // Atomic: write a sibling tmp file, then rename over the
                // target. Two writers can race (the GTK app and the iced
                // shell share this file), and a reader must never see a
                // torn half-write — parsing one as "empty workspace" and
                // saving it back is how a workspace gets wiped.
                let tmp = path.with_extension("toml.tmp");
                let result = fs::write(&tmp, content).and_then(|_| fs::rename(&tmp, path));
                match result {
                    Ok(()) => log::debug!("Saved projects list to {}", path.display()),
                    Err(e) => log::error!("Failed to write saved projects: {e}"),
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
        self.process_order.remove(dir);
        self.expanded.remove(dir);
        self.deleted_processes.remove(dir);
        self.custom_commands.remove(dir);
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

    pub fn set_last_used(&mut self, dir: &str, timestamp: u64) {
        self.last_used.insert(dir.to_string(), timestamp);
        self.save();
    }

    /// 0 = never used.
    pub fn get_last_used(&self, dir: &str) -> u64 {
        self.last_used.get(dir).copied().unwrap_or(0)
    }

    pub fn add_deleted_process(&mut self, dir: &str, process_name: &str) {
        let list = self.deleted_processes.entry(dir.to_string()).or_default();
        if !list.iter().any(|n| n == process_name) {
            list.push(process_name.to_string());
            self.save();
        }
    }

    pub fn unmark_process_deleted(&mut self, dir: &str, process_name: &str) {
        let mut changed = false;
        if let Some(list) = self.deleted_processes.get_mut(dir) {
            let before = list.len();
            list.retain(|n| n != process_name);
            if list.len() != before {
                changed = true;
            }
            if list.is_empty() {
                self.deleted_processes.remove(dir);
            }
        }
        if changed {
            self.save();
        }
    }

    pub fn has_deleted_processes(&self, dir: &str) -> bool {
        self.deleted_processes
            .get(dir)
            .is_some_and(|list| !list.is_empty())
    }

    pub fn is_process_deleted(&self, dir: &str, process_name: &str) -> bool {
        self.deleted_processes
            .get(dir)
            .is_some_and(|list| list.iter().any(|n| n == process_name))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A bound instance round-trips through the real setter → `save()` →
    /// `load_from()` path, which had no coverage at all: every setter
    /// persists, so exercising one used to mean writing the real config.
    #[test]
    fn a_setter_persists_and_reloads() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("projects.toml");

        let mut saved = SavedProjects::load_from(&file);
        saved.add("/p/one");
        saved.set_name("/p/one", "One");
        saved.set_icon("/p/one", Some("/p/one/logo.svg".into()));

        let reloaded = SavedProjects::load_from(&file);
        assert_eq!(reloaded.directories, vec!["/p/one".to_string()]);
        assert_eq!(reloaded.get_name("/p/one").map(String::as_str), Some("One"));
        assert_eq!(
            reloaded.get_icon("/p/one").map(String::as_str),
            Some("/p/one/logo.svg")
        );
    }

    /// The regression guard for the actual incident: `default()` — what a
    /// test reaches for — must come back UNBOUND, so the setters it calls
    /// persist nowhere. Before the `path` field they targeted
    /// `~/.config/tuxflow/projects.toml` and replaced a real workspace.
    ///
    /// The assertion is on the binding rather than on some file's absence:
    /// an unbound `save()` would have written the developer's real config,
    /// and a test cannot check that target without either reading the
    /// developer's own file or racing a running app against it. `path: None`
    /// plus `save()`'s early return on it is the whole safety property.
    #[test]
    fn a_default_instance_is_unbound() {
        let mut saved = SavedProjects::default();
        saved.set_icon("/p/one", Some("/p/one/logo.svg".into()));
        saved.add("/p/one");
        saved.set_last_used("/p/one", 42);

        assert!(
            saved.path.is_none(),
            "default() must not be bound to any file"
        );
        // Inert means "does not persist", not "does not work" — the three
        // mutations above still landed in memory, which is what the existing
        // merge_saved tests rely on.
        assert_eq!(saved.get_last_used("/p/one"), 42);
        assert_eq!(saved.directories, vec!["/p/one".to_string()]);
    }

    /// The counterpart: a bound instance really does write. Together with
    /// the test above this is the safety property in full — bound persists,
    /// unbound cannot.
    #[test]
    fn a_bound_instance_writes_its_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("projects.toml");

        let mut saved = SavedProjects::load_from(&file);
        assert!(!file.exists(), "nothing written before the first mutation");

        saved.add("/p/one");
        assert!(file.exists(), "a setter on a bound instance must persist");
    }

    /// The `path` field is plumbing and must never reach the file — an
    /// unknown key would be read back as data, and a plain value emitted
    /// after the tables would not be valid TOML at all.
    #[test]
    fn the_bound_path_is_not_serialized() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("projects.toml");

        let mut saved = SavedProjects::load_from(&file);
        saved.add("/p/one");

        let written = fs::read_to_string(&file).expect("must have been written");
        assert!(
            !written.contains("path"),
            "`path` leaked into the config file:\n{written}"
        );
    }
}
