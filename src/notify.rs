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
//!
//! A notification about a process carries a "default" action — the click
//! on its body — and the click comes back as `ActionInvoked`, which the
//! app answers by showing that process (`clicks`). Without the action the
//! daemon only launches the desktop entry, which the single-instance
//! socket turns into "raise the window" on whatever project was showing.
//! The signal is sent to the notification's SENDER alone (GNOME Shell and
//! Cinnamon unicast it), so it is read on the same lifelong connection —
//! a second reason that connection must be the one every call goes out on.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use iced::futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use iced::futures::{Stream, StreamExt};
use tuxflow_core::config::settings::NotificationSettings;
use tuxflow_core::util::agents::AgentKind;
use tuxflow_core::util::sounds::{NotificationKind, sound_for};
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::Value;

const APP_NAME: &str = "TuxFlow";
/// The `desktop-entry` hint gio sends: lets the daemon show the app's own
/// name and icon for the source when the .desktop file is installed.
const DESKTOP_ID: &str = "com.tuxflow.TuxFlow";
const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const IFACE: &str = "org.freedesktop.Notifications";
/// The spec's name for a click on the notification itself; daemons don't
/// draw it as a button.
const DEFAULT_ACTION: &str = "default";

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
    // Subscribed before the first Notify goes out on it, so no click can
    // arrive unheard. Clicks are a nicety: notifications still go out
    // when the subscription fails.
    if let Err(e) = listen(&conn) {
        log::warn!("notification clicks unavailable: {e}");
    }
    *guard = Some(conn.clone());
    Ok(conn)
}

fn forget_bus() {
    if let Some(slot) = BUS.get() {
        *slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// The open notifications that name a process, both ways round: a click
/// resolves its id to the process's `term_id` (stable across runs, unlike
/// an entry index), and a new notification about a process REPLACES that
/// process's previous one — each process keeps one banner saying where it
/// is now, not a stack of "waiting for input"s. Pruned on
/// `NotificationClosed` and on replacement, so it holds at most one id
/// per process.
#[derive(Default)]
struct Shown {
    term_of: HashMap<u32, u64>,
    id_of: HashMap<u64, u32>,
}

static SHOWN: OnceLock<Mutex<Shown>> = OnceLock::new();

fn shown() -> std::sync::MutexGuard<'static, Shown> {
    SHOWN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

impl Shown {
    fn record(&mut self, id: u32, term: u64) {
        if let Some(old) = self.id_of.insert(term, id)
            && old != id
        {
            self.term_of.remove(&old);
        }
        self.term_of.insert(id, term);
    }

    fn closed(&mut self, id: u32) {
        if let Some(term) = self.term_of.remove(&id)
            && self.id_of.get(&term) == Some(&id)
        {
            self.id_of.remove(&term);
        }
    }
}

/// Clicked notifications, as the `term_id` they named — from the listener
/// thread to the app's `clicks` stream.
type Clicks = (UnboundedSender<u64>, Mutex<Option<UnboundedReceiver<u64>>>);

static CLICKS: OnceLock<Clicks> = OnceLock::new();

fn click_channel() -> &'static Clicks {
    CLICKS.get_or_init(|| {
        let (tx, rx) = unbounded();
        (tx, Mutex::new(Some(rx)))
    })
}

/// The processes whose notifications get clicked, for the app to show.
/// One subscriber: a second call gets an empty stream.
pub fn clicks() -> impl Stream<Item = u64> + Send {
    let rx = click_channel()
        .1
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    iced::futures::stream::iter(rx).flatten()
}

/// Follow `ActionInvoked` and `NotificationClosed` on `conn`. The match
/// rule is registered here, synchronously; the reading happens on a plain
/// thread (never one of the runtime's blocking slots, which a closing
/// window would wait on forever) that ends with its connection.
fn listen(conn: &Connection) -> zbus::Result<()> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(IFACE)?
        .path(PATH)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, conn, None)?;
    std::thread::spawn(move || {
        for msg in signals {
            let Ok(msg) = msg else { break };
            let header = msg.header();
            match header.member().map(|m| m.as_str()) {
                Some("ActionInvoked") => {
                    let Ok((id, action)) = msg.body().deserialize::<(u32, String)>() else {
                        continue;
                    };
                    let term = shown().term_of.get(&id).copied();
                    if action == DEFAULT_ACTION
                        && let Some(term) = term
                    {
                        let _ = click_channel().0.unbounded_send(term);
                    }
                }
                Some("NotificationClosed") => {
                    if let Ok((id, _reason)) = msg.body().deserialize::<(u32, u32)>() {
                        shown().closed(id);
                    }
                }
                _ => {}
            }
        }
    });
    Ok(())
}

/// One `Notify` call; returns the daemon's notification id. `term` names
/// the process the notification is about: it gets the click action and
/// replaces that process's previous notification.
fn notify(summary: &str, body: &str, icon: &str, term: Option<u64>) -> zbus::Result<u32> {
    let conn = bus()?;
    let proxy = Proxy::new(&conn, DEST, PATH, IFACE)?;
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", Value::from(DESKTOP_ID));
    let (replaces, actions) = match term {
        Some(term) => (
            shown().id_of.get(&term).copied().unwrap_or(0),
            vec![DEFAULT_ACTION, "Show"],
        ),
        None => (0, Vec::new()),
    };
    let id: u32 = proxy.call(
        "Notify",
        &(
            APP_NAME, replaces, icon, summary, body, actions, hints, -1i32,
        ),
    )?;
    if let Some(term) = term {
        shown().record(id, term);
    }
    Ok(id)
}

/// GTK's `send`: a notification with an optional file icon (and, given
/// the process's `term`, a click that shows it), then the sound — `kind` picks the cue from the chosen sound's pack (core's
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
    term: Option<u64>,
) {
    let summary = summary.to_string();
    let body = body.to_string();
    // Only a file the daemon can open; GTK's FileIcon does the same check.
    let icon = icon
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    // The bus call blocks; keep it off the UI thread.
    std::thread::spawn(move || match notify(&summary, &body, &icon, term) {
        Ok(id) => log::debug!("notification #{id}: {summary} / {body}"),
        // The bus answered, so the connection is fine (no daemon, say):
        // keep it. Replacing it would leave its click listener behind,
        // holding a connection nothing sends on.
        Err(e @ (zbus::Error::MethodError(..) | zbus::Error::FDO(_))) => {
            log::warn!("notification failed: {e}")
        }
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
    send(
        ns,
        NotificationKind::Test,
        "TuxFlow",
        "test",
        None,
        None,
        None,
    );
}

pub fn crash(
    ns: &NotificationSettings,
    project: &str,
    process: &str,
    code: Option<i32>,
    icon: Option<&Path>,
    term: Option<u64>,
) {
    let body = match code {
        Some(code) => format!("{process} crashed (exit {code})"),
        None => format!("{process} crashed"),
    };
    send(
        ns,
        NotificationKind::Crash,
        project,
        &body,
        icon,
        None,
        term,
    );
}

pub fn auto_restart(
    ns: &NotificationSettings,
    project: &str,
    process: &str,
    attempt: u32,
    icon: Option<&Path>,
    term: Option<u64>,
) {
    send(
        ns,
        NotificationKind::AutoRestart,
        project,
        &format!("{process} crashed — restarting ({attempt})"),
        icon,
        None,
        term,
    );
}

/// The caller fires it once per outage — a long outage must not ping
/// every backoff tick.
pub fn disconnect(
    ns: &NotificationSettings,
    project: &str,
    process: &str,
    icon: Option<&Path>,
    term: Option<u64>,
) {
    send(
        ns,
        NotificationKind::Disconnect,
        project,
        &format!("{process}: connection lost — reconnecting (it keeps running on the host)"),
        icon,
        None,
        term,
    );
}

/// GTK's `notify_file_watch_restart`: a `restart_when_changed` pattern hit.
pub fn file_watch_restart(
    ns: &NotificationSettings,
    project: &str,
    process: &str,
    icon: Option<&Path>,
    term: Option<u64>,
) {
    send(
        ns,
        NotificationKind::FileWatchRestart,
        project,
        &format!("{process}: file change \u{2192} restart"),
        icon,
        None,
        term,
    );
}

pub fn finish(
    ns: &NotificationSettings,
    project: &str,
    process: &str,
    icon: Option<&Path>,
    term: Option<u64>,
) {
    send(
        ns,
        NotificationKind::Finish,
        project,
        &format!("{process} finished"),
        icon,
        None,
        term,
    );
}

/// An agent finished its turn or needs the user — GTK's `notify_agent_idle`,
/// fired by Claude's hooks, else the terminal bell or the silence fallback;
/// `detail` says which ("waiting for input" when only the bell knows). Carries
/// the agent's own sound when one is configured. OpenCode emits its own
/// desktop notifications, so TuxFlow stays silent for it.
pub fn agent_idle(
    ns: &NotificationSettings,
    project: &str,
    process: &str,
    icon: Option<&Path>,
    kind: AgentKind,
    term: Option<u64>,
    detail: &str,
) {
    if kind == AgentKind::OpenCode {
        return;
    }
    send(
        ns,
        NotificationKind::AgentIdle,
        project,
        &format!("{process}: {detail}"),
        icon,
        per_agent_sound(ns, kind),
        term,
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

    /// One banner per process: a replacement that comes back under a new
    /// id forgets the old one, and a late close of the old id must not
    /// orphan the process's current notification.
    #[test]
    fn shown_keeps_one_id_per_process() {
        let mut shown = Shown::default();
        shown.record(1, 7);
        shown.record(2, 7);
        assert_eq!(shown.term_of.get(&1), None);
        assert_eq!(shown.term_of.get(&2), Some(&7));
        shown.closed(1);
        assert_eq!(shown.id_of.get(&7), Some(&2));
        // Replaced in place (same id back): still one entry.
        shown.record(2, 7);
        assert_eq!(shown.term_of.len(), 1);
        shown.closed(2);
        assert!(shown.term_of.is_empty() && shown.id_of.is_empty());
    }
}
