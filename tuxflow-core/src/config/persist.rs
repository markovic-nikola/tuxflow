//! Reading and writing the TOML files under `~/.config/tuxflow` and
//! `~/.local/state/tuxflow` — one implementation, so every file gets the
//! same atomic write.

use std::fs;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Parse `path`, or `None` when it is absent, unreadable or malformed (the
/// last two are logged). `what` names the file in the log.
pub fn read_toml<T: DeserializeOwned>(path: &Path, what: &str) -> Option<T> {
    if !path.exists() {
        return None;
    }
    match fs::read_to_string(path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(value) => {
                log::debug!("Loaded {what} from {}", path.display());
                Some(value)
            }
            Err(e) => {
                log::warn!("Failed to parse {what}: {e}");
                None
            }
        },
        Err(e) => {
            log::warn!("Failed to read {what}: {e}");
            None
        }
    }
}

/// Serialize `value` to `path`, creating the directory. Failures are logged,
/// never raised: a lost save must not take the window down with it.
///
/// Atomic: write a sibling tmp file, then rename over the target. Two
/// writers can race (two TuxFlow versions sharing a file, the background
/// geometry save), and a reader must never see a torn half-write — parsing
/// one as "empty workspace" and saving it back is how a workspace gets wiped.
pub fn write_toml<T: Serialize>(value: &T, path: &Path, what: &str) {
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        log::error!("Failed to create directory for {what}: {e}");
        return;
    }
    match toml::to_string_pretty(value) {
        Ok(content) => {
            let tmp = path.with_extension("toml.tmp");
            match fs::write(&tmp, content).and_then(|_| fs::rename(&tmp, path)) {
                Ok(()) => log::debug!("Saved {what} to {}", path.display()),
                Err(e) => log::error!("Failed to write {what}: {e}"),
            }
        }
        Err(e) => log::error!("Failed to serialize {what}: {e}"),
    }
}
