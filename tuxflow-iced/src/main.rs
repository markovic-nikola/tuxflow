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
mod settings_ui;
mod theme;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event as AEvent;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::ClipboardType;
use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Element, Length, Size, Subscription, Task};
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
use tuxflow_core::config::settings::AppSettings;

/// Ports-poll cadence: fast while a run is settling (a new forward just
/// opened), backed off once nothing new appears (GTK behavior).
const POLL_FAST: Duration = Duration::from_secs(2);
const POLL_SLOW: Duration = Duration::from_secs(30);
/// Provisional badges get this long to firm up before auto-open fires.
const AUTO_OPEN_GRACE: Duration = Duration::from_secs(5);

/// GTK sidebar parity: AdwOverlaySplitView sizes the sidebar at a quarter
/// of the window, clamped to the GTK app's min/max (window.rs: 220–400).
const SIDEBAR_FRACTION: f32 = 0.25;
const SIDEBAR_MIN: f32 = 220.0;
const SIDEBAR_MAX: f32 = 400.0;

fn main() -> iced::Result {
    env_logger::init();
    // VTE set TERM for its children silently; on this stack it is the
    // embedder's job (spike finding — top/less break without it).
    alacritty_terminal::tty::setup_env();

    // GTK parity: reopen with the last session's geometry, saved on close.
    // Position is X11 — Wayland ignores Specific placement and only honors
    // size and maximized. A saved position is passed even when maximized so
    // the window maximizes on the monitor it was closed on.
    let window = tuxflow_core::config::settings::AppSettings::load().window;
    iced::application(App::new, App::update, App::view)
        .theme(|_: &App| iced::Theme::Dark)
        .title(|app: &App| match app.active_project() {
            Some(p) => format!("TuxFlow — {}", p.name),
            None => String::from("TuxFlow"),
        })
        .window(iced::window::Settings {
            size: Size {
                width: window.width.max(1) as f32,
                height: window.height.max(1) as f32,
            },
            maximized: window.maximized,
            position: match (window.x, window.y) {
                (Some(x), Some(y)) => {
                    iced::window::Position::Specific(iced::Point::new(x as f32, y as f32))
                }
                _ => iced::window::Position::Default,
            },
            ..Default::default()
        })
        .exit_on_close_request(false)
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
    git: Option<tuxflow_core::remote::git::GitStatus>,
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
    working_dir: String,
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
    /// The shared settings.toml — this shell's single authority. The
    /// settings view mutates it and saves immediately on every change,
    /// like the GTK dialog's per-row save points.
    settings: AppSettings,
    /// Settings view state; `Some` = the main pane shows settings.
    settings_ui: Option<settings_ui::State>,
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
    /// Live inner size (resize events); the sidebar takes a fraction of
    /// the width (GTK parity). The *position* is never tracked from Moved
    /// events — those carry the client-area point while restore sets the
    /// frame origin, so a save/restore cycle through them drifts the
    /// window by the WM's frame extents. Saves query
    /// `window::position()` (winit outer_position) instead.
    window_size: Size,
    /// Bumped on every move/resize; the debounced save only fires for the
    /// newest generation, so a drag writes once, not per pixel.
    geometry_gen: u64,
    next_project_id: u64,
    next_term_id: u64,
}

/// Probe success: project name if configured, process configs, live tmux
/// sessions. Failure: message + whether it's worth retrying.
type ProbeResult = Result<(Option<String>, Vec<ProcessConfig>, Vec<String>), (String, bool)>;

#[derive(Debug, Clone)]
enum Event {
    Terminal(iced_term::Event),
    WindowResized(Size),
    /// A Moved event arrived — only ever a trigger for the debounced
    /// save, never a position source (see `window_size` field comment).
    WindowMoved,
    WindowCloseRequested(iced::window::Id),
    WindowClose {
        id: iced::window::Id,
        maximized: bool,
        position: Option<iced::Point>,
    },
    /// Debounced geometry persistence — `make dev-iced` kills the process
    /// on rebuild, so waiting for a clean close would lose every move.
    GeometrySettled(u64),
    SaveGeometry {
        maximized: bool,
        position: Option<iced::Point>,
    },
    /// Post-launch placement correction (GTK's restore_window_placement
    /// trick): measure where the frame actually landed and fix the delta.
    RestoreSettle,
    RestoreMeasured {
        id: iced::window::Id,
        actual: Option<iced::Point>,
    },
    OpenSettings,
    SettingsMsg(settings_ui::Msg),
    Probed {
        project: u64,
        result: ProbeResult,
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
    GitTick,
    GitPolled {
        project: u64,
        status: Option<tuxflow_core::remote::git::GitStatus>,
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
    FormWorkingDir(String),
    FormToggleStartWith(bool),
    FormToggleAutoRestart(bool),
    FormToggleOpenBrowser(bool),
    DeleteProcess,
    AddCommandSubmit,
    AddCommandCancel,
}

impl App {
    fn new() -> (Self, Task<Event>) {
        let settings = AppSettings::load();
        theme::set_accents(
            &settings.appearance.local_accent_color,
            &settings.appearance.remote_accent_color,
        );
        let saved = SavedProjects::load();
        // Insurance: keep a .bak of the last known-good (non-empty)
        // workspace before this process ever saves. One wipe was enough.
        if !saved.directories.is_empty()
            && let Some(dir) = dirs::config_dir()
        {
            let file = dir.join("tuxflow/projects.toml");
            let _ = std::fs::copy(&file, file.with_extension("toml.bak"));
        }
        let mut app = App {
            projects: Vec::new(),
            active: 0,
            saved,
            app_keys: AppKeys::from_settings(&settings.keybindings),
            settings_ui: None,
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
            window_size: Size {
                width: settings.window.width.max(1) as f32,
                height: settings.window.height.max(1) as f32,
            },
            geometry_gen: 0,
            next_project_id: 0,
            next_term_id: 0,
            settings,
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

        let mut keys: Vec<String> = if app.saved.directories.is_empty() {
            // Nothing saved and no args: live in the cwd, unpersisted.
            vec![ProjectLocation::Local(std::env::current_dir().unwrap_or_default()).key()]
        } else {
            app.saved.directories.clone()
        };
        // GTK sidebar parity: recently used projects first (stable for
        // never-used ones, which keep their saved order).
        if app.settings.sidebar.recent_first {
            let mut indexed: Vec<(usize, String)> = keys.into_iter().enumerate().collect();
            let saved = &app.saved;
            indexed.sort_by_key(|(i, k)| (std::cmp::Reverse(saved.get_last_used(k)), *i));
            keys = indexed.into_iter().map(|(_, k)| k).collect();
        }
        for key in keys {
            tasks.push(app.open_project(&key));
        }
        if std::env::var("TUXFLOW_ICED_UI").as_deref() == Ok("edit") {
            tasks.push(Task::done(Event::OpenEditProcess));
        }
        if std::env::var("TUXFLOW_ICED_UI").as_deref() == Ok("settings") {
            tasks.push(Task::done(Event::OpenSettings));
        }
        // Placement correction after the WM settles (see RestoreSettle).
        if app.settings.window.x.is_some() && !app.settings.window.maximized {
            tasks.push(Task::perform(
                tokio::time::sleep(Duration::from_millis(300)),
                |_| Event::RestoreSettle,
            ));
        }
        tasks.push(Task::done(Event::GitTick));

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
            git: None,
            location,
        };

        match project.location.clone() {
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
        }
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
        let font = self.term_font();
        let scrollback = self.settings.appearance.scrollback_lines as usize;
        let palette = theme::terminal_palette(&self.settings.appearance.terminal_theme);
        let project = &mut self.projects[pidx];
        let settings = processes::spawn_settings(
            &project.location,
            &mut project.entries[index],
            font,
            scrollback,
            palette,
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
                notify::crash(&self.settings.notifications, &project_name, &name, *code)
            }
            Status::Restarting(attempt) => {
                notify::auto_restart(&self.settings.notifications, &project_name, &name, *attempt)
            }
            Status::Reconnecting(_) if !entry.outage_notified => {
                entry.outage_notified = true;
                notify::disconnect(&project_name, &name);
            }
            Status::Stopped if !stopping_was => {
                notify::finish(&self.settings.notifications, &project_name, &name)
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

    /// Poll git for the ACTIVE project on a worker — on switch and on
    /// the 20 s tick. One project at a time; 24 parallel ssh polls would
    /// be rude to the mux.
    fn poll_git(&self) -> Task<Event> {
        let Some(project) = self.active_project() else {
            return Task::none();
        };
        if !matches!(project.phase, Phase::Ready) {
            return Task::none();
        }
        let id = project.id;
        let location = project.location.clone();
        Task::perform(
            tokio::task::spawn_blocking(move || tuxflow_core::remote::git::query_status(&location)),
            move |joined| Event::GitPolled {
                project: id,
                status: joined.ok().flatten(),
            },
        )
    }

    /// Move the selected process within its project and persist the order
    /// — the keyboard stand-in for GTK's sidebar drag-and-drop.
    fn move_selected(&mut self, delta: i32) -> Task<Event> {
        let Some(project) = self.projects.get_mut(self.active) else {
            return Task::none();
        };
        let from = project.selected;
        let to = from as i32 + delta;
        if to < 0 || to as usize >= project.entries.len() {
            return Task::none();
        }
        project.entries.swap(from, to as usize);
        project.selected = to as usize;
        let key = project.key();
        let order: Vec<String> = project
            .entries
            .iter()
            .map(|e| e.config.name.clone())
            .collect();
        self.saved.set_process_order(&key, order);
        Task::none()
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
            AppAction::MoveProcessUp => self.move_selected(-1),
            AppAction::MoveProcessDown => self.move_selected(1),
            AppAction::Settings => {
                self.settings_ui = Some(settings_ui::State::new(&self.settings));
                Task::none()
            }
            AppAction::SelectProcessN(n) => {
                if let Some(project) = self.projects.get_mut(self.active)
                    && (n as usize) <= project.entries.len()
                {
                    project.selected = n as usize - 1;
                    return self.focus_selected_terminal();
                }
                Task::none()
            }
            AppAction::SelectProjectN(n) => {
                if (n as usize) <= self.projects.len() {
                    self.active = n as usize - 1;
                    return self.focus_selected_terminal();
                }
                Task::none()
            }
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
        Task::batch([self.focus_selected_terminal(), self.poll_git()])
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

    /// The terminal font per current settings. `Font::with_name` wants
    /// `&'static str`; family changes are rare, so leaking one small
    /// string per change is the accepted iced idiom.
    fn term_font(&self) -> iced_term::settings::FontSettings {
        let a = &self.settings.appearance;
        let family = if a.font_family.is_empty() || a.font_family == "Monospace" {
            iced::font::Family::Monospace
        } else {
            iced::font::Family::Name(Box::leak(a.font_family.clone().into_boxed_str()))
        };
        let weight = match a.font_weight {
            0..=149 => iced::font::Weight::Thin,
            150..=249 => iced::font::Weight::ExtraLight,
            250..=349 => iced::font::Weight::Light,
            350..=449 => iced::font::Weight::Normal,
            450..=549 => iced::font::Weight::Medium,
            550..=649 => iced::font::Weight::Semibold,
            650..=749 => iced::font::Weight::Bold,
            750..=849 => iced::font::Weight::ExtraBold,
            _ => iced::font::Weight::Black,
        };
        iced_term::settings::FontSettings {
            size: (a.font_size as f32).clamp(6.0, 32.0),
            // GTK's line_height 1.0 is "normal"; iced_term's normal is a
            // 1.3 scale factor — map proportionally.
            scale_factor: ((a.line_height as f32) * 1.3).clamp(1.0, 2.6),
            font_type: iced::Font {
                family,
                weight,
                ..iced::Font::MONOSPACE
            },
        }
    }

    /// Push the current font settings into every live terminal.
    fn broadcast_font(&mut self) {
        let font = self.term_font();
        for project in &mut self.projects {
            for entry in &mut project.entries {
                if let Some(term) = entry.terminal.as_mut() {
                    term.handle(iced_term::Command::ChangeFont(font.clone()));
                }
            }
        }
    }

    /// Push the current terminal color scheme into every live terminal.
    fn broadcast_theme(&mut self) {
        let name = self.settings.appearance.terminal_theme.clone();
        for project in &mut self.projects {
            for entry in &mut project.entries {
                if let Some(term) = entry.terminal.as_mut() {
                    term.handle(iced_term::Command::ChangeTheme(Box::new(
                        theme::terminal_palette(&name),
                    )));
                }
            }
        }
    }

    /// Ctrl+= / Ctrl+- — persisted, so the size survives a relaunch (the
    /// GTK app saves it from its settings dialog the same way).
    fn change_font(&mut self, delta: f32) -> Task<Event> {
        let a = &mut self.settings.appearance;
        a.font_size = ((a.font_size as f32 + delta).clamp(8.0, 32.0)) as u32;
        self.settings.save();
        self.broadcast_font();
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
            Event::WindowResized(size) => {
                self.window_size = size;
                self.debounce_geometry_save()
            }
            Event::WindowMoved => self.debounce_geometry_save(),
            Event::GeometrySettled(generation) => {
                if generation != self.geometry_gen {
                    return Task::none();
                }
                iced::window::latest().and_then(|id| {
                    iced::window::is_maximized(id).then(move |maximized| {
                        iced::window::position(id).map(move |position| Event::SaveGeometry {
                            maximized,
                            position,
                        })
                    })
                })
            }
            Event::SaveGeometry {
                maximized,
                position,
            } => {
                self.save_window_state(maximized, position);
                Task::none()
            }
            // WMs disagree on whether a requested position applies to the
            // frame or to the client area (they differ by the decoration
            // extents), so a fixed convention drifts on half of them.
            // Measure where the frame actually landed and correct once by
            // the delta — the GTK shell does the same 200 ms after map.
            Event::RestoreSettle => iced::window::latest().and_then(|id| {
                iced::window::position(id).map(move |actual| Event::RestoreMeasured { id, actual })
            }),
            Event::RestoreMeasured { id, actual } => {
                let w = &self.settings.window;
                log::info!(
                    "restore measure: actual {actual:?} saved ({:?},{:?})",
                    w.x,
                    w.y
                );
                if let (Some(sx), Some(sy), Some(actual), false) = (w.x, w.y, actual, w.maximized) {
                    let (dx, dy) = (actual.x - sx as f32, actual.y - sy as f32);
                    if dx != 0.0 || dy != 0.0 {
                        log::info!("restore correction: delta ({dx},{dy})");
                        return iced::window::move_to(
                            id,
                            iced::Point::new(sx as f32 - dx, sy as f32 - dy),
                        );
                    }
                }
                Task::none()
            }
            Event::OpenSettings => {
                self.settings_ui = Some(settings_ui::State::new(&self.settings));
                Task::none()
            }
            Event::SettingsMsg(msg) => self.handle_settings(msg),
            Event::WindowCloseRequested(id) => {
                // Maximized and the frame position are queryable only, so
                // fetch both before saving.
                iced::window::is_maximized(id).then(move |maximized| {
                    iced::window::position(id).map(move |position| Event::WindowClose {
                        id,
                        maximized,
                        position,
                    })
                })
            }
            Event::WindowClose {
                id,
                maximized,
                position,
            } => {
                self.save_window_state(maximized, position);
                iced::window::close(id)
            }
            Event::Probed { project, result } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                match result {
                    Ok((name, configs, live_sessions)) => {
                        let key = self.projects[pidx].key();
                        if self.saved.get_name(&key).is_none()
                            && let Some(name) = name
                        {
                            self.projects[pidx].name = name;
                        }
                        let merged = processes::merge_saved(configs, &self.saved, &key);
                        self.projects[pidx].entries = processes::entries_from(merged);
                        self.projects[pidx].phase = Phase::Ready;
                        let boot = self.boot_processes(pidx, &live_sessions);
                        // The chip shouldn't wait for the next 20 s tick.
                        let git = if pidx == self.active {
                            self.poll_git()
                        } else {
                            Task::none()
                        };
                        Task::batch([boot, git])
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
                let switched = self.active != pidx;
                self.active = pidx;
                self.projects[pidx].selected = index;
                let focus = match self.projects[pidx]
                    .entries
                    .get(index)
                    .and_then(|e| e.terminal.as_ref())
                {
                    Some(term) => TerminalView::focus(term.widget_id().clone()),
                    None => Task::none(),
                };
                if switched {
                    Task::batch([focus, self.poll_git()])
                } else {
                    focus
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
                    // GTK parity: single-expand collapses the others when
                    // one opens.
                    if self.projects[pidx].expanded && self.settings.sidebar.single_project_expand {
                        for (i, p) in self.projects.iter_mut().enumerate() {
                            if i != pidx {
                                p.expanded = false;
                            }
                        }
                    }
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
            Event::GitTick => {
                let poll = self.poll_git();
                let next = Task::perform(tokio::time::sleep(Duration::from_secs(20)), |_| {
                    Event::GitTick
                });
                Task::batch([poll, next])
            }
            Event::GitPolled { project, status } => {
                if let Some(pidx) = self.project_index(project) {
                    self.projects[pidx].git = status;
                }
                Task::none()
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
                // Hotkey capture owns the keyboard while recording.
                if let Some(state) = &mut self.settings_ui
                    && let Some(action) = state.capturing
                {
                    return self.finish_capture(action, &key, modifiers);
                }
                // Esc closes settings like it closes the other panels.
                if self.settings_ui.is_some()
                    && matches!(
                        key.as_ref(),
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                    )
                {
                    self.settings_ui = None;
                    return Task::none();
                }
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
                if let Some(project) = self.active_project()
                    && let Some(entry) = project.entries.get(project.selected)
                    && let Some(url) = browser_url(project, &entry.config.name)
                {
                    log::info!("open badge {url}");
                    if let Err(e) = open::that(&url) {
                        log::warn!("open {url} failed: {e}");
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
                        working_dir: String::new(),
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
                if let Some(project) = self.active_project()
                    && let Some(entry) = project.entries.get(project.selected)
                {
                    self.add_command = Some(ProcessForm {
                        name: entry.config.name.clone(),
                        command: entry.config.command.clone(),
                        working_dir: entry.config.working_dir.clone().unwrap_or_default(),
                        agent: entry.config.category == ProcessCategory::Agent,
                        start_with_project: entry.config.start_with_project,
                        auto_restart: entry.config.auto_restart,
                        open_in_browser: entry.config.open_in_browser,
                        editing: Some((project.id, project.selected)),
                        original_category: entry.config.category.clone(),
                    });
                }
                Task::none()
            }
            Event::FormWorkingDir(v) => {
                if let Some(form) = &mut self.add_command {
                    form.working_dir = v;
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
                    let wd = form.working_dir.trim();
                    config.working_dir = if wd.is_empty() {
                        None
                    } else {
                        Some(wd.to_string())
                    };
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
                let wd = form.working_dir.trim();
                let config = ProcessConfig {
                    name,
                    command: command.to_string(),
                    working_dir: if wd.is_empty() {
                        None
                    } else {
                        Some(wd.to_string())
                    },
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
                    iced_term::actions::Action::ChangeTitle(title) => {
                        self.projects[pidx].entries[index].title = Some(title);
                        Task::none()
                    }
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

        // The GTK header bar's gear, relocated: this shell has no header.
        let gear = button(text("\u{2699}").size(14))
            .padding([4, 9])
            .style(theme::ghost(LOCAL_ACCENT))
            .on_press(Event::OpenSettings);

        container(column![
            scrollable(col)
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new().width(4).scroller_width(4),
                ))
                .style(theme::overlay_scrollbar)
                .height(Length::Fill)
                .width(Length::Fill),
            container(row![add, gear].spacing(6).align_y(iced::Alignment::Center)).padding([8, 10]),
        ])
        .width((self.window_size.width * SIDEBAR_FRACTION).clamp(SIDEBAR_MIN, SIDEBAR_MAX))
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
                    clipped_label(text(&project.name).size(13).font(bold()).color(TEXT)),
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
            clipped_label(text(name).size(12.5)),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        // Ctrl+1..9 hints (settings-gated) on the active project's rows —
        // the chords the digit switcher actually honors.
        if self.settings.sidebar.show_keybind_hints && pidx == self.active && index < 9 {
            content = content.push(text(format!("\u{2303}{}", index + 1)).size(9).color(DIM));
        }

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
        if let Some(state) = &self.settings_ui {
            return settings_ui::view(state, &self.settings).map(Event::SettingsMsg);
        }
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
        let title: String = entry
            .title
            .as_deref()
            .unwrap_or_default()
            .chars()
            .take(70)
            .collect();
        let mut controls = row![
            text(&entry.config.name).size(13.5).font(bold()).color(TEXT),
            container(text(status_word).size(10.5))
                .padding([3, 10])
                .style(theme::status_pill(status_color)),
            clipped_label(text(title).size(11).color(DIM)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        if display_badge(project, &entry.config.name).is_some() {
            controls = controls.push(
                button(text("\u{2197} open").size(11.5))
                    .padding([4, 12])
                    .style(theme::pill_button(accent))
                    .on_press(Event::OpenBadge),
            );
        }
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

        if entry.config.category == ProcessCategory::Agent
            && entry.terminal.is_some()
            && self.settings.tools.agent_composer
        {
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
            text_input(
                "working directory \u{2014} optional, defaults to the project",
                &form.working_dir,
            )
            .on_input(Event::FormWorkingDir)
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

        let mut bar = row![text(left).size(11).color(TEXT_SECONDARY)]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        if let Some(git) = self.active_project().and_then(|p| p.git.as_ref()) {
            let mut label = format!("\u{2387} {}", git.branch);
            if git.ahead > 0 {
                label.push_str(&format!(" \u{2191}{}", git.ahead));
            }
            if git.behind > 0 {
                label.push_str(&format!(" \u{2193}{}", git.behind));
            }
            if git.changed > 0 {
                label.push_str(&format!(" \u{00b1}{}", git.changed));
            }
            bar = bar.push(
                container(text(label).size(10.5))
                    .padding([2, 9])
                    .style(theme::pill),
            );
        }
        bar = bar.push(iced::widget::space::horizontal());
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

    /// Arm (or re-arm) the debounced geometry save: one write ~1 s after
    /// the last move/resize. Without it, only a clean close would save —
    /// and `make dev-iced` (cargo watch) kills the process on rebuild.
    fn debounce_geometry_save(&mut self) -> Task<Event> {
        self.geometry_gen += 1;
        let generation = self.geometry_gen;
        Task::perform(tokio::time::sleep(Duration::from_secs(1)), move |_| {
            Event::GeometrySettled(generation)
        })
    }

    /// A capture-mode keypress from the hotkeys page: bind it, or report
    /// the conflict, GTK-style.
    fn finish_capture(
        &mut self,
        action: tuxflow_core::config::keybindings::ShortcutAction,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Task<Event> {
        use iced::keyboard::Key;
        let Some(state) = &mut self.settings_ui else {
            return Task::none();
        };
        // Esc cancels; lone modifiers keep listening.
        if matches!(key.as_ref(), Key::Named(iced::keyboard::key::Named::Escape)) {
            state.capturing = None;
            return Task::none();
        }
        let Some(display) = keys::chord_string(key, modifiers) else {
            return Task::none();
        };
        // Conflict: some *other* action already holds this chord.
        let conflict = tuxflow_core::config::keybindings::action_metadata()
            .into_iter()
            .find(|(other, _, _)| {
                *other != action && self.settings.keybindings.get(*other) == display
            });
        if let Some((_, holder, _)) = conflict {
            state.capturing = None;
            state.conflict = Some((action, holder));
            return Task::none();
        }
        state.capturing = None;
        state.conflict = None;
        self.settings.keybindings.set(action, display);
        self.settings.save();
        self.rebuild_keys();
        Task::none()
    }

    /// New chords take effect now: rebuild the matcher and re-reserve in
    /// every live terminal (reservations are additive; a stale reservation
    /// maps to an app action that no longer matches, which is inert).
    fn rebuild_keys(&mut self) {
        self.app_keys = AppKeys::from_settings(&self.settings.keybindings);
        let reservations = self.app_keys.reservations();
        for project in &mut self.projects {
            for entry in &mut project.entries {
                if let Some(term) = entry.terminal.as_mut() {
                    term.handle(iced_term::Command::AddBindings(reservations.clone()));
                }
            }
        }
    }

    /// One settings change: mutate, save immediately (GTK-dialog manners),
    /// and apply live wherever this shell has the consumer.
    fn handle_settings(&mut self, msg: settings_ui::Msg) -> Task<Event> {
        use settings_ui::Msg;
        // Interacting clears stale capture feedback.
        if let Some(state) = &mut self.settings_ui
            && !matches!(msg, Msg::Capture(_))
        {
            state.conflict = None;
        }
        match msg {
            Msg::Close => {
                self.settings_ui = None;
                return self.focus_selected_terminal();
            }
            Msg::Page(page) => {
                if let Some(state) = &mut self.settings_ui {
                    state.page = page;
                    state.capturing = None;
                    state.copied = None;
                    state.sound_error = None;
                }
                return Task::none();
            }
            Msg::ColorScheme(label) => {
                self.settings.appearance.theme = label.to_lowercase();
            }
            Msg::AccentApp(label) => {
                self.settings.appearance.accent_color = accent_name_for_label(label);
            }
            Msg::AccentLocal(label) => {
                self.settings.appearance.local_accent_color = accent_name_for_label(label);
                self.apply_accents();
            }
            Msg::AccentRemote(label) => {
                self.settings.appearance.remote_accent_color = accent_name_for_label(label);
                self.apply_accents();
            }
            Msg::TermTheme(label) => {
                let idx = tuxflow_core::config::palette::theme_choices()
                    .iter()
                    .position(|l| *l == label)
                    .unwrap_or(0);
                self.settings.appearance.terminal_theme =
                    tuxflow_core::config::palette::theme_name(idx as u32).to_string();
                self.broadcast_theme();
            }
            Msg::FontFamilyDraft(value) => {
                if let Some(state) = &mut self.settings_ui {
                    state.font_family_draft = value;
                }
                return Task::none();
            }
            Msg::FontFamilyApply => {
                if let Some(state) = &self.settings_ui {
                    let family = state.font_family_draft.trim();
                    self.settings.appearance.font_family = if family.is_empty() {
                        String::from("Monospace")
                    } else {
                        family.to_string()
                    };
                }
                self.broadcast_font();
            }
            Msg::FontSize(v) => {
                self.settings.appearance.font_size = v;
                self.broadcast_font();
            }
            Msg::FontWeight(v) => {
                self.settings.appearance.font_weight = v;
                self.broadcast_font();
            }
            Msg::BoldWeight(v) => {
                self.settings.appearance.bold_font_weight = v;
            }
            Msg::LineHeight(v) => {
                self.settings.appearance.line_height = v;
                self.broadcast_font();
            }
            Msg::LetterSpacing(v) => {
                self.settings.appearance.letter_spacing = v;
            }
            Msg::Scrollback(v) => {
                self.settings.appearance.scrollback_lines = v;
            }
            Msg::SingleExpand(v) => {
                self.settings.sidebar.single_project_expand = v;
                if v {
                    self.collapse_all_but_active();
                }
            }
            Msg::AutoHide(v) => self.settings.sidebar.auto_hide_sidebar = v,
            Msg::KeybindHints(v) => self.settings.sidebar.show_keybind_hints = v,
            Msg::RecentFirst(v) => {
                self.settings.sidebar.recent_first = v;
                if v {
                    self.sort_projects_recent_first();
                }
            }
            Msg::NotifyCrash(v) => self.settings.notifications.on_crash = v,
            Msg::NotifyRestart(v) => self.settings.notifications.on_auto_restart = v,
            Msg::NotifyFileWatch(v) => self.settings.notifications.on_file_watch_restart = v,
            Msg::NotifyFinish(v) => self.settings.notifications.on_process_finish = v,
            Msg::NotifyAgentIdle(v) => self.settings.notifications.on_agent_idle = v,
            Msg::NotifySilenceFallback(v) => {
                self.settings.notifications.on_agent_idle_silence_fallback = v;
            }
            Msg::IdleThreshold(v) => self.settings.notifications.agent_idle_silence_seconds = v,
            Msg::SuppressFocused(v) => self.settings.notifications.suppress_when_focused = v,
            Msg::SoundEnabled(v) => self.settings.notifications.sound_enabled = v,
            Msg::Sound(label) => {
                if let Some(id) = sound_id_for_label(label) {
                    self.settings.notifications.sound_name = id;
                }
            }
            Msg::AgentSound(agent, label) => {
                let value = sound_id_for_label(label);
                match agent {
                    0 => self.settings.notifications.claude_sound_name = value,
                    1 => self.settings.notifications.codex_sound_name = value,
                    _ => self.settings.notifications.gemini_sound_name = value,
                }
            }
            Msg::TestNotification => {
                notify::test(&self.settings.notifications);
                return Task::none();
            }
            Msg::PreviewSound(agent) => {
                let n = &self.settings.notifications;
                let id = match agent {
                    Some(0) => n.claude_sound_name.clone(),
                    Some(1) => n.codex_sound_name.clone(),
                    Some(_) => n.gemini_sound_name.clone(),
                    None => None,
                }
                .unwrap_or_else(|| n.sound_name.clone());
                let result = tuxflow_core::util::sounds::play_sound(&id);
                if let Some(state) = &mut self.settings_ui {
                    state.sound_error = result.err();
                }
                return Task::none();
            }
            Msg::Capture(action) => {
                if let Some(state) = &mut self.settings_ui {
                    state.conflict = None;
                    state.capturing = Some(action);
                }
                return Task::none();
            }
            Msg::ResetKeys => {
                self.settings.keybindings =
                    tuxflow_core::config::keybindings::KeybindingsSettings::default();
                self.rebuild_keys();
            }
            Msg::Composer(v) => self.settings.tools.agent_composer = v,
            Msg::RemoteMic(v) => self.settings.tools.remote_microphone = v,
            Msg::Editor(label) => {
                if let Some((cmd, _)) = tuxflow_core::config::settings::EDITOR_CHOICES
                    .iter()
                    .find(|(_, l)| *l == label)
                {
                    self.settings.tools.default_editor = cmd.to_string();
                }
            }
            Msg::ReuseEditor(v) => self.settings.tools.reuse_editor_window = v,
            Msg::TerminalApp(label) => {
                if let Some((cmd, _)) = tuxflow_core::config::settings::TERMINAL_CHOICES
                    .iter()
                    .find(|(_, l)| *l == label)
                {
                    self.settings.tools.default_terminal = cmd.to_string();
                }
            }
            Msg::McpEnabled(v) => self.settings.integrations.mcp_enabled = v,
            Msg::ToggleSetup(idx) => {
                if let Some(state) = &mut self.settings_ui {
                    state.setup_open = if state.setup_open == Some(idx) {
                        None
                    } else {
                        Some(idx)
                    };
                    state.copied = None;
                }
                return Task::none();
            }
            Msg::CopySetup(tool, config) => {
                if let Some(state) = &mut self.settings_ui {
                    state.copied = Some(tool);
                }
                return iced::clipboard::write(config.to_string());
            }
            Msg::OpenSource => {
                if let Err(e) = open::that("https://github.com/markovic-nikola/tuxflow") {
                    log::warn!("open source url: {e}");
                }
                return Task::none();
            }
        }
        self.settings.save();
        Task::none()
    }

    fn apply_accents(&mut self) {
        theme::set_accents(
            &self.settings.appearance.local_accent_color,
            &self.settings.appearance.remote_accent_color,
        );
    }

    /// Single-expand just switched on: only the active project stays open.
    fn collapse_all_but_active(&mut self) {
        for (i, project) in self.projects.iter_mut().enumerate() {
            project.expanded = i == self.active;
        }
    }

    /// Live re-sort for the recent-first toggle (project ids keep timers
    /// safe — only the vec order changes).
    fn sort_projects_recent_first(&mut self) {
        let active_id = self.projects.get(self.active).map(|p| p.id);
        let saved = &self.saved;
        self.projects
            .sort_by_key(|p| std::cmp::Reverse(saved.get_last_used(&p.key())));
        if let Some(id) = active_id
            && let Some(idx) = self.projects.iter().position(|p| p.id == id)
        {
            self.active = idx;
        }
    }

    /// GTK-parity close save: reload from disk first (the GTK app may have
    /// saved while we ran — don't clobber its edits), then write only the
    /// window geometry. A maximized close keeps the last normal size and
    /// position on disk, so unmaximizing after relaunch restores them.
    fn save_window_state(&self, maximized: bool, position: Option<iced::Point>) {
        let mut settings = tuxflow_core::config::settings::AppSettings::load();
        settings.window.maximized = maximized;
        if !maximized {
            settings.window.width = self.window_size.width as i32;
            settings.window.height = self.window_size.height as i32;
            // `None` (Wayland: no positioning) keeps the values on disk.
            if let Some(pos) = position {
                settings.window.x = Some(pos.x as i32);
                settings.window.y = Some(pos.y as i32);
            }
        }
        settings.save();
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
            iced::window::resize_events().map(|(_, size)| Event::WindowResized(size)),
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Window(iced::window::Event::Moved(_)) => Some(Event::WindowMoved),
                _ => None,
            }),
            // exit_on_close_request(false): the close button routes through
            // update so geometry gets saved first (GTK parity).
            iced::window::close_requests().map(Event::WindowCloseRequested),
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

/// Palette label ("Green") back to its settings name ("green").
fn accent_name_for_label(label: &str) -> String {
    tuxflow_core::config::palette::ACCENT_COLORS
        .iter()
        .find(|c| c.label == label)
        .map(|c| c.name)
        .unwrap_or(tuxflow_core::config::palette::FALLBACK_LOCAL)
        .to_string()
}

/// Sound label ("Sound 3") back to its id ("sound3"); "(Use default)" and
/// unknown labels clear the override.
fn sound_id_for_label(label: &str) -> Option<String> {
    tuxflow_core::util::sounds::BUNDLED_SOUNDS
        .iter()
        .find(|b| b.label == label)
        .map(|b| b.id.to_string())
}

/// A row label that soaks up the slack: it takes the leftover width and
/// clips a too-long name at the edge, instead of wrapping or shoving the
/// chips to its right out of the row (text paints past its bounds unless a
/// clipping container cuts it).
fn clipped_label(label: iced::widget::Text<'_>) -> Element<'_, Event> {
    container(label.wrapping(text::Wrapping::None))
        .width(Length::Fill)
        .clip(true)
        .into()
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
