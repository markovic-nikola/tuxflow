//! TuxFlow's iced shell — migration M4: the multi-project workspace.
//!
//! `tuxflow-iced [path | ssh://host/dir]…`. Projects come from
//! `~/.config/tuxflow/projects.toml` (plus any CLI args, which persist),
//! each with its own process list (config or detection, overlaid with the
//! user's custom commands/deletions/order — same policy as the GTK app),
//! ports, tunnels and poll cadence. Add project / add command / add agent
//! run as inline forms; closing a project detaches its remote sessions.

mod processes;
mod theme;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event as AEvent;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::ClipboardType;
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Color, Element, Length, Size, Subscription, Task};
use iced_term::{BackendCommand, TerminalView};
use tuxflow_core::config::projects::SavedProjects;
use tuxflow_core::config::schema::{ProcessCategory, ProcessConfig};
use tuxflow_core::remote::probe::ProbeError;
use tuxflow_core::remote::tunnel::TunnelManager;
use tuxflow_core::remote::{self, ProjectLocation};
use tuxflow_core::util::port_detector::{PortDetector, remap_url_port, rewrite_clicked_url};

use processes::{ProcessEntry, Status, plan_after_exit};
use theme::{CRASHED, DIM, LOCAL_ACCENT, REMOTE_ACCENT, RUNNING, SIDEBAR_BG, WORKING};

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

struct AddCommandForm {
    name: String,
    command: String,
    agent: bool,
}

struct App {
    projects: Vec<ProjectState>,
    /// Index of the project owning the main pane.
    active: usize,
    saved: SavedProjects,
    composer: String,
    add_project: Option<String>,
    add_command: Option<AddCommandForm>,
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
    OpenAddProject,
    AddProjectInput(String),
    AddProjectSubmit,
    AddProjectCancel,
    OpenAddCommand {
        agent: bool,
    },
    AddCommandName(String),
    AddCommandCommand(String),
    AddCommandSubmit,
    AddCommandCancel,
}

impl App {
    fn new() -> (Self, Task<Event>) {
        let mut app = App {
            projects: Vec::new(),
            active: 0,
            saved: SavedProjects::load(),
            composer: String::new(),
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

        let project = &mut self.projects[pidx];
        let settings = processes::spawn_settings(&project.location, &mut project.entries[index]);
        let entry = &mut project.entries[index];
        match iced_term::Terminal::new(id, settings) {
            Ok(term) => {
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
                    self.add_command = Some(AddCommandForm {
                        name: String::new(),
                        command: String::new(),
                        agent,
                    });
                }
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
                    start_with_project: false,
                    auto_restart: false,
                    open_in_browser: false,
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
                self.saved.save();
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
        let sidebar = self.view_sidebar();
        let main = self.view_main();
        let status_bar = self.view_status_bar();

        column![
            row![
                container(sidebar).width(260).height(Length::Fill),
                container(main).width(Length::Fill).height(Length::Fill),
            ]
            .spacing(2),
            status_bar,
        ]
        .into()
    }

    fn view_sidebar(&'_ self) -> Element<'_, Event> {
        let mut col = column![].spacing(2).padding(8);

        for (pidx, project) in self.projects.iter().enumerate() {
            col = col.push(self.view_project_header(pidx, project));
            if project.expanded {
                match &project.phase {
                    Phase::Loading => {
                        col = col.push(
                            container(text("connecting…").size(11).color(DIM)).padding([2, 18]),
                        );
                    }
                    Phase::Failed(_, retryable) => {
                        let mut r = row![text("unreachable").size(11).color(CRASHED)].spacing(6);
                        if *retryable {
                            r = r.push(
                                button(text("retry").size(10))
                                    .padding([0, 6])
                                    .style(button::text)
                                    .on_press(Event::RetryProbe(project.id)),
                            );
                        }
                        col = col.push(container(r).padding([2, 18]));
                    }
                    Phase::Ready => {
                        for (label, category) in [
                            ("AGENTS", ProcessCategory::Agent),
                            ("COMMANDS", ProcessCategory::Command),
                            ("TERMINALS", ProcessCategory::Terminal),
                        ] {
                            let members: Vec<usize> = (0..project.entries.len())
                                .filter(|&i| project.entries[i].config.category == category)
                                .collect();
                            if members.is_empty() && category != ProcessCategory::Terminal {
                                continue;
                            }
                            let mut header = row![text(label).size(10).color(DIM)].spacing(6);
                            if category == ProcessCategory::Terminal {
                                header = header.push(
                                    button(text("+").size(10))
                                        .padding([0, 5])
                                        .style(button::text)
                                        .on_press(Event::AddTerminal(project.id)),
                                );
                            }
                            col = col.push(container(header).padding([3, 14]));
                            for i in members {
                                col = col.push(self.view_row(pidx, i));
                            }
                        }
                    }
                }
            }
        }

        col = col.push(iced::widget::space::vertical());
        col = col.push(
            button(text("+ project").size(12))
                .width(Length::Fill)
                .style(button::text)
                .on_press(Event::OpenAddProject),
        );

        container(col)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(SIDEBAR_BG)),
                ..Default::default()
            })
            .height(Length::Fill)
            .into()
    }

    fn view_project_header<'a>(
        &'a self,
        pidx: usize,
        project: &'a ProjectState,
    ) -> Element<'a, Event> {
        // The accent hue says where the project lives (ui/accent.rs).
        let accent = if project.location.is_remote() {
            REMOTE_ACCENT
        } else {
            LOCAL_ACCENT
        };
        let chevron = if project.expanded { "▾" } else { "▸" };
        let counter = format!("{}/{}", project.running(), project.entries.len());

        let name_style = if pidx == self.active {
            Color::WHITE
        } else {
            Color::from_rgb(0.8, 0.8, 0.82)
        };

        row![
            button(
                row![
                    text(chevron).size(11).color(DIM),
                    text("●").size(11).color(accent),
                    text(&project.name).size(14).color(name_style),
                    iced::widget::space::horizontal(),
                    text(counter).size(10).color(DIM),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .padding([4, 6])
            .style(button::text)
            .on_press(Event::ToggleExpanded(project.id)),
            button(text("✕").size(10).color(DIM))
                .padding([2, 4])
                .style(button::text)
                .on_press(Event::CloseProject(project.id)),
        ]
        .align_y(iced::Alignment::Center)
        .into()
    }

    fn view_row(&'_ self, pidx: usize, index: usize) -> Element<'_, Event> {
        let project = &self.projects[pidx];
        let entry = &project.entries[index];
        let (dot_color, dot) = match entry.status {
            Status::Running => (RUNNING, "●"),
            Status::Stopped => (DIM, "○"),
            Status::Crashed(_) => (CRASHED, "●"),
            Status::Restarting(_) | Status::Reconnecting(_) => (WORKING, "●"),
        };
        let name = entry
            .config
            .display_name
            .as_deref()
            .unwrap_or(&entry.config.name);

        let mut content = row![
            text(dot).size(12).color(dot_color),
            text(name).size(13),
            iced::widget::space::horizontal(),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        if let Some(port) = project.ports.get_port(&entry.config.name) {
            let local = project.port_map.get(&port).copied().unwrap_or(port);
            content = content.push(text(local.to_string()).size(11).color(DIM));
        }
        match entry.status {
            Status::Restarting(attempt) => {
                content = content.push(
                    text(format!(
                        "retry {attempt}/{}",
                        processes::MAX_RESTART_ATTEMPTS
                    ))
                    .size(10)
                    .color(DIM),
                );
            }
            Status::Reconnecting(attempt) => {
                content = content.push(text(format!("reconnect {attempt}")).size(10).color(DIM));
            }
            _ => {}
        }

        let selected = pidx == self.active && index == project.selected;
        let style = if selected {
            button::secondary
        } else {
            button::text
        };
        container(
            button(content)
                .width(Length::Fill)
                .padding([3, 8])
                .style(style)
                .on_press(Event::SelectProcess {
                    project: project.id,
                    index,
                }),
        )
        .padding([0, 10])
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
                    text("no projects").size(14).color(DIM),
                    button(text("+ add project").size(13)).on_press(Event::OpenAddProject),
                ]
                .spacing(12)
                .align_x(iced::Alignment::Center),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
        };

        match &project.phase {
            Phase::Loading => {
                return container(
                    text(format!("connecting to {}…", project.key()))
                        .size(14)
                        .color(DIM),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
            }
            Phase::Failed(message, retryable) => {
                let mut col = column![text(message).size(14).color(CRASHED)]
                    .spacing(12)
                    .align_x(iced::Alignment::Center);
                if *retryable {
                    col = col.push(action_button("⟳ retry", Event::RetryProbe(project.id)));
                }
                return container(col)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into();
            }
            Phase::Ready => {}
        }

        let Some(entry) = project.entries.get(project.selected) else {
            return container(
                column![
                    text("no processes").size(14).color(DIM),
                    row![
                        action_button("+ command", Event::OpenAddCommand { agent: false }),
                        action_button("+ agent", Event::OpenAddCommand { agent: true }),
                    ]
                    .spacing(8),
                ]
                .spacing(12)
                .align_x(iced::Alignment::Center),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
        };

        let mut controls = row![text(&entry.config.name).size(14)]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        controls = controls.push(iced::widget::space::horizontal());
        controls = controls
            .push(action_button(
                "+ command",
                Event::OpenAddCommand { agent: false },
            ))
            .push(action_button(
                "+ agent",
                Event::OpenAddCommand { agent: true },
            ));
        match entry.status {
            Status::Running => {
                controls = controls
                    .push(action_button(
                        "⟳ restart",
                        Event::Restart {
                            project: project.id,
                            index: project.selected,
                        },
                    ))
                    .push(action_button(
                        "■ stop",
                        Event::Stop {
                            project: project.id,
                            index: project.selected,
                        },
                    ));
            }
            Status::Restarting(_) | Status::Reconnecting(_) => {
                controls = controls.push(action_button(
                    "■ cancel",
                    Event::Stop {
                        project: project.id,
                        index: project.selected,
                    },
                ));
            }
            Status::Stopped | Status::Crashed(_) => {
                controls = controls.push(action_button(
                    "▶ start",
                    Event::Start {
                        project: project.id,
                        index: project.selected,
                    },
                ));
            }
        }

        let body: Element<'_, Event> = match &entry.terminal {
            Some(term) => TerminalView::show(term).map(Event::Terminal),
            None => {
                let label = match entry.status {
                    Status::Crashed(Some(code)) => {
                        format!("crashed (exit {code}) — ▶ to start again")
                    }
                    Status::Crashed(None) => String::from("crashed — ▶ to start again"),
                    Status::Restarting(attempt) => format!(
                        "restarting (attempt {attempt}/{})…",
                        processes::MAX_RESTART_ATTEMPTS
                    ),
                    Status::Reconnecting(attempt) => format!(
                        "connection lost — reconnecting (attempt {attempt})… \
                         the process keeps running on the host"
                    ),
                    _ => String::from("stopped — ▶ to start"),
                };
                container(text(label).size(14).color(DIM))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
            }
        };

        let mut col = column![
            container(controls).padding([4, 8]),
            container(body).width(Length::Fill).height(Length::Fill),
        ];

        // The composer under agent terminals (GTK composer_bar parity).
        if entry.config.category == ProcessCategory::Agent && entry.terminal.is_some() {
            col = col.push(
                container(
                    row![
                        text_input("message to agent — Enter sends", &self.composer)
                            .on_input(Event::ComposerChanged)
                            .on_submit(Event::ComposerSend)
                            .size(13),
                        action_button("send", Event::ComposerSend),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center),
                )
                .padding([4, 8]),
            );
        }

        col.into()
    }

    fn view_add_command(&'_ self, form: &'_ AddCommandForm) -> Element<'_, Event> {
        let title = if form.agent {
            "add agent"
        } else {
            "add command"
        };
        container(
            column![
                text(title).size(16),
                text_input("name (e.g. web)", &form.name)
                    .on_input(Event::AddCommandName)
                    .size(14)
                    .width(420),
                text_input("command (e.g. npm run dev)", &form.command)
                    .on_input(Event::AddCommandCommand)
                    .on_submit(Event::AddCommandSubmit)
                    .size(14)
                    .width(420),
                row![
                    action_button("add & start", Event::AddCommandSubmit),
                    action_button("cancel", Event::AddCommandCancel),
                ]
                .spacing(8),
            ]
            .spacing(12)
            .align_x(iced::Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    }

    fn view_status_bar(&'_ self) -> Element<'_, Event> {
        let (left, badge) = match self.active_project() {
            Some(project) => {
                let left = format!(
                    "{} — {}/{} running",
                    project.name,
                    project.running(),
                    project.entries.len()
                );
                let badge = project
                    .entries
                    .get(project.selected)
                    .and_then(|e| display_badge(project, &e.config.name))
                    .map(|b| format!("● {b}"))
                    .unwrap_or_default();
                (left, badge)
            }
            None => (String::from("no projects"), String::new()),
        };

        container(
            row![
                text(left).size(12),
                iced::widget::space::horizontal(),
                text(badge).size(12),
            ]
            .spacing(8),
        )
        .padding([4, 8])
        .width(Length::Fill)
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
        Subscription::batch(subs).map(Event::Terminal)
    }
}

fn view_add_project(input: &str) -> Element<'_, Event> {
    container(
        column![
            text("add project").size(16),
            text_input("/path/to/project  or  ssh://host/path", input)
                .on_input(Event::AddProjectInput)
                .on_submit(Event::AddProjectSubmit)
                .size(14)
                .width(460),
            row![
                action_button("open", Event::AddProjectSubmit),
                action_button("cancel", Event::AddProjectCancel),
            ]
            .spacing(8),
        ]
        .spacing(12)
        .align_x(iced::Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
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

fn action_button(label: &str, event: Event) -> button::Button<'_, Event> {
    button(text(label).size(12))
        .padding([2, 8])
        .style(button::secondary)
        .on_press(event)
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
