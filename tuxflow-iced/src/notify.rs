//! Desktop notifications for process lifecycle events, honoring the
//! shared settings.toml notification flags (read-only). Mirrors the GTK
//! app's surface: crash, auto-restart, connection lost (once per outage),
//! clean finish. Best-effort — a missing notification daemon must never
//! affect the app.

use tuxflow_core::config::settings::NotificationSettings;

fn send(summary: &str, body: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    // notify-rust blocks on DBus; keep it off the UI thread.
    std::thread::spawn(move || {
        let result = notify_rust::Notification::new()
            .summary(&summary)
            .body(&body)
            .appname("TuxFlow")
            .show();
        if let Err(e) = result {
            log::debug!("notification failed: {e}");
        }
    });
}

pub fn crash(ns: &NotificationSettings, project: &str, process: &str, code: Option<i32>) {
    if !ns.on_crash {
        return;
    }
    let body = match code {
        Some(code) => format!("{process} crashed (exit {code})"),
        None => format!("{process} crashed"),
    };
    send(project, &body);
}

pub fn auto_restart(ns: &NotificationSettings, project: &str, process: &str, attempt: u32) {
    if !ns.on_auto_restart {
        return;
    }
    send(
        project,
        &format!("{process} crashed — restarting ({attempt})"),
    );
}

/// Unconditional like the GTK app, but the caller fires it once per
/// outage — a long outage must not ping every backoff tick.
pub fn disconnect(project: &str, process: &str) {
    send(
        project,
        &format!("{process}: connection lost — reconnecting (it keeps running on the host)"),
    );
}

pub fn finish(ns: &NotificationSettings, project: &str, process: &str) {
    if !ns.on_process_finish {
        return;
    }
    send(project, &format!("{process} finished"));
}
