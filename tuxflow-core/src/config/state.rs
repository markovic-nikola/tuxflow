//! Machine-local UI state: `$XDG_STATE_HOME/tuxflow/state.toml`
//! (`~/.local/state/tuxflow/state.toml`).
//!
//! `settings.toml` and `projects.toml` are the user's configuration, and
//! people sync them between machines (chezmoi, a dotfiles repo). What lives
//! here is what the app remembers about ONE machine's session: the window
//! geometry (a portrait monitor's size is wrong on a laptop) and when each
//! project was last used. Both move on ordinary use — `last_used` on every
//! start — so kept in the synced files they turned each working day into a
//! commit, and two machines syncing the same file into a guaranteed
//! conflict. Which sidebar groups are open lived here too until 2026-10-09
//! and went back to `projects.toml`: it moves only on a click, and the
//! machines share the same projects, so switching desks should not mean
//! re-opening the sidebar.

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
    /// Where the sidebar's open/closed map lived 2026-09-24 → 2026-10-09.
    /// Read to hand to [`SavedProjects::adopt_expanded`], never written
    /// back — the next save of this file drops it.
    #[serde(rename = "expanded", skip_serializing)]
    legacy_expanded: BTreeMap<String, bool>,
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
            legacy_expanded: BTreeMap::new(),
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

    /// The pre-2026-10-09 open/closed map, for [`SavedProjects::adopt_expanded`].
    pub fn take_legacy_expanded(&mut self) -> BTreeMap<String, bool> {
        std::mem::take(&mut self.legacy_expanded)
    }

    /// A project was removed from the workspace.
    pub fn forget(&mut self, key: &str) {
        if self.last_used.remove(key).is_some() {
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
        assert!(file.exists(), "the migration must be written at once");

        let reloaded = LocalState::load_from(&file, || panic!("migrated twice"));
        assert_eq!(reloaded.get_last_used("/p/one"), 100);
        assert_eq!(reloaded.window.height, 924);
    }

    /// The point of the split: the synced files stop carrying the keys.
    /// They still PARSE them (that is how the migration reads them), and
    /// the rest of each file survives the round trip — `[expanded]`
    /// included, which is synced again since 2026-10-09.
    #[test]
    fn the_synced_files_no_longer_write_machine_state() {
        let (settings, saved) = legacy();

        let settings_out = toml::to_string_pretty(&settings).expect("settings serialise");
        assert!(!settings_out.contains("[window]"), "{settings_out}");
        assert!(settings_out.contains("font_size = 15"), "{settings_out}");

        let projects_out = toml::to_string_pretty(&saved).expect("projects serialise");
        assert!(!projects_out.contains("last_used"), "{projects_out}");
        assert!(projects_out.contains("[expanded]"), "{projects_out}");
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
        assert_eq!(
            LocalState::load_from(&file, LocalState::default).get_last_used("/p/one"),
            42
        );

        state.forget("/p/one");
        let reloaded = LocalState::load_from(&file, LocalState::default);
        assert_eq!(reloaded.get_last_used("/p/one"), 0);
    }

    /// The open/closed map moves back to the synced file once, and the
    /// state file stops carrying it on its next save. A synced file that
    /// already has one (another machine moved first) keeps it.
    #[test]
    fn expanded_moves_back_to_the_synced_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state_file = dir.path().join("state.toml");
        let projects_file = dir.path().join("projects.toml");
        fs::write(
            &state_file,
            "[last_used]\n\"/p/one\" = 1\n\n[expanded]\n\"/p/one\" = false\n\"/p/gone\" = false\n",
        )
        .expect("write state");
        fs::write(&projects_file, "directories = [\"/p/one\", \"/p/two\"]\n")
            .expect("write projects");

        let mut state = LocalState::load_from(&state_file, || panic!("must not migrate"));
        let mut saved = SavedProjects::load_from(&projects_file);
        saved.adopt_expanded(state.take_legacy_expanded());
        assert_eq!(saved.is_expanded("/p/one"), Some(false));
        assert_eq!(
            saved.is_expanded("/p/gone"),
            None,
            "removed projects are not carried over"
        );

        let reloaded = SavedProjects::load_from(&projects_file);
        assert_eq!(reloaded.is_expanded("/p/one"), Some(false));

        state.set_last_used("/p/one", 2);
        let state_out = fs::read_to_string(&state_file).expect("read state");
        assert!(!state_out.contains("expanded"), "{state_out}");

        let mut other = reloaded;
        other.record_expanded([("/p/two".to_string(), true)]);
        assert_eq!(
            other.is_expanded("/p/two"),
            None,
            "open is the default, not written"
        );
        other.record_expanded([("/p/two".to_string(), false)]);
        assert_eq!(other.is_expanded("/p/two"), Some(false));

        other.adopt_expanded(BTreeMap::from([("/p/one".to_string(), true)]));
        assert_eq!(
            other.is_expanded("/p/one"),
            Some(false),
            "the synced copy wins"
        );
    }

    #[test]
    fn a_default_instance_is_unbound() {
        let mut state = LocalState::default();
        state.set_last_used("/p/one", 42);
        assert!(state.path.is_none());
    }
}
