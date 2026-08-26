//! Process model for the iced shell: loading (tuxflow.toml or stack
//! detection), spawn settings, and the auto-restart policy — mirrored from
//! the GTK app's process/auto_restart.rs so both shells behave identically.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tuxflow_core::config::loader;
use tuxflow_core::config::schema::{ProcessCategory, ProcessConfig};
use tuxflow_core::detect::detector;

// The GTK app's policy constants, verbatim (process/auto_restart.rs).
pub const MAX_RESTART_ATTEMPTS: u32 = 5;
const BASE_DELAY_MS: u64 = 1000;
const MAX_BACKOFF_EXPONENT: u32 = 5;
/// A run this long resets the attempt counter — failures hours apart must
/// not accumulate into a give-up.
const STABLE_RUN_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Stopped,
    Running,
    /// Crashed and not coming back (auto-restart off, or gave up).
    Crashed(Option<i32>),
    /// Crashed; restart scheduled (attempt number shown in the sidebar).
    Restarting(u32),
}

pub struct ProcessEntry {
    pub config: ProcessConfig,
    pub status: Status,
    pub terminal: Option<iced_term::Terminal>,
    /// Fresh per spawn — subscription identity must change across restarts
    /// or iced would not start a stream for the new terminal.
    pub term_id: Option<u64>,
    /// ChildExit code observed before the Exit event finalizes the run.
    pub last_exit: Option<i32>,
    /// User pressed stop — the coming exit is expected, not a crash.
    pub stopping: bool,
    pub restart_attempts: u32,
    /// Bumped on every manual action; stale RestartDue timers carry the old
    /// value and are ignored.
    pub restart_generation: u64,
    pub started_at: Option<Instant>,
}

impl ProcessEntry {
    pub fn new(config: ProcessConfig) -> Self {
        Self {
            config,
            status: Status::Stopped,
            terminal: None,
            term_id: None,
            last_exit: None,
            stopping: false,
            restart_attempts: 0,
            restart_generation: 0,
            started_at: None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, Status::Running)
    }
}

/// Project name + process list for a directory: tuxflow.toml when present,
/// stack detection otherwise (conservative variant — same as the GTK app's
/// startup path). SSH-category entries are held back until M2 (remote).
pub fn load_project(dir: &Path) -> (String, Vec<ProcessEntry>) {
    let (name, configs) = match loader::find_config(dir).and_then(|p| loader::load_config(&p).ok())
    {
        Some(config) => (config.project.name, config.process),
        None => {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| String::from("project"));
            let processes = detector::detect_stacks_conservative(dir)
                .into_iter()
                .flat_map(|stack| stack.suggested_processes)
                .collect();
            (name, processes)
        }
    };

    let entries = configs
        .into_iter()
        .filter(|c| c.category != ProcessCategory::SSH)
        .map(ProcessEntry::new)
        .collect();
    (name, entries)
}

/// Spawn settings for a process — the GTK recipe: `$SHELL -li -c <command>`
/// (login + interactive so nvm/cargo PATHs from rc files exist), config env
/// overlaid on the parent's, cwd from the config or the project dir. A
/// plain terminal (empty command) gets an interactive login shell instead.
pub fn backend_settings(
    config: &ProcessConfig,
    project_dir: &Path,
) -> iced_term::settings::Settings {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    let args = if config.command.is_empty() {
        vec!["-l".into()]
    } else {
        vec!["-li".into(), "-c".into(), config.command.clone()]
    };
    let working_directory = Some(
        config
            .working_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| project_dir.to_path_buf()),
    );

    iced_term::settings::Settings {
        backend: iced_term::settings::BackendSettings {
            program: shell,
            args,
            working_directory,
            env: config
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// What a finished run means, and whether a restart gets scheduled.
/// Pure so the whole policy is unit-testable.
///
/// Returns the new status, the new attempt counter, and the backoff delay
/// when a restart should be scheduled.
pub fn plan_after_exit(
    auto_restart: bool,
    stopping: bool,
    exit_code: Option<i32>,
    run_duration: Option<Duration>,
    attempts: u32,
) -> (Status, u32, Option<Duration>) {
    // User-initiated stops and clean exits end the story.
    if stopping || exit_code == Some(0) {
        return (Status::Stopped, 0, None);
    }

    if !auto_restart {
        return (Status::Crashed(exit_code), 0, None);
    }

    // A stable run forgives past failures.
    let attempts = match run_duration {
        Some(run) if run.as_secs() >= STABLE_RUN_SECS => 0,
        _ => attempts,
    };

    let attempt = attempts + 1;
    if attempt > MAX_RESTART_ATTEMPTS {
        return (Status::Crashed(exit_code), attempts, None);
    }

    (
        Status::Restarting(attempt),
        attempt,
        Some(backoff_delay(attempt)),
    )
}

/// 1 s · 2^(attempt−1), capped at 32 s — the GTK app's curve.
pub fn backoff_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(MAX_BACKOFF_EXPONENT);
    Duration::from_millis(BASE_DELAY_MS * 2u64.pow(exponent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_curve_matches_gtk_app() {
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(2), Duration::from_secs(2));
        assert_eq!(backoff_delay(3), Duration::from_secs(4));
        assert_eq!(backoff_delay(6), Duration::from_secs(32));
        assert_eq!(backoff_delay(60), Duration::from_secs(32), "capped");
    }

    #[test]
    fn user_stop_is_never_a_crash() {
        let (status, attempts, delay) =
            plan_after_exit(true, true, Some(137), Some(Duration::from_secs(1)), 3);
        assert_eq!(status, Status::Stopped);
        assert_eq!(attempts, 0);
        assert!(delay.is_none());
    }

    #[test]
    fn clean_exit_stops() {
        let (status, _, delay) = plan_after_exit(true, false, Some(0), None, 0);
        assert_eq!(status, Status::Stopped);
        assert!(delay.is_none());
    }

    #[test]
    fn crash_without_auto_restart_stays_down() {
        let (status, _, delay) = plan_after_exit(false, false, Some(1), None, 0);
        assert_eq!(status, Status::Crashed(Some(1)));
        assert!(delay.is_none());
    }

    #[test]
    fn crash_schedules_restart_with_backoff() {
        let (status, attempts, delay) =
            plan_after_exit(true, false, Some(1), Some(Duration::from_secs(2)), 0);
        assert_eq!(status, Status::Restarting(1));
        assert_eq!(attempts, 1);
        assert_eq!(delay, Some(Duration::from_secs(1)));

        let (status, attempts, delay) =
            plan_after_exit(true, false, Some(1), Some(Duration::from_secs(2)), attempts);
        assert_eq!(status, Status::Restarting(2));
        assert_eq!(attempts, 2);
        assert_eq!(delay, Some(Duration::from_secs(2)));
    }

    #[test]
    fn stable_run_resets_the_counter() {
        let (status, attempts, delay) = plan_after_exit(
            true,
            false,
            Some(1),
            Some(Duration::from_secs(STABLE_RUN_SECS)),
            MAX_RESTART_ATTEMPTS,
        );
        assert_eq!(status, Status::Restarting(1));
        assert_eq!(attempts, 1);
        assert_eq!(delay, Some(Duration::from_secs(1)));
    }

    #[test]
    fn gives_up_after_max_attempts() {
        let (status, _, delay) = plan_after_exit(
            true,
            false,
            Some(1),
            Some(Duration::from_secs(1)),
            MAX_RESTART_ATTEMPTS,
        );
        assert_eq!(status, Status::Crashed(Some(1)));
        assert!(delay.is_none());
    }

    /// A signal-killed child yields Exit without a ChildExit code (spike
    /// finding) — treated as a crash, not a clean stop.
    #[test]
    fn signal_kill_without_code_is_a_crash() {
        let (status, ..) = plan_after_exit(false, false, None, None, 0);
        assert_eq!(status, Status::Crashed(None));
    }
}
