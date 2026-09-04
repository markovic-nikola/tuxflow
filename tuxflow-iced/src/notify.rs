//! Desktop notifications for process lifecycle events, honoring the
//! shared settings.toml notification flags. Mirrors the GTK app's
//! surface (`util/notifications.rs`): crash, auto-restart, connection
//! lost (once per outage), clean finish, and the agent's "waiting for
//! input" — each carrying the project's icon and the configured sound
//! (per-agent overrides for the idle one). Best-effort — a missing
//! notification daemon must never affect the app.
//!
//! The flags that decide WHETHER to fire (`on_*`, `suppress_when_focused`)
//! are checked by the caller in main.rs, where the window focus and the
//! visible terminal live; this module only knows how to send.
//!
//! Sent as a plain `org.freedesktop.Notifications.Notify` over ONE bus
//! connection kept for the app's lifetime — what gio does for the GTK
//! shell. That is load-bearing, not tidiness: Cinnamon (and GNOME Shell,
//! same lineage) watches each notification's SENDER name and destroys the
//! whole source, banner included, the moment that name leaves the bus —
//! provided it could map the sender's PID to a window, which an app with a
//! window always satisfies. notify-rust opens a fresh connection per
//! `show()` and drops it on return, so every banner was closed by the
//! daemon ~3 ms after it was accepted (reason 2, "dismissed"): the sound
//! played, nothing was ever seen. A windowless `notify-send` never trips
//! it, which is why probes from a shell looked fine.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use tuxflow_core::config::settings::NotificationSettings;
use tuxflow_core::util::agents::AgentKind;
use tuxflow_core::util::sounds::{NotificationKind, sound_for};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

const APP_NAME: &str = "TuxFlow";
/// The `desktop-entry` hint gio sends: lets the daemon show the app's own
/// name and icon for the source when the .desktop file is installed.
const DESKTOP_ID: &str = "com.tuxflow.TuxFlowIced";

/// The app's notification connection, opened on first use and kept.
/// Dropped on a failed call so the next send reconnects rather than
/// retrying a dead socket forever.
static BUS: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();

fn bus() -> zbus::Result<Connection> {
    let slot = BUS.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(conn) = guard.as_ref() {
        return Ok(conn.clone());
    }
    let conn = Connection::session()?;
    *guard = Some(conn.clone());
    Ok(conn)
}

fn forget_bus() {
    if let Some(slot) = BUS.get() {
        *slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// One `Notify` call; returns the daemon's notification id.
fn notify(summary: &str, body: &str, icon: &str) -> zbus::Result<u32> {
    let conn = bus()?;
    let proxy = Proxy::new(
        &conn,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )?;
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", Value::from(DESKTOP_ID));
    let actions: Vec<&str> = Vec::new();
    proxy.call(
        "Notify",
        &(APP_NAME, 0u32, icon, summary, body, actions, hints, -1i32),
    )
}

/// GTK's `send`: a notification with an optional file icon, then the
/// sound — `kind` picks the cue from the chosen sound's pack (core's
/// `sound_for`: crash → error, finish → success, outages → warning),
/// `sound` overrides that whole (the per-agent idle sounds), and nothing
/// plays while sounds are off. paplay is fire-and-forget; failure is
/// logged in core.
fn send(
    ns: &NotificationSettings,
    kind: NotificationKind,
    summary: &str,
    body: &str,
    icon: Option<&Path>,
    sound: Option<&str>,
) {
    let summary = summary.to_string();
    let body = body.to_string();
    // Only a file the daemon can open; GTK's FileIcon does the same check.
    let icon = icon
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    // The bus call blocks; keep it off the UI thread.
    std::thread::spawn(move || match notify(&summary, &body, &icon) {
        Ok(id) => log::debug!("notification #{id}: {summary} / {body}"),
        Err(e) => {
            log::warn!("notification failed: {e}");
            forget_bus();
        }
    });
    if ns.sound_enabled {
        let id = sound.unwrap_or_else(|| sound_for(&ns.sound_name, kind));
        let _ = tuxflow_core::util::sounds::play_sound(id);
    }
}

/// The idle sound for this agent kind, when the settings pick one. GTK's
/// `per_agent_sound`: only the three agents with a settings row have an
/// override; the rest fall back to the global sound.
fn per_agent_sound(ns: &NotificationSettings, kind: AgentKind) -> Option<&str> {
    match kind {
        AgentKind::Claude => ns.claude_sound_name.as_deref(),
        AgentKind::Codex => ns.codex_sound_name.as_deref(),
        AgentKind::Gemini => ns.gemini_sound_name.as_deref(),
        AgentKind::OpenCode | AgentKind::Unknown => None,
    }
}

/// The settings page's "Send Test" button.
pub fn test(ns: &NotificationSettings) {
    send(ns, NotificationKind::Test, "TuxFlow", "test", None, None);
}

pub fn crash(
    ns: &NotificationSettings,
    project: &str,
    process: &str,
    code: Option<i32>,
    icon: Option<&Path>,
) {
    let body = match code {
        Some(code) => format!("{process} crashed (exit {code})"),
        None => format!("{process} crashed"),
    };
    send(ns, NotificationKind::Crash, project, &body, icon, None);
}

pub fn auto_restart(
    ns: &NotificationSettings,
    project: &str,
    process: &str,
    attempt: u32,
    icon: Option<&Path>,
) {
    send(
        ns,
        NotificationKind::AutoRestart,
        project,
        &format!("{process} crashed — restarting ({attempt})"),
        icon,
        None,
    );
}

/// The caller fires it once per outage — a long outage must not ping
/// every backoff tick.
pub fn disconnect(ns: &NotificationSettings, project: &str, process: &str, icon: Option<&Path>) {
    send(
        ns,
        NotificationKind::Disconnect,
        project,
        &format!("{process}: connection lost — reconnecting (it keeps running on the host)"),
        icon,
        None,
    );
}

pub fn finish(ns: &NotificationSettings, project: &str, process: &str, icon: Option<&Path>) {
    send(
        ns,
        NotificationKind::Finish,
        project,
        &format!("{process} finished"),
        icon,
        None,
    );
}

/// An agent finished its turn — GTK's `notify_agent_idle`, fired on the
/// terminal bell (the primary signal) or by the silence fallback. Carries
/// the agent's own sound when one is configured. OpenCode emits its own
/// desktop notifications, so TuxFlow stays silent for it.
pub fn agent_idle(
    ns: &NotificationSettings,
    project: &str,
    process: &str,
    icon: Option<&Path>,
    kind: AgentKind,
) {
    if kind == AgentKind::OpenCode {
        return;
    }
    send(
        ns,
        NotificationKind::AgentIdle,
        project,
        &format!("{process}: waiting for input"),
        icon,
        per_agent_sound(ns, kind),
    );
}

/// The microphone bridge to a remote host couldn't be brought up. Worth
/// interrupting for: every downstream symptom (agents reporting no
/// microphone, voice "failing repeatedly and paused") points nowhere near
/// the cause. Unconditional like GTK's `notify_mic_bridge_failed`, and
/// carrying the sound like every other kind.
pub fn mic_bridge_failed(ns: &NotificationSettings, host: &str, reason: &str) {
    send(
        ns,
        NotificationKind::MicBridgeFailed,
        "Microphone bridge unavailable",
        &format!("Voice input won't work on {host} \u{2014} {reason}"),
        None,
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> NotificationSettings {
        NotificationSettings {
            sound_name: "cinematic-notification".into(),
            claude_sound_name: Some("soft-badge".into()),
            codex_sound_name: None,
            gemini_sound_name: Some("glass-success".into()),
            ..NotificationSettings::default()
        }
    }

    /// GTK's rule: an agent with a configured sound gets it, one whose
    /// row is empty falls back (None → the caller uses the global pick),
    /// and the kinds without a row never override.
    #[test]
    fn per_agent_sound_overrides_only_where_configured() {
        let ns = settings();
        assert_eq!(per_agent_sound(&ns, AgentKind::Claude), Some("soft-badge"));
        assert_eq!(
            per_agent_sound(&ns, AgentKind::Gemini),
            Some("glass-success")
        );
        assert_eq!(per_agent_sound(&ns, AgentKind::Codex), None);
        assert_eq!(per_agent_sound(&ns, AgentKind::OpenCode), None);
        assert_eq!(per_agent_sound(&ns, AgentKind::Unknown), None);
    }
}
