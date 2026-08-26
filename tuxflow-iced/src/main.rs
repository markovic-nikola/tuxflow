//! TuxFlow's iced shell — migration M2: remote projects.
//!
//! `tuxflow-iced [path | ssh://host/dir]`. Remote projects probe over ssh
//! on a worker (config or detection via SshFs), spawn their processes
//! inside host-side tmux sessions through core's wrap_remote_command
//! (connection loss and app quit only detach), reattach live sessions at
//! startup, treat ssh's exit 255 as "reconnecting" rather than a crash,
//! and auto-tunnel every port the output scanner finds.

mod processes;

use std::collections::HashMap;
use std::time::Instant;

use alacritty_terminal::event::Event as AEvent;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::ClipboardType;
use iced::widget::{button, column, container, row, text};
use iced::{Color, Element, Length, Size, Subscription, Task};
use iced_term::{BackendCommand, TerminalView};
use tuxflow_core::config::schema::{ProcessCategory, ProcessConfig};
use tuxflow_core::remote::probe::ProbeError;
use tuxflow_core::remote::tunnel::TunnelManager;
use tuxflow_core::remote::{self, ProjectLocation};
use tuxflow_core::util::port_detector::{PortDetector, remap_url_port};

use processes::{ProcessEntry, Status, plan_after_exit};

fn main() -> iced::Result {
    // VTE set TERM for its children silently; on this stack it is the
    // embedder's job (spike finding — top/less break without it).
    alacritty_terminal::tty::setup_env();

    iced::application(App::new, App::update, App::view)
        .title(|app: &App| format!("TuxFlow — {}", app.project_name))
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

struct App {
    location: ProjectLocation,
    project_name: String,
    phase: Phase,
    entries: Vec<ProcessEntry>,
    selected: usize,
    ports: PortDetector,
    /// Remote projects: ssh -L forwards for every port the scanner sees,
    /// and the remote→local mapping for display (remaps on collision).
    tunnels: Option<TunnelManager>,
    port_map: HashMap<u16, u16>,
    next_term_id: u64,
    terminals_created: usize,
}

#[derive(Debug, Clone)]
enum Event {
    Terminal(iced_term::Event),
    /// Worker finished the remote probe: (project name if configured,
    /// process configs, live tmux sessions) or (message, retryable).
    Probed(Result<(Option<String>, Vec<ProcessConfig>, Vec<String>), (String, bool)>),
    RetryProbe,
    Select(usize),
    Start(usize),
    Stop(usize),
    Restart(usize),
    AddTerminal,
    /// Backoff timer fired; the generation guards against timers scheduled
    /// before a manual action superseded them.
    RestartDue {
        index: usize,
        generation: u64,
    },
}

impl App {
    fn new() -> (Self, Task<Event>) {
        let location = match std::env::args().nth(1) {
            Some(arg) => {
                let loc = ProjectLocation::parse(&arg);
                match loc {
                    // Relative paths resolve against the launch cwd.
                    ProjectLocation::Local(p) => {
                        ProjectLocation::Local(p.canonicalize().unwrap_or(p))
                    }
                    remote => remote,
                }
            }
            None => ProjectLocation::Local(std::env::current_dir().unwrap_or_default()),
        };

        let mut app = App {
            project_name: location.base_name(),
            tunnels: location.host().map(TunnelManager::new),
            location,
            phase: Phase::Loading,
            entries: Vec::new(),
            selected: 0,
            ports: PortDetector::new(),
            port_map: HashMap::new(),
            next_term_id: 0,
            terminals_created: 0,
        };

        let boot = match app.location.clone() {
            ProjectLocation::Local(dir) => {
                let (name, entries) = processes::load_local_project(&dir);
                app.project_name = name;
                app.entries = entries;
                app.phase = Phase::Ready;
                app.boot_processes(&[])
            }
            ProjectLocation::Ssh { .. } => app.probe_task(),
        };

        (app, boot)
    }

    /// Kick the blocking ssh probe onto a worker; the UI shows Loading.
    fn probe_task(&mut self) -> Task<Event> {
        self.phase = Phase::Loading;
        let (Some(host), dir) = (
            self.location.host().map(String::from),
            self.location.dir_str(),
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
            |joined| {
                Event::Probed(
                    joined.unwrap_or_else(|e| Err((format!("probe worker died: {e}"), false))),
                )
            },
        )
    }

    /// Start what should be up after load: sessions still alive on the host
    /// (reattach — the UI must never show "stopped" for a running detached
    /// process) and start_with_project ones.
    fn boot_processes(&mut self, live_sessions: &[String]) -> Task<Event> {
        let key = self.location.key();
        let mut tasks = Vec::new();
        for i in 0..self.entries.len() {
            let name = self.entries[i].config.name.clone();
            let live = live_sessions.contains(&remote::remote_session_name(&key, &name));
            if live || self.entries[i].config.start_with_project {
                tasks.push(self.start(i));
            }
        }
        if self.entries.is_empty() {
            tasks.push(self.add_terminal());
        }
        Task::batch(tasks)
    }

    /// Spawn (or respawn/reattach) a process's terminal. Fresh terminal id
    /// each time: subscription identity must change or iced keeps the dead
    /// stream.
    fn start(&mut self, index: usize) -> Task<Event> {
        let id = self.next_term_id;
        self.next_term_id += 1;

        let settings = processes::spawn_settings(&self.location, &mut self.entries[index]);
        let entry = &mut self.entries[index];
        match iced_term::Terminal::new(id, settings) {
            Ok(term) => {
                let focus = TerminalView::focus(term.widget_id().clone());
                entry.terminal = Some(term);
                entry.term_id = Some(id);
                entry.status = Status::Running;
                entry.last_exit = None;
                entry.stopping = false;
                entry.started_at = Some(Instant::now());
                self.ports.clear(&entry.config.name);
                self.selected = index;
                focus
            }
            Err(err) => {
                log::error!("failed to spawn {}: {err}", entry.config.name);
                entry.status = Status::Crashed(None);
                Task::none()
            }
        }
    }

    /// Manual start: forgives past failures and cancels pending timers.
    fn start_fresh(&mut self, index: usize) -> Task<Event> {
        let entry = &mut self.entries[index];
        entry.restart_attempts = 0;
        entry.restart_generation += 1;
        self.start(index)
    }

    /// Stop. Remote: explicitly kill the host-side session first (the local
    /// PTY teardown only detaches it), fire-and-forget, and make the next
    /// spawn clear any survivor instead of reattaching. Local: dropping the
    /// terminal makes alacritty's Pty::drop SIGHUP + reap the child on the
    /// PTY thread — never blocking the UI.
    fn stop(&mut self, index: usize) {
        if let Some(host) = self.location.host() {
            let entry = &mut self.entries[index];
            if entry.config.category != ProcessCategory::SSH {
                let session = entry.remote_session.take();
                if let Some(pidfile) = entry.remote_pidfile.take() {
                    remote::remote_kill(host, &pidfile, session.as_deref());
                    entry.remote_fresh_next = true;
                }
            }
        }
        let entry = &mut self.entries[index];
        entry.stopping = true;
        entry.restart_generation += 1;
        entry.restart_attempts = 0;
        entry.terminal = None;
        entry.term_id = None;
        entry.status = Status::Stopped;
        self.maybe_drop_tunnels();
    }

    /// Forwards live only while something runs — the next run rediscovers
    /// its ports instead of inheriting stale forwards (GTK behavior).
    fn maybe_drop_tunnels(&mut self) {
        if self.entries.iter().any(|e| e.is_running()) {
            return;
        }
        if let Some(tunnels) = &mut self.tunnels {
            tunnels.close_all();
        }
        self.port_map.clear();
    }

    fn add_terminal(&mut self) -> Task<Event> {
        self.terminals_created += 1;
        let config = ProcessConfig {
            name: format!("terminal {}", self.terminals_created),
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
        self.entries.push(ProcessEntry::new(config));
        self.start(self.entries.len() - 1)
    }

    /// A terminal's run ended (Exit event) — classify it and schedule what
    /// the policy asks for (restart with backoff, endless reconnect, or
    /// nothing).
    fn finalize_exit(&mut self, index: usize) -> Task<Event> {
        let connection_loss = self.location.is_remote()
            && self.entries[index].config.category != ProcessCategory::SSH
            && self.entries[index].last_exit == Some(255);

        let entry = &mut self.entries[index];
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
                    index,
                    generation,
                })
            }
            None => Task::none(),
        };
        self.maybe_drop_tunnels();
        task
    }

    fn entry_index_for_term(&self, term_id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.term_id == Some(term_id))
    }

    /// Feed the scanner (remote output arrives hard-wrapped at pane width —
    /// scan_output_wrapped rejoins it) and keep a forward alive for every
    /// local port it has seen.
    fn rescan_ports(&mut self, index: usize) {
        let Some(term) = self.entries[index].terminal.as_ref() else {
            return;
        };
        let name = self.entries[index].config.name.clone();
        let dump = visible_text(term);
        if self.location.is_remote() {
            let cols = term.backend().renderable_content().terminal_size.columns();
            self.ports.scan_output_wrapped(&name, &dump, cols);
        } else {
            self.ports.scan_output(&name, &dump);
        }

        if let Some(tunnels) = &mut self.tunnels {
            for port in self.ports.all_local_ports(&name) {
                if let Some(local) = tunnels.ensure(port) {
                    self.port_map.insert(port, local);
                }
            }
        }
    }

    /// The port/URL to show for a process: on remote projects, mapped
    /// through the tunnels (the terminal shows the host's port; locally
    /// that port is the forward's — possibly remapped).
    fn display_badge(&self, name: &str) -> Option<String> {
        let port = self.ports.get_port(name)?;
        let local = self.port_map.get(&port).copied().unwrap_or(port);
        match self.ports.get_url(name) {
            Some(url) => Some(remap_url_port(url, port, local)),
            None => Some(format!("port {local}")),
        }
    }

    fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Probed(Ok((name, configs, live_sessions))) => {
                self.project_name = name.unwrap_or_else(|| self.location.base_name());
                self.entries = processes::entries_from(configs);
                self.phase = Phase::Ready;
                self.boot_processes(&live_sessions)
            }
            Event::Probed(Err((message, retryable))) => {
                self.phase = Phase::Failed(message, retryable);
                Task::none()
            }
            Event::RetryProbe => self.probe_task(),
            Event::Select(index) => {
                self.selected = index;
                match self.entries.get(index).and_then(|e| e.terminal.as_ref()) {
                    Some(term) => TerminalView::focus(term.widget_id().clone()),
                    None => Task::none(),
                }
            }
            Event::Start(index) | Event::Restart(index) => {
                if self.entries[index].terminal.is_some() {
                    self.stop(index);
                }
                self.start_fresh(index)
            }
            Event::Stop(index) => {
                self.stop(index);
                Task::none()
            }
            Event::AddTerminal => self.add_terminal(),
            Event::RestartDue { index, generation } => {
                let due = self.entries.get(index).is_some_and(|e| {
                    e.restart_generation == generation
                        && matches!(e.status, Status::Restarting(_) | Status::Reconnecting(_))
                });
                if due { self.start(index) } else { Task::none() }
            }
            Event::Terminal(iced_term::Event::BackendCall(term_id, cmd)) => {
                let Some(index) = self.entry_index_for_term(term_id) else {
                    return Task::none();
                };

                let mut side_task = Task::none();
                let mut rescan = false;
                if let BackendCommand::ProcessAlacrittyEvent(ev) = &cmd {
                    match ev {
                        AEvent::Wakeup => rescan = true,
                        AEvent::ChildExit(code) => {
                            self.entries[index].last_exit = Some(*code);
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
                    let entry = &mut self.entries[index];
                    match entry.terminal.as_mut() {
                        Some(term) => term.handle(iced_term::Command::ProxyToBackend(cmd)),
                        None => iced_term::actions::Action::Ignore,
                    }
                };

                let action_task = match action {
                    iced_term::actions::Action::Shutdown => self.finalize_exit(index),
                    iced_term::actions::Action::PublishSelection(text) => {
                        iced::clipboard::write_primary(text)
                    }
                    _ => Task::none(),
                };

                if rescan {
                    self.rescan_ports(index);
                }

                Task::batch([side_task, action_task])
            }
        }
    }

    fn view(&'_ self) -> Element<'_, Event> {
        match &self.phase {
            Phase::Loading => container(
                text(format!("connecting to {}…", self.location.key()))
                    .size(14)
                    .color(DIM),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into(),
            Phase::Failed(message, retryable) => {
                let mut col = column![
                    text(message)
                        .size(14)
                        .color(Color::from_rgb(0.87, 0.32, 0.32)),
                ]
                .spacing(12)
                .align_x(iced::Alignment::Center);
                if *retryable {
                    col = col.push(action_button("⟳ retry", Event::RetryProbe));
                }
                container(col)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
            }
            Phase::Ready => {
                let sidebar = self.view_sidebar();
                let main = self.view_main();
                let status_bar = self.view_status_bar();

                column![
                    row![
                        container(sidebar).width(240).height(Length::Fill),
                        container(main).width(Length::Fill).height(Length::Fill),
                    ]
                    .spacing(2),
                    status_bar,
                ]
                .into()
            }
        }
    }

    fn view_sidebar(&'_ self) -> Element<'_, Event> {
        let mut header_row = row![
            text(&self.project_name).size(15),
            iced::widget::space::horizontal(),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        if self.location.is_remote() {
            // The remote accent hue — where this project lives.
            header_row = header_row.push(
                text("remote")
                    .size(10)
                    .color(Color::from_rgb(1.0, 0.81, 0.36)),
            );
        }
        let mut col = column![header_row].spacing(4).padding(8);

        for (label, category) in [
            ("AGENTS", ProcessCategory::Agent),
            ("COMMANDS", ProcessCategory::Command),
            ("TERMINALS", ProcessCategory::Terminal),
        ] {
            let members: Vec<usize> = (0..self.entries.len())
                .filter(|&i| self.entries[i].config.category == category)
                .collect();
            if members.is_empty() && category != ProcessCategory::Terminal {
                continue;
            }

            let mut header = row![text(label).size(11).color(DIM)].spacing(6);
            if category == ProcessCategory::Terminal {
                header = header.push(
                    button(text("+").size(11))
                        .padding([0, 6])
                        .style(button::text)
                        .on_press(Event::AddTerminal),
                );
            }
            col = col.push(container(header).padding([6, 2]));

            for i in members {
                col = col.push(self.view_row(i));
            }
        }

        container(col)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.09, 0.09, 0.11))),
                ..Default::default()
            })
            .height(Length::Fill)
            .into()
    }

    fn view_row(&'_ self, index: usize) -> Element<'_, Event> {
        let entry = &self.entries[index];
        let (dot_color, dot) = match entry.status {
            Status::Running => (Color::from_rgb(0.30, 0.78, 0.40), "●"),
            Status::Stopped => (DIM, "○"),
            Status::Crashed(_) => (Color::from_rgb(0.87, 0.32, 0.32), "●"),
            Status::Restarting(_) | Status::Reconnecting(_) => {
                (Color::from_rgb(0.92, 0.72, 0.25), "●")
            }
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

        if let Some(port) = self.ports.get_port(&entry.config.name) {
            let local = self.port_map.get(&port).copied().unwrap_or(port);
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

        let style = if index == self.selected {
            button::secondary
        } else {
            button::text
        };
        button(content)
            .width(Length::Fill)
            .padding([4, 8])
            .style(style)
            .on_press(Event::Select(index))
            .into()
    }

    fn view_main(&'_ self) -> Element<'_, Event> {
        let Some(entry) = self.entries.get(self.selected) else {
            return container(text("no processes")).padding(16).into();
        };

        let mut controls = row![text(&entry.config.name).size(14)]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        controls = controls.push(iced::widget::space::horizontal());
        match entry.status {
            Status::Running => {
                controls = controls
                    .push(action_button("⟳ restart", Event::Restart(self.selected)))
                    .push(action_button("■ stop", Event::Stop(self.selected)));
            }
            Status::Restarting(_) | Status::Reconnecting(_) => {
                controls = controls.push(action_button("■ cancel", Event::Stop(self.selected)));
            }
            Status::Stopped | Status::Crashed(_) => {
                controls = controls.push(action_button("▶ start", Event::Start(self.selected)));
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

        column![
            container(controls).padding([4, 8]),
            container(body).width(Length::Fill).height(Length::Fill),
        ]
        .into()
    }

    fn view_status_bar(&'_ self) -> Element<'_, Event> {
        let running = self.entries.iter().filter(|e| e.is_running()).count();
        let left = format!(
            "{} — {running}/{} running",
            self.project_name,
            self.entries.len()
        );
        let badge = self
            .entries
            .get(self.selected)
            .and_then(|e| self.display_badge(&e.config.name))
            .map(|b| format!("● {b}"))
            .unwrap_or_default();

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
            .entries
            .iter()
            .filter_map(|e| e.terminal.as_ref())
            .map(|t| t.subscription())
            .collect();
        Subscription::batch(subs).map(Event::Terminal)
    }
}

const DIM: Color = Color::from_rgb(0.55, 0.55, 0.58);

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
