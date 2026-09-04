//! Bundled notification sounds, shared by both shells. The audio bytes
//! ship inside the executable and are extracted to the user's cache dir
//! the first time a given sound plays, so `paplay` has a file path to
//! feed it. The files are uisfx renders (CC0) — see data/sounds/CREDITS.md.

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

macro_rules! snd {
    ($pack:literal, $pack_label:literal, $cue:literal, $cue_label:literal) => {
        BundledSound {
            id: concat!($pack, "-", $cue),
            label: concat!($pack_label, " \u{b7} ", $cue_label),
            data: include_bytes!(concat!("../../../data/sounds/", $pack, "-", $cue, ".ogg")),
        }
    };
}

/// Registry of bundled notification sounds. Order here is the order shown
/// in the settings dropdown: grouped by pack, cues in a fixed order.
///
/// The set is a full grid — every pack carries every cue — so a user who
/// likes one pack's `error` finds its `success` next to it. The grid is
/// pinned by `full_grid` below; add a pack or a cue by adding its whole
/// row or column (see data/sounds/CREDITS.md for provenance).
pub static BUNDLED_SOUNDS: &[BundledSound] = &[
    snd!("minimal", "Minimal", "notification", "Notification"),
    snd!("minimal", "Minimal", "success", "Success"),
    snd!("minimal", "Minimal", "error", "Error"),
    snd!("minimal", "Minimal", "warning", "Warning"),
    snd!("minimal", "Minimal", "badge", "Badge"),
    snd!("minimal", "Minimal", "reward", "Reward"),
    snd!("minimal", "Minimal", "achievement", "Achievement"),
    snd!("minimal", "Minimal", "checkpoint", "Checkpoint"),
    snd!("soft", "Soft", "notification", "Notification"),
    snd!("soft", "Soft", "success", "Success"),
    snd!("soft", "Soft", "error", "Error"),
    snd!("soft", "Soft", "warning", "Warning"),
    snd!("soft", "Soft", "badge", "Badge"),
    snd!("soft", "Soft", "reward", "Reward"),
    snd!("soft", "Soft", "achievement", "Achievement"),
    snd!("soft", "Soft", "checkpoint", "Checkpoint"),
    snd!("glass", "Glass", "notification", "Notification"),
    snd!("glass", "Glass", "success", "Success"),
    snd!("glass", "Glass", "error", "Error"),
    snd!("glass", "Glass", "warning", "Warning"),
    snd!("glass", "Glass", "badge", "Badge"),
    snd!("glass", "Glass", "reward", "Reward"),
    snd!("glass", "Glass", "achievement", "Achievement"),
    snd!("glass", "Glass", "checkpoint", "Checkpoint"),
    snd!("arcade", "Arcade", "notification", "Notification"),
    snd!("arcade", "Arcade", "success", "Success"),
    snd!("arcade", "Arcade", "error", "Error"),
    snd!("arcade", "Arcade", "warning", "Warning"),
    snd!("arcade", "Arcade", "badge", "Badge"),
    snd!("arcade", "Arcade", "reward", "Reward"),
    snd!("arcade", "Arcade", "achievement", "Achievement"),
    snd!("arcade", "Arcade", "checkpoint", "Checkpoint"),
    snd!("organic", "Organic", "notification", "Notification"),
    snd!("organic", "Organic", "success", "Success"),
    snd!("organic", "Organic", "error", "Error"),
    snd!("organic", "Organic", "warning", "Warning"),
    snd!("organic", "Organic", "badge", "Badge"),
    snd!("organic", "Organic", "reward", "Reward"),
    snd!("organic", "Organic", "achievement", "Achievement"),
    snd!("organic", "Organic", "checkpoint", "Checkpoint"),
    snd!("dreamy", "Dreamy", "notification", "Notification"),
    snd!("dreamy", "Dreamy", "success", "Success"),
    snd!("dreamy", "Dreamy", "error", "Error"),
    snd!("dreamy", "Dreamy", "warning", "Warning"),
    snd!("dreamy", "Dreamy", "badge", "Badge"),
    snd!("dreamy", "Dreamy", "reward", "Reward"),
    snd!("dreamy", "Dreamy", "achievement", "Achievement"),
    snd!("dreamy", "Dreamy", "checkpoint", "Checkpoint"),
    snd!("scifi", "Sci-fi", "notification", "Notification"),
    snd!("scifi", "Sci-fi", "success", "Success"),
    snd!("scifi", "Sci-fi", "error", "Error"),
    snd!("scifi", "Sci-fi", "warning", "Warning"),
    snd!("scifi", "Sci-fi", "badge", "Badge"),
    snd!("scifi", "Sci-fi", "reward", "Reward"),
    snd!("scifi", "Sci-fi", "achievement", "Achievement"),
    snd!("scifi", "Sci-fi", "checkpoint", "Checkpoint"),
    snd!("rubber", "Rubber", "notification", "Notification"),
    snd!("rubber", "Rubber", "success", "Success"),
    snd!("rubber", "Rubber", "error", "Error"),
    snd!("rubber", "Rubber", "warning", "Warning"),
    snd!("rubber", "Rubber", "badge", "Badge"),
    snd!("rubber", "Rubber", "reward", "Reward"),
    snd!("rubber", "Rubber", "achievement", "Achievement"),
    snd!("rubber", "Rubber", "checkpoint", "Checkpoint"),
    snd!("cinematic", "Cinematic", "notification", "Notification"),
    snd!("cinematic", "Cinematic", "success", "Success"),
    snd!("cinematic", "Cinematic", "error", "Error"),
    snd!("cinematic", "Cinematic", "warning", "Warning"),
    snd!("cinematic", "Cinematic", "badge", "Badge"),
    snd!("cinematic", "Cinematic", "reward", "Reward"),
    snd!("cinematic", "Cinematic", "achievement", "Achievement"),
    snd!("cinematic", "Cinematic", "checkpoint", "Checkpoint"),
];

/// A cue in the bundled grid — every pack carries each of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    Notification,
    Success,
    Error,
    Warning,
    Badge,
    Reward,
    Achievement,
    Checkpoint,
}

impl Cue {
    pub fn as_str(self) -> &'static str {
        match self {
            Cue::Notification => "notification",
            Cue::Success => "success",
            Cue::Error => "error",
            Cue::Warning => "warning",
            Cue::Badge => "badge",
            Cue::Reward => "reward",
            Cue::Achievement => "achievement",
            Cue::Checkpoint => "checkpoint",
        }
    }
}

/// What a notification is about, which decides which cue of the chosen
/// pack it plays. Shared by both shells so the mapping can't drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind {
    /// An agent finished its turn — the everyday one, plays the chosen
    /// sound as-is (per-agent overrides replace it whole).
    AgentIdle,
    /// The settings page's Send Test — the chosen sound, so the preview
    /// is what the user will actually hear most.
    Test,
    /// A file change triggered a restart — routine, the chosen sound.
    FileWatchRestart,
    /// A process ended cleanly.
    Finish,
    /// A process died with a non-zero exit.
    Crash,
    /// Crash detection is bringing a process back.
    AutoRestart,
    /// A remote process lost its ssh link (once per outage).
    Disconnect,
    /// A remote project's host couldn't be reached at load.
    RemoteUnreachable,
    /// The microphone bridge to a host failed to come up.
    MicBridgeFailed,
}

impl NotificationKind {
    /// The sibling cue this kind borrows from the chosen sound's pack;
    /// `None` plays the chosen sound itself.
    pub fn cue(self) -> Option<Cue> {
        match self {
            NotificationKind::AgentIdle
            | NotificationKind::Test
            | NotificationKind::FileWatchRestart => None,
            NotificationKind::Finish => Some(Cue::Success),
            NotificationKind::Crash => Some(Cue::Error),
            NotificationKind::AutoRestart
            | NotificationKind::Disconnect
            | NotificationKind::RemoteUnreachable
            | NotificationKind::MicBridgeFailed => Some(Cue::Warning),
        }
    }
}

/// The chosen sound if it is bundled, else the default — the same
/// resolution `play_sound` applies to a stale saved id.
fn resolve(chosen: &str) -> &'static BundledSound {
    let chosen = chosen.trim();
    BUNDLED_SOUNDS
        .iter()
        .find(|s| s.id == chosen)
        .or_else(|| BUNDLED_SOUNDS.iter().find(|s| s.id == DEFAULT_SOUND_ID))
        .expect("registry has the default sound")
}

/// The sound a notification of `kind` plays given the user's chosen one:
/// the outcome kinds take their cue from the chosen sound's PACK (a user
/// on `cinematic-notification` hears `cinematic-error` for a crash), the
/// rest play the chosen sound itself. Always a bundled id.
pub fn sound_for(chosen: &str, kind: NotificationKind) -> &'static str {
    let base = resolve(chosen);
    let Some(cue) = kind.cue() else {
        return base.id;
    };
    let pack = base.id.split_once('-').map(|(p, _)| p).unwrap_or(base.id);
    let wanted = format!("{pack}-{}", cue.as_str());
    BUNDLED_SOUNDS
        .iter()
        .find(|s| s.id == wanted)
        .map(|s| s.id)
        .unwrap_or(base.id)
}

/// Fallback sound ID used when the saved `sound_name` doesn't match any
/// bundled sound (e.g. settings file predates the switch to bundled sounds).
pub const DEFAULT_SOUND_ID: &str = "cinematic-notification";

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

    // Name the stream: PulseAudio/WirePlumber remember per-application
    // routing by client name, so a user who moves "TuxFlow" to their
    // earbuds once in the sound settings keeps it there — an anonymous
    // "paplay" stream follows the system default sink, which is not always
    // the device being listened on. `media.role=event` is the freedesktop
    // role for notification sounds (mixer sliders group by it).
    match Command::new("paplay")
        .arg("--client-name=TuxFlow")
        .arg("--property=media.role=event")
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            // Reap it: a spawned child nobody waits on stays a zombie for the
            // app's lifetime, one per notification played.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
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
///
/// A cached file whose size differs from the bundled bytes is rewritten: ids
/// never change, but the audio behind one can (re-mastered, re-encoded), and
/// an "exists" check alone would keep playing the old render forever.
fn ensure_cached(sound: &BundledSound) -> Result<PathBuf, String> {
    let cache_root = cache_dir();
    std::fs::create_dir_all(&cache_root)
        .map_err(|e| format!("could not create sound cache dir: {e}"))?;
    let path = cache_root.join(format!("{}.ogg", sound.id));
    let current = std::fs::metadata(&path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len());
    if current != Some(sound.data.len() as u64) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn split(id: &str) -> (&str, &str) {
        id.split_once('-').expect("id is pack-cue")
    }

    /// Every pack carries every cue, nothing is listed twice, and the
    /// default is a real entry — a half-row would leave a pack with no
    /// "error" to pair with its "success".
    #[test]
    fn full_grid() {
        let ids: BTreeSet<&str> = BUNDLED_SOUNDS.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), BUNDLED_SOUNDS.len(), "duplicate ids");
        let packs: BTreeSet<&str> = ids.iter().map(|id| split(id).0).collect();
        let cues: BTreeSet<&str> = ids.iter().map(|id| split(id).1).collect();
        for p in &packs {
            for c in &cues {
                assert!(ids.contains(format!("{p}-{c}").as_str()), "missing {p}-{c}");
            }
        }
        assert_eq!(ids.len(), packs.len() * cues.len());
        assert!(ids.contains(DEFAULT_SOUND_ID));
    }

    /// The outcome kinds borrow the sibling cue of the chosen pack; the
    /// neutral kinds play the choice itself; a stale id derives from the
    /// default rather than playing nothing.
    #[test]
    fn kinds_borrow_the_chosen_packs_cue() {
        use NotificationKind::*;
        assert_eq!(
            sound_for("cinematic-notification", Crash),
            "cinematic-error"
        );
        assert_eq!(sound_for("soft-badge", Finish), "soft-success");
        assert_eq!(sound_for("arcade-reward", Disconnect), "arcade-warning");
        assert_eq!(sound_for("arcade-reward", AutoRestart), "arcade-warning");
        assert_eq!(
            sound_for("dreamy-checkpoint", AgentIdle),
            "dreamy-checkpoint"
        );
        assert_eq!(sound_for("dreamy-checkpoint", Test), "dreamy-checkpoint");
        assert_eq!(sound_for("sound1", Crash), "cinematic-error");
        assert_eq!(sound_for("sound1", AgentIdle), DEFAULT_SOUND_ID);
        for kind in [
            Finish,
            Crash,
            AutoRestart,
            Disconnect,
            RemoteUnreachable,
            MicBridgeFailed,
        ] {
            let cue = kind.cue().expect("outcome kind has a cue");
            assert!(
                BUNDLED_SOUNDS.iter().any(|s| s.id.ends_with(cue.as_str())),
                "{cue:?} missing"
            );
        }
    }

    #[test]
    fn labels_are_unique_and_map_back() {
        let labels: BTreeSet<&str> = BUNDLED_SOUNDS.iter().map(|s| s.label).collect();
        assert_eq!(
            labels.len(),
            BUNDLED_SOUNDS.len(),
            "labels must round-trip to ids"
        );
    }
}
