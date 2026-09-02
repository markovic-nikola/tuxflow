use crate::actions::Action;
use crate::settings::BackendSettings;
use alacritty_terminal::event::{
    Event, EventListener, Notify, OnResize, WindowSize,
};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Indexed, Scroll};
use alacritty_terminal::index::{
    Boundary, Column, Direction, Line, Point, Side,
};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{
    self, cell::Cell, test::TermSize, viewport_to_point, Term, TermMode,
};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi;
use iced::keyboard::Modifiers;
use iced_core::Size;
use std::borrow::Cow;
use std::cmp::min;
use std::io::Result;
use std::ops::{Index, RangeInclusive};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

const URL_REGEX: &str = r#"(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file://|git://|ssh:|ftp://)[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>"\s{-}\^⟨⟩`]+"#;

/// Undo what a run left the emulator wearing, before the next one starts in
/// the same grid: alt screen (which would hide the scrollback the respawn
/// exists to keep), mouse reporting, bracketed paste, application cursor
/// keys, a scroll region, a hidden cursor, leftover SGR. Deliberately NOT
/// RIS (`\x1bc`) — a full reset clears the history along with the modes.
/// DECSTBM homes the cursor, so the region reset is bracketed by
/// DECSC/DECRC to leave the cursor where the last run parked it.
const RESET_BETWEEN_RUNS: &[u8] =
    b"\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\
\x1b[?2004l\x1b[?1l\x1b7\x1b[r\x1b8\x1b[?7h\x1b[?25h\x1b[m";

/// Read what the child left in the PTY before tearing the loop down —
/// alacritty calls this `hold`, and only a terminal that OUTLIVES its child
/// has anywhere to put it. Ours does now, and a command short enough to
/// print and exit within one poll (`echo`, a failing build) is exactly the
/// case where the whole run is in that last unread buffer: the child-exit
/// arm of the loop breaks out without reading when this is off, so the run
/// ends with a blank terminal and nothing to show for it.
const DRAIN_ON_EXIT: bool = true;

#[derive(Debug, Clone)]
pub enum Command {
    Write(Vec<u8>),
    Scroll(i32),
    Resize(Option<Size<f32>>, Option<Size<f32>>),
    SelectStart(SelectionType, (f32, f32)),
    SelectUpdate((f32, f32)),
    /// A selection gesture ended (button release). Extracts the selected
    /// text so the embedder can publish it to PRIMARY.
    SelectRelease,
    /// A gesture that could have selected ended while the APPLICATION owns
    /// the mouse: a report-mode drag that crossed a cell boundary, or a
    /// double/triple report click. The widget selected nothing — every
    /// event went to the app as reports — so there is no text to carry;
    /// the embedder is told because only it can reach wherever the app
    /// keeps its selections (tmux: the newest paste buffer, over ssh).
    ReportedSelectionGesture,
    /// Space auto-repeat / release in a hold-reporting terminal (patch 22);
    /// bounces off the backend as the matching `Action`, writes nothing.
    HoldRepeat,
    HoldRelease,
    /// Find the next scrollback match for a regex (VTE `search_set_regex`
    /// + `search_find_next/previous` parity). A changed pattern restarts
    ///   from the visible edge; a repeated one advances, wrapping around.
    SearchNext(String, Direction),
    /// Drop the active search and its highlight.
    SearchClear,
    ProcessLink(LinkAction, Point),
    MouseReport(MouseButton, Modifiers, Point, bool),
    /// An event off the PTY/Term channel, tagged with the RUN GENERATION it
    /// was sent under (see [`Backend::run_generation`]). A terminal spans
    /// runs since `respawn`, so an embedder attributing exits to processes
    /// must drop events whose generation is not the current one — a child
    /// that died just as the user hit restart parks its `ChildExit`/`Exit`
    /// in the queue, and unstamped they would read as the NEW run crashing.
    ProcessAlacrittyEvent(u64, Event),
}

#[derive(Debug, Clone)]
pub enum MouseMode {
    Sgr,
    Normal(bool),
}

impl From<TermMode> for MouseMode {
    fn from(term_mode: TermMode) -> Self {
        if term_mode.contains(TermMode::SGR_MOUSE) {
            MouseMode::Sgr
        } else if term_mode.contains(TermMode::UTF8_MOUSE) {
            MouseMode::Normal(true)
        } else {
            MouseMode::Normal(false)
        }
    }
}

#[derive(Debug, Clone)]
pub enum MouseButton {
    LeftButton = 0,
    MiddleButton = 1,
    RightButton = 2,
    LeftMove = 32,
    MiddleMove = 33,
    RightMove = 34,
    NoneMove = 35,
    ScrollUp = 64,
    ScrollDown = 65,
    Other = 99,
}

#[derive(Debug, Clone)]
pub enum LinkAction {
    Clear,
    Hover,
    Open,
}

#[derive(Clone, Copy, Debug)]
pub struct TerminalSize {
    // f32 on purpose: these ARE the font's advance/line-height. Truncating
    // to u16 desyncs a merged text run from the cell grid within ~3 cells.
    pub cell_width: f32,
    pub cell_height: f32,
    num_cols: u16,
    num_lines: u16,
    layout_width: f32,
    layout_height: f32,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cell_width: 1.0,
            cell_height: 1.0,
            num_cols: 80,
            num_lines: 50,
            layout_width: 80.0,
            layout_height: 50.0,
        }
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn columns(&self) -> usize {
        self.num_cols as usize
    }

    fn last_column(&self) -> Column {
        Column(self.num_cols as usize - 1)
    }

    fn bottommost_line(&self) -> Line {
        Line(self.num_lines as i32 - 1)
    }

    fn screen_lines(&self) -> usize {
        self.num_lines as usize
    }
}

impl From<TerminalSize> for WindowSize {
    fn from(size: TerminalSize) -> Self {
        Self {
            num_lines: size.num_lines,
            num_cols: size.num_cols,
            cell_width: size.cell_width as u16,
            cell_height: size.cell_height as u16,
        }
    }
}

pub struct Backend {
    /// Kept for `respawn`: the PTY is opened with it (alacritty tags the
    /// child's window id with it).
    id: u64,
    term: Arc<FairMutex<Term<EventProxy>>>,
    size: TerminalSize,
    notifier: Notifier,
    /// The sender every PTY loop of this terminal reports through — a
    /// respawned loop must reach the SAME subscription as the first one.
    event_proxy: EventProxy,
    /// Which run the terminal is on, bumped by `respawn` just before the
    /// new PTY opens. Shared with every event sender (the loops' proxies
    /// AND `Term`'s own, which emits `Exit` — a per-instance stamp would
    /// freeze Term's at its birth value), read at send time so queued
    /// events carry the run that actually produced them.
    run_generation: Arc<AtomicU64>,
    /// Parser for bytes the embedder writes into the grid itself (run
    /// banners). Separate from the PTY loop's own parser, which is why
    /// feeding is only safe between runs — see `feed`.
    parser: ansi::Processor,
    last_content: RenderableContent,
    pub(crate) url_regex: RegexSearch,
    search: Option<SearchState>,
}

/// Active scrollback search: the compiled regex is cached across
/// next/previous steps and recompiled only when the pattern changes.
struct SearchState {
    pattern: String,
    regex: RegexSearch,
    focused: Option<Match>,
}

impl Backend {
    pub fn new(
        id: u64,
        pty_event_proxy_sender: mpsc::UnboundedSender<(u64, Event)>,
        settings: BackendSettings,
    ) -> Result<Self> {
        // The terminal knobs ride in through BackendSettings; everything
        // not exposed keeps alacritty's default.
        let config = term::Config {
            scrolling_history: settings.scrolling_history,
            semantic_escape_chars: settings.semantic_escape_chars.clone(),
            kitty_keyboard: settings.kitty_keyboard,
            osc52: settings.osc52,
            ..term::Config::default()
        };

        let terminal_size = TerminalSize::default();
        let pty = spawn_pty(id, terminal_size, &settings)?;

        let run_generation = Arc::new(AtomicU64::new(1));
        let event_proxy = EventProxy {
            sender: pty_event_proxy_sender,
            run: run_generation.clone(),
        };

        let mut term = Term::new(config, &terminal_size, event_proxy.clone());

        let cursor = term.grid_mut().cursor_cell().clone();

        let initial_content = RenderableContent {
            cells: snapshot_cells(&term),
            display_offset: term.grid().display_offset(),
            cursor_point: term.grid().cursor.point,
            selectable_range: None,
            terminal_mode: *term.mode(),
            terminal_size,
            cursor: cursor.clone(),
            hovered_hyperlink: None,
            hovered_url: None,
            search_matches: Vec::new(),
        };

        let term = Arc::new(FairMutex::new(term));

        let pty_event_loop = EventLoop::new(
            term.clone(),
            event_proxy.clone(),
            pty,
            DRAIN_ON_EXIT,
            false,
        )?;

        let notifier = Notifier(pty_event_loop.channel());

        let _ = pty_event_loop.spawn();

        Ok(Self {
            id,
            term: term.clone(),
            size: terminal_size,
            notifier,
            event_proxy,
            run_generation,
            parser: ansi::Processor::new(),
            last_content: initial_content,
            url_regex: RegexSearch::new(URL_REGEX).expect("invalid url regexp"),
            search: None,
        })
    }

    /// The generation of the CURRENT run — compare against the stamp on
    /// [`Command::ProcessAlacrittyEvent`] to drop events a previous run
    /// parked in the queue before `respawn` replaced it.
    pub fn run_generation(&self) -> u64 {
        self.run_generation.load(Ordering::Relaxed)
    }

    /// End the PTY session — the child gets the same SIGHUP `Drop` sends
    /// (the loop's exit drops the PTY, which kills and reaps it) — but keep
    /// the grid. Stopping a process must not erase the output it produced;
    /// dropping the whole terminal for that is what left an embedder with
    /// nothing to show.
    pub fn shutdown(&self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }

    /// Write bytes straight into the grid, bypassing the PTY — the
    /// embedder's own annotations (why a run ended, where the next begins).
    ///
    /// Safe only BETWEEN runs: this parser and the PTY loop's are separate
    /// state machines over one grid, so feeding while a child is writing
    /// can interleave halfway through either one's escape sequence.
    pub fn feed(&mut self, bytes: &[u8]) {
        let term = self.term.clone();
        let mut term = term.lock();
        self.parser.advance(&mut *term, bytes);
    }

    /// Wipe the grid — viewport, scrollback and cursor — leaving the child
    /// alone. Unlike `feed` this is safe WHILE a process is writing: it
    /// mutates the grid under the same lock the PTY loop takes, instead of
    /// driving a second parser across it.
    ///
    /// The order is load-bearing. On the primary screen alacritty
    /// implements `ClearMode::All` as "scroll the viewport up into the
    /// history" — the xterm behaviour that lets `clear` keep your
    /// scrollback — so the history has to be dropped AFTER it, never
    /// before, or what we just cleared is sitting in it.
    pub fn clear(&mut self) {
        use alacritty_terminal::vte::ansi::Handler;

        let term = self.term.clone();
        let mut term = term.lock();
        term.clear_screen(ansi::ClearMode::All);
        term.clear_screen(ansi::ClearMode::Saved);
        // `clear_viewport` doesn't move the cursor, so the next line of
        // output would otherwise land where the old one was, under a
        // screenful of blank rows.
        term.goto(0, 0);
        // A user who cleared while scrolled up is asking to see the empty
        // screen, not the position they were reading at.
        term.scroll_display(Scroll::Bottom);
    }

    /// Start a new child in the SAME grid: the finished run's output stays
    /// in the scrollback and the new one appends below `banner`.
    ///
    /// The order is load-bearing. Tidy first (a banner fed before it would
    /// be discarded with the alt screen), banner second, spawn last — a
    /// banner fed after the spawn would race the new child's first bytes
    /// through the second parser `feed` warns about.
    pub fn respawn(
        &mut self,
        settings: &BackendSettings,
        banner: &[u8],
    ) -> Result<()> {
        // Usually already gone (the child exited on its own); a process
        // that was stopped and started again still has a loop to end.
        self.shutdown();
        self.feed(RESET_BETWEEN_RUNS);
        self.feed(banner);
        // New run, new generation — bumped just before the spawn so every
        // event of the new child carries it, while anything the OLD run
        // already sent still wears the stamp it was sent under.
        self.run_generation.fetch_add(1, Ordering::Relaxed);
        let pty = spawn_pty(self.id, self.size, settings)?;
        let pty_event_loop = EventLoop::new(
            self.term.clone(),
            self.event_proxy.clone(),
            pty,
            DRAIN_ON_EXIT,
            false,
        )?;
        self.notifier = Notifier(pty_event_loop.channel());
        let _ = pty_event_loop.spawn();
        Ok(())
    }

    pub fn handle(&mut self, cmd: Command) -> Action {
        // Event bookkeeping and mouse reports never touch the grid — handle
        // them without the terminal lock so the UI thread doesn't contend
        // with a flooding PTY thread.
        match cmd {
            Command::ProcessAlacrittyEvent(_, event) => {
                return match event {
                    Event::Exit => Action::Shutdown,
                    Event::Title(title) => Action::ChangeTitle(title),
                    Event::PtyWrite(pty) => {
                        self.notifier.notify(pty.into_bytes());
                        Action::Ignore
                    },
                    _ => Action::Ignore,
                };
            },
            Command::MouseReport(button, modifiers, point, pressed) => {
                self.process_mouse_report(button, modifiers, point, pressed);
                return Action::Ignore;
            },
            Command::ReportedSelectionGesture => {
                return Action::ReportedSelectionGesture;
            },
            Command::HoldRepeat => {
                return Action::HoldRepeat;
            },
            Command::HoldRelease => {
                return Action::HoldRelease;
            },
            _ => {},
        }

        let mut action = Action::default();
        let term = self.term.clone();
        let mut term = term.lock();
        match cmd {
            Command::Write(input) => {
                self.write(input);
                term.scroll_display(Scroll::Bottom);
            },
            Command::Scroll(delta) => {
                self.scroll(&mut term, delta);
            },
            Command::Resize(layout_size, font_measure) => {
                self.resize(&mut term, layout_size, font_measure);
            },
            Command::SelectStart(selection_type, (x, y)) => {
                self.start_selection(&mut term, selection_type, x, y);
            },
            Command::SelectUpdate((x, y)) => {
                self.update_selection(&mut term, x, y);
            },
            Command::SelectRelease => {
                // Runs through the ordered command queue, so every
                // SelectStart/SelectUpdate of the gesture has already been
                // applied — reading the selection here cannot race the drag.
                if let Some(text) = term.selection_to_string() {
                    if !text.is_empty() {
                        action = Action::PublishSelection(text);
                    }
                }
            },
            Command::SearchNext(pattern, direction) => {
                action = self.search_next(&mut term, pattern, direction);
            },
            Command::SearchClear => {
                self.search = None;
            },
            Command::ProcessLink(link_action, point) => {
                action = self.process_link_action(&term, link_action, point);
            },
            Command::ProcessAlacrittyEvent(..)
            | Command::MouseReport(..)
            | Command::ReportedSelectionGesture
            | Command::HoldRepeat
            | Command::HoldRelease => {
                unreachable!()
            },
        };

        action
    }

    fn search_next(
        &mut self,
        terminal: &mut Term<EventProxy>,
        pattern: String,
        direction: Direction,
    ) -> Action {
        // (Re)compile on pattern change. An unfinishable regex — the user
        // mid-typing `(` — reports "no match" instead of erroring.
        if self.search.as_ref().is_none_or(|s| s.pattern != pattern) {
            self.search =
                RegexSearch::new(&pattern).ok().map(|regex| SearchState {
                    pattern,
                    regex,
                    focused: None,
                });
        }
        let Some(search) = &mut self.search else {
            return Action::SearchResult(false);
        };

        let display_offset = terminal.grid().display_offset() as i32;
        let origin = match &search.focused {
            // Step past the focused match so "next" advances.
            // Boundary::None wraps at the grid edges — the same idiom
            // search_next itself uses, so a lone match keeps being found.
            Some(m) => match direction {
                Direction::Right => m.end().add(&*terminal, Boundary::None, 1),
                Direction::Left => m.start().sub(&*terminal, Boundary::None, 1),
            },
            // Fresh pattern: start at the visible edge facing the search
            // direction, so the nearest match is found first.
            None => match direction {
                Direction::Right => {
                    Point::new(Line(-display_offset), Column(0))
                },
                Direction::Left => Point::new(
                    Line(terminal.screen_lines() as i32 - 1 - display_offset),
                    terminal.last_column(),
                ),
            },
        };

        match terminal.search_next(
            &mut search.regex,
            origin,
            direction,
            Side::Left,
            None,
        ) {
            Some(m) => {
                terminal.scroll_to_point(*m.start());
                search.focused = Some(m);
                Action::SearchResult(true)
            },
            None => {
                search.focused = None;
                Action::SearchResult(false)
            },
        }
    }

    fn process_link_action(
        &mut self,
        terminal: &Term<EventProxy>,
        link_action: LinkAction,
        point: Point,
    ) -> Action {
        match link_action {
            LinkAction::Hover => {
                let hovered = self.regex_match_at(
                    terminal,
                    point,
                    &mut self.url_regex.clone(),
                );
                self.last_content.hovered_url =
                    hovered.as_ref().map(|range| extract_text(terminal, range));
                self.last_content.hovered_hyperlink = hovered;
                Action::Ignore
            },
            LinkAction::Clear => {
                self.last_content.hovered_hyperlink = None;
                self.last_content.hovered_url = None;
                Action::Ignore
            },
            // The embedder opens the URL, not the widget: a remote
            // project's URL names the HOST's port, which locally is dead
            // or someone else's forward — it must be rewritten through the
            // tunnel map (and the forward created on demand) first.
            LinkAction::Open => match &self.last_content.hovered_url {
                Some(url) => Action::OpenUrl(url.clone()),
                None => Action::Ignore,
            },
        }
    }

    fn process_mouse_report(
        &self,
        button: MouseButton,
        modifiers: Modifiers,
        point: Point,
        pressed: bool,
    ) {
        let mut mods = 0;
        if modifiers.contains(Modifiers::SHIFT) {
            mods += 4;
        }
        if modifiers.contains(Modifiers::ALT) {
            mods += 8;
        }
        if modifiers.contains(Modifiers::COMMAND) {
            mods += 16;
        }

        match MouseMode::from(self.last_content.terminal_mode) {
            MouseMode::Sgr => {
                self.sgr_mouse_report(point, button as u8 + mods, pressed)
            },
            MouseMode::Normal(is_utf8) => {
                if pressed {
                    self.normal_mouse_report(
                        point,
                        button as u8 + mods,
                        is_utf8,
                    )
                } else {
                    self.normal_mouse_report(point, 3 + mods, is_utf8)
                }
            },
        }
    }

    fn sgr_mouse_report(&self, point: Point, button: u8, pressed: bool) {
        let c = if pressed { 'M' } else { 'm' };

        let msg = format!(
            "\x1b[<{};{};{}{}",
            button,
            point.column + 1,
            point.line + 1,
            c
        );

        self.notifier.notify(msg.as_bytes().to_vec());
    }

    fn normal_mouse_report(&self, point: Point, button: u8, is_utf8: bool) {
        let Point { line, column } = point;
        let max_point = if is_utf8 { 2015 } else { 223 };

        if line >= max_point || column >= max_point {
            return;
        }

        let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];

        let mouse_pos_encode = |pos: usize| -> Vec<u8> {
            let pos = 32 + 1 + pos;
            let first = 0xC0 + pos / 64;
            let second = 0x80 + (pos & 63);
            vec![first as u8, second as u8]
        };

        if is_utf8 && column >= Column(95) {
            msg.append(&mut mouse_pos_encode(column.0));
        } else {
            msg.push(32 + 1 + column.0 as u8);
        }

        if is_utf8 && line >= 95 {
            msg.append(&mut mouse_pos_encode(line.0 as usize));
        } else {
            msg.push(32 + 1 + line.0 as u8);
        }

        self.notifier.notify(msg);
    }

    fn start_selection(
        &mut self,
        terminal: &mut Term<EventProxy>,
        selection_type: SelectionType,
        x: f32,
        y: f32,
    ) {
        let location = Self::selection_point(
            x,
            y,
            &self.size,
            terminal.grid().display_offset(),
        );
        terminal.selection = Some(Selection::new(
            selection_type,
            location,
            self.selection_side(x),
        ));
    }

    fn update_selection(
        &mut self,
        terminal: &mut Term<EventProxy>,
        x: f32,
        y: f32,
    ) {
        let display_offset = terminal.grid().display_offset();
        if let Some(ref mut selection) = terminal.selection {
            let location =
                Self::selection_point(x, y, &self.size, display_offset);
            selection.update(location, self.selection_side(x));
        }
    }

    pub fn selection_point(
        x: f32,
        y: f32,
        terminal_size: &TerminalSize,
        display_offset: usize,
    ) -> Point {
        let col = (x / terminal_size.cell_width) as usize;
        let col = min(Column(col), Column(terminal_size.num_cols as usize - 1));

        let line = (y / terminal_size.cell_height) as usize;
        let line = min(line, terminal_size.num_lines as usize - 1);

        viewport_to_point(display_offset, Point::new(line, col))
    }

    fn selection_side(&self, x: f32) -> Side {
        let cell_x = x % self.size.cell_width;
        let half_cell_width = self.size.cell_width / 2.0;

        if cell_x > half_cell_width {
            Side::Right
        } else {
            Side::Left
        }
    }

    fn resize(
        &mut self,
        terminal: &mut Term<EventProxy>,
        layout_size: Option<Size<f32>>,
        font_measure: Option<Size<f32>>,
    ) {
        if let Some(size) = layout_size {
            self.size.layout_height = size.height;
            self.size.layout_width = size.width;
        };

        if let Some(size) = font_measure {
            self.size.cell_height = size.height;
            self.size.cell_width = size.width;
        }

        let lines =
            (self.size.layout_height / self.size.cell_height).floor() as u16;
        let cols =
            (self.size.layout_width / self.size.cell_width).floor() as u16;
        if lines > 0 && cols > 0 {
            self.size.num_lines = lines;
            self.size.num_cols = cols;
            self.notifier.on_resize(self.size.into());
            terminal.resize(TermSize::new(
                self.size.num_cols as usize,
                self.size.num_lines as usize,
            ));
        }
    }

    fn write<I: Into<Cow<'static, [u8]>>>(&self, input: I) {
        self.notifier.notify(input);
    }

    fn scroll(&mut self, terminal: &mut Term<EventProxy>, delta_value: i32) {
        if delta_value != 0 {
            let scroll = Scroll::Delta(delta_value);
            if terminal
                .mode()
                .contains(TermMode::ALTERNATE_SCROLL | TermMode::ALT_SCREEN)
            {
                let line_cmd = if delta_value > 0 { b'A' } else { b'B' };
                let mut content = vec![];

                for _ in 0..delta_value.abs() {
                    content.push(0x1b);
                    content.push(b'O');
                    content.push(line_cmd);
                }

                self.notifier.notify(content);
            } else {
                terminal.grid_mut().scroll_display(scroll);
            }
        }
    }

    pub fn selectable_content(&self) -> String {
        // alacritty's own extraction: keeps line breaks (the cell-walk this
        // replaced glued a multi-line copy into one line), skips wide-char
        // spacers, and rejoins soft-wrapped lines.
        self.term.lock().selection_to_string().unwrap_or_default()
    }

    /// Snapshot the viewport. Returns false when the PTY thread holds the
    /// parse lock — never park the UI thread on it; a flood's next Wakeup
    /// retries, and the last Wakeup of any burst finds the lock free.
    pub fn sync(&mut self) -> bool {
        let term = self.term.clone();
        let Some(mut term) = term.try_lock_unfair() else {
            return false;
        };
        self.internal_sync(&mut term);
        true
    }

    fn internal_sync(&mut self, terminal: &mut Term<EventProxy>) {
        let selectable_range = match &terminal.selection {
            Some(s) => s.to_range(terminal),
            None => None,
        };

        let cursor = terminal.grid_mut().cursor_cell().clone();
        self.last_content.cells = snapshot_cells(terminal);
        self.last_content.display_offset = terminal.grid().display_offset();
        self.last_content.cursor_point = terminal.grid().cursor.point;
        self.last_content.selectable_range = selectable_range;
        self.last_content.cursor = cursor.clone();
        self.last_content.terminal_mode = *terminal.mode();
        self.last_content.terminal_size = self.size;
        // Highlights come from the live grid, never from search-time
        // coordinates — `focused` survives only as the stepping anchor.
        self.last_content.search_matches = match self.search.as_mut() {
            Some(search) => {
                visible_regex_match_iter(terminal, &mut search.regex).collect()
            },
            None => Vec::new(),
        };
    }

    pub fn renderable_content(&self) -> &RenderableContent {
        &self.last_content
    }

    /// Based on alacritty/src/display/hint.rs > regex_match_at
    /// Retrieve the match, if the specified point is inside the content matching the regex.
    fn regex_match_at(
        &self,
        terminal: &Term<EventProxy>,
        point: Point,
        regex: &mut RegexSearch,
    ) -> Option<Match> {
        let x = visible_regex_match_iter(terminal, regex)
            .find(|rm| rm.contains(&point));
        x
    }
}

/// Open a PTY running `settings`' program. Shared by the first spawn and
/// every `respawn`, so a second run of a process cannot drift from the
/// first in how it is launched.
fn spawn_pty(
    id: u64,
    size: TerminalSize,
    settings: &BackendSettings,
) -> Result<tty::Pty> {
    let pty_config = tty::Options {
        shell: Some(tty::Shell::new(
            settings.program.clone(),
            settings.args.clone(),
        )),
        working_directory: settings.working_directory.clone(),
        env: settings.env.clone(),
        ..tty::Options::default()
    };
    tty::new(&pty_config, size.into(), id)
}

/// Copied from alacritty/src/display/hint.rs:
/// Iterate over all visible regex matches.
fn visible_regex_match_iter<'a>(
    term: &'a Term<EventProxy>,
    regex: &'a mut RegexSearch,
) -> impl Iterator<Item = Match> + 'a {
    let viewport_start = Line(-(term.grid().display_offset() as i32));
    let viewport_end = viewport_start + term.bottommost_line();
    let mut start =
        term.line_search_left(Point::new(viewport_start, Column(0)));
    let mut end = term.line_search_right(Point::new(viewport_end, Column(0)));
    start.line = start.line.max(viewport_start - 100);
    end.line = end.line.min(viewport_end + 100);

    RegexIter::new(start, end, Direction::Right, term, regex)
        .skip_while(move |rm| rm.end().line < viewport_start)
        .take_while(move |rm| rm.start().line <= viewport_end)
}

pub struct RenderableContent {
    /// Visible viewport cells only — never the scrollback history. Cloning
    /// the full grid per sync is what froze scrolling on long histories.
    pub cells: Vec<Indexed<Cell>>,
    pub display_offset: usize,
    pub cursor_point: Point,
    pub hovered_hyperlink: Option<RangeInclusive<Point>>,
    pub hovered_url: Option<String>,
    /// Visible scrollback-search matches, recomputed from the live grid at
    /// every sync. NOT stored from search time: grid lines rotate when new
    /// output scrolls in, so a coordinate kept across syncs highlights the
    /// line BELOW the text it matched ("typed 555, highlighted 556").
    pub search_matches: Vec<RangeInclusive<Point>>,
    pub selectable_range: Option<SelectionRange>,
    pub cursor: Cell,
    pub terminal_mode: TermMode,
    pub terminal_size: TerminalSize,
}

impl Default for RenderableContent {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            display_offset: 0,
            cursor_point: Point::default(),
            hovered_hyperlink: None,
            hovered_url: None,
            search_matches: Vec::new(),
            selectable_range: None,
            cursor: Cell::default(),
            terminal_mode: TermMode::empty(),
            terminal_size: TerminalSize::default(),
        }
    }
}

fn snapshot_cells(term: &Term<EventProxy>) -> Vec<Indexed<Cell>> {
    term.grid()
        .display_iter()
        .map(|indexed| Indexed {
            point: indexed.point,
            cell: indexed.cell.clone(),
        })
        .collect()
}

fn extract_text(
    term: &Term<EventProxy>,
    range: &RangeInclusive<Point>,
) -> String {
    let grid = term.grid();
    let mut text = String::from(grid.index(*range.start()).c);
    for indexed in grid.iter_from(*range.start()) {
        text.push(indexed.c);
        if indexed.point == *range.end() {
            break;
        }
    }
    text
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

#[derive(Clone)]
pub struct EventProxy {
    sender: mpsc::UnboundedSender<(u64, Event)>,
    /// The backend's run counter, read at SEND time: an event queued before
    /// a respawn carries the run that produced it, not the run that happens
    /// to be current when the embedder finally processes it.
    run: Arc<AtomicU64>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        // Called from the PTY thread, sometimes while it holds the terminal
        // lock — must never block (see the channel comment in terminal.rs).
        let _ = self.sender.send((self.run.load(Ordering::Relaxed), event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn settings(command: &str) -> BackendSettings {
        BackendSettings {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), command.into()],
            ..BackendSettings::default()
        }
    }

    /// Everything the grid holds right now, one line per row.
    fn screen(backend: &mut Backend) -> String {
        backend.sync();
        let mut out = String::new();
        let mut line = backend.last_content.cells.first().map(|c| c.point.line);
        for cell in &backend.last_content.cells {
            if Some(cell.point.line) != line {
                out.push('\n');
                line = Some(cell.point.line);
            }
            out.push(cell.cell.c);
        }
        out
    }

    /// Run the PTY until the child exits, draining events as the embedder
    /// does; returns the run generation the Exit was stamped with. Panics
    /// rather than hanging forever if the child never ends.
    fn run_to_exit(rx: &mut mpsc::UnboundedReceiver<(u64, Event)>) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok((run, Event::Exit)) => return run,
                Ok(_) => continue,
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        panic!("child never exited");
    }

    /// The whole point of `respawn`: a second run does NOT cost the user
    /// what the first one printed.
    #[test]
    fn respawn_keeps_the_finished_run_on_screen() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut backend =
            Backend::new(1, tx, settings("echo first-run")).expect("spawn");
        run_to_exit(&mut rx);
        assert!(screen(&mut backend).contains("first-run"));

        backend
            .respawn(&settings("echo second-run"), b"\r\n-- restarted --\r\n")
            .expect("respawn");
        run_to_exit(&mut rx);

        let screen = screen(&mut backend);
        assert!(screen.contains("first-run"), "lost the first run: {screen}");
        assert!(screen.contains("-- restarted --"), "no banner: {screen}");
        assert!(screen.contains("second-run"), "no second run: {screen}");
    }

    /// A run that died inside a full-screen TUI must not take the
    /// scrollback with it — the tidy leaves the alt screen first.
    #[test]
    fn respawn_returns_from_the_alternate_screen() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        // Print, then switch to the alt screen and die there.
        let mut backend = Backend::new(
            2,
            tx,
            settings("echo before-tui; printf '\\033[?1049h'"),
        )
        .expect("spawn");
        run_to_exit(&mut rx);
        assert!(
            !screen(&mut backend).contains("before-tui"),
            "still primary"
        );

        backend
            .respawn(&settings("echo after-tui"), b"")
            .expect("respawn");
        run_to_exit(&mut rx);

        let screen = screen(&mut backend);
        assert!(screen.contains("before-tui"), "alt screen kept: {screen}");
        assert!(screen.contains("after-tui"), "no second run: {screen}");
    }

    /// The attribution contract: each run's events wear ITS generation, so
    /// an embedder can drop what a dead run parked in the queue instead of
    /// blaming the run that replaced it (a crash landing exactly on the
    /// restart click used to flip the fresh run to Crashed and feed a
    /// banner into its running grid).
    #[test]
    fn exit_events_carry_their_runs_generation() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut backend =
            Backend::new(4, tx, settings("echo one")).expect("spawn");
        let first = backend.run_generation();
        assert_eq!(run_to_exit(&mut rx), first, "first run's stamp");

        backend
            .respawn(&settings("echo two"), b"")
            .expect("respawn");
        assert_eq!(backend.run_generation(), first + 1);
        assert_eq!(
            run_to_exit(&mut rx),
            first + 1,
            "second run's Exit must wear the bumped stamp"
        );
    }

    /// `shutdown` is the stop button: the child dies, the output stays.
    #[test]
    fn shutdown_ends_the_child_but_keeps_the_grid() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut backend =
            Backend::new(3, tx, settings("echo running; sleep 60"))
                .expect("spawn");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !screen(&mut backend).contains("running") {
            assert!(Instant::now() < deadline, "child never printed");
            let _ = rx.try_recv();
            std::thread::sleep(Duration::from_millis(10));
        }

        backend.shutdown();
        // The kill lands when the loop drops the PTY; the grid is ours
        // either way.
        std::thread::sleep(Duration::from_millis(300));
        assert!(screen(&mut backend).contains("running"));
    }
}
