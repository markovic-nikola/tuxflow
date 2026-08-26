//! TuxFlow's iced shell — migration M4: the multi-project workspace.
//!
//! `tuxflow-iced [path | ssh://host/dir]…`. Projects come from
//! `~/.config/tuxflow/projects.toml` (plus any CLI args, which persist),
//! each with its own process list (config or detection, overlaid with the
//! user's custom commands/deletions/order — same policy as the GTK app),
//! ports, tunnels and poll cadence. Add project / add command / add agent
//! run as inline forms; closing a project detaches its remote sessions.

mod keys;
mod notify;
mod processes;
mod theme;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event as AEvent;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::ClipboardType;
use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Color, Element, Length, Size, Subscription, Task};
use iced_term::{BackendCommand, SearchDirection, TerminalView};
use tuxflow_core::config::projects::SavedProjects;
use tuxflow_core::config::schema::{ProcessCategory, ProcessConfig};
use tuxflow_core::remote::probe::ProbeError;
use tuxflow_core::remote::tunnel::TunnelManager;
use tuxflow_core::remote::{self, ProjectLocation};
use tuxflow_core::util::port_detector::{PortDetector, remap_url_port, rewrite_clicked_url};

use keys::{AppAction, AppKeys};
use processes::{ProcessEntry, Status, plan_after_exit};
use theme::{CRASHED, DIM, LOCAL_ACCENT, RESTARTING, STOPPED, TEXT, TEXT_SECONDARY, accent_for};

/// Ports-poll cadence: fast while a run is settling (a new forward just
/// opened), backed off once nothing new appears (GTK behavior).
const POLL_FAST: Duration = Duration::from_secs(2);
const POLL_SLOW: Duration = Duration::from_secs(30);
/// Provisional badges get this long to firm up before auto-open fires.
const AUTO_OPEN_GRACE: Duration = Duration::from_secs(5);

fn main() -> iced::Result {
    env_logger::init();
    // VTE set TERM for its children silently; on this stack it is the
    // embedder's job (spike finding — top/less break without it).
    alacritty_terminal::tty::setup_env();

    iced::application(App::new, App::update, App::view)
        .theme(|_: &App| iced::Theme::Dark)
        .title(|app: &App| match app.active_project() {
            Some(p) => format!("TuxFlow — {}", p.name),
            None => String::from("TuxFlow"),
        })
        .window_size(Size {
            width: 1280.0,
            height: 760.0,
        })
        .subscription(App::subscription)
        .run()
}

enum Phase {
    /// Remote probe in flight on a worker.
    Loading,
    Ready,
    /// Probe failed; bool = retryable (host unreachable vs bad project).
    Failed(String, bool),
}

/// One open project: its own processes, port knowledge, tunnels and poll
/// cadence. `id` is stable across closes — timer events must never land on
/// whatever project inherited a vec index.
struct ProjectState {
    id: u64,
    location: ProjectLocation,
    name: String,
    phase: Phase,
    entries: Vec<ProcessEntry>,
    selected: usize,
    expanded: bool,
    ports: PortDetector,
    tunnels: Option<TunnelManager>,
    port_map: HashMap<u16, u16>,
    poll_interval: Duration,
    poll_chain_started: bool,
    terminals_created: usize,
}

impl ProjectState {
    fn key(&self) -> String {
        self.location.key()
    }

    fn running(&self) -> usize {
        self.entries.iter().filter(|e| e.is_running()).count()
    }
}

struct ProcessForm {
    name: String,
    command: String,
    agent: bool,
    start_with_project: bool,
    auto_restart: bool,
    open_in_browser: bool,
    /// Some((project id, entry index)) when editing an existing process.
    editing: Option<(u64, usize)>,
    original_category: ProcessCategory,
}

struct App {
    projects: Vec<ProjectState>,
    /// Index of the project owning the main pane.
    active: usize,
    saved: SavedProjects,
    app_keys: AppKeys,
    notifications: tuxflow_core::config::settings::NotificationSettings,
    font_size: f32,
    scrollback: usize,
    composer: String,
    search_open: bool,
    search_query: String,
    search_hit: Option<bool>,
    search_input: iced::widget::Id,
    palette_open: bool,
    palette_query: String,
    palette_index: usize,
    palette_input: iced::widget::Id,
    add_project: Option<String>,
    add_command: Option<ProcessForm>,
    next_project_id: u64,
    next_term_id: u64,
}

#[derive(Debug, Clone)]
enum Event {
    Terminal(iced_term::Event),
    /// Worker finished a remote probe: (project name if configured,
    /// process configs, live tmux sessions) or (message, retryable).
    Probed {
        project: u64,
        result: Result<(Option<String>, Vec<ProcessConfig>, Vec<String>), (String, bool)>,
    },
    RetryProbe(u64),
    SelectProcess {
        project: u64,
        index: usize,
    },
    Start {
        project: u64,
        index: usize,
    },
    Stop {
        project: u64,
        index: usize,
    },
    Restart {
        project: u64,
        index: usize,
    },
    AddTerminal(u64),
    ToggleExpanded(u64),
    CloseProject(u64),
    RestartDue {
        project: u64,
        index: usize,
        generation: u64,
    },
    PortsPollTick(u64),
    PortsPolled {
        project: u64,
        session_ports: HashMap<String, Vec<u16>>,
    },
    AutoOpenDue {
        project: u64,
        index: usize,
        generation: u64,
    },
    ComposerChanged(String),
    ComposerSend,
    SearchQueryChanged(String),
    SearchStep(SearchDirection),
    SearchClose,
    PaletteInput(String),
    PaletteSubmit,
    PaletteSelect {
        project: u64,
        index: usize,
    },
    /// Ignored-status keys — the widget consumed everything it wanted
    /// (Ctrl+Shift+V with TEXT on the clipboard never reaches here).
    Hotkey(iced::keyboard::Event),
    /// The image-paste worker finished: bytes to feed the terminal that
    /// initiated the paste (a typed path, or Ctrl+V for agents).
    ImagePasted {
        project: u64,
        term: u64,
        result: Result<Vec<u8>, String>,
    },
    /// Click on the status-bar port pill: open the (tunnel-mapped) URL.
    OpenBadge,
    OpenAddProject,
    AddProjectInput(String),
    AddProjectSubmit,
    AddProjectCancel,
    OpenAddCommand {
        agent: bool,
    },
    OpenEditProcess,
    AddCommandName(String),
    AddCommandCommand(String),
    FormToggleStartWith(bool),
    FormToggleAutoRestart(bool),
    FormToggleOpenBrowser(bool),
    DeleteProcess,
    AddCommandSubmit,
    AddCommandCancel,
}

impl App {
    fn new() -> (Self, Task<Event>) {
        let settings = tuxflow_core::config::settings::AppSettings::load();
        let saved = SavedProjects::load();
        // Insurance: keep a .bak of the last known-good (non-empty)
        // workspace before this process ever saves. One wipe was enough.
        if !saved.directories.is_empty() {
            if let Some(dir) = dirs::config_dir() {
                let file = dir.join("tuxflow/projects.toml");
                let _ = std::fs::copy(&file, file.with_extension("toml.bak"));
            }
        }
        let mut app = App {
            projects: Vec::new(),
            active: 0,
            saved,
            app_keys: AppKeys::from_settings(&settings.keybindings),
            notifications: settings.notifications.clone(),
            font_size: (settings.appearance.font_size as f32).clamp(8.0, 32.0),
            scrollback: settings.appearance.scrollback_lines as usize,
            composer: String::new(),
            // Headless design/debug hook: force panels open for screenshots.
            search_open: std::env::var("TUXFLOW_ICED_UI").as_deref() == Ok("search"),
            search_query: String::new(),
            search_hit: None,
            search_input: iced::widget::Id::unique(),
            palette_open: std::env::var("TUXFLOW_ICED_UI").as_deref() == Ok("palette"),
            palette_query: String::new(),
            palette_index: 0,
            palette_input: iced::widget::Id::unique(),
            add_project: None,
            add_command: None,
            next_project_id: 0,
            next_term_id: 0,
        };

        let mut tasks = Vec::new();

        // CLI args join the persisted workspace.
        let args: Vec<String> = std::env::args().skip(1).collect();
        for arg in &args {
            let key = normalize_key(arg);
            if !app.saved.directories.iter().any(|d| d == &key) {
                app.saved.add(&key);
                app.saved.save();
            }
        }

        let keys: Vec<String> = if app.saved.directories.is_empty() {
            // Nothing saved and no args: live in the cwd, unpersisted.
            vec![ProjectLocation::Local(std::env::current_dir().unwrap_or_default()).key()]
        } else {
            app.saved.directories.clone()
        };
        for key in keys {
            tasks.push(app.open_project(&key));
        }
        if std::env::var("TUXFLOW_ICED_UI").as_deref() == Ok("edit") {
            tasks.push(Task::done(Event::OpenEditProcess));
        }

        (app, Task::batch(tasks))
    }

    fn saved_has(&self, saved: &SavedProjects, key: &str) -> bool {
        saved.directories.iter().any(|d| d == key)
    }

    fn open_project(&mut self, key: &str) -> Task<Event> {
        let location = ProjectLocation::parse(key);
        let id = self.next_project_id;
        self.next_project_id += 1;

        let mut project = ProjectState {
            id,
            name: self
                .saved
                .get_name(key)
                .cloned()
                .unwrap_or_else(|| location.base_name()),
            expanded: self.saved.is_expanded(key).unwrap_or(true),
            phase: Phase::Loading,
            entries: Vec::new(),
            selected: 0,
            ports: PortDetector::new(),
            tunnels: location.host().map(TunnelManager::new),
            port_map: HashMap::new(),
            poll_interval: POLL_FAST,
            poll_chain_started: false,
            terminals_created: 0,
            location,
        };

        let task = match project.location.clone() {
            ProjectLocation::Local(dir) => {
                let (name, configs) = processes::load_local_configs(&dir);
                if self.saved.get_name(key).is_none() {
                    project.name = name;
                }
                let merged = processes::merge_saved(configs, &self.saved, key);
                project.entries = processes::entries_from(merged);
                project.phase = Phase::Ready;
                self.projects.push(project);
                let pidx = self.projects.len() - 1;
                self.boot_processes(pidx, &[])
            }
            ProjectLocation::Ssh { .. } => {
                self.projects.push(project);
                self.probe_task(self.projects.len() - 1)
            }
        };
        task
    }

    fn project_index(&self, id: u64) -> Option<usize> {
        self.projects.iter().position(|p| p.id == id)
    }

    fn active_project(&self) -> Option<&ProjectState> {
        self.projects.get(self.active)
    }

    /// Kick the blocking ssh probe onto a worker; the project shows Loading.
    fn probe_task(&mut self, pidx: usize) -> Task<Event> {
        let project = &mut self.projects[pidx];
        project.phase = Phase::Loading;
        let id = project.id;
        let (Some(host), dir) = (
            project.location.host().map(String::from),
            project.location.dir_str(),
        ) else {
            return Task::none();
        };
        Task::perform(
            tokio::task::spawn_blocking(move || {
                remote::probe::probe_remote(&host, &dir, true)
                    .map(|p| {
                        let name = p.config.as_ref().map(|c| c.project.name.clone());
                        let configs = match p.config {
                            Some(c) => c.process,
                            None => p
                                .stacks
                                .into_iter()
                                .flat_map(|s| s.suggested_processes)
                                .collect(),
                        };
                        (name, configs, p.live_sessions)
                    })
                    .map_err(|e| {
                        let retryable = matches!(e, ProbeError::Unreachable(_));
                        (e.to_string(), retryable)
                    })
            }),
            move |joined| Event::Probed {
                project: id,
                result: joined.unwrap_or_else(|e| Err((format!("probe worker died: {e}"), false))),
            },
        )
    }

    /// Start what should be up after load: sessions still alive on the host
    /// (reattach — never show "stopped" for a running detached process) and
    /// start_with_project ones (those count as user-initiated).
    fn boot_processes(&mut self, pidx: usize, live_sessions: &[String]) -> Task<Event> {
        let key = self.projects[pidx].key();
        let mut tasks = Vec::new();
        for i in 0..self.projects[pidx].entries.len() {
            let name = self.projects[pidx].entries[i].config.name.clone();
            let live = live_sessions.contains(&remote::remote_session_name(&key, &name));
            if live {
                tasks.push(self.start(pidx, i));
            } else if self.projects[pidx].entries[i].config.start_with_project {
                tasks.push(self.start_fresh(pidx, i));
            }
        }
        // Remote: begin the self-perpetuating ports-poll chain, once.
        if self.projects[pidx].tunnels.is_some() && !self.projects[pidx].poll_chain_started {
            self.projects[pidx].poll_chain_started = true;
            tasks.push(self.schedule_poll(pidx));
        }
        Task::batch(tasks)
    }

    fn schedule_poll(&self, pidx: usize) -> Task<Event> {
        let id = self.projects[pidx].id;
        Task::perform(
            tokio::time::sleep(self.projects[pidx].poll_interval),
            move |_| Event::PortsPollTick(id),
        )
    }

    /// Spawn (or respawn/reattach) a process's terminal. Fresh terminal id
    /// each time: subscription identity must change or iced keeps the dead
    /// stream.
    fn start(&mut self, pidx: usize, index: usize) -> Task<Event> {
        let id = self.next_term_id;
        self.next_term_id += 1;

        let reservations = self.app_keys.reservations();
        let font_size = self.font_size;
        let scrollback = self.scrollback;
        let project = &mut self.projects[pidx];
        let settings = processes::spawn_settings(
            &project.location,
            &mut project.entries[index],
            font_size,
            scrollback,
        );
        let entry = &mut project.entries[index];
        match iced_term::Terminal::new(id, settings) {
            Ok(mut term) => {
                // Reserve the app's chords before the first keystroke —
                // the stock bindings would type them into the shell.
                term.handle(iced_term::Command::AddBindings(reservations));
                let focus = TerminalView::focus(term.widget_id().clone());
                entry.terminal = Some(term);
                entry.term_id = Some(id);
                entry.status = Status::Running;
                entry.last_exit = None;
                entry.stopping = false;
                entry.auto_open_grace = false;
                entry.started_at = Some(Instant::now());
                let name = entry.config.name.clone();
                project.ports.clear(&name);
                project.selected = index;
                self.active = pidx;
                focus
            }
            Err(err) => {
                log::error!("failed to spawn {}: {err}", entry.config.name);
                entry.status = Status::Crashed(None);
                Task::none()
            }
        }
    }

    /// Manual start: forgives past failures, cancels pending timers, arms
    /// the one-shot auto-open, and stamps the project recently-used.
    fn start_fresh(&mut self, pidx: usize, index: usize) -> Task<Event> {
        let key = self.projects[pidx].key();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.saved.set_last_used(&key, now);
        self.saved.save();

        let entry = &mut self.projects[pidx].entries[index];
        entry.restart_attempts = 0;
        entry.restart_generation += 1;
        entry.pending_auto_open = entry.config.open_in_browser;
        entry.auto_open_grace = false;
        entry.outage_notified = false;
        self.start(pidx, index)
    }

    /// Stop. Remote: explicitly kill the host-side session first (the local
    /// PTY teardown only detaches it), fire-and-forget, and make the next
    /// spawn clear any survivor. Local: dropping the terminal SIGHUPs the
    /// child on the PTY thread.
    fn stop(&mut self, pidx: usize, index: usize) {
        let project = &mut self.projects[pidx];
        if let Some(host) = project.location.host() {
            let entry = &mut project.entries[index];
            if entry.config.category != ProcessCategory::SSH {
                let session = entry.remote_session.take();
                if let Some(pidfile) = entry.remote_pidfile.take() {
                    remote::remote_kill(host, &pidfile, session.as_deref());
                    entry.remote_fresh_next = true;
                }
            }
        }
        let entry = &mut project.entries[index];
        entry.stopping = true;
        entry.restart_generation += 1;
        entry.restart_attempts = 0;
        entry.pending_auto_open = false;
        entry.terminal = None;
        entry.term_id = None;
        entry.status = Status::Stopped;
        self.maybe_drop_tunnels(pidx);
    }

    /// Forwards live only while something runs — the next run rediscovers
    /// its ports instead of inheriting stale forwards (GTK behavior).
    fn maybe_drop_tunnels(&mut self, pidx: usize) {
        let project = &mut self.projects[pidx];
        if project.entries.iter().any(|e| e.is_running()) {
            return;
        }
        if let Some(tunnels) = &mut project.tunnels {
            tunnels.close_all();
        }
        project.port_map.clear();
    }

    fn add_terminal(&mut self, pidx: usize) -> Task<Event> {
        self.projects[pidx].terminals_created += 1;
        let config = ProcessConfig {
            name: format!("terminal {}", self.projects[pidx].terminals_created),
            command: String::new(),
            working_dir: None,
            start_with_project: false,
            auto_restart: false,
            open_in_browser: false,
            restart_when_changed: Vec::new(),
            env: Default::default(),
            category: ProcessCategory::Terminal,
            auto_named: true,
            display_name: None,
        };
        self.projects[pidx].entries.push(ProcessEntry::new(config));
        let index = self.projects[pidx].entries.len() - 1;
        self.start(pidx, index)
    }

    /// Close a project: local processes die with their PTYs, remote
    /// sessions DETACH (kill only happens on explicit per-process stop) —
    /// the same contract as quitting the app.
    fn close_project(&mut self, pidx: usize) {
        let key = self.projects[pidx].key();
        if let Some(tunnels) = &mut self.projects[pidx].tunnels {
            tunnels.close_all();
        }
        self.projects.remove(pidx);
        self.saved.remove(&key);
        self.saved.save();
        if self.active >= self.projects.len() {
            self.active = self.projects.len().saturating_sub(1);
        }
    }

    /// A terminal's run ended (Exit event) — classify and schedule what the
    /// policy asks for (restart with backoff, endless reconnect, nothing).
    fn finalize_exit(&mut self, pidx: usize, index: usize) -> Task<Event> {
        let project = &mut self.projects[pidx];
        let connection_loss = project.location.is_remote()
            && project.entries[index].config.category != ProcessCategory::SSH
            && project.entries[index].last_exit == Some(255);

        let project_id = project.id;
        let entry = &mut project.entries[index];
        entry.terminal = None;
        entry.term_id = None;

        let run = entry.started_at.map(|t| t.elapsed());
        let stopping_was = entry.stopping;
        let (status, attempts, delay) = plan_after_exit(
            entry.config.auto_restart,
            entry.stopping,
            connection_loss,
            entry.last_exit,
            run,
            entry.restart_attempts,
        );
        entry.status = status;
        entry.restart_attempts = attempts;
        entry.stopping = false;

        // Desktop notifications, per the shared settings (GTK parity).
        let project_name = self.projects[pidx].name.clone();
        let entry = &mut self.projects[pidx].entries[index];
        let name = entry.config.name.clone();
        match &entry.status {
            Status::Crashed(code) => {
                notify::crash(&self.notifications, &project_name, &name, *code)
            }
            Status::Restarting(attempt) => {
                notify::auto_restart(&self.notifications, &project_name, &name, *attempt)
            }
            Status::Reconnecting(_) if !entry.outage_notified => {
                entry.outage_notified = true;
                notify::disconnect(&project_name, &name);
            }
            Status::Stopped if !stopping_was => {
                notify::finish(&self.notifications, &project_name, &name)
            }
            _ => {}
        }

        let entry = &mut self.projects[pidx].entries[index];
        let task = match delay {
            Some(delay) => {
                let generation = entry.restart_generation;
                Task::perform(tokio::time::sleep(delay), move |_| Event::RestartDue {
                    project: project_id,
                    index,
                    generation,
                })
            }
            None => Task::none(),
        };
        self.maybe_drop_tunnels(pidx);
        task
    }

    fn entry_for_term(&self, term_id: u64) -> Option<(usize, usize)> {
        for (pidx, project) in self.projects.iter().enumerate() {
            if let Some(eidx) = project
                .entries
                .iter()
                .position(|e| e.term_id == Some(term_id))
            {
                return Some((pidx, eidx));
            }
        }
        None
    }

    /// Feed the scanner (remote output arrives hard-wrapped at pane width),
    /// keep a forward alive for every local port it has seen, and let
    /// auto-open react to the new badge.
    fn rescan_ports(&mut self, pidx: usize, index: usize) -> Task<Event> {
        let project = &mut self.projects[pidx];
        let Some(term) = project.entries[index].terminal.as_ref() else {
            return Task::none();
        };
        let name = project.entries[index].config.name.clone();
        let dump = visible_text(term);
        if project.location.is_remote() {
            let cols = term.backend().renderable_content().terminal_size.columns();
            project.ports.scan_output_wrapped(&name, &dump, cols);
        } else {
            project.ports.scan_output(&name, &dump);
        }

        if let Some(tunnels) = &mut project.tunnels {
            for port in project.ports.all_local_ports(&name) {
                if let Some(local) = tunnels.ensure(port) {
                    project.port_map.insert(port, local);
                }
            }
        }
        self.maybe_auto_open(pidx, index)
    }

    /// The one-shot browser open: fires when the badge is final, or arms a
    /// 5 s grace when only a provisional badge exists.
    fn maybe_auto_open(&mut self, pidx: usize, index: usize) -> Task<Event> {
        let project = &self.projects[pidx];
        let entry = &project.entries[index];
        let name = entry.config.name.clone();
        if !entry.pending_auto_open || !project.ports.has_port(&name) {
            return Task::none();
        }
        if project.ports.badge_final(&name) {
            self.open_in_browser(pidx, index);
            Task::none()
        } else if !entry.auto_open_grace {
            let project_id = project.id;
            self.projects[pidx].entries[index].auto_open_grace = true;
            let generation = self.projects[pidx].entries[index].restart_generation;
            Task::perform(tokio::time::sleep(AUTO_OPEN_GRACE), move |_| {
                Event::AutoOpenDue {
                    project: project_id,
                    index,
                    generation,
                }
            })
        } else {
            Task::none()
        }
    }

    /// Dispatch a matched app shortcut.
    fn apply_action(&mut self, action: AppAction) -> Task<Event> {
        match action {
            AppAction::TerminalSearch => {
                self.search_open = true;
                iced::widget::operation::focus(self.search_input.clone())
            }
            AppAction::CommandPalette => {
                self.palette_open = true;
                self.palette_query.clear();
                self.palette_index = 0;
                iced::widget::operation::focus(self.palette_input.clone())
            }
            AppAction::PrevProcess => self.step_process(-1),
            AppAction::NextProcess => self.step_process(1),
            AppAction::PrevProject => self.step_project(-1),
            AppAction::NextProject => self.step_project(1),
            AppAction::NewTerminal => match self.projects.get(self.active) {
                Some(_) => self.add_terminal(self.active),
                None => Task::none(),
            },
            AppAction::CloseProcess => self.close_selected_process(),
            AppAction::FontIncrease => self.change_font(1.0),
            AppAction::FontDecrease => self.change_font(-1.0),
        }
    }

    fn step_process(&mut self, delta: i32) -> Task<Event> {
        let Some(project) = self.projects.get_mut(self.active) else {
            return Task::none();
        };
        let n = project.entries.len();
        if n == 0 {
            return Task::none();
        }
        project.selected = ((project.selected as i32 + delta).rem_euclid(n as i32)) as usize;
        self.focus_selected_terminal()
    }

    fn step_project(&mut self, delta: i32) -> Task<Event> {
        let n = self.projects.len();
        if n == 0 {
            return Task::none();
        }
        self.active = ((self.active as i32 + delta).rem_euclid(n as i32)) as usize;
        self.focus_selected_terminal()
    }

    /// Close = GTK's "Close Agent/Terminal": ad-hoc terminals disappear,
    /// anything else just stops.
    fn close_selected_process(&mut self) -> Task<Event> {
        let Some(project) = self.projects.get(self.active) else {
            return Task::none();
        };
        let (pidx, index) = (self.active, project.selected);
        let Some(entry) = self.projects[pidx].entries.get(index) else {
            return Task::none();
        };
        let is_adhoc_terminal =
            entry.config.category == ProcessCategory::Terminal && entry.config.auto_named;
        self.stop(pidx, index);
        if is_adhoc_terminal {
            self.projects[pidx].entries.remove(index);
            let n = self.projects[pidx].entries.len();
            if n > 0 {
                self.projects[pidx].selected = index.min(n - 1);
            } else {
                self.projects[pidx].selected = 0;
            }
        }
        Task::none()
    }

    fn change_font(&mut self, delta: f32) -> Task<Event> {
        self.font_size = (self.font_size + delta).clamp(8.0, 32.0);
        let size = self.font_size;
        for project in &mut self.projects {
            for entry in &mut project.entries {
                if let Some(term) = entry.terminal.as_mut() {
                    term.handle(iced_term::Command::ChangeFont(
                        iced_term::settings::FontSettings {
                            size,
                            ..Default::default()
                        },
                    ));
                }
            }
        }
        Task::none()
    }

    /// Route a search command to the active selected terminal and record
    /// whether it found anything (the bar shows "no match").
    fn send_search(&mut self, cmd: BackendCommand) {
        let Some(project) = self.projects.get_mut(self.active) else {
            return;
        };
        let selected = project.selected;
        if let Some(term) = project
            .entries
            .get_mut(selected)
            .and_then(|e| e.terminal.as_mut())
        {
            let action = term.handle(iced_term::Command::ProxyToBackend(cmd));
            if let iced_term::actions::Action::SearchResult(found) = action {
                self.search_hit = Some(found);
            }
        }
    }

    fn focus_selected_terminal(&self) -> Task<Event> {
        match self
            .projects
            .get(self.active)
            .and_then(|p| p.entries.get(p.selected))
            .and_then(|e| e.terminal.as_ref())
        {
            Some(term) => TerminalView::focus(term.widget_id().clone()),
            None => Task::none(),
        }
    }

    fn close_palette(&mut self) -> Task<Event> {
        self.palette_open = false;
        self.palette_query.clear();
        self.palette_index = 0;
        self.focus_selected_terminal()
    }

    /// (project id, entry index) pairs matching the palette query, in
    /// sidebar order. Case-insensitive substring over "project process".
    fn palette_matches(&self) -> Vec<(u64, usize)> {
        let needle = self.palette_query.to_lowercase();
        let mut out = Vec::new();
        for project in &self.projects {
            for (i, entry) in project.entries.iter().enumerate() {
                let hay = format!("{} {}", project.name, entry.config.name).to_lowercase();
                if needle.is_empty() || hay.contains(&needle) {
                    out.push((project.id, i));
                }
            }
        }
        out
    }

    /// Ctrl+Shift+V with an image-only clipboard — the GTK app's flow,
    /// verbatim: upload the PNG to the host's clipboard-shim slot; AGENT
    /// terminals then get a real Ctrl+V (the agent "reads the clipboard"
    /// through the shim and shows its native attachment UI), others get
    /// the remote path typed. Local projects: agents get Ctrl+V (they can
    /// read the real clipboard themselves), others get a temp-file path.
    /// Clipboard read + encode + ssh all run on a worker.
    fn paste_image(&mut self) -> Task<Event> {
        let Some(project) = self.active_project() else {
            return Task::none();
        };
        let Some(entry) = project.entries.get(project.selected) else {
            return Task::none();
        };
        let (Some(term), true) = (entry.term_id, entry.terminal.is_some()) else {
            return Task::none();
        };
        let project_id = project.id;
        let host = project.location.host().map(String::from);
        let is_agent = entry.config.category == ProcessCategory::Agent;

        Task::perform(
            tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
                let image = arboard::Clipboard::new()
                    .and_then(|mut cb| cb.get_image())
                    .map_err(|e| format!("no image on clipboard: {e}"))?;
                match (host, is_agent) {
                    (None, true) => {
                        // Local agent: it reads the real clipboard itself.
                        Ok(vec![0x16])
                    }
                    (None, false) => {
                        let png = encode_png(&image)?;
                        let path = std::env::temp_dir()
                            .join(format!(".tuxflow-img-{}.png", std::process::id()));
                        std::fs::write(&path, png).map_err(|e| e.to_string())?;
                        Ok(format!("{} ", path.display()).into_bytes())
                    }
                    (Some(host), agent) => {
                        let png = encode_png(&image)?;
                        let path = remote::upload_clipboard_image(&host, &png)?;
                        log::info!("image paste: uploaded to {host}:{path}");
                        if agent {
                            Ok(vec![0x16])
                        } else {
                            Ok(format!("{path} ").into_bytes())
                        }
                    }
                }
            }),
            move |joined| Event::ImagePasted {
                project: project_id,
                term,
                result: joined.unwrap_or_else(|e| Err(format!("paste worker died: {e}"))),
            },
        )
    }

    fn open_in_browser(&mut self, pidx: usize, index: usize) {
        let name = self.projects[pidx].entries[index].config.name.clone();
        self.projects[pidx].entries[index].pending_auto_open = false;
        let Some(url) = browser_url(&self.projects[pidx], &name) else {
            return;
        };
        log::info!("auto-open {url}");
        if let Err(e) = open::that(&url) {
            log::warn!("auto-open {url} failed: {e}");
        }
    }

    fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Probed { project, result } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                match result {
                    Ok((name, configs, live_sessions)) => {
                        let key = self.projects[pidx].key();
                        if self.saved.get_name(&key).is_none() {
                            if let Some(name) = name {
                                self.projects[pidx].name = name;
                            }
                        }
                        let merged = processes::merge_saved(configs, &self.saved, &key);
                        self.projects[pidx].entries = processes::entries_from(merged);
                        self.projects[pidx].phase = Phase::Ready;
                        self.boot_processes(pidx, &live_sessions)
                    }
                    Err((message, retryable)) => {
                        self.projects[pidx].phase = Phase::Failed(message, retryable);
                        Task::none()
                    }
                }
            }
            Event::RetryProbe(project) => match self.project_index(project) {
                Some(pidx) => self.probe_task(pidx),
                None => Task::none(),
            },
            Event::SelectProcess { project, index } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                self.active = pidx;
                self.projects[pidx].selected = index;
                match self.projects[pidx]
                    .entries
                    .get(index)
                    .and_then(|e| e.terminal.as_ref())
                {
                    Some(term) => TerminalView::focus(term.widget_id().clone()),
                    None => Task::none(),
                }
            }
            Event::Start { project, index } | Event::Restart { project, index } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                if self.projects[pidx].entries[index].terminal.is_some() {
                    self.stop(pidx, index);
                }
                self.start_fresh(pidx, index)
            }
            Event::Stop { project, index } => {
                if let Some(pidx) = self.project_index(project) {
                    self.stop(pidx, index);
                }
                Task::none()
            }
            Event::AddTerminal(project) => match self.project_index(project) {
                Some(pidx) => self.add_terminal(pidx),
                None => Task::none(),
            },
            Event::ToggleExpanded(project) => {
                if let Some(pidx) = self.project_index(project) {
                    self.projects[pidx].expanded = !self.projects[pidx].expanded;
                    let key = self.projects[pidx].key();
                    self.saved.set_expanded(&key, self.projects[pidx].expanded);
                    self.saved.save();
                }
                Task::none()
            }
            Event::CloseProject(project) => {
                if let Some(pidx) = self.project_index(project) {
                    self.close_project(pidx);
                }
                Task::none()
            }
            Event::RestartDue {
                project,
                index,
                generation,
            } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                let due = self.projects[pidx].entries.get(index).is_some_and(|e| {
                    e.restart_generation == generation
                        && matches!(e.status, Status::Restarting(_) | Status::Reconnecting(_))
                });
                if due {
                    self.start(pidx, index)
                } else {
                    Task::none()
                }
            }
            Event::PortsPollTick(project) => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                let sessions: Vec<String> = self.projects[pidx]
                    .entries
                    .iter()
                    .filter(|e| e.is_running())
                    .filter_map(|e| e.remote_session.clone())
                    .collect();
                let host = self.projects[pidx].location.host().map(String::from);
                match (host, sessions.is_empty()) {
                    (Some(host), false) => Task::perform(
                        tokio::task::spawn_blocking(move || {
                            remote::ports::session_ports(&host, &sessions)
                        }),
                        move |joined| Event::PortsPolled {
                            project,
                            session_ports: joined.unwrap_or_default(),
                        },
                    ),
                    _ => {
                        self.projects[pidx].poll_interval = POLL_SLOW;
                        self.schedule_poll(pidx)
                    }
                }
            }
            Event::PortsPolled {
                project,
                session_ports,
            } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                // Everything the host-side walk found forwards 1:1
                // (ensure_exact): remote dev servers bake their own port
                // into URLs they serve. A taken local port is a hard
                // failure by design, not a remap.
                let mut opened = false;
                let proj = &mut self.projects[pidx];
                for (session, ports) in &session_ports {
                    let ours = proj
                        .entries
                        .iter()
                        .any(|e| e.remote_session.as_deref() == Some(session));
                    if !ours {
                        continue;
                    }
                    for &port in ports {
                        if proj.port_map.contains_key(&port) {
                            continue;
                        }
                        if let Some(tunnels) = &mut proj.tunnels {
                            match tunnels.ensure_exact(port) {
                                Some(local) => {
                                    proj.port_map.insert(port, local);
                                    opened = true;
                                    log::info!("exact forward {port} for {session}");
                                }
                                None => {
                                    log::warn!("exact forward for {port} failed — local port taken")
                                }
                            }
                        }
                    }
                }
                proj.poll_interval = if opened {
                    POLL_FAST
                } else {
                    (proj.poll_interval * 2).min(POLL_SLOW)
                };
                self.schedule_poll(pidx)
            }
            Event::AutoOpenDue {
                project,
                index,
                generation,
            } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                let due = self.projects[pidx]
                    .entries
                    .get(index)
                    .is_some_and(|e| e.restart_generation == generation && e.pending_auto_open);
                let name = self.projects[pidx]
                    .entries
                    .get(index)
                    .map(|e| e.config.name.clone())
                    .unwrap_or_default();
                if due && self.projects[pidx].ports.has_port(&name) {
                    self.open_in_browser(pidx, index);
                }
                Task::none()
            }
            Event::Hotkey(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                // Palette navigation first (its input consumes typing but
                // not Esc/arrows).
                if self.palette_open {
                    return match key.as_ref() {
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => {
                            self.close_palette()
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowUp) => {
                            self.palette_index = self.palette_index.saturating_sub(1);
                            Task::none()
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowDown) => {
                            let max = self.palette_matches().len().saturating_sub(1);
                            self.palette_index = (self.palette_index + 1).min(max);
                            Task::none()
                        }
                        _ => Task::none(),
                    };
                }
                if self.search_open
                    && matches!(
                        key.as_ref(),
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                    )
                {
                    return self.update(Event::SearchClose);
                }
                if let Some(action) = self.app_keys.action_for(&key, modifiers) {
                    return self.apply_action(action);
                }
                match key.as_ref() {
                    // Reaching here means the widget found no TEXT to paste
                    // — the clipboard holds an image (or nothing).
                    iced::keyboard::Key::Character(c)
                        if c.eq_ignore_ascii_case("v")
                            && modifiers.control()
                            && modifiers.shift() =>
                    {
                        self.paste_image()
                    }
                    _ => Task::none(),
                }
            }
            Event::Hotkey(_) => Task::none(),
            Event::SearchQueryChanged(query) => {
                self.search_query = query;
                if self.search_query.is_empty() {
                    self.search_hit = None;
                    self.send_search(BackendCommand::SearchClear);
                } else {
                    let cmd = BackendCommand::SearchNext(
                        self.search_query.clone(),
                        SearchDirection::Left,
                    );
                    self.send_search(cmd);
                }
                Task::none()
            }
            Event::SearchStep(direction) => {
                if !self.search_query.is_empty() {
                    let cmd = BackendCommand::SearchNext(self.search_query.clone(), direction);
                    self.send_search(cmd);
                }
                Task::none()
            }
            Event::SearchClose => {
                self.search_open = false;
                self.search_query.clear();
                self.search_hit = None;
                self.send_search(BackendCommand::SearchClear);
                self.focus_selected_terminal()
            }
            Event::PaletteInput(value) => {
                self.palette_query = value;
                self.palette_index = 0;
                Task::none()
            }
            Event::PaletteSubmit => {
                let target = self.palette_matches().get(self.palette_index).copied();
                match target {
                    Some((project, index)) => {
                        let close = self.close_palette();
                        let select = self.update(Event::SelectProcess { project, index });
                        Task::batch([close, select])
                    }
                    None => self.close_palette(),
                }
            }
            Event::PaletteSelect { project, index } => {
                let close = self.close_palette();
                let select = self.update(Event::SelectProcess { project, index });
                Task::batch([close, select])
            }
            Event::ImagePasted {
                project,
                term,
                result,
            } => {
                match result {
                    Ok(bytes) => {
                        // Only if the initiating terminal is still alive —
                        // a restart between paste and upload must not type
                        // a stale path into the fresh run.
                        let target = self.project_index(project).and_then(|pidx| {
                            self.projects[pidx]
                                .entries
                                .iter_mut()
                                .find(|e| e.term_id == Some(term))
                                .and_then(|e| e.terminal.as_mut())
                        });
                        if let Some(terminal) = target {
                            terminal.handle(iced_term::Command::ProxyToBackend(
                                BackendCommand::Write(bytes),
                            ));
                        }
                    }
                    Err(e) => log::error!("image paste failed: {e}"),
                }
                Task::none()
            }
            Event::ComposerChanged(value) => {
                self.composer = value;
                Task::none()
            }
            Event::ComposerSend => {
                // The composer types into the selected terminal like the
                // GTK composer_bar does via feed_child — local input beats
                // ssh typing latency for remote agents.
                if !self.composer.is_empty() {
                    let selected = self
                        .projects
                        .get_mut(self.active)
                        .and_then(|p| p.entries.get_mut(p.selected))
                        .and_then(|e| e.terminal.as_mut());
                    if let Some(term) = selected {
                        let mut bytes = self.composer.clone().into_bytes();
                        bytes.push(b'\r');
                        term.handle(iced_term::Command::ProxyToBackend(BackendCommand::Write(
                            bytes,
                        )));
                        self.composer.clear();
                    }
                }
                Task::none()
            }
            Event::OpenBadge => {
                if let Some(project) = self.active_project() {
                    if let Some(entry) = project.entries.get(project.selected) {
                        if let Some(url) = browser_url(project, &entry.config.name) {
                            log::info!("open badge {url}");
                            if let Err(e) = open::that(&url) {
                                log::warn!("open {url} failed: {e}");
                            }
                        }
                    }
                }
                Task::none()
            }
            Event::OpenAddProject => {
                self.add_project = Some(String::new());
                Task::none()
            }
            Event::AddProjectInput(value) => {
                self.add_project = Some(value);
                Task::none()
            }
            Event::AddProjectCancel => {
                self.add_project = None;
                Task::none()
            }
            Event::AddProjectSubmit => {
                let Some(input) = self.add_project.take() else {
                    return Task::none();
                };
                let input = input.trim().to_string();
                if input.is_empty() {
                    return Task::none();
                }
                let key = normalize_key(&input);
                if self.projects.iter().any(|p| p.key() == key) {
                    return Task::none();
                }
                if !self.saved_has(&self.saved, &key) {
                    self.saved.add(&key);
                    self.saved.save();
                }
                let task = self.open_project(&key);
                self.active = self.projects.len() - 1;
                task
            }
            Event::OpenAddCommand { agent } => {
                if self.active_project().is_some() {
                    self.add_command = Some(ProcessForm {
                        name: String::new(),
                        command: String::new(),
                        agent,
                        start_with_project: false,
                        auto_restart: false,
                        open_in_browser: false,
                        editing: None,
                        original_category: if agent {
                            ProcessCategory::Agent
                        } else {
                            ProcessCategory::Command
                        },
                    });
                }
                Task::none()
            }
            Event::OpenEditProcess => {
                if let Some(project) = self.active_project() {
                    if let Some(entry) = project.entries.get(project.selected) {
                        self.add_command = Some(ProcessForm {
                            name: entry.config.name.clone(),
                            command: entry.config.command.clone(),
                            agent: entry.config.category == ProcessCategory::Agent,
                            start_with_project: entry.config.start_with_project,
                            auto_restart: entry.config.auto_restart,
                            open_in_browser: entry.config.open_in_browser,
                            editing: Some((project.id, project.selected)),
                            original_category: entry.config.category.clone(),
                        });
                    }
                }
                Task::none()
            }
            Event::FormToggleStartWith(v) => {
                if let Some(form) = &mut self.add_command {
                    form.start_with_project = v;
                }
                Task::none()
            }
            Event::FormToggleAutoRestart(v) => {
                if let Some(form) = &mut self.add_command {
                    form.auto_restart = v;
                }
                Task::none()
            }
            Event::FormToggleOpenBrowser(v) => {
                if let Some(form) = &mut self.add_command {
                    form.open_in_browser = v;
                }
                Task::none()
            }
            Event::DeleteProcess => {
                let Some(form) = self.add_command.take() else {
                    return Task::none();
                };
                let Some((project_id, index)) = form.editing else {
                    return Task::none();
                };
                let Some(pidx) = self.project_index(project_id) else {
                    return Task::none();
                };
                if self.projects[pidx].entries.get(index).is_none() {
                    return Task::none();
                }
                self.stop(pidx, index);
                let key = self.projects[pidx].key();
                let name = self.projects[pidx].entries[index].config.name.clone();
                // The user's edit of a DETECTED process lives in
                // custom_commands; deleting must both drop the custom copy
                // and remember the deletion, or detection resurrects it on
                // the next load.
                let is_custom = self
                    .saved
                    .get_custom_commands(&key)
                    .is_some_and(|l| l.iter().any(|c| c.name == name));
                if is_custom {
                    self.saved.remove_custom_command(&key, &name);
                }
                self.saved.add_deleted_process(&key, &name);
                self.projects[pidx].entries.remove(index);
                let n = self.projects[pidx].entries.len();
                self.projects[pidx].selected = if n == 0 { 0 } else { index.min(n - 1) };
                Task::none()
            }
            Event::AddCommandName(value) => {
                if let Some(form) = &mut self.add_command {
                    form.name = value;
                }
                Task::none()
            }
            Event::AddCommandCommand(value) => {
                if let Some(form) = &mut self.add_command {
                    form.command = value;
                }
                Task::none()
            }
            Event::AddCommandCancel => {
                self.add_command = None;
                Task::none()
            }
            Event::AddCommandSubmit => {
                let Some(form) = self.add_command.take() else {
                    return Task::none();
                };
                if let Some((project_id, index)) = form.editing {
                    // Edit: persist as the custom command that overrides
                    // same-named detection on every future load.
                    let Some(pidx) = self.project_index(project_id) else {
                        return Task::none();
                    };
                    let Some(entry) = self.projects[pidx].entries.get_mut(index) else {
                        return Task::none();
                    };
                    let command = form.command.trim();
                    if command.is_empty() {
                        self.add_command = Some(form);
                        return Task::none();
                    }
                    let mut config = entry.config.clone();
                    config.command = command.to_string();
                    config.start_with_project = form.start_with_project;
                    config.auto_restart = form.auto_restart;
                    config.open_in_browser = form.open_in_browser;
                    entry.config = config.clone();
                    let key = self.projects[pidx].key();
                    self.saved.add_custom_command(&key, config);
                    return Task::none();
                }
                let (name, command) = (form.name.trim().to_string(), form.command.trim());
                if name.is_empty() || command.is_empty() {
                    self.add_command = Some(form);
                    return Task::none();
                }
                let pidx = self.active;
                if self.projects.get(pidx).is_none()
                    || self.projects[pidx]
                        .entries
                        .iter()
                        .any(|e| e.config.name == name)
                {
                    return Task::none();
                }
                let config = ProcessConfig {
                    name,
                    command: command.to_string(),
                    working_dir: None,
                    start_with_project: form.start_with_project,
                    auto_restart: form.auto_restart,
                    open_in_browser: form.open_in_browser,
                    restart_when_changed: Vec::new(),
                    env: Default::default(),
                    category: if form.agent {
                        ProcessCategory::Agent
                    } else {
                        ProcessCategory::Command
                    },
                    auto_named: false,
                    display_name: None,
                };
                // Persist as a custom command — survives restarts and
                // overrides same-named detection, like the GTK dialogs.
                let key = self.projects[pidx].key();
                self.saved.add_custom_command(&key, config.clone());
                self.projects[pidx].entries.push(ProcessEntry::new(config));
                let index = self.projects[pidx].entries.len() - 1;
                self.start_fresh(pidx, index)
            }
            Event::Terminal(iced_term::Event::BackendCall(term_id, cmd)) => {
                let Some((pidx, index)) = self.entry_for_term(term_id) else {
                    return Task::none();
                };

                let mut side_task = Task::none();
                let mut rescan = false;
                if let BackendCommand::ProcessAlacrittyEvent(ev) = &cmd {
                    match ev {
                        AEvent::Wakeup => rescan = true,
                        AEvent::ChildExit(code) => {
                            self.projects[pidx].entries[index].last_exit = Some(*code);
                        }
                        AEvent::ClipboardStore(ty, data) if !data.is_empty() => {
                            // OSC 52 — agents' and tmux's copies; empty
                            // clears are ignored (multiplex emits them).
                            side_task = match ty {
                                ClipboardType::Clipboard => iced::clipboard::write(data.clone()),
                                ClipboardType::Selection => {
                                    iced::clipboard::write_primary(data.clone())
                                }
                            };
                        }
                        _ => {}
                    }
                }

                let action = {
                    let entry = &mut self.projects[pidx].entries[index];
                    match entry.terminal.as_mut() {
                        Some(term) => term.handle(iced_term::Command::ProxyToBackend(cmd)),
                        None => iced_term::actions::Action::Ignore,
                    }
                };

                let action_task = match action {
                    iced_term::actions::Action::Shutdown => self.finalize_exit(pidx, index),
                    iced_term::actions::Action::PublishSelection(text) => {
                        iced::clipboard::write_primary(text)
                    }
                    iced_term::actions::Action::OpenUrl(url) => {
                        // Ctrl+click: the terminal shows the HOST's port —
                        // rewrite through the tunnel map, creating/reviving
                        // the forward the click is about to use.
                        let project = &mut self.projects[pidx];
                        let tunnels = &mut project.tunnels;
                        let port_map = &mut project.port_map;
                        let rewritten = rewrite_clicked_url(&url, |port| {
                            let local = tunnels.as_mut()?.ensure(port)?;
                            port_map.insert(port, local);
                            Some(local)
                        });
                        log::info!("open link {rewritten}");
                        if let Err(e) = open::that(&rewritten) {
                            log::warn!("open {rewritten} failed: {e}");
                        }
                        Task::none()
                    }
                    _ => Task::none(),
                };

                let scan_task = if rescan {
                    self.rescan_ports(pidx, index)
                } else {
                    Task::none()
                };

                Task::batch([side_task, action_task, scan_task])
            }
        }
    }

    fn view(&'_ self) -> Element<'_, Event> {
        let body = row![
            self.view_sidebar(),
            vline(),
            container(self.view_main())
                .width(Length::Fill)
                .height(Length::Fill),
        ]
        .height(Length::Fill);

        let base: Element<'_, Event> = column![body, hline(), self.view_status_bar()].into();

        if self.palette_open {
            iced::widget::stack![base, self.view_palette()].into()
        } else {
            base
        }
    }

    /// The command palette: a dimmed backdrop with a centered card —
    /// type to filter every process across every project, Enter jumps.
    fn view_palette(&'_ self) -> Element<'_, Event> {
        let matches = self.palette_matches();
        let mut list = column![].spacing(1);
        for (row_i, (project_id, index)) in matches.iter().take(12).enumerate() {
            let Some(project) = self.projects.iter().find(|p| p.id == *project_id) else {
                continue;
            };
            let Some(entry) = project.entries.get(*index) else {
                continue;
            };
            let remote = project.location.is_remote();
            let accent = accent_for(remote);
            let dot_color = match entry.status {
                Status::Running => accent,
                Status::Stopped => STOPPED,
                Status::Crashed(_) => CRASHED,
                Status::Restarting(_) | Status::Reconnecting(_) => RESTARTING,
            };
            list = list.push(
                button(
                    row![
                        text("\u{25cf}").size(10).color(dot_color),
                        text(&project.name).size(12).color(DIM),
                        text(&entry.config.name).size(13).color(TEXT),
                        iced::widget::space::horizontal(),
                    ]
                    .spacing(9)
                    .align_y(iced::Alignment::Center),
                )
                .width(Length::Fill)
                .padding([6, 12])
                .style(theme::process_row(accent, row_i == self.palette_index))
                .on_press(Event::PaletteSelect {
                    project: *project_id,
                    index: *index,
                }),
            );
        }

        let card = container(
            column![
                text_input("jump to a process\u{2026}", &self.palette_query)
                    .id(self.palette_input.clone())
                    .on_input(Event::PaletteInput)
                    .on_submit(Event::PaletteSubmit)
                    .style(theme::input(LOCAL_ACCENT))
                    .padding([8, 14])
                    .size(14),
                list,
            ]
            .spacing(10)
            .width(560),
        )
        .padding(12)
        .style(theme::form_card);

        container(card)
            .center_x(Length::Fill)
            .padding(iced::Padding {
                top: 90.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| iced::widget::container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.0, 0.0, 0.0, 0.4,
                ))),
                ..Default::default()
            })
            .into()
    }

    fn view_sidebar(&'_ self) -> Element<'_, Event> {
        let mut col = column![].spacing(10).padding([12, 10]);
        for (pidx, project) in self.projects.iter().enumerate() {
            col = col.push(self.view_project_block(pidx, project));
        }

        let add = button(
            text("+ project")
                .size(12)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .padding([6, 10])
        .style(theme::pill_button(LOCAL_ACCENT))
        .on_press(Event::OpenAddProject);

        container(column![
            scrollable(col).height(Length::Fill).width(Length::Fill),
            container(add).padding([8, 10]),
        ])
        .width(268)
        .height(Length::Fill)
        .style(theme::ground)
        .into()
    }

    /// One floating project card; the active one is lit by its accent
    /// gradient.
    fn view_project_block<'a>(
        &'a self,
        pidx: usize,
        project: &'a ProjectState,
    ) -> Element<'a, Event> {
        let remote = project.location.is_remote();
        let accent = accent_for(remote);
        let active = pidx == self.active;

        // 26px initials square.
        let initials: String = project
            .name
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(2)
            .collect::<String>()
            .to_uppercase();
        let icon = container(text(initials).size(9).font(bold()))
            .center_x(26)
            .center_y(26)
            .style(theme::icon_square(accent, remote));

        let counter = format!("{}/{}", project.running(), project.entries.len());
        let header = row![
            button(
                row![
                    icon,
                    text(&project.name).size(13).font(bold()).color(TEXT),
                    iced::widget::space::horizontal(),
                    container(text(counter).size(10))
                        .padding([2, 8])
                        .style(theme::pill),
                ]
                .spacing(9)
                .align_y(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .padding([2, 4])
            .style(theme::project_row(accent))
            .on_press(Event::ToggleExpanded(project.id)),
            button(text("\u{00d7}").size(11))
                .padding([3, 7])
                .style(theme::ghost(CRASHED))
                .on_press(Event::CloseProject(project.id)),
        ]
        .spacing(2)
        .align_y(iced::Alignment::Center);

        let mut block = column![header].spacing(2);

        if project.expanded {
            match &project.phase {
                Phase::Loading => {
                    block = block.push(
                        container(text("connecting\u{2026}").size(11).color(DIM)).padding([3, 10]),
                    );
                }
                Phase::Failed(_, retryable) => {
                    let mut r = row![text("unreachable").size(11).color(CRASHED)].spacing(8);
                    if *retryable {
                        r = r.push(
                            button(text("retry").size(10))
                                .padding([1, 8])
                                .style(theme::pill_button(accent))
                                .on_press(Event::RetryProbe(project.id)),
                        );
                    }
                    block = block.push(container(r).padding([3, 10]));
                }
                Phase::Ready => {
                    for category in [
                        ProcessCategory::Agent,
                        ProcessCategory::Command,
                        ProcessCategory::SSH,
                        ProcessCategory::Terminal,
                    ] {
                        let members: Vec<usize> = (0..project.entries.len())
                            .filter(|&i| project.entries[i].config.category == category)
                            .collect();
                        if members.is_empty() {
                            continue;
                        }
                        for i in members {
                            block = block.push(self.view_row(pidx, i));
                        }
                        block = block.push(container(column![]).height(3));
                    }
                    block = block.push(
                        container(
                            button(text("+ terminal").size(10))
                                .padding([2, 9])
                                .style(theme::pill_button(accent))
                                .on_press(Event::AddTerminal(project.id)),
                        )
                        .padding([1, 4]),
                    );
                }
            }
        }

        container(block)
            .width(Length::Fill)
            .padding(8)
            .style(theme::project_card(accent, active))
            .into()
    }

    fn view_row(&'_ self, pidx: usize, index: usize) -> Element<'_, Event> {
        let project = &self.projects[pidx];
        let remote = project.location.is_remote();
        let accent = accent_for(remote);
        let entry = &project.entries[index];

        let (dot_color, dot) = match entry.status {
            Status::Running => (accent, "\u{25cf}"),
            Status::Stopped => (STOPPED, "\u{25cf}"),
            Status::Crashed(_) => (CRASHED, "\u{25cf}"),
            Status::Restarting(_) | Status::Reconnecting(_) => (RESTARTING, "\u{25cf}"),
        };
        let name = entry
            .config
            .display_name
            .as_deref()
            .unwrap_or(&entry.config.name);

        let mut content = row![
            text(dot).size(10).color(dot_color),
            text(name).size(12.5),
            iced::widget::space::horizontal(),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        if let Some(port) = project.ports.get_port(&entry.config.name) {
            let local = project.port_map.get(&port).copied().unwrap_or(port);
            content = content.push(
                container(text(local.to_string()).size(10))
                    .padding([1, 7])
                    .style(theme::pill),
            );
        }
        match entry.status {
            Status::Restarting(attempt) => {
                content = content.push(
                    text(format!(
                        "retry {attempt}/{}",
                        processes::MAX_RESTART_ATTEMPTS
                    ))
                    .size(9)
                    .color(RESTARTING),
                );
            }
            Status::Reconnecting(attempt) => {
                content = content.push(
                    text(format!("reconnect {attempt}"))
                        .size(9)
                        .color(RESTARTING),
                );
            }
            _ => {}
        }

        let selected = pidx == self.active && index == project.selected;
        button(content)
            .width(Length::Fill)
            .padding([5, 9])
            .style(theme::process_row(accent, selected))
            .on_press(Event::SelectProcess {
                project: project.id,
                index,
            })
            .into()
    }

    fn view_main(&'_ self) -> Element<'_, Event> {
        if let Some(form) = &self.add_command {
            return self.view_add_command(form);
        }
        if let Some(input) = &self.add_project {
            return view_add_project(input);
        }

        let Some(project) = self.active_project() else {
            return container(
                column![
                    text("no projects yet").size(14).color(DIM),
                    button(text("+ add project").size(13))
                        .padding([7, 16])
                        .style(theme::primary(LOCAL_ACCENT))
                        .on_press(Event::OpenAddProject),
                ]
                .spacing(14)
                .align_x(iced::Alignment::Center),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(theme::terminal_pane)
            .into();
        };
        let remote = project.location.is_remote();
        let accent = accent_for(remote);

        match &project.phase {
            Phase::Loading => {
                return container(
                    text(format!("connecting to {}\u{2026}", project.key()))
                        .size(14)
                        .color(DIM),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(theme::terminal_pane)
                .into();
            }
            Phase::Failed(message, retryable) => {
                let mut col = column![text(message).size(14).color(CRASHED)]
                    .spacing(14)
                    .align_x(iced::Alignment::Center);
                if *retryable {
                    col = col.push(
                        button(text("\u{27f3} retry").size(12))
                            .padding([6, 14])
                            .style(theme::primary(accent))
                            .on_press(Event::RetryProbe(project.id)),
                    );
                }
                return container(col)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(theme::terminal_pane)
                    .into();
            }
            Phase::Ready => {}
        }

        let Some(entry) = project.entries.get(project.selected) else {
            return container(
                column![
                    text("no processes").size(14).color(DIM),
                    row![
                        button(text("+ command").size(12))
                            .padding([6, 14])
                            .style(theme::primary(accent))
                            .on_press(Event::OpenAddCommand { agent: false }),
                        button(text("+ agent").size(12))
                            .padding([6, 14])
                            .style(theme::primary(accent))
                            .on_press(Event::OpenAddCommand { agent: true }),
                    ]
                    .spacing(8),
                ]
                .spacing(14)
                .align_x(iced::Alignment::Center),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(theme::terminal_pane)
            .into();
        };

        let (status_color, status_word) = match &entry.status {
            Status::Running => (accent, String::from("\u{25cf} running")),
            Status::Stopped => (STOPPED, String::from("\u{25cf} stopped")),
            Status::Crashed(Some(code)) => {
                (CRASHED, format!("\u{25cf} crashed \u{00b7} exit {code}"))
            }
            Status::Crashed(None) => (CRASHED, String::from("\u{25cf} crashed")),
            Status::Restarting(n) => (
                RESTARTING,
                format!(
                    "\u{25cf} restarting {n}/{}",
                    processes::MAX_RESTART_ATTEMPTS
                ),
            ),
            Status::Reconnecting(n) => (RESTARTING, format!("\u{25cf} reconnecting \u{00b7} {n}")),
        };
        let mut controls = row![
            text(&entry.config.name).size(13.5).font(bold()).color(TEXT),
            container(text(status_word).size(10.5))
                .padding([3, 10])
                .style(theme::status_pill(status_color)),
            iced::widget::space::horizontal(),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        controls = controls
            .push(
                button(text("\u{270e} edit").size(11.5))
                    .padding([4, 12])
                    .style(theme::pill_button(accent))
                    .on_press(Event::OpenEditProcess),
            )
            .push(
                button(text("+ command").size(11.5))
                    .padding([4, 12])
                    .style(theme::pill_button(accent))
                    .on_press(Event::OpenAddCommand { agent: false }),
            )
            .push(
                button(text("+ agent").size(11.5))
                    .padding([4, 12])
                    .style(theme::pill_button(accent))
                    .on_press(Event::OpenAddCommand { agent: true }),
            );
        match entry.status {
            Status::Running => {
                controls = controls
                    .push(
                        button(text("\u{27f3} restart").size(11.5))
                            .padding([4, 12])
                            .style(theme::pill_button(accent))
                            .on_press(Event::Restart {
                                project: project.id,
                                index: project.selected,
                            }),
                    )
                    .push(
                        button(text("\u{25a0} stop").size(11.5))
                            .padding([4, 12])
                            .style(theme::pill_intent(accent, CRASHED))
                            .on_press(Event::Stop {
                                project: project.id,
                                index: project.selected,
                            }),
                    );
            }
            Status::Restarting(_) | Status::Reconnecting(_) => {
                controls = controls.push(
                    button(text("\u{25a0} cancel").size(11.5))
                        .padding([4, 12])
                        .style(theme::pill_intent(accent, CRASHED))
                        .on_press(Event::Stop {
                            project: project.id,
                            index: project.selected,
                        }),
                );
            }
            Status::Stopped | Status::Crashed(_) => {
                controls = controls.push(
                    button(text("\u{25b6} start").size(11.5))
                        .padding([4, 12])
                        .style(theme::primary(accent))
                        .on_press(Event::Start {
                            project: project.id,
                            index: project.selected,
                        }),
                );
            }
        }

        let body: Element<'_, Event> = match &entry.terminal {
            Some(term) => container(TerminalView::show(term).map(Event::Terminal))
                .padding(iced::Padding {
                    top: 6.0,
                    right: 2.0,
                    bottom: 4.0,
                    left: 8.0,
                })
                .style(theme::terminal_pane)
                .into(),
            None => {
                let label = match entry.status {
                    Status::Crashed(Some(code)) => {
                        format!("crashed with exit {code} \u{2014} start again when ready")
                    }
                    Status::Crashed(None) => {
                        String::from("crashed \u{2014} start again when ready")
                    }
                    Status::Restarting(attempt) => format!(
                        "restarting \u{00b7} attempt {attempt}/{}",
                        processes::MAX_RESTART_ATTEMPTS
                    ),
                    Status::Reconnecting(attempt) => format!(
                        "connection lost \u{2014} reconnecting (attempt {attempt}). \
                         The process keeps running on the host."
                    ),
                    _ => String::from("not running"),
                };
                container(text(label).size(13).color(DIM))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(theme::terminal_pane)
                    .into()
            }
        };

        let mut col = column![
            container(controls)
                .padding([7, 12])
                .width(Length::Fill)
                .style(theme::chrome),
            hline(),
        ];

        if self.search_open {
            let hint: Element<'_, Event> = match self.search_hit {
                Some(false) => text("no match").size(11).color(CRASHED).into(),
                _ => text("").size(11).into(),
            };
            col = col
                .push(
                    container(
                        row![
                            text_input("search scrollback (regex)\u{2026}", &self.search_query)
                                .id(self.search_input.clone())
                                .on_input(Event::SearchQueryChanged)
                                .on_submit(Event::SearchStep(SearchDirection::Left))
                                .style(theme::input(accent))
                                .padding([5, 12])
                                .size(12.5),
                            hint,
                            button(text("\u{25b4}").size(12))
                                .padding([3, 9])
                                .style(theme::pill_button(accent))
                                .on_press(Event::SearchStep(SearchDirection::Left)),
                            button(text("\u{25be}").size(12))
                                .padding([3, 9])
                                .style(theme::pill_button(accent))
                                .on_press(Event::SearchStep(SearchDirection::Right)),
                            button(text("\u{00d7}").size(11))
                                .padding([3, 8])
                                .style(theme::ghost(CRASHED))
                                .on_press(Event::SearchClose),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding([6, 10])
                    .style(theme::chrome),
                )
                .push(hline());
        }

        col = col.push(container(body).width(Length::Fill).height(Length::Fill));

        if entry.config.category == ProcessCategory::Agent && entry.terminal.is_some() {
            let placeholder = format!("message to {}\u{2026}", entry.config.name);
            col = col.push(hline()).push(
                container(
                    row![
                        text_input(&placeholder, &self.composer)
                            .on_input(Event::ComposerChanged)
                            .on_submit(Event::ComposerSend)
                            .style(theme::input(accent))
                            .padding([7, 14])
                            .size(13),
                        button(text("send").size(12).font(bold()))
                            .padding([7, 16])
                            .style(theme::primary(accent))
                            .on_press(Event::ComposerSend),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                )
                .padding([8, 10])
                .style(theme::chrome),
            );
        }

        col.into()
    }

    fn view_add_command<'a>(&'a self, form: &'a ProcessForm) -> Element<'a, Event> {
        let accent = self
            .active_project()
            .map(|p| accent_for(p.location.is_remote()))
            .unwrap_or(LOCAL_ACCENT);
        let editing = form.editing.is_some();
        let title = match (editing, form.agent) {
            (true, _) => "edit process",
            (false, true) => "add agent",
            (false, false) => "add command",
        };

        let name_row: Element<'_, Event> = if editing {
            row![
                text(&form.name).size(15).font(bold()),
                text(match form.original_category {
                    ProcessCategory::Agent => "agent",
                    ProcessCategory::Command => "command",
                    ProcessCategory::Terminal => "terminal",
                    ProcessCategory::SSH => "ssh",
                })
                .size(11)
                .color(DIM),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            text_input("name \u{2014} e.g. web", &form.name)
                .on_input(Event::AddCommandName)
                .style(theme::input(accent))
                .padding([8, 14])
                .size(13)
                .into()
        };

        let mut col = column![
            text(title).size(16).font(bold()),
            name_row,
            text_input("command \u{2014} e.g. npm run dev", &form.command)
                .on_input(Event::AddCommandCommand)
                .on_submit(Event::AddCommandSubmit)
                .style(theme::input(accent))
                .padding([8, 14])
                .size(13),
            iced::widget::checkbox(form.start_with_project)
                .label("start with project")
                .on_toggle(Event::FormToggleStartWith)
                .size(16)
                .text_size(12.5),
            iced::widget::checkbox(form.auto_restart)
                .label("restart on crash")
                .on_toggle(Event::FormToggleAutoRestart)
                .size(16)
                .text_size(12.5),
            iced::widget::checkbox(form.open_in_browser)
                .label("open in browser when a port appears")
                .on_toggle(Event::FormToggleOpenBrowser)
                .size(16)
                .text_size(12.5),
        ]
        .spacing(14);

        let mut buttons = row![
            button(
                text(if editing { "save" } else { "add & start" })
                    .size(12)
                    .font(bold()),
            )
            .padding([7, 16])
            .style(theme::primary(accent))
            .on_press(Event::AddCommandSubmit),
            button(text("cancel").size(12))
                .padding([7, 16])
                .style(theme::pill_button(accent))
                .on_press(Event::AddCommandCancel),
        ]
        .spacing(8);
        if editing {
            buttons = buttons.push(iced::widget::space::horizontal()).push(
                button(text("delete process").size(12))
                    .padding([7, 16])
                    .style(theme::pill_intent(accent, CRASHED))
                    .on_press(Event::DeleteProcess),
            );
        }
        col = col.push(buttons);

        form_card(col)
    }

    fn view_status_bar(&'_ self) -> Element<'_, Event> {
        let (left, badge, accent) = match self.active_project() {
            Some(project) => {
                let left = format!(
                    "{} \u{2014} {}/{} running",
                    project.name,
                    project.running(),
                    project.entries.len()
                );
                let badge = project
                    .entries
                    .get(project.selected)
                    .and_then(|e| display_badge(project, &e.config.name))
                    .unwrap_or_default();
                (left, badge, accent_for(project.location.is_remote()))
            }
            None => (String::from("no projects"), String::new(), LOCAL_ACCENT),
        };

        let mut bar = row![
            text(left).size(11).color(TEXT_SECONDARY),
            iced::widget::space::horizontal(),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        if !badge.is_empty() {
            bar = bar.push(
                button(
                    row![
                        text("\u{25cf}").size(9).color(accent),
                        text(badge).size(11),
                        text("\u{2197}").size(10).color(DIM),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center),
                )
                .padding([3, 10])
                .style(theme::pill_button(accent))
                .on_press(Event::OpenBadge),
            );
        }

        container(bar)
            .padding([5, 12])
            .width(Length::Fill)
            .style(theme::chrome)
            .into()
    }

    fn subscription(&self) -> Subscription<Event> {
        let subs: Vec<_> = self
            .projects
            .iter()
            .flat_map(|p| p.entries.iter())
            .filter_map(|e| e.terminal.as_ref())
            .map(|t| t.subscription())
            .collect();
        Subscription::batch([
            Subscription::batch(subs).map(Event::Terminal),
            // Ignored-status keys only — anything a focused widget consumed
            // never reaches the hotkeys.
            iced::keyboard::listen().map(Event::Hotkey),
        ])
    }
}

/// RGBA8 (what arboard hands over) → PNG bytes.
fn encode_png(image: &arboard::ImageData) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width as u32, image.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&image.bytes)
            .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn view_add_project(input: &str) -> Element<'_, Event> {
    form_card(
        column![
            text("add project").size(16).font(bold()),
            text_input("/path/to/project  or  ssh://host/path", input)
                .on_input(Event::AddProjectInput)
                .on_submit(Event::AddProjectSubmit)
                .style(theme::input(LOCAL_ACCENT))
                .padding([8, 14])
                .size(13),
            row![
                button(text("open").size(12).font(bold()))
                    .padding([7, 16])
                    .style(theme::primary(LOCAL_ACCENT))
                    .on_press(Event::AddProjectSubmit),
                button(text("cancel").size(12))
                    .padding([7, 16])
                    .style(theme::pill_button(LOCAL_ACCENT))
                    .on_press(Event::AddProjectCancel),
            ]
            .spacing(8),
        ]
        .spacing(14),
    )
}

/// Centered elevated card on the terminal surface.
fn form_card(content: iced::widget::Column<'_, Event>) -> Element<'_, Event> {
    container(
        container(content.width(420))
            .padding(24)
            .style(theme::form_card),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .style(theme::terminal_pane)
    .into()
}

/// Canonical project key from user input: ssh URLs pass through, local
/// paths canonicalize (relative ones against the cwd).
fn normalize_key(input: &str) -> String {
    match ProjectLocation::parse(input) {
        ProjectLocation::Local(p) => ProjectLocation::Local(p.canonicalize().unwrap_or(p)).key(),
        remote => remote.key(),
    }
}

/// The port/URL to show for a process: on remote projects, mapped through
/// the tunnels (the terminal shows the host's port; locally that port is
/// the forward's — possibly remapped).
fn display_badge(project: &ProjectState, name: &str) -> Option<String> {
    let port = project.ports.get_port(name)?;
    let local = project.port_map.get(&port).copied().unwrap_or(port);
    match project.ports.get_url(name) {
        Some(url) => Some(remap_url_port(url, port, local)),
        None => Some(format!("port {local}")),
    }
}

/// A full URL for the browser, tunnel-mapped on remote projects.
fn browser_url(project: &ProjectState, name: &str) -> Option<String> {
    let port = project.ports.get_port(name)?;
    let local = project.port_map.get(&port).copied().unwrap_or(port);
    match project.ports.get_url(name) {
        Some(url) => Some(remap_url_port(url, port, local)),
        None => Some(format!("http://localhost:{local}")),
    }
}

/// Displayed grid as trimmed lines — the detector's input, like VTE's
/// `text_range_format` feed in the GTK app.
fn visible_text(term: &iced_term::Terminal) -> String {
    let content = term.backend().renderable_content();
    let mut lines: Vec<String> = Vec::new();
    let mut current_line: Option<Line> = None;
    for indexed in &content.cells {
        if current_line != Some(indexed.point.line) {
            current_line = Some(indexed.point.line);
            lines.push(String::new());
        }
        lines.last_mut().unwrap().push(indexed.c);
    }
    lines
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn bold() -> iced::Font {
    iced::Font {
        weight: iced::font::Weight::Bold,
        ..iced::Font::DEFAULT
    }
}

/// 1px hairline (style.css alpha(@borders, .3)) — horizontal.
fn hline() -> Element<'static, Event> {
    container(column![])
        .width(Length::Fill)
        .height(1)
        .style(theme::hairline)
        .into()
}

/// 1px hairline — vertical.
fn vline() -> Element<'static, Event> {
    container(column![])
        .width(1)
        .height(Length::Fill)
        .style(theme::hairline)
        .into()
}
