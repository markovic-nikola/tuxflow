//! TuxFlow's iced shell — migration M1: livable for local projects.
//!
//! Sidebar of processes (tuxflow.toml or stack detection) grouped by
//! category, each backed by an iced_term terminal on the alacritty
//! backend; start/stop/restart, auto-restart with the GTK app's backoff
//! policy, and per-process port badges from tuxflow-core's detector.

mod processes;

use std::path::PathBuf;
use std::time::Instant;

use alacritty_terminal::event::Event as AEvent;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::ClipboardType;
use iced::widget::{button, column, container, row, text};
use iced::{Color, Element, Length, Size, Subscription, Task};
use iced_term::{BackendCommand, TerminalView};
use tuxflow_core::config::schema::{ProcessCategory, ProcessConfig};
use tuxflow_core::util::port_detector::PortDetector;

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

struct App {
    project_name: String,
    project_dir: PathBuf,
    entries: Vec<ProcessEntry>,
    selected: usize,
    ports: PortDetector,
    next_term_id: u64,
    terminals_created: usize,
}

#[derive(Debug, Clone)]
enum Event {
    Terminal(iced_term::Event),
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
        let project_dir = std::env::current_dir().unwrap_or_default();
        let (project_name, entries) = processes::load_project(&project_dir);

        let mut app = App {
            project_name,
            project_dir,
            entries,
            selected: 0,
            ports: PortDetector::new(),
            next_term_id: 0,
            terminals_created: 0,
        };

        // start_with_project processes come up on launch (GTK parity).
        let mut tasks = Vec::new();
        for i in 0..app.entries.len() {
            if app.entries[i].config.start_with_project {
                tasks.push(app.start(i));
            }
        }
        // An empty project still gets a terminal to live in.
        if app.entries.is_empty() {
            tasks.push(app.add_terminal());
        }

        (app, Task::batch(tasks))
    }

    /// Spawn (or respawn) a process's terminal. Fresh terminal id each time:
    /// subscription identity must change or iced keeps the dead stream.
    fn start(&mut self, index: usize) -> Task<Event> {
        let id = self.next_term_id;
        self.next_term_id += 1;

        let entry = &mut self.entries[index];
        let settings = processes::backend_settings(&entry.config, &self.project_dir);
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

    /// Stop = drop the terminal. Backend teardown sends the PTY loop a
    /// shutdown; alacritty's Pty::drop SIGHUPs and reaps the child on that
    /// thread — never blocking the UI.
    fn stop(&mut self, index: usize) {
        let entry = &mut self.entries[index];
        entry.stopping = true;
        entry.restart_generation += 1;
        entry.restart_attempts = 0;
        entry.terminal = None;
        entry.term_id = None;
        entry.status = Status::Stopped;
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

    /// A terminal's run ended (Exit event) — classify it and schedule the
    /// restart the policy asks for.
    fn finalize_exit(&mut self, index: usize) -> Task<Event> {
        let entry = &mut self.entries[index];
        entry.terminal = None;
        entry.term_id = None;

        let run = entry.started_at.map(|t| t.elapsed());
        let (status, attempts, delay) = plan_after_exit(
            entry.config.auto_restart,
            entry.stopping,
            entry.last_exit,
            run,
            entry.restart_attempts,
        );
        entry.status = status;
        entry.restart_attempts = attempts;
        entry.stopping = false;

        match delay {
            Some(delay) => {
                let index_owned = index;
                let generation = entry.restart_generation;
                Task::perform(tokio::time::sleep(delay), move |_| Event::RestartDue {
                    index: index_owned,
                    generation,
                })
            }
            None => Task::none(),
        }
    }

    fn entry_index_for_term(&self, term_id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.term_id == Some(term_id))
    }

    fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Select(index) => {
                self.selected = index;
                match self.entries.get(index).and_then(|e| e.terminal.as_ref()) {
                    Some(term) => TerminalView::focus(term.widget_id().clone()),
                    None => Task::none(),
                }
            }
            Event::Start(index) | Event::Restart(index) => {
                // Restart of a running process: drop first, then respawn —
                // the dropped run's exit never arrives (its stream died),
                // so no crash bookkeeping fires for it.
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
                let live = self
                    .entries
                    .get(index)
                    .is_some_and(|e| e.restart_generation == generation);
                if live && matches!(self.entries[index].status, Status::Restarting(_)) {
                    self.start(index)
                } else {
                    Task::none()
                }
            }
            Event::Terminal(iced_term::Event::BackendCall(term_id, cmd)) => {
                let Some(index) = self.entry_index_for_term(term_id) else {
                    return Task::none();
                };

                let mut side_task = Task::none();
                let mut rescan_ports = false;
                if let BackendCommand::ProcessAlacrittyEvent(ev) = &cmd {
                    match ev {
                        AEvent::Wakeup => rescan_ports = true,
                        AEvent::ChildExit(code) => {
                            self.entries[index].last_exit = Some(*code);
                        }
                        AEvent::ClipboardStore(ty, data) if !data.is_empty() => {
                            // OSC 52; empty clears are ignored (multiplex
                            // emits them in the wild).
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

                if rescan_ports {
                    if let Some(term) = self.entries[index].terminal.as_ref() {
                        let name = self.entries[index].config.name.clone();
                        self.ports.scan_output(&name, &visible_text(term));
                    }
                }

                Task::batch([side_task, action_task])
            }
        }
    }

    fn view(&'_ self) -> Element<'_, Event> {
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

    fn view_sidebar(&'_ self) -> Element<'_, Event> {
        let mut col = column![text(&self.project_name).size(15)]
            .spacing(4)
            .padding(8);

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
            Status::Restarting(_) => (Color::from_rgb(0.92, 0.72, 0.25), "●"),
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
            content = content.push(text(port.to_string()).size(11).color(DIM));
        }
        if let Status::Restarting(attempt) = entry.status {
            content = content.push(
                text(format!(
                    "retry {attempt}/{}",
                    processes::MAX_RESTART_ATTEMPTS
                ))
                .size(10)
                .color(DIM),
            );
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
            Status::Restarting(_) => {
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
            .and_then(|e| {
                self.ports
                    .get_url(&e.config.name)
                    .map(String::from)
                    .or_else(|| {
                        self.ports
                            .get_port(&e.config.name)
                            .map(|p| format!("port {p}"))
                    })
            })
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
