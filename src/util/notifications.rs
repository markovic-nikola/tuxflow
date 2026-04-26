use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gtk4::gio;
use gtk4::prelude::*;

use crate::config::settings::AppSettings;

/// A notification sound bundled into the binary. The audio bytes ship inside
/// the executable and are extracted to the user's cache dir the first time a
/// given sound plays, so `paplay` has a file path to feed it.
pub struct BundledSound {
    /// Stable identifier stored in settings. Never change once shipped.
    pub id: &'static str,
    /// Human-friendly label shown in the settings dropdown.
    pub label: &'static str,
    data: &'static [u8],
}

/// Registry of bundled notification sounds. Order here is the order shown in
/// the settings dropdown.
pub static BUNDLED_SOUNDS: &[BundledSound] = &[
    BundledSound {
        id: "sound1",
        label: "Sound 1",
        data: include_bytes!("../../data/sounds/sound1.ogg"),
    },
    BundledSound {
        id: "sound2",
        label: "Sound 2",
        data: include_bytes!("../../data/sounds/sound2.ogg"),
    },
    BundledSound {
        id: "sound3",
        label: "Sound 3",
        data: include_bytes!("../../data/sounds/sound3.ogg"),
    },
    BundledSound {
        id: "sound4",
        label: "Sound 4",
        data: include_bytes!("../../data/sounds/sound4.ogg"),
    },
    BundledSound {
        id: "sound5",
        label: "Sound 5",
        data: include_bytes!("../../data/sounds/sound5.ogg"),
    },
    BundledSound {
        id: "sound6",
        label: "Sound 6",
        data: include_bytes!("../../data/sounds/sound6.ogg"),
    },
    BundledSound {
        id: "sound7",
        label: "Sound 7",
        data: include_bytes!("../../data/sounds/sound7.ogg"),
    },
    BundledSound {
        id: "sound8",
        label: "Sound 8",
        data: include_bytes!("../../data/sounds/sound8.ogg"),
    },
    BundledSound {
        id: "sound9",
        label: "Sound 9",
        data: include_bytes!("../../data/sounds/sound9.ogg"),
    },
    BundledSound {
        id: "sound10",
        label: "Sound 10",
        data: include_bytes!("../../data/sounds/sound10.ogg"),
    },
    BundledSound {
        id: "sound11",
        label: "Sound 11",
        data: include_bytes!("../../data/sounds/sound11.ogg"),
    },
    BundledSound {
        id: "sound12",
        label: "Sound 12",
        data: include_bytes!("../../data/sounds/sound12.ogg"),
    },
    BundledSound {
        id: "sound13",
        label: "Sound 13",
        data: include_bytes!("../../data/sounds/sound13.ogg"),
    },
    BundledSound {
        id: "sound14",
        label: "Sound 14",
        data: include_bytes!("../../data/sounds/sound14.ogg"),
    },
    BundledSound {
        id: "sound15",
        label: "Sound 15",
        data: include_bytes!("../../data/sounds/sound15.ogg"),
    },
    BundledSound {
        id: "sound16",
        label: "Sound 16",
        data: include_bytes!("../../data/sounds/sound16.ogg"),
    },
    BundledSound {
        id: "sound17",
        label: "Sound 17",
        data: include_bytes!("../../data/sounds/sound17.ogg"),
    },
    BundledSound {
        id: "sound18",
        label: "Sound 18",
        data: include_bytes!("../../data/sounds/sound18.ogg"),
    },
];

/// Fallback sound ID used when the saved `sound_name` doesn't match any
/// bundled sound (e.g. settings file predates the switch to bundled sounds).
pub const DEFAULT_SOUND_ID: &str = "sound1";

/// Identifies a built-in AI agent so notifications can apply per-agent
/// preferences (e.g. a different sound for Claude vs. Codex) or suppression
/// rules (OpenCode emits its own desktop notifications, so TuxFlow stays
/// silent for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Unknown,
}

impl AgentKind {
    /// Derived from the first whitespace-separated token of `ProcessConfig.command`.
    /// Matches the agent labels set by `new_agent:<kind>` in the command
    /// palette (see `src/ui/window.rs`).
    pub fn from_command(command: &str) -> Self {
        // Take the executable name (basename of first whitespace-separated token,
        // lowercased) so paths and shell aliases both resolve. Common aliases
        // like `cc` for Claude Code are recognized.
        let token = command.split_whitespace().next().unwrap_or("");
        let exe = token.rsplit('/').next().unwrap_or(token).to_lowercase();
        match exe.as_str() {
            "claude" | "claude-code" | "cc" => Self::Claude,
            "codex" => Self::Codex,
            "gemini" => Self::Gemini,
            "opencode" => Self::OpenCode,
            _ => Self::Unknown,
        }
    }
}

/// Internal: send a desktop notification, optionally with a file-based icon.
fn send(title: &str, body: &str, icon_path: Option<&Path>, sound_override: Option<&str>) {
    let notification = gio::Notification::new(title);
    notification.set_body(Some(body));

    if let Some(path) = icon_path
        && path.is_file()
    {
        let file = gio::File::for_path(path);
        let icon = gio::FileIcon::new(&file);
        notification.set_icon(&icon);
    }

    if let Some(app) = gio::Application::default() {
        app.send_notification(None, &notification);
    } else {
        log::warn!("No application instance for notification: {title}");
    }

    maybe_play_sound(sound_override);
}

fn maybe_play_sound(override_sound_id: Option<&str>) {
    let settings = AppSettings::load();
    if !settings.notifications.sound_enabled {
        return;
    }
    let id = override_sound_id.unwrap_or(&settings.notifications.sound_name);
    let _ = play_sound(id);
}

fn per_agent_sound(
    ns: &crate::config::settings::NotificationSettings,
    kind: AgentKind,
) -> Option<String> {
    let id = match kind {
        AgentKind::Claude => ns.claude_sound_name.as_deref(),
        AgentKind::Codex => ns.codex_sound_name.as_deref(),
        AgentKind::Gemini => ns.gemini_sound_name.as_deref(),
        AgentKind::OpenCode | AgentKind::Unknown => None,
    };
    id.map(|s| s.to_string())
}

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
        .ok_or_else(|| format!("no bundled sounds available"))?;
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

pub fn notify_crash(project_name: &str, process_name: &str, icon_path: Option<&Path>) {
    send(
        project_name,
        &format!("{process_name}: crashed"),
        icon_path,
        None,
    );
}

pub fn notify_restart(
    project_name: &str,
    process_name: &str,
    attempt: u32,
    icon_path: Option<&Path>,
) {
    send(
        project_name,
        &format!("{process_name}: restarting (attempt {attempt})"),
        icon_path,
        None,
    );
}

pub fn notify_finish(project_name: &str, process_name: &str, icon_path: Option<&Path>) {
    send(
        project_name,
        &format!("{process_name}: finished"),
        icon_path,
        None,
    );
}

pub fn notify_agent_idle(
    project_name: &str,
    process_name: &str,
    icon_path: Option<&Path>,
    kind: AgentKind,
) {
    // OpenCode emits its own desktop notifications — don't double up.
    if kind == AgentKind::OpenCode {
        return;
    }
    let settings = AppSettings::load();
    let sound_override = per_agent_sound(&settings.notifications, kind);
    send(
        project_name,
        &format!("{process_name}: waiting for input"),
        icon_path,
        sound_override.as_deref(),
    );
}

pub fn notify_file_watch_restart(project_name: &str, process_name: &str, icon_path: Option<&Path>) {
    send(
        project_name,
        &format!("{process_name}: file change → restart"),
        icon_path,
        None,
    );
}
