//! Machine-local UI state: `$XDG_STATE_HOME/tuxflow/state.toml`
//! (`~/.local/state/tuxflow/state.toml`).
//!
//! `settings.toml` and `projects.toml` are the user's configuration, and
//! people sync them between machines (chezmoi, a dotfiles repo). What lives
//! here is what the app remembers about ONE machine's session: the window
//! geometry (a portrait monitor's size is wrong on a laptop), when each
//! project was last used and which sidebar groups are open. All three move
//! on ordinary use — `last_used` on every start — so kept in the synced
//! files they turned each working day into a commit, and two machines
//! syncing the same file into a guaranteed conflict.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::persist::{read_toml, write_toml};
use super::projects::SavedProjects;
use super::settings::AppSettings;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalState {
    /// Unix seconds of the last user-visible activity (a process starting)
    /// per project key. Drives the sidebar's "recently used first" sort.
    pub last_used: BTreeMap<String, u64>,
    /// Sidebar group open/closed per project key. Absent = open.
    pub expanded: BTreeMap<String, bool>,
    pub window: WindowSettings,
    /// The file every mutation writes back to; `None` (what `default()`
    /// gives) writes nowhere — the same safety rule as `SavedProjects`.
    #[serde(skip)]
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub monitor: Option<String>,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 1200,
            height: 800,
            maximized: false,
            x: None,
            y: None,
            monitor: None,
        }
    }
}

impl LocalState {
    fn state_path() -> PathBuf {
        dirs::state_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/state"))
            .join("tuxflow")
            .join("state.toml")
    }

    pub fn load() -> Self {
        Self::load_from(Self::state_path(), || {
            Self::from_legacy(&AppSettings::load(), &SavedProjects::load())
        })
    }

    /// Load from `path`, bound for later saves. With no file there yet this
    /// is the first launch since the split: `legacy` supplies the values
    /// the synced files still carry, and they are written out AT ONCE —
    /// the next save of either synced file drops its copy, so a migration
    /// deferred to the first state change would find nothing left to take.
    pub fn load_from(path: impl Into<PathBuf>, legacy: impl FnOnce() -> Self) -> Self {
        let path = path.into();
        let mut state = match read_toml(&path, "local state") {
            Some(state) => state,
            None if path.exists() => Self::default(),
            None => {
                let state = legacy();
                state.write_to(&path);
                state
            }
        };
        state.path = Some(path);
        state
    }

    /// What the pre-split files held, or defaults where they held nothing.
    pub fn from_legacy(settings: &AppSettings, saved: &SavedProjects) -> Self {
        Self {
            last_used: saved.legacy_last_used.clone(),
            expanded: saved.legacy_expanded.clone(),
            window: settings.legacy_window.clone().unwrap_or_default(),
            path: None,
        }
    }

    pub fn save(&self) {
        match self.path.as_deref() {
            Some(path) => self.write_to(path),
            None => log::error!("LocalState::save() on an unbound instance — ignored"),
        }
    }

    fn write_to(&self, path: &Path) {
        write_toml(self, path, "local state");
    }

    pub fn set_last_used(&mut self, key: &str, timestamp: u64) {
        self.last_used.insert(key.to_string(), timestamp);
        self.save();
    }

    /// 0 = never used.
    pub fn get_last_used(&self, key: &str) -> u64 {
        self.last_used.get(key).copied().unwrap_or(0)
    }

    pub fn set_expanded(&mut self, key: &str, expanded: bool) {
        self.expanded.insert(key.to_string(), expanded);
        self.save();
    }

    pub fn is_expanded(&self, key: &str) -> Option<bool> {
        self.expanded.get(key).copied()
    }

    /// A project was removed from the workspace.
    pub fn forget(&mut self, key: &str) {
        let had = self.last_used.remove(key).is_some() | self.expanded.remove(key).is_some();
        if had {
            self.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const LEGACY_SETTINGS: &str = r#"
[appearance]
font_size = 15

[window]
width = 1080
height = 924
maximized = false
x = 0
y = 0
monitor = "DP-0"
"#;

    const LEGACY_PROJECTS: &str = r#"
directories = ["/p/one", "/p/two"]

[expanded]
"/p/one" = false

[last_used]
"/p/one" = 100
"/p/two" = 200
"#;

    fn legacy() -> (AppSettings, SavedProjects) {
        (
            toml::from_str(LEGACY_SETTINGS).expect("settings fixture"),
            toml::from_str(LEGACY_PROJECTS).expect("projects fixture"),
        )
    }

    /// First launch after the split: the old keys seed the state file, and
    /// it is on disk before anything else runs.
    #[test]
    fn a_missing_state_file_is_seeded_from_the_legacy_keys() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("state.toml");
        let (settings, saved) = legacy();

        let state = LocalState::load_from(&file, || LocalState::from_legacy(&settings, &saved));

        assert_eq!(state.window.width, 1080);
        assert_eq!(state.window.monitor.as_deref(), Some("DP-0"));
        assert_eq!(state.get_last_used("/p/two"), 200);
        assert_eq!(state.is_expanded("/p/one"), Some(false));
        assert!(file.exists(), "the migration must be written at once");

        let reloaded = LocalState::load_from(&file, || panic!("migrated twice"));
        assert_eq!(reloaded.get_last_used("/p/one"), 100);
        assert_eq!(reloaded.window.height, 924);
    }

    /// The point of the split: the synced files stop carrying the keys.
    /// They still PARSE them (that is how the migration reads them), and
    /// the rest of each file survives the round trip.
    #[test]
    fn the_synced_files_no_longer_write_machine_state() {
        let (settings, saved) = legacy();

        let settings_out = toml::to_string_pretty(&settings).expect("settings serialise");
        assert!(!settings_out.contains("[window]"), "{settings_out}");
        assert!(settings_out.contains("font_size = 15"), "{settings_out}");

        let projects_out = toml::to_string_pretty(&saved).expect("projects serialise");
        assert!(!projects_out.contains("last_used"), "{projects_out}");
        assert!(!projects_out.contains("expanded"), "{projects_out}");
        assert!(projects_out.contains("/p/two"), "{projects_out}");
    }

    /// A state file that exists but does not parse must not re-run the
    /// migration: by then the synced files may already have dropped their
    /// copies, and "seeding" from them would silently reset the geometry
    /// the next migration would expect to find.
    #[test]
    fn a_corrupt_state_file_falls_back_to_defaults_not_legacy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("state.toml");
        fs::write(&file, "this is = = not toml").expect("write fixture");

        let state = LocalState::load_from(&file, || panic!("must not migrate"));
        assert_eq!(state.window.width, WindowSettings::default().width);
    }

    #[test]
    fn setters_persist_and_forget_clears_a_project() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("state.toml");

        let mut state = LocalState::load_from(&file, LocalState::default);
        state.set_last_used("/p/one", 42);
        state.set_expanded("/p/one", false);
        assert_eq!(
            LocalState::load_from(&file, LocalState::default).get_last_used("/p/one"),
            42
        );

        state.forget("/p/one");
        let reloaded = LocalState::load_from(&file, LocalState::default);
        assert_eq!(reloaded.get_last_used("/p/one"), 0);
        assert_eq!(reloaded.is_expanded("/p/one"), None);
    }

    #[test]
    fn a_default_instance_is_unbound() {
        let mut state = LocalState::default();
        state.set_last_used("/p/one", 42);
        assert!(state.path.is_none());
    }
}
