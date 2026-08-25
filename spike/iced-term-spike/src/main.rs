//! TuxFlow GTK-replacement spike: multi-terminal window on iced + iced_term.
//!
//! Interactive checklist lives in ../README.md. The window mimics the shape
//! TuxFlow needs: several live terminals (pane grid), a composer input that
//! types into the focused terminal (feed_child parity), OSC 52 interception
//! into the system clipboard/PRIMARY, and a scrape action standing in for
//! port/URL detection.

use alacritty_terminal::event::Event as AEvent;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::ClipboardType;
use iced::widget::pane_grid::{self, PaneGrid};
use iced::widget::{button, column, container, responsive, row, text, text_input};
use iced::{Element, Length, Size, Subscription, Task, window};
use iced_term::{BackendCommand, TerminalView};
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Perf probes for the step-8 flood investigation. `TUXFLOW_SPIKE_STRESS=1`
/// auto-spawns two flood panes + two idle ones; the watchdog thread reports
/// UI-thread stalls (a proxy for the desktop's not-responding dialog).
static APP_START: OnceLock<Instant> = OnceLock::new();
static LAST_VIEW_MS: AtomicU64 = AtomicU64::new(0);

fn ui_watchdog() {
    let start = *APP_START.get_or_init(Instant::now);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let now = start.elapsed().as_millis() as u64;
            let last = LAST_VIEW_MS.load(Ordering::Relaxed);
            if last != 0 && now.saturating_sub(last) > 2000 {
                eprintln!(
                    "[perf] UI thread stalled for {}ms",
                    now.saturating_sub(last)
                );
                if now.saturating_sub(last) > 5000
                    && std::env::var("TUXFLOW_SPIKE_ABORT_ON_STALL").is_ok()
                {
                    // Die under a debugger so all-thread backtraces show
                    // exactly where the deadlock sits.
                    std::process::abort();
                }
            }
        }
    });
}

fn main() -> iced::Result {
    // What Alacritty's own main() does before spawning PTYs — neither
    // alacritty_terminal nor iced_term calls it. Pins TERM to an installed
    // terminfo (alacritty if present, else xterm-256color) and advertises
    // COLORTERM=truecolor. Without it children inherit the launcher's TERM,
    // and a missing terminfo leaves full-screen apps (top/less/htop) unable
    // to enter the alternate screen.
    alacritty_terminal::tty::setup_env();
    APP_START.get_or_init(Instant::now);
    ui_watchdog();

    iced::application(App::new, App::update, App::view)
        .title(|_: &App| String::from("TuxFlow spike — iced_term"))
        .window_size(Size {
            width: 1280.0,
            height: 760.0,
        })
        .subscription(App::subscription)
        .run()
}

struct App {
    panes: pane_grid::State<Pane>,
    tabs: HashMap<u64, iced_term::Terminal>,
    titles: HashMap<u64, String>,
    term_settings: iced_term::settings::Settings,
    panes_created: usize,
    focus: Option<pane_grid::Pane>,
    composer: String,
    log: VecDeque<String>,
}

#[derive(Clone, Copy)]
struct Pane {
    id: u64,
}

#[derive(Debug, Clone)]
enum Event {
    Split(pane_grid::Axis, pane_grid::Pane),
    Clicked(pane_grid::Pane),
    Resized(pane_grid::ResizeEvent),
    Close(pane_grid::Pane),
    Terminal(iced_term::Event),
    ComposerChanged(String),
    ComposerSend,
    Scrape,
}

impl App {
    fn new() -> (Self, Task<Event>) {
        let (panes, _) = pane_grid::State::new(Pane { id: 0 });
        let system_shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());

        let term_settings = iced_term::settings::Settings {
            backend: iced_term::settings::BackendSettings {
                program: system_shell,
                ..Default::default()
            },
            ..Default::default()
        };

        let tab = iced_term::Terminal::new(0, term_settings.clone())
            .expect("failed to create the initial terminal");
        let mut tabs = HashMap::new();
        tabs.insert(0, tab);

        let mut panes = panes;
        let mut panes_created = 1;
        if std::env::var("TUXFLOW_SPIKE_STRESS").is_ok() {
            let flood_settings = iced_term::settings::Settings {
                backend: iced_term::settings::BackendSettings {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "yes".into()],
                    ..Default::default()
                },
                ..term_settings.clone()
            };
            let first = *panes.iter().next().unwrap().0;
            let (right, _) = panes
                .split(pane_grid::Axis::Vertical, first, Pane { id: 1 })
                .unwrap();
            panes
                .split(pane_grid::Axis::Horizontal, first, Pane { id: 2 })
                .unwrap();
            panes
                .split(pane_grid::Axis::Horizontal, right, Pane { id: 3 })
                .unwrap();
            for id in 1..=3u64 {
                let settings = if id <= 2 {
                    flood_settings.clone()
                } else {
                    term_settings.clone()
                };
                tabs.insert(
                    id,
                    iced_term::Terminal::new(id, settings)
                        .expect("failed to create a stress terminal"),
                );
            }
            panes_created = 4;
        }

        (
            App {
                panes,
                tabs,
                titles: HashMap::new(),
                term_settings,
                panes_created,
                focus: None,
                composer: String::new(),
                log: VecDeque::new(),
            },
            Task::none(),
        )
    }

    fn log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > 3 {
            self.log.pop_front();
        }
    }

    fn focused_terminal_id(&self) -> Option<u64> {
        let pane = self.focus?;
        Some(self.panes.get(pane)?.id)
    }

    fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Split(axis, pane) => {
                let id = self.panes_created as u64;
                let result = self.panes.split(axis, pane, Pane { id });
                let tab = iced_term::Terminal::new(id, self.term_settings.clone())
                    .expect("failed to create a terminal");
                let focus_task = TerminalView::focus(tab.widget_id().clone());
                self.tabs.insert(id, tab);
                if let Some((pane, _)) = result {
                    self.focus = Some(pane);
                }
                self.panes_created += 1;
                focus_task
            }
            Event::Clicked(pane) => {
                self.focus = Some(pane);
                let id = self.panes.get(pane).unwrap().id;
                let tab = self.tabs.get(&id).unwrap();
                TerminalView::focus(tab.widget_id().clone())
            }
            Event::Resized(pane_grid::ResizeEvent { split, ratio }) => {
                self.panes.resize(split, ratio);
                Task::none()
            }
            Event::Close(pane) => self.close_pane(pane),
            Event::Terminal(iced_term::Event::BackendCall(id, cmd)) => {
                // Everything VTE exposes as signals arrives here as data —
                // peek before proxying to the widget.
                let mut side_task = Task::none();
                if let BackendCommand::ProcessAlacrittyEvent(ev) = &cmd {
                    side_task = self.observe_alacritty_event(id, ev);
                }

                let t0 = Instant::now();
                let action = match self.tabs.get_mut(&id) {
                    Some(tab) => tab.handle(iced_term::Command::ProxyToBackend(cmd)),
                    None => iced_term::actions::Action::Ignore,
                };
                let dt = t0.elapsed();
                if dt.as_millis() > 10 {
                    eprintln!("[perf] handle tab {id} took {dt:?}");
                }
                let proxy_task = match action {
                    iced_term::actions::Action::Shutdown => {
                        let pane = self
                            .panes
                            .iter()
                            .find(|(_, p)| p.id == id)
                            .map(|(pane, _)| *pane);
                        match pane {
                            Some(pane) => self.close_pane(pane),
                            None => Task::none(),
                        }
                    }
                    iced_term::actions::Action::ChangeTitle(title) => {
                        self.titles.insert(id, title);
                        Task::none()
                    }
                    iced_term::actions::Action::Ignore => Task::none(),
                };
                Task::batch([side_task, proxy_task])
            }
            Event::ComposerChanged(value) => {
                self.composer = value;
                Task::none()
            }
            Event::ComposerSend => {
                // feed_child parity: the composer types into the focused
                // terminal exactly like TuxFlow's composer_bar does via VTE.
                if let Some(id) = self.focused_terminal_id() {
                    let mut bytes = self.composer.clone().into_bytes();
                    bytes.push(b'\r');
                    if let Some(tab) = self.tabs.get_mut(&id) {
                        tab.handle(iced_term::Command::ProxyToBackend(BackendCommand::Write(
                            bytes,
                        )));
                    }
                    self.composer.clear();
                }
                Task::none()
            }
            Event::Scrape => {
                // Port/URL detection parity: read displayed text from the
                // grid (VTE equivalent: contents-changed + text_range_format).
                if let Some(id) = self.focused_terminal_id() {
                    if let Some(tab) = self.tabs.get(&id) {
                        let dump = visible_text(tab);
                        let badge = dump
                            .lines()
                            .rev()
                            .find_map(|l| find_local_url(l).map(String::from));
                        println!("──── scrape of terminal {id} ────\n{dump}");
                        self.log(match badge {
                            Some(url) => {
                                format!("scrape: badge candidate {url}")
                            }
                            None => "scrape: no local URL/port on screen".to_string(),
                        });
                    }
                }
                Task::none()
            }
        }
    }

    /// The OSC 52 story that VTE cannot do: agent/tmux copies land in the
    /// real clipboard (and PRIMARY for the Selection target).
    fn observe_alacritty_event(&mut self, id: u64, ev: &AEvent) -> Task<Event> {
        match ev {
            AEvent::ClipboardStore(ty, data) => {
                if data.is_empty() {
                    // Seen in the wild (step 7: multiplex/tmux emit OSC 52
                    // clears) — don't wipe the user's clipboard.
                    self.log(format!("term {id}: OSC 52 clear — ignored"));
                    return Task::none();
                }
                let (target, task) = match ty {
                    ClipboardType::Clipboard => ("clipboard", iced::clipboard::write(data.clone())),
                    ClipboardType::Selection => {
                        ("PRIMARY", iced::clipboard::write_primary(data.clone()))
                    }
                };
                self.log(format!(
                    "term {id}: OSC 52 copy → {target} ({} bytes)",
                    data.len()
                ));
                task
            }
            AEvent::ChildExit(code) => {
                self.log(format!("term {id}: child exited with code {code}"));
                Task::none()
            }
            AEvent::Bell => {
                self.log(format!("term {id}: bell"));
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn close_pane(&mut self, pane: pane_grid::Pane) -> Task<Event> {
        let id = self.panes.get(pane).map(|p| p.id);
        if let Some((closed, sibling)) = self.panes.close(pane) {
            self.tabs.remove(&closed.id);
            self.titles.remove(&closed.id);
            self.focus = Some(sibling);
            let sibling_id = self.panes.get(sibling).unwrap().id;
            let tab = self.tabs.get(&sibling_id).unwrap();
            TerminalView::focus(tab.widget_id().clone())
        } else {
            // Last pane: drop the tab so the PTY shuts down, then quit.
            if let Some(id) = id {
                self.tabs.remove(&id);
            }
            window::latest().and_then(window::close)
        }
    }

    fn view(&'_ self) -> Element<'_, Event> {
        if let Some(start) = APP_START.get() {
            LAST_VIEW_MS.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
        let focus = self.focus;
        let total_panes = self.panes.len();

        let pane_grid = PaneGrid::new(&self.panes, |pane, state, _| {
            let id = state.id;
            let is_focused = focus == Some(pane);
            let title = self
                .titles
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("terminal {id}"));

            let mut controls = row![].spacing(4);
            controls = controls.push(
                button(text("│").size(12))
                    .padding(4)
                    .on_press(Event::Split(pane_grid::Axis::Vertical, pane)),
            );
            controls = controls.push(
                button(text("─").size(12))
                    .padding(4)
                    .on_press(Event::Split(pane_grid::Axis::Horizontal, pane)),
            );
            let mut close = button(text("✕").size(12)).padding(4);
            if total_panes > 1 {
                close = close.on_press(Event::Close(pane));
            }
            controls = controls.push(close);

            let title_bar = pane_grid::TitleBar::new(text(title).size(13).color(if is_focused {
                iced::Color::from_rgb(1.0, 0.8, 0.36)
            } else {
                iced::Color::from_rgb(0.6, 0.6, 0.6)
            }))
            .controls(Element::from(controls))
            .padding(6);

            pane_grid::Content::new(responsive(move |_| {
                let tab = self.tabs.get(&id).expect("tab with target id not found");
                container(TerminalView::show(tab).map(Event::Terminal))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }))
            .title_bar(title_bar)
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .spacing(4)
        .on_click(Event::Clicked)
        .on_resize(8, Event::Resized);

        let composer = row![
            text_input(
                "composer — Enter sends to focused terminal (feed_child parity)",
                &self.composer
            )
            .on_input(Event::ComposerChanged)
            .on_submit(Event::ComposerSend)
            .size(14),
            button(text("Send").size(14)).on_press(Event::ComposerSend),
            button(text("Scrape").size(14)).on_press(Event::Scrape),
        ]
        .spacing(6);

        let mode_line = match self.focused_terminal_id().and_then(|id| self.tabs.get(&id)) {
            Some(tab) => format!(
                "mode: {:?}",
                tab.backend().renderable_content().terminal_mode
            ),
            None => "mode: (no focused terminal — click one)".to_string(),
        };

        let status = column![
            text(mode_line).size(12),
            text(self.log.iter().cloned().collect::<Vec<_>>().join("\n")).size(12),
        ]
        .spacing(2);

        column![
            container(pane_grid)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(4),
            container(composer).padding([0, 4]),
            container(status).padding(4),
        ]
        .spacing(4)
        .into()
    }

    fn subscription(&self) -> Subscription<Event> {
        let subscriptions: Vec<_> = self.tabs.values().map(|tab| tab.subscription()).collect();
        Subscription::batch(subscriptions).map(Event::Terminal)
    }
}

/// Same scrape the headless probes use — displayed grid as trimmed lines.
fn visible_text(tab: &iced_term::Terminal) -> String {
    let content = tab.backend().renderable_content();
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

/// Stand-in for util/port_detector.rs — just enough to prove the data source.
fn find_local_url(line: &str) -> Option<&str> {
    let start = line
        .find("http://localhost")
        .or_else(|| line.find("http://127.0.0.1"))?;
    let rest = &line[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(&rest[..end])
}
