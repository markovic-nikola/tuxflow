use std::path::Path;

use gtk4::gio;
use gtk4::prelude::*;

use crate::config::settings::AppSettings;

// The bundled-sound registry and playback moved to core (shared with the
// iced shell's settings window); re-exported so call sites keep reading
// `util::notifications::BUNDLED_SOUNDS` etc.
pub use tuxflow_core::util::sounds::{BUNDLED_SOUNDS, play_sound};
use tuxflow_core::util::sounds::{NotificationKind, sound_for};

// AgentKind and the per-agent resume table moved to core (shared with
// the iced shell's context menus); re-exported so call sites keep reading
// `util::notifications::AgentKind` etc.
pub use tuxflow_core::util::agents::{AgentKind, resume_command_for};

/// Internal: send a desktop notification, optionally with a file-based icon.
/// `kind` picks the cue from the chosen sound's pack (core's `sound_for`:
/// crash → error, finish → success, outages → warning); `sound_override`
/// replaces that whole (the per-agent idle sounds).
fn send(
    kind: NotificationKind,
    title: &str,
    body: &str,
    icon_path: Option<&Path>,
    sound_override: Option<&str>,
) {
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

    maybe_play_sound(kind, sound_override);
}

fn maybe_play_sound(kind: NotificationKind, override_sound_id: Option<&str>) {
    let settings = AppSettings::load();
    if !settings.notifications.sound_enabled {
        return;
    }
    let id =
        override_sound_id.unwrap_or_else(|| sound_for(&settings.notifications.sound_name, kind));
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

pub fn notify_crash(project_name: &str, process_name: &str, icon_path: Option<&Path>) {
    send(
        NotificationKind::Crash,
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
        NotificationKind::AutoRestart,
        project_name,
        &format!("{process_name}: restarting (attempt {attempt})"),
        icon_path,
        None,
    );
}

/// Remote process lost its ssh connection (exit 255) and is being restarted.
pub fn notify_reconnect(
    project_name: &str,
    process_name: &str,
    attempt: u32,
    icon_path: Option<&Path>,
) {
    send(
        NotificationKind::Disconnect,
        project_name,
        &format!("{process_name}: connection lost — reconnecting (attempt {attempt})"),
        icon_path,
        None,
    );
}

/// Remote process lost its ssh connection and won't be restarted automatically.
pub fn notify_disconnect(project_name: &str, process_name: &str, icon_path: Option<&Path>) {
    send(
        NotificationKind::Disconnect,
        project_name,
        &format!("{process_name}: ssh connection lost"),
        icon_path,
        None,
    );
}

/// A remote project's host couldn't be reached while loading it at startup;
/// TuxFlow keeps retrying in the background. Fired once per outage.
pub fn notify_remote_unreachable(project_name: &str, host: &str) {
    send(
        NotificationKind::RemoteUnreachable,
        project_name,
        &format!("Can't reach {host} — will keep retrying in the background"),
        None,
        None,
    );
}

/// The microphone bridge to a remote host couldn't be brought up. Worth
/// interrupting for: every downstream symptom (agents reporting no
/// microphone, voice "failing repeatedly") points nowhere near the cause.
pub fn notify_mic_bridge_failed(host: &str, reason: &str) {
    send(
        NotificationKind::MicBridgeFailed,
        "Microphone bridge unavailable",
        &format!("Voice input won't work on {host} — {reason}"),
        None,
        None,
    );
}

pub fn notify_finish(project_name: &str, process_name: &str, icon_path: Option<&Path>) {
    send(
        NotificationKind::Finish,
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
        NotificationKind::AgentIdle,
        project_name,
        &format!("{process_name}: waiting for input"),
        icon_path,
        sound_override.as_deref(),
    );
}

pub fn notify_file_watch_restart(project_name: &str, process_name: &str, icon_path: Option<&Path>) {
    send(
        NotificationKind::FileWatchRestart,
        project_name,
        &format!("{process_name}: file change → restart"),
        icon_path,
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_command_for_known_agents() {
        assert_eq!(
            resume_command_for("claude").as_deref(),
            Some("claude --continue")
        );
        assert_eq!(
            resume_command_for("opencode").as_deref(),
            Some("opencode --continue")
        );
        assert_eq!(
            resume_command_for("codex").as_deref(),
            Some("codex resume --last")
        );
    }

    #[test]
    fn resume_command_drops_extra_args_and_keeps_path() {
        assert_eq!(
            resume_command_for("/home/me/bin/claude --model opus").as_deref(),
            Some("/home/me/bin/claude --continue")
        );
        assert_eq!(
            resume_command_for("codex --foo bar").as_deref(),
            Some("codex resume --last")
        );
    }

    #[test]
    fn resume_command_for_unsupported_returns_none() {
        assert!(resume_command_for("gemini").is_none());
        assert!(resume_command_for("npx claude-code").is_none());
        assert!(resume_command_for("").is_none());
    }
}
