use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gtk4::glib;
use gtk4::prelude::*;
use vte4::prelude::*;

use crate::config::schema::{ProcessCategory, ProcessConfig};
use crate::config::settings::AppSettings;
use crate::remote::ProjectLocation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    Stopped,
    Running,
    Crashed,
    Restarting,
}

pub struct ManagedProcess {
    pub id: String,
    pub config: ProcessConfig,
    pub terminal: Option<vte4::Terminal>,
    pub status: ProcessStatus,
    pub pid: Option<i32>,
    pub pid_cell: Option<Rc<RefCell<Option<i32>>>>,
    pub restart_count: u32,
    pub started_at: Option<Instant>,
    /// Called once when the terminal is first created (lazy materialization).
    /// The callback receives the new terminal and should connect signals,
    /// swap the placeholder in the GTK Stack, etc.
    pub on_materialized: Option<Box<dyn Fn(&vte4::Terminal)>>,
    /// Shared name cell used by the auto-restart handler so it tracks renames.
    pub name_cell: Option<crate::process::auto_restart::ProcessNameCell>,
    /// Shared qualified name cell (project::process) so on_materialized and
    /// signal handlers track renames.
    pub qname_cell: Option<Rc<RefCell<String>>>,
    /// Last VTE `contents-changed` timestamp. Populated for Agent-category
    /// processes only; used by the idle-silence fallback ticker.
    pub last_activity: Option<Rc<Cell<Instant>>>,
    /// `contents-changed` events since the ticker last looked. A working
    /// agent repaints continuously (dozens per tick); trailing repaints
    /// after it finishes are single events — the rate gates the working
    /// indicator so it doesn't flap on stragglers.
    pub activity_burst: Option<Rc<Cell<u32>>>,
    /// Edge-trigger guard for the idle-silence ticker: `true` once a silence
    /// notification has fired for the current quiet period, reset on next
    /// `contents-changed`.
    pub is_idle: Option<Rc<Cell<bool>>>,
    /// Remote projects only: pidfile on the ssh host holding this spawn's
    /// remote login-session PID, used for an explicit remote kill on Stop
    /// (covers hosts without tmux).
    pub remote_pidfile: Option<String>,
    /// Remote projects only: tmux session name on the ssh host. Kept across
    /// respawns so a reconnect reattaches to the still-running session;
    /// cleared by kill(), which also tears the session down.
    pub remote_session: Option<String>,
    /// Set by kill() for remote processes: the next spawn must kill any
    /// surviving tmux session before creating one, because the explicit
    /// remote kill is fire-and-forget and a quick restart could race it.
    pub remote_fresh_next: bool,
    /// One-shot: open the detected URL in the browser when it appears.
    /// Armed by user-initiated spawns of processes with
    /// `config.open_in_browser`; never by auto-restarts or reattaches.
    /// Consumed by the URL-detection handler in window.rs.
    pub auto_open_armed: bool,
    /// When the first URL was detected while `auto_open_armed` — starts the
    /// grace window in which a provisional URL may still be upgraded (e.g.
    /// `shopify app dev` prints tunnel URLs before its "Preview URL:").
    pub auto_open_first_url: Option<Instant>,
}

impl ManagedProcess {
    fn new(config: ProcessConfig) -> Self {
        let id = config.name.clone();
        Self {
            id,
            config,
            terminal: None,
            status: ProcessStatus::Stopped,
            pid: None,
            pid_cell: None,
            restart_count: 0,
            started_at: None,
            on_materialized: None,
            name_cell: None,
            qname_cell: None,
            last_activity: None,
            activity_burst: None,
            is_idle: None,
            remote_pidfile: None,
            remote_session: None,
            remote_fresh_next: false,
            auto_open_armed: false,
            auto_open_first_url: None,
        }
    }

    /// Lazily create the VTE terminal if it doesn't exist yet.
    /// Fires the `on_materialized` callback on first creation.
    pub fn ensure_terminal(&mut self, settings: &AppSettings) -> &vte4::Terminal {
        if self.terminal.is_none() {
            let terminal = Self::create_terminal(&self.config, settings);
            if let Some(cb) = self.on_materialized.take() {
                cb(&terminal);
            }
            self.terminal = Some(terminal);
        }
        self.terminal.as_ref().unwrap()
    }

    fn create_terminal(config: &ProcessConfig, settings: &AppSettings) -> vte4::Terminal {
        let terminal = vte4::Terminal::new();
        terminal.set_scroll_on_output(false);
        terminal.set_scroll_on_keystroke(true);
        terminal.set_clear_background(true);
        terminal.set_vexpand(true);
        terminal.set_hexpand(true);

        Self::apply_settings_to_terminal(&terminal, settings);

        crate::ui::terminal_theme::apply(&terminal, &settings.appearance.terminal_theme);

        // Register URL matching for clickable links
        Self::setup_url_matching(&terminal);

        // Auto-copy selection to clipboard
        terminal.connect_selection_changed(|term| {
            if term.has_selection() {
                term.copy_clipboard_format(vte4::Format::Text);
            }
        });

        // Handle Ctrl+click to open URLs (deny the gesture otherwise so VTE
        // keeps its native text-selection behaviour).
        let gesture = gtk4::GestureClick::new();
        gesture.set_button(1);
        let term_ref = terminal.clone();
        gesture.connect_pressed(move |gesture, _, x, y| {
            if let Some(event) = gesture.current_event()
                && event
                    .modifier_state()
                    .contains(gtk4::gdk::ModifierType::CONTROL_MASK)
            {
                let (url_opt, _tag) = term_ref.check_match_at(x, y);
                if let Some(url) = url_opt {
                    let _ = std::process::Command::new("xdg-open")
                        .arg(url.as_str())
                        .spawn();
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    return;
                }
            }
            gesture.set_state(gtk4::EventSequenceState::Denied);
        });
        terminal.add_controller(gesture);

        terminal
    }

    pub fn apply_settings_to_terminal(terminal: &vte4::Terminal, settings: &AppSettings) {
        use gtk4::pango;
        let font_str = format!(
            "{} {}",
            settings.appearance.font_family, settings.appearance.font_size
        );
        let mut font_desc = pango::FontDescription::from_string(&font_str);
        font_desc.set_weight(pango::Weight::__Unknown(
            settings.appearance.font_weight as i32,
        ));
        terminal.set_font(Some(&font_desc));
        terminal.set_scrollback_lines(settings.appearance.scrollback_lines as i64);
        // NOTE: set_cell_height_scale causes rendering artifacts in VTE —
        // ghost fragments of erased text remain in the inter-line gap pixels.
        // Disabled until VTE fixes this upstream. Line height setting is kept
        // in the UI but has no effect.
        // if (settings.appearance.line_height - 1.0).abs() > f64::EPSILON {
        //     terminal.set_cell_height_scale(settings.appearance.line_height);
        // }
        if settings.appearance.letter_spacing.abs() > f64::EPSILON {
            terminal.set_cell_width_scale(1.0 + settings.appearance.letter_spacing / 10.0);
        }
        // Bold weight is applied through the terminal theme's bold attribute;
        // VTE uses the font description for normal weight and derives bold from it.
        // We set bold_is_bright based on whether bold weight differs from normal.
        terminal.set_bold_is_bright(
            settings.appearance.bold_font_weight != settings.appearance.font_weight,
        );
    }

    fn setup_url_matching(terminal: &vte4::Terminal) {
        // PCRE2_MULTILINE is required by VTE for match_add_regex
        const PCRE2_MULTILINE: u32 = 0x00000400;

        // Match HTTP/HTTPS URLs
        let url_pattern = "https?://[^\\s<>'\"]+";
        if let Ok(regex) = vte4::Regex::for_match(url_pattern, PCRE2_MULTILINE) {
            terminal.match_add_regex(&regex, 0);
        }
        // Match localhost:port
        let localhost_pattern = "localhost:\\d+";
        if let Ok(regex) = vte4::Regex::for_match(localhost_pattern, PCRE2_MULTILINE) {
            terminal.match_add_regex(&regex, 0);
        }
    }
}

pub type ProcessManagerRef = Rc<RefCell<ProcessManager>>;

pub struct ProcessManager {
    processes: HashMap<String, ManagedProcess>,
    order: Vec<String>,
    on_status_change: Option<Box<dyn Fn(&str, ProcessStatus)>>,
    on_pid_change: Option<Rc<dyn Fn(i32, bool)>>,
    on_file_watch_restart: Option<Rc<dyn Fn(&str)>>,
    settings: AppSettings,
    /// Where this project's files live; remote managers wrap commands in ssh.
    location: ProjectLocation,
    /// Per-project wiring for one process (auto-restart, terminal signal
    /// hookup, port detection, clipboard bridge). Built once in
    /// `wire_project`; call for every process added AFTER project load so
    /// dynamically added ones behave identically to load-time ones.
    /// Invoke via a clone with no manager borrow held — it re-borrows.
    wiring_factory: Option<Rc<dyn Fn(&str)>>,
}

impl ProcessManager {
    pub fn new(location: ProjectLocation) -> ProcessManagerRef {
        Rc::new(RefCell::new(Self {
            processes: HashMap::new(),
            order: Vec::new(),
            on_status_change: None,
            on_pid_change: None,
            on_file_watch_restart: None,
            settings: AppSettings::load(),
            location,
            wiring_factory: None,
        }))
    }

    pub fn set_wiring_factory(&mut self, f: Rc<dyn Fn(&str)>) {
        self.wiring_factory = Some(f);
    }

    pub fn wiring_factory(&self) -> Option<Rc<dyn Fn(&str)>> {
        self.wiring_factory.clone()
    }

    pub fn location(&self) -> &ProjectLocation {
        &self.location
    }

    pub fn settings(&self) -> &AppSettings {
        &self.settings
    }

    pub fn set_on_status_change(&mut self, cb: impl Fn(&str, ProcessStatus) + 'static) {
        self.on_status_change = Some(Box::new(cb));
    }

    /// Callback fired when a file-watch pattern triggers a restart. Receives the
    /// bare process name. UI layer is responsible for the notification gating.
    pub fn set_on_file_watch_restart(&mut self, cb: impl Fn(&str) + 'static) {
        self.on_file_watch_restart = Some(Rc::new(cb));
    }

    pub fn file_watch_restart_callback(&self) -> Option<Rc<dyn Fn(&str)>> {
        self.on_file_watch_restart.clone()
    }

    /// Callback fires with (pid, acquired). `acquired=true` on spawn, `false` on kill.
    pub fn set_on_pid_change(&mut self, cb: impl Fn(i32, bool) + 'static) {
        self.on_pid_change = Some(Rc::new(cb));
    }

    pub fn add_process(&mut self, config: ProcessConfig) {
        let name = config.name.clone();
        let proc = ManagedProcess::new(config);
        if !self.processes.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.processes.insert(name, proc);
    }

    /// Eagerly create the terminal for a process (used for dynamically added processes
    /// that need to be visible immediately).
    pub fn materialize_process(&mut self, name: &str) {
        if let Some(proc) = self.processes.get_mut(name) {
            proc.ensure_terminal(&self.settings);
        }
    }

    pub fn spawn(&mut self, name: &str) {
        self.spawn_inner(name, None, true);
    }

    /// Spawn without arming the open-in-browser one-shot. For spawns the
    /// user didn't ask for right now: reattaching to live remote sessions
    /// at project load.
    pub fn spawn_quiet(&mut self, name: &str) {
        self.spawn_inner(name, None, false);
    }

    /// Like `spawn`, but uses `command_override` as the shell command instead
    /// of the persisted `proc.config.command`. The saved config is not
    /// modified — a subsequent plain `spawn` reverts to the original command.
    pub fn spawn_with_command_override(&mut self, name: &str, command_override: &str) {
        self.spawn_inner(name, Some(command_override), true);
    }

    fn spawn_inner(&mut self, name: &str, command_override: Option<&str>, user_initiated: bool) {
        // Ensure terminal exists before spawning
        {
            let settings = self.settings.clone();
            if let Some(proc) = self.processes.get_mut(name) {
                proc.ensure_terminal(&settings);
            }
        }

        let Some(proc) = self.processes.get_mut(name) else {
            log::warn!("Process not found: {name}");
            return;
        };

        if proc.status == ProcessStatus::Running {
            log::info!("Process {name} already running");
            return;
        }
        let was_restarting = proc.status == ProcessStatus::Restarting;

        let terminal = proc.terminal.as_ref().unwrap();

        // Reset terminal to ensure a clean PTY (needed after crash/exit)
        terminal.reset(true, true);

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let command: &str = command_override.unwrap_or(&proc.config.command);

        // Remote projects: run the command on the ssh host, in its remote
        // working_dir, via `exec ssh -t` — the VTE child *is* ssh, so PID
        // capture, exit codes, and auto-restart keep working unchanged. The
        // command itself lives in a tmux session on the host, so connection
        // loss and app quit merely detach it; respawning reattaches.
        // SSH-category processes are already ssh invocations; leave them be.
        // The remote path also can't be VTE's cwd, so it goes into the wrap.
        let remote_command: String;
        let (command, working_dir): (&str, Option<String>) = match &self.location {
            ProjectLocation::Ssh { host, .. } if proc.config.category != ProcessCategory::SSH => {
                let remote_dir = proc
                    .config
                    .working_dir
                    .clone()
                    .unwrap_or_else(|| self.location.dir_str());
                // Fresh pidfile per spawn so kill() can end the remote session
                // explicitly (SIGHUP-trapping processes survive PTY teardown).
                let pidfile = crate::remote::new_remote_pidfile();
                // Reuse the previous spawn's tmux session so reconnects
                // reattach; kill() clears it, and the deterministic name
                // also lets a fresh app launch adopt sessions still running
                // from the previous one.
                let fresh = std::mem::take(&mut proc.remote_fresh_next);
                let session = proc.remote_session.clone().unwrap_or_else(|| {
                    crate::remote::remote_session_name(&self.location.key(), name)
                });
                remote_command = crate::remote::wrap_remote_command(
                    host,
                    &remote_dir,
                    &proc.config.env,
                    command,
                    Some(&pidfile),
                    &session,
                    fresh,
                );
                proc.remote_pidfile = Some(pidfile);
                proc.remote_session = Some(session);
                (remote_command.as_str(), None)
            }
            _ => (command, proc.config.working_dir.clone()),
        };

        // Build argv: shell -li -c "command"
        // Use login (-l) + interactive (-i) shell so all profile scripts are
        // sourced. zsh only reads ~/.zshrc for interactive shells, and many
        // users set PATH there (nvm, go, cargo, etc.), so -i is required.
        // The VTE PTY already provides a terminal, so -i is safe here.
        let argv = [shell.as_str(), "-li", "-c", command];

        // Build envv: merge parent environment with config overrides.
        // VTE treats a non-empty envv as the *complete* environment, so we must
        // always include the full parent env. When launched from a .desktop file
        // the parent env is minimal, but -l above ensures the shell sources its
        // login profile (PATH, nvm, etc.).
        let config_env: std::collections::HashMap<&str, &str> = proc
            .config
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let env_strings: Vec<String> = if config_env.is_empty() {
            // No overrides — pass empty envv so VTE inherits parent env as-is
            Vec::new()
        } else {
            // Start with parent env, then overlay config values
            let mut merged: std::collections::HashMap<String, String> = std::env::vars().collect();
            for (k, v) in &config_env {
                merged.insert(k.to_string(), v.to_string());
            }
            merged.iter().map(|(k, v)| format!("{k}={v}")).collect()
        };
        let env_refs: Vec<&str> = env_strings.iter().map(|s| s.as_str()).collect();

        let name_clone = name.to_string();

        // Use a shared cell to capture the PID from the async callback
        let pid_cell: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));
        let pid_cell_ref = pid_cell.clone();
        let pid_cb = self.on_pid_change.clone();

        terminal.spawn_async(
            vte4::PtyFlags::DEFAULT,
            working_dir.as_deref(),
            &argv,
            &env_refs,
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            gtk4::gio::Cancellable::NONE,
            move |result| match result {
                Ok(pid) => {
                    log::info!("Spawned process {name_clone} with PID {pid:?}");
                    *pid_cell_ref.borrow_mut() = Some(pid.0);
                    if let Some(ref cb) = pid_cb {
                        cb(pid.0, true);
                    }
                }
                Err(e) => {
                    log::error!("Failed to spawn process {name_clone}: {e}");
                }
            },
        );

        // Store pid_cell for later retrieval
        proc.pid_cell = Some(pid_cell);
        proc.status = ProcessStatus::Running;
        proc.started_at = Some(Instant::now());
        // Manual starts begin a fresh incident; auto-restart respawns keep
        // the attempt counter so crash loops can give up and backoff grows.
        if !was_restarting {
            proc.restart_count = 0;
        }
        // Arm the browser one-shot only for starts the user asked for —
        // an auto-restart or reconnect popping a tab would be noise.
        proc.auto_open_armed = user_initiated && !was_restarting && proc.config.open_in_browser;
        proc.auto_open_first_url = None;

        let name_owned = name.to_string();
        if let Some(ref cb) = self.on_status_change {
            cb(&name_owned, ProcessStatus::Running);
        }
    }

    pub fn kill(&mut self, name: &str) {
        self.kill_inner(name, true);
    }

    /// `kill_remote`: whether to tear down the remote tmux session / login
    /// session too. `false` kills only the local side (the ssh client),
    /// which merely detaches the remote session — it keeps running on the
    /// host and the next spawn reattaches.
    fn kill_inner(&mut self, name: &str, kill_remote: bool) {
        let remote_host = self.location.host().map(str::to_string);
        let Some(proc) = self.processes.get_mut(name) else {
            return;
        };

        if proc.status != ProcessStatus::Running {
            return;
        }

        // Remote processes: also kill the remote session explicitly. The PTY
        // teardown below only detaches the tmux session (and on tmux-less
        // hosts, SIGHUP-trapping processes would survive it).
        if kill_remote && let Some(host) = &remote_host {
            let session = proc.remote_session.take();
            if let Some(pidfile) = proc.remote_pidfile.take() {
                crate::remote::remote_kill(host, &pidfile, session.as_deref());
            }
            // The remote kill above is fire-and-forget — make the next spawn
            // clear any surviving session inline instead of reattaching to it.
            proc.remote_fresh_next = true;
        }

        // Kill the entire process tree rooted at the spawned PID.
        // VTE spawns `shell -li -c "command"`, and the child command may fork
        // further processes (e.g., npm → node). Sending SIGTERM to just the
        // process group (-pid) may miss child processes that created their own
        // process groups. We collect all descendant PIDs via /proc and signal
        // each one individually.
        if let Some(ref pid_cell) = proc.pid_cell
            && let Some(pid) = *pid_cell.borrow()
        {
            if let Some(ref cb) = self.on_pid_change {
                cb(pid, false);
            }
            let all_pids = collect_process_tree(pid);
            // SIGTERM all processes in the tree (children first, then root)
            for &p in all_pids.iter().rev() {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(p),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }
            // Also signal the process group in case some children share it
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-pid),
                nix::sys::signal::Signal::SIGTERM,
            );
            // Force kill after a delay in case SIGTERM is ignored
            glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                for &p in all_pids.iter().rev() {
                    let _ = nix::sys::signal::kill(
                        nix::unistd::Pid::from_raw(p),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                }
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(-pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            });
        }

        if let Some(ref terminal) = proc.terminal {
            terminal.reset(true, true);
        }
        proc.status = ProcessStatus::Stopped;
        proc.pid = None;
        proc.pid_cell = None;
        proc.started_at = None;

        if let Some(ref cb) = self.on_status_change {
            cb(name, ProcessStatus::Stopped);
        }

        log::info!("Killed process {name}");
    }

    pub fn restart(&mut self, name: &str) {
        self.kill(name);
        self.spawn(name);
    }

    pub fn spawn_project_group(&mut self) {
        let names: Vec<String> = self
            .processes
            .values()
            .filter(|p| p.config.start_with_project)
            .map(|p| p.config.name.clone())
            .collect();

        for name in names {
            self.spawn(&name);
        }
    }

    pub fn apply_terminal_theme(&mut self, theme_name: &str) {
        self.settings.appearance.terminal_theme = theme_name.to_string();
        for proc in self.processes.values() {
            if let Some(ref terminal) = proc.terminal {
                crate::ui::terminal_theme::apply(terminal, theme_name);
            }
        }
    }

    pub fn apply_font_settings(&mut self, settings: &AppSettings) {
        self.settings = settings.clone();
        for proc in self.processes.values() {
            if let Some(ref terminal) = proc.terminal {
                ManagedProcess::apply_settings_to_terminal(terminal, settings);
            }
        }
    }

    pub fn get_process(&self, name: &str) -> Option<&ManagedProcess> {
        self.processes.get(name)
    }

    pub fn get_process_mut(&mut self, name: &str) -> Option<&mut ManagedProcess> {
        self.processes.get_mut(name)
    }

    pub fn process_names(&self) -> &[String] {
        &self.order
    }

    pub fn processes_by_category(&self, category: ProcessCategory) -> Vec<&ManagedProcess> {
        self.order
            .iter()
            .filter_map(|name| self.processes.get(name))
            .filter(|p| p.config.category == category)
            .collect()
    }

    pub fn running_count(&self) -> usize {
        self.processes
            .values()
            .filter(|p| p.status == ProcessStatus::Running)
            .count()
    }

    pub fn running_names(&self) -> Vec<&str> {
        self.order
            .iter()
            .filter_map(|name| {
                self.processes
                    .get(name)
                    .filter(|p| p.status == ProcessStatus::Running)
                    .map(|_| name.as_str())
            })
            .collect()
    }

    /// Running process names in sidebar visual order: grouped by category
    /// (Agent, Command, Terminal, SSH), preserving each category's saved order.
    /// The category order MUST stay in sync with the `categories` array in
    /// `ui/sidebar/project_list.rs` so Ctrl+1..9 matches what the sidebar shows.
    pub fn running_names_in_sidebar_order(&self) -> Vec<&str> {
        use crate::config::schema::ProcessCategory::*;
        [Agent, Command, Terminal, SSH]
            .into_iter()
            .flat_map(|cat| {
                self.processes_by_category(cat)
                    .into_iter()
                    .filter(|p| p.status == ProcessStatus::Running)
                    .map(|p| p.config.name.as_str())
            })
            .collect()
    }

    pub fn total_count(&self) -> usize {
        self.processes.len()
    }

    pub fn stop_all(&mut self) {
        let names: Vec<String> = self
            .processes
            .values()
            .filter(|p| p.status == ProcessStatus::Running)
            .map(|p| p.config.name.clone())
            .collect();
        for name in names {
            self.kill(&name);
        }
    }

    /// Stop only the local side of every running process. For remote
    /// projects that kills the ssh clients, *detaching* the tmux sessions —
    /// the processes keep running on the host and the next launch
    /// reattaches. Used at app shutdown.
    pub fn detach_all(&mut self) {
        let names: Vec<String> = self
            .processes
            .values()
            .filter(|p| p.status == ProcessStatus::Running)
            .map(|p| p.config.name.clone())
            .collect();
        for name in names {
            self.kill_inner(&name, false);
        }
    }

    pub fn running_pids(&self) -> Vec<(String, i32)> {
        self.processes
            .values()
            .filter(|p| p.status == ProcessStatus::Running)
            .filter_map(|p| {
                p.pid_cell
                    .as_ref()
                    .and_then(|cell| *cell.borrow())
                    .map(|pid| (p.config.name.clone(), pid))
            })
            .collect()
    }

    pub fn remove_process(&mut self, name: &str) {
        // Kill first if running
        self.kill(name);
        self.processes.remove(name);
        self.order.retain(|n| n != name);
        log::info!("Removed process {name}");
    }

    pub fn notify_status_change(&self, name: &str, status: ProcessStatus) {
        if let Some(ref cb) = self.on_status_change {
            cb(name, status);
        }
    }

    pub fn restart_all(&mut self) {
        let names: Vec<String> = self
            .processes
            .values()
            .filter(|p| p.status == ProcessStatus::Running)
            .map(|p| p.config.name.clone())
            .collect();
        for name in names {
            self.restart(&name);
        }
    }

    pub fn update_process_config(&mut self, old_name: &str, new_config: ProcessConfig) -> bool {
        let name_changed = old_name != new_config.name;
        let new_name = new_config.name.clone();

        if let Some(mut proc) = self.processes.remove(old_name) {
            proc.config = new_config;
            proc.id = new_name.clone();

            // Update the shared name cell so auto-restart handlers track the rename
            if name_changed {
                if let Some(ref cell) = proc.name_cell {
                    *cell.borrow_mut() = new_name.clone();
                }
            }

            self.processes.insert(new_name.clone(), proc);

            if name_changed && let Some(entry) = self.order.iter_mut().find(|n| *n == old_name) {
                *entry = new_name;
            }
        }

        name_changed
    }

    pub fn reorder_process(&mut self, process_name: &str, target_name: &str, before: bool) {
        let Some(src_idx) = self.order.iter().position(|n| n == process_name) else {
            return;
        };
        let name = self.order.remove(src_idx);
        let target_idx = self
            .order
            .iter()
            .position(|n| n == target_name)
            .unwrap_or(0);
        let insert_idx = if before { target_idx } else { target_idx + 1 };
        self.order.insert(insert_idx, name);
    }

    /// Reorder processes to match a saved order. Names not in `saved_order`
    /// keep their relative position and are appended at the end.
    pub fn apply_saved_order(&mut self, saved_order: &[String]) {
        self.order.sort_by_key(|name| {
            saved_order
                .iter()
                .position(|s| s == name)
                .unwrap_or(usize::MAX)
        });
    }
}

/// Walk /proc to collect all PIDs in the process tree rooted at `root_pid`.
/// Used by both `ProcessManager::kill` and `PidFile::kill_orphans`.
/// Returns the root followed by all descendants (breadth-first).
pub fn collect_process_tree(root_pid: i32) -> Vec<i32> {
    use std::collections::VecDeque;
    use std::fs;

    // Build a map of parent → children by scanning /proc
    let mut children_map: HashMap<i32, Vec<i32>> = HashMap::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
                continue;
            };
            let stat_path = entry.path().join("stat");
            if let Ok(stat) = fs::read_to_string(&stat_path) {
                // Format: pid (comm) state ppid ...
                // comm can contain spaces/parens, so find the last ')' first
                if let Some(after_comm) = stat.rfind(')') {
                    let fields: Vec<&str> = stat[after_comm + 2..].split_whitespace().collect();
                    // fields[0] = state, fields[1] = ppid
                    if let Some(ppid) = fields.get(1).and_then(|s| s.parse::<i32>().ok()) {
                        children_map.entry(ppid).or_default().push(pid);
                    }
                }
            }
        }
    }

    // BFS from root_pid
    let mut result = vec![root_pid];
    let mut queue = VecDeque::new();
    queue.push_back(root_pid);
    while let Some(pid) = queue.pop_front() {
        if let Some(kids) = children_map.get(&pid) {
            for &kid in kids {
                result.push(kid);
                queue.push_back(kid);
            }
        }
    }
    result
}
