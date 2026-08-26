//! TuxFlow's iced shell — migration step M0: the three-crate wiring proof.
//!
//! One real terminal (the vendored iced_term fork) with tuxflow-core doing
//! what it does in the GTK app: the project's tuxflow.toml read through
//! `config::loader`, and `util::port_detector` scanning terminal output for
//! the port/URL badge. Start a dev server in the terminal and the badge
//! appears — the signature TuxFlow behavior, already alive on this stack.

use alacritty_terminal::event::Event as AEvent;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::ClipboardType;
use iced::widget::{column, container, row, text};
use iced::{Element, Length, Size, Subscription, Task};
use iced_term::{BackendCommand, TerminalView};
use tuxflow_core::config::loader;
use tuxflow_core::util::port_detector::PortDetector;

const TERM_ID: u64 = 0;
const PROCESS: &str = "shell";

fn main() -> iced::Result {
    // VTE set TERM for its children silently; on this stack it is the
    // embedder's job (spike finding — top/less break without it).
    alacritty_terminal::tty::setup_env();

    iced::application(App::new, App::update, App::view)
        .title(|app: &App| match &app.project {
            Some(name) => format!("TuxFlow — {name}"),
            None => String::from("TuxFlow"),
        })
        .window_size(Size {
            width: 1100.0,
            height: 700.0,
        })
        .subscription(App::subscription)
        .run()
}

struct App {
    term: iced_term::Terminal,
    project: Option<String>,
    processes: usize,
    ports: PortDetector,
}

#[derive(Debug, Clone)]
enum Event {
    Terminal(iced_term::Event),
}

impl App {
    fn new() -> (Self, Task<Event>) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let config = loader::find_config(&cwd).and_then(|p| loader::load_config(&p).ok());

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let term = iced_term::Terminal::new(
            TERM_ID,
            iced_term::settings::Settings {
                backend: iced_term::settings::BackendSettings {
                    program: shell,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("failed to spawn the terminal");
        let focus = TerminalView::focus(term.widget_id().clone());

        (
            App {
                term,
                project: config.as_ref().map(|c| c.project.name.clone()),
                processes: config.map(|c| c.process.len()).unwrap_or(0),
                ports: PortDetector::new(),
            },
            focus,
        )
    }

    fn update(&mut self, event: Event) -> Task<Event> {
        let Event::Terminal(iced_term::Event::BackendCall(_, cmd)) = event;

        // Same data sources the GTK app scrapes from VTE signals.
        let mut side_task = Task::none();
        let mut rescan_ports = false;
        if let BackendCommand::ProcessAlacrittyEvent(ev) = &cmd {
            match ev {
                AEvent::Wakeup => rescan_ports = true,
                AEvent::ClipboardStore(ty, data) if !data.is_empty() => {
                    // OSC 52 — the capability that retires the tmux
                    // clipboard bridge. Empty clears are ignored (multiplex
                    // emits them in the wild).
                    side_task = match ty {
                        ClipboardType::Clipboard => iced::clipboard::write(data.clone()),
                        ClipboardType::Selection => iced::clipboard::write_primary(data.clone()),
                    };
                }
                _ => {}
            }
        }

        let action = self.term.handle(iced_term::Command::ProxyToBackend(cmd));

        let action_task = match action {
            iced_term::actions::Action::Shutdown => iced::exit(),
            iced_term::actions::Action::PublishSelection(text) => {
                iced::clipboard::write_primary(text)
            }
            _ => Task::none(),
        };

        if rescan_ports {
            // The badge pipeline, verbatim from the GTK app: displayed text
            // through the port detector.
            self.ports.scan_output(PROCESS, &visible_text(&self.term));
        }

        Task::batch([side_task, action_task])
    }

    fn view(&'_ self) -> Element<'_, Event> {
        let project = match &self.project {
            Some(name) => format!("{name} — {} configured processes", self.processes),
            None => String::from("no tuxflow.toml in cwd"),
        };
        let badge = match (self.ports.get_url(PROCESS), self.ports.get_port(PROCESS)) {
            (Some(url), _) => format!("● {url}"),
            (None, Some(port)) => format!("● port {port}"),
            (None, None) => String::from("○ no port detected"),
        };

        let status = row![
            text(project).size(13),
            iced::widget::space::horizontal(),
            text(badge).size(13),
        ]
        .spacing(8);

        column![
            container(TerminalView::show(&self.term).map(Event::Terminal))
                .width(Length::Fill)
                .height(Length::Fill),
            container(status).padding([4, 8]).width(Length::Fill),
        ]
        .into()
    }

    fn subscription(&self) -> Subscription<Event> {
        self.term.subscription().map(Event::Terminal)
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
