//! Headless probes of the iced_term backend (alacritty_terminal underneath).
//!
//! Each test scores one row of the VTE-parity table in ../README.md. They run
//! against a real PTY and need no display server, so they double as the
//! regression suite for whatever terminal stack a GTK replacement ends up on.

use alacritty_terminal::event::Event as AEvent;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::ClipboardType;
use iced::Size;
use iced::keyboard::Modifiers;
use iced_term::TermMode;
use iced_term::actions::Action;
use iced_term::backend::{Backend, Command, LinkAction, MouseButton};
use iced_term::settings::BackendSettings;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

struct Probe {
    backend: Backend,
    rx: mpsc::UnboundedReceiver<AEvent>,
    events: Vec<AEvent>,
}

impl Probe {
    fn spawn(script: &str) -> Self {
        Self::spawn_program("/bin/sh", vec!["-c".into(), script.into()])
    }

    fn spawn_program(program: &str, args: Vec<String>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let backend = Backend::new(
            1,
            tx,
            BackendSettings {
                program: program.into(),
                args,
                ..Default::default()
            },
        )
        .expect("failed to spawn PTY backend");
        Self {
            backend,
            rx,
            events: Vec::new(),
        }
    }

    /// Drain pending alacritty events, feeding them back into the backend the
    /// way the widget's subscription loop does (PtyWrite replies, exit, ...).
    fn pump(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            self.backend
                .handle(Command::ProcessAlacrittyEvent(ev.clone()));
            self.events.push(ev);
        }
    }

    /// Pump + sync until `pred(visible_text, events)` holds, or time out.
    fn wait(&mut self, secs: u64, pred: impl Fn(&str, &[AEvent], TermMode) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            self.pump();
            self.backend.sync();
            let mode = self.backend.renderable_content().terminal_mode;
            if pred(&self.visible_text(), &self.events, mode) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The displayed grid as trimmed lines — what VTE's `text_range_format`
    /// scraping (port/URL detection) needs from a replacement.
    fn visible_text(&self) -> String {
        let content = self.backend.renderable_content();
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
}

/// PTY spawn works and displayed text is programmatically readable.
#[test]
fn pty_echo_and_grid_scrape() {
    let mut probe = Probe::spawn(r#"printf "hello-from-pty"; sleep 2"#);
    assert!(
        probe.wait(5, |text, _, _| text.contains("hello-from-pty")),
        "child output never appeared in the grid; got:\n{}",
        probe.visible_text()
    );
}

/// OSC 52 copy (what agents emit, what tmux re-emits with set-clipboard on)
/// surfaces as a decoded ClipboardStore event. VTE drops this entirely.
#[test]
fn osc52_clipboard_store_event() {
    let mut probe =
        Probe::spawn(r#"printf '\033]52;c;%s\007' "$(printf %s hello-osc52 | base64)"; sleep 2"#);
    assert!(
        probe.wait(5, |_, events, _| events.iter().any(|ev| matches!(
            ev,
            AEvent::ClipboardStore(ClipboardType::Clipboard, s) if s == "hello-osc52"
        ))),
        "no decoded ClipboardStore event; events: {:?}",
        probe.events
    );
}

/// Child exit code reaches the embedder (VTE `child-exited` parity — TuxFlow's
/// crash detection needs this). Note: a signal-killed child emits Exit only.
#[test]
fn child_exit_code_events() {
    let mut probe = Probe::spawn("exit 7");
    assert!(
        probe.wait(5, |_, events, _| {
            events.iter().any(|ev| matches!(ev, AEvent::ChildExit(7)))
                && events.iter().any(|ev| matches!(ev, AEvent::Exit))
        }),
        "expected ChildExit(7) then Exit; events: {:?}",
        probe.events
    );
}

/// OSC 2 window title reaches the embedder (VTE `window-title-changed` parity).
#[test]
fn title_change_event() {
    let mut probe = Probe::spawn(r#"printf '\033]2;spike-title\007'; sleep 2"#);
    assert!(
        probe.wait(5, |_, events, _| events
            .iter()
            .any(|ev| matches!(ev, AEvent::Title(t) if t == "spike-title"))),
        "no Title event; events: {:?}",
        probe.events
    );
}

/// Programmatic writes reach the PTY (VTE `feed_child` parity — composer bar,
/// remote clipboard/image bridges all type into the terminal this way).
#[test]
fn write_to_pty_feed_child() {
    let mut probe = Probe::spawn_program("/bin/cat", vec![]);
    probe
        .backend
        .handle(Command::Write(b"tuxflow-composer\r".to_vec()));
    assert!(
        probe.wait(5, |text, _, _| text.contains("tuxflow-composer")),
        "written bytes never echoed back; got:\n{}",
        probe.visible_text()
    );
}

/// The app enabling xterm mouse reporting (tmux does this) is tracked, and a
/// synthesized click round-trips through the PTY as a correct SGR report.
#[test]
fn mouse_mode_tracking_and_sgr_report() {
    let mut probe = Probe::spawn(concat!(
        "stty raw -echo; ",
        r#"printf '\033[?1000h\033[?1006h'; "#,
        "dd bs=1 count=9 2>/dev/null | cat -v; sleep 2"
    ));

    assert!(
        probe.wait(5, |_, _, mode| {
            mode.contains(TermMode::SGR_MOUSE) && mode.contains(TermMode::MOUSE_REPORT_CLICK)
        }),
        "terminal never entered SGR mouse-report mode"
    );

    probe.backend.handle(Command::MouseReport(
        MouseButton::LeftButton,
        Modifiers::empty(),
        Point::new(Line(0), Column(4)),
        true,
    ));

    // The child reads the 9 report bytes and prints them back via cat -v.
    assert!(
        probe.wait(5, |text, _, _| text.contains("^[[<0;5;1M")),
        "SGR mouse report never came back through the PTY; got:\n{}",
        probe.visible_text()
    );
}

/// Flood control: a PTY blasting output (`yes`) must not wedge the event
/// pipeline — the child runs to completion and its exit is observed while
/// the embedder keeps draining events.
#[test]
fn output_flood_completes() {
    let mut probe = Probe::spawn("yes | head -n 2000000; exit 0");
    assert!(
        probe.wait(30, |_, events, _| events
            .iter()
            .any(|ev| matches!(ev, AEvent::ChildExit(0)))),
        "flood child never finished; {} events seen",
        probe.events.len()
    );
}

/// Built-in URL regex matching at a grid point (VTE `match_add_regex` +
/// Ctrl+click parity — feeds TuxFlow's url_rewriter/tunnel hook).
#[test]
fn url_regex_hover_match() {
    let mut probe = Probe::spawn(r#"printf "http://localhost:5173/x"; sleep 2"#);
    assert!(probe.wait(5, |text, _, _| text.contains("http://localhost")));

    probe.backend.handle(Command::ProcessLink(
        LinkAction::Hover,
        Point::new(Line(0), Column(8)),
    ));
    let matched = probe.backend.renderable_content().hovered_hyperlink.clone();
    assert!(
        matched.is_some(),
        "hover at column 8 did not match the URL; got:\n{}",
        probe.visible_text()
    );
    assert_eq!(
        probe.backend.renderable_content().hovered_url.as_deref(),
        Some("http://localhost:5173/x")
    );
}

/// Scrollback search (VTE `search_set_regex`/`find_next` parity): a regex
/// match in history is found, focused for highlighting, and scrolled into
/// view; repeats wrap; invalid patterns and clears are safe.
#[test]
fn scrollback_search_scrolls_to_match_and_wraps() {
    use alacritty_terminal::index::Direction;

    let mut probe = Probe::spawn(r#"printf 'needle-alpha\n'; seq 1 100; sleep 2"#);
    assert!(probe.wait(5, |text, _, _| text.contains("100")));

    // The needle scrolled out of the 50-line viewport long ago.
    assert!(!probe.visible_text().contains("needle-alpha"));

    let action = probe
        .backend
        .handle(Command::SearchNext("needle-[a-z]+".into(), Direction::Left));
    assert_eq!(action, Action::SearchResult(true));
    probe.backend.sync();
    let content = probe.backend.renderable_content();
    assert!(
        content.display_offset > 0,
        "viewport did not scroll into history"
    );
    assert!(
        content.search_match.is_some(),
        "no focused match to highlight"
    );
    assert!(
        probe.visible_text().contains("needle-alpha"),
        "match not scrolled into view; got:\n{}",
        probe.visible_text()
    );

    // Sole match: stepping again wraps around and finds it again.
    let action = probe
        .backend
        .handle(Command::SearchNext("needle-[a-z]+".into(), Direction::Left));
    assert_eq!(action, Action::SearchResult(true));

    // A regex the user is mid-typing must not panic or match.
    let action = probe
        .backend
        .handle(Command::SearchNext("(".into(), Direction::Left));
    assert_eq!(action, Action::SearchResult(false));

    // Clearing drops the highlight.
    probe.backend.handle(Command::SearchClear);
    probe.backend.sync();
    assert!(probe.backend.renderable_content().search_match.is_none());
}

/// A finished mouse selection surfaces its text for PRIMARY (VTE publishes
/// the selection internally; here it must round-trip as an Action). The
/// multi-line case pins the newline handling — the naive cell-walk this
/// replaced glued lines together.
#[test]
fn selection_release_publishes_text() {
    let mut probe = Probe::spawn(r#"printf "abc def\nghi"; sleep 2"#);
    assert!(probe.wait(5, |text, _, _| text.contains("ghi")));

    // Default TerminalSize has 1.0×1.0 cells, so pixels == grid coords:
    // drag from (0,0) to line 1, column 2 (right side → inclusive).
    probe
        .backend
        .handle(Command::SelectStart(SelectionType::Simple, (0.0, 0.0)));
    probe.backend.handle(Command::SelectUpdate((2.9, 1.0)));
    let action = probe.backend.handle(Command::SelectRelease);
    assert_eq!(
        action,
        Action::PublishSelection("abc def\nghi".into()),
        "selection text did not round-trip; visible:\n{}",
        probe.visible_text()
    );

    // A plain click (empty selection) must not publish anything.
    probe
        .backend
        .handle(Command::SelectStart(SelectionType::Simple, (0.0, 0.0)));
    let action = probe.backend.handle(Command::SelectRelease);
    assert_eq!(action, Action::Ignore);
}

/// Resize reflows the PTY and the grid (TuxFlow's wrap-rejoin logic reads
/// column_count; here we control the grid dimensions directly).
#[test]
fn resize_and_wrap() {
    let mut probe = Probe::spawn(concat!(
        "sleep 0.3; ",
        r#"printf "AAAAAAAAAABBBBBBBBBBCCCCCCCCCC"; sleep 2"#
    ));
    probe
        .backend
        .handle(Command::Resize(Some(Size::new(20.0, 10.0)), None));

    assert!(
        probe.wait(5, |text, _, _| {
            let lines: Vec<&str> = text.lines().collect();
            lines.iter().any(|l| *l == "AAAAAAAAAABBBBBBBBBB")
                && lines.iter().any(|l| l.starts_with("CCCCCCCCCC"))
        }),
        "30 chars did not wrap across a 20-column grid; got:\n{}",
        probe.visible_text()
    );
}
