//! Bundled notification sounds, shared by both shells. The audio bytes
//! ship inside the executable and are extracted to the user's cache dir
//! the first time a given sound plays, so `paplay` has a file path to
//! feed it.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// A notification sound bundled into the binary.
pub struct BundledSound {
    /// Stable identifier stored in settings. Never change once shipped.
    pub id: &'static str,
    /// Human-friendly label shown in the settings dropdown.
    pub label: &'static str,
    data: &'static [u8],
}

macro_rules! bundled {
    ($($n:literal),+ $(,)?) => {
        &[$(BundledSound {
            id: concat!("sound", $n),
            label: concat!("Sound ", $n),
            data: include_bytes!(concat!("../../../data/sounds/sound", $n, ".ogg")),
        }),+]
    };
}

/// Registry of bundled notification sounds. Order here is the order shown
/// in the settings dropdown.
pub static BUNDLED_SOUNDS: &[BundledSound] = bundled!(
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18
);

/// Fallback sound ID used when the saved `sound_name` doesn't match any
/// bundled sound (e.g. settings file predates the switch to bundled sounds).
pub const DEFAULT_SOUND_ID: &str = "sound1";

/// Plays a bundled notification sound by ID.
///
/// Returns `Ok(())` when playback was dispatched, or `Err(reason)` when the
/// sound couldn't be played (unknown ID or `paplay` not available). Callers
/// that want user-facing feedback should surface the error.
pub fn play_sound(sound_id: &str) -> Result<(), String> {
    let sound_id = sound_id.trim();
    // Accept the saved ID if it matches a bundled sound; otherwise fall back to
    // the default. Keeps notifications audible across upgrades even when the
    // saved ID is stale (e.g. from when sound names came from system themes).
    let sound = BUNDLED_SOUNDS
        .iter()
        .find(|s| s.id == sound_id)
        .or_else(|| BUNDLED_SOUNDS.iter().find(|s| s.id == DEFAULT_SOUND_ID))
        .ok_or_else(|| "no bundled sounds available".to_string())?;
    let path = ensure_cached(sound)?;

    match Command::new("paplay")
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = format!(
                "paplay not available — install pulseaudio-utils to enable TuxFlow sound ({e})"
            );
            log::warn!("{msg}");
            Err(msg)
        }
    }
}

/// Extracts a bundled sound to the user's cache dir if not already there and
/// returns the on-disk path. Repeat calls are cheap — just a stat + path build.
fn ensure_cached(sound: &BundledSound) -> Result<PathBuf, String> {
    let cache_root = cache_dir();
    std::fs::create_dir_all(&cache_root)
        .map_err(|e| format!("could not create sound cache dir: {e}"))?;
    let path = cache_root.join(format!("{}.ogg", sound.id));
    if !path.is_file() {
        std::fs::write(&path, sound.data)
            .map_err(|e| format!("could not write cached sound: {e}"))?;
    }
    Ok(path)
}

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("tuxflow").join("sounds")
}
