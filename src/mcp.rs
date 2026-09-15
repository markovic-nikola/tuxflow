//! MCP in the iced shell: what a project's socket answers with, and the
//! request wrapper that carries an agent's command into `update()`.
//!
//! The server itself is core's (`mcp/server.rs`, one thread and one
//! Unix socket per project); this shell gives each project an ISOLATED
//! bridge (`McpBridge::isolated`) rather than the GTK app's process-wide
//! table, so an agent connected to project A's socket sees A's
//! processes and nothing else. Snapshots are rewritten from the live
//! entries whenever their fingerprint moves (checked at the top of
//! `update()`, the running-tier idiom), and commands arrive as events —
//! the tool call parks on a oneshot until the handler answers it.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tuxflow_core::mcp::bridge::{McpCommand, ProcessSnapshot, now_unix};

use crate::processes::{ProcessEntry, Status};

/// An agent's command on its way to `update()`. `Event` is `Clone` +
/// `Debug` and a `McpCommand` is neither (it carries a oneshot reply), so
/// it rides in a shared slot the handler takes exactly once.
#[derive(Clone)]
pub struct Request(Arc<Mutex<Option<McpCommand>>>);

impl Request {
    pub fn new(command: McpCommand) -> Self {
        Self(Arc::new(Mutex::new(Some(command))))
    }

    pub fn take(&self) -> Option<McpCommand> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpRequest")
    }
}

/// The status word an agent reads. Restart/reconnect attempts are folded
/// into the word: the count is sidebar detail, and "Restarting" alone is
/// what should stop an agent from starting its own copy.
pub fn status_word(status: &Status) -> &'static str {
    match status {
        Status::Stopped => "Stopped",
        Status::Running => "Running",
        Status::Crashed(_) => "Crashed",
        Status::Restarting(_) => "Restarting",
        Status::Reconnecting(_) => "Reconnecting",
    }
}

/// One process as the socket reports it. `url` is the detected badge URL
/// as PRINTED — the host's port on a remote project, never the tunnel's
/// local remap, because the agent asking is on the host — and only while
/// something is serving it: the detector keeps a stopped process's last
/// URL for the sidebar, but handed to an agent beside "Stopped" it reads
/// as an address to try. Reconnecting counts as serving (the command is
/// alive on the host; only the link is down).
pub fn snapshot(
    entry: &ProcessEntry,
    url: Option<&str>,
    project_dir: &str,
    now: Instant,
) -> ProcessSnapshot {
    ProcessSnapshot {
        name: entry.config.name.clone(),
        status: status_word(&entry.status).to_string(),
        command: entry.config.command.clone(),
        category: format!("{:?}", entry.config.category),
        working_dir: Some(
            entry
                .config
                .working_dir
                .clone()
                .unwrap_or_else(|| project_dir.to_string()),
        ),
        url: url
            .filter(|_| matches!(entry.status, Status::Running | Status::Reconnecting(_)))
            .map(str::to_string),
        pid: None,
        restart_count: entry.restart_attempts,
        started_unix: entry
            .is_running()
            .then(|| {
                entry
                    .started_at
                    .map(|t| now_unix().saturating_sub(now.saturating_duration_since(t).as_secs()))
            })
            .flatten(),
    }
}

/// Everything the snapshots are built from, hashed without allocating —
/// this runs on every event, and the rebuild only when it moves.
pub fn fingerprint<'a>(entries: impl Iterator<Item = (&'a ProcessEntry, Option<&'a str>)>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (entry, url) in entries {
        entry.config.name.hash(&mut h);
        entry.config.command.hash(&mut h);
        entry.config.working_dir.hash(&mut h);
        entry.status.hash(&mut h);
        entry.restart_attempts.hash(&mut h);
        entry.run_id.hash(&mut h);
        url.hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuxflow_core::config::schema::{ProcessCategory, ProcessConfig};

    fn entry(name: &str, status: Status) -> ProcessEntry {
        let mut e = ProcessEntry::new(ProcessConfig {
            name: name.into(),
            command: format!("run {name}"),
            working_dir: None,
            start_with_project: false,
            auto_restart: false,
            open_in_browser: false,
            restart_when_changed: Vec::new(),
            env: Default::default(),
            category: ProcessCategory::Command,
            auto_named: false,
            display_name: None,
        });
        e.status = status;
        e
    }

    #[test]
    fn snapshot_reports_url_and_defaults_working_dir_to_the_project() {
        let mut e = entry("dev", Status::Running);
        e.started_at = Some(Instant::now() - std::time::Duration::from_secs(90));
        let s = snapshot(
            &e,
            Some("http://localhost:5173"),
            "/srv/app",
            Instant::now(),
        );
        assert_eq!(s.status, "Running");
        assert_eq!(s.url.as_deref(), Some("http://localhost:5173"));
        assert_eq!(s.working_dir.as_deref(), Some("/srv/app"));
        let up = s.uptime_secs().expect("running → uptime");
        assert!((89..=91).contains(&up), "uptime {up}");
    }

    #[test]
    fn stopped_process_has_no_uptime_and_no_url_even_with_stale_ones() {
        let mut e = entry("dev", Status::Stopped);
        e.started_at = Some(Instant::now());
        let s = snapshot(
            &e,
            Some("http://localhost:5173"),
            "/srv/app",
            Instant::now(),
        );
        assert_eq!(s.started_unix, None);
        assert_eq!(s.uptime_secs(), None);
        assert_eq!(s.url, None, "a stopped process serves nothing");
        let mut r = entry("dev", Status::Reconnecting(1));
        r.started_at = Some(Instant::now());
        let s = snapshot(
            &r,
            Some("http://localhost:5173"),
            "/srv/app",
            Instant::now(),
        );
        assert!(s.url.is_some(), "still alive on the host");
    }

    #[test]
    fn fingerprint_moves_with_status_and_url_only() {
        let a = entry("dev", Status::Running);
        let base = fingerprint([(&a, None)].into_iter());
        assert_eq!(base, fingerprint([(&a, None)].into_iter()), "stable");
        assert_ne!(
            base,
            fingerprint([(&a, Some("http://localhost:3000"))].into_iter()),
            "a detected URL is a change the agent must see"
        );
        let b = entry("dev", Status::Restarting(2));
        assert_ne!(base, fingerprint([(&b, None)].into_iter()));
        // A later run of the same process is a new snapshot (its start time
        // moved) even when nothing else did.
        let mut c = entry("dev", Status::Running);
        c.run_id = a.run_id + 1;
        assert_ne!(base, fingerprint([(&c, None)].into_iter()));
    }
}
