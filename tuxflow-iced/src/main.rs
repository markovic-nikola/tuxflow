//! TuxFlow's iced shell — migration M4: the multi-project workspace.
//!
//! `tuxflow-iced [path | ssh://host/dir]…`. Projects come from
//! `~/.config/tuxflow/projects.toml` (plus any CLI args, which persist),
//! each with its own process list (config or detection, overlaid with the
//! user's custom commands/deletions/order — same policy as the GTK app),
//! ports, tunnels and poll cadence. Add project / add command / add agent
//! run as inline forms; closing a project detaches its remote sessions.

mod add_project;
mod edit_project;
mod git_view;
mod keys;
mod notify;
mod processes;
mod settings_ui;
mod status_dot;
mod theme;
mod widgets;

use std::collections::HashMap;
use std::path::PathBuf;
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
use tuxflow_core::config::ssh;
use tuxflow_core::detect::detector;
use tuxflow_core::remote::probe::ProbeError;
use tuxflow_core::remote::tunnel::TunnelManager;
use tuxflow_core::remote::{self, ProjectLocation};
use tuxflow_core::util::activity;
use tuxflow_core::util::agents::{self, resume_command_for};
use tuxflow_core::util::banner;
use tuxflow_core::util::icon_detector;
use tuxflow_core::util::port_detector::{self, PortDetector, remap_url_port, rewrite_clicked_url};

use keys::{AppAction, AppKeys};
use processes::{ProcessEntry, Status, plan_after_exit};
use status_dot::status_dot;
use theme::{
    CRASHED, DIM, GIT_ADDED, GIT_BEHIND, GIT_REMOVED, LOCAL_ACCENT, RESTARTING, STOPPED, TEXT,
    TEXT_SECONDARY, accent_for,
};
use tuxflow_core::config::settings::AppSettings;

/// Ports-poll cadence: fast while a run is settling (a new forward just
/// opened), backed off once nothing new appears (GTK behavior).
const POLL_FAST: Duration = Duration::from_secs(2);
const POLL_SLOW: Duration = Duration::from_secs(30);
/// Provisional badges get this long to firm up before auto-open fires.
const AUTO_OPEN_GRACE: Duration = Duration::from_secs(5);

/// Frame cadence shared by every [`Anim`] ramp — ~60fps.
const FRAME: Duration = Duration::from_millis(16);
/// The sidebar cluster's slide-in (design round F).
const HOVER_SLIDE_MS: f32 = 140.0;
/// The sidebar's collapse/expand glide. Longer than the hover glide: it
/// moves the whole window's layout, and Adwaita's own flap takes ~200ms.
const SIDEBAR_SLIDE_MS: f32 = 180.0;

/// One pass of the working-agent sweep across a project card. Slow: this
/// says "something is thinking", it is not a progress bar.
const SWEEP_MS: f32 = 2600.0;
/// Its own cadence, a third of [`FRAME`]'s. The glides last 140–180 ms;
/// this chain runs for as long as an agent stays busy, which can be
/// minutes, and every frame repaints the WHOLE window — where GTK's
/// equivalent spinner is a 14 px cairo widget that redraws alone. Measured
/// on a release build under llvmpipe, against the same app with the sweep
/// off and a terminal printing at 20 Hz beside it: 30 fps cost ~25 % of a
/// core, 20 fps ~9 %. A soft band with no edges, crossing in 2.6 s, moves
/// ~7 px per frame here — there is nothing at 30 fps worth triple that.
const SWEEP_FRAME: Duration = Duration::from_millis(50);

/// GTK sidebar parity: AdwOverlaySplitView sizes the sidebar at a quarter
/// of the window, clamped to the GTK app's min/max (window.rs: 220–400).
const SIDEBAR_FRACTION: f32 = 0.25;
const SIDEBAR_MIN: f32 = 220.0;
const SIDEBAR_MAX: f32 = 400.0;
/// Width of the collapsed icon rail, and so the floor of the collapse
/// glide: 16px icon + 7px button padding either side + 4px rail padding.
const SIDEBAR_RAIL: f32 = 38.0;

/// The sidebar's category sections, in the order they are drawn. This is
/// the single source for both the render loop and [`App::switch_targets`],
/// which numbers the Ctrl+1..9 hints by position in the drawn sequence —
/// the two orders diverging is exactly what makes a row advertise a chord
/// that lands somewhere else. GTK keeps the same pair in sync by hand
/// (`project_list.rs`'s `categories` vs `running_names_in_sidebar_order`,
/// each carrying a comment pointing at the other); here there is one array.
const SIDEBAR_CATEGORIES: [ProcessCategory; 4] = [
    ProcessCategory::Agent,
    ProcessCategory::Command,
    ProcessCategory::Terminal,
    ProcessCategory::SSH,
];

/// How many processes the digit switcher can reach — Ctrl+1..9.
const SWITCH_SLOTS: usize = 9;

/// Entry indices of one project's rows in the order the sidebar draws
/// them: grouped by category, each keeping its saved order.
fn sidebar_order(entries: &[ProcessEntry]) -> impl Iterator<Item = usize> + '_ {
    SIDEBAR_CATEGORIES.iter().flat_map(move |cat| {
        (0..entries.len()).filter(move |&i| entries[i].config.category == *cat)
    })
}

/// The word on the separator a new run starts under, read off the status
/// the run is REPLACING — the entry is still wearing the outgoing run's
/// state when [`App::start`] asks.
fn run_label(status: &Status) -> &'static str {
    match status {
        Status::Reconnecting(_) => "reconnecting",
        Status::Restarting(_) => "auto-restart",
        _ => "restarted",
    }
}

/// The switcher sequence over a workspace's per-project entry lists. Split
/// out of [`App::switch_targets`] so the ordering rules can be tested
/// without standing up a live workspace.
fn switch_targets_of<'a>(
    projects: impl IntoIterator<Item = &'a [ProcessEntry]>,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (pidx, entries) in projects.into_iter().enumerate() {
        for i in sidebar_order(entries) {
            if entries[i].is_running() {
                out.push((pidx, i));
                if out.len() == SWITCH_SLOTS {
                    return out;
                }
            }
        }
    }
    out
}

/// A 0..1 progress ramp advanced by a self-scheduling chain of frame
/// ticks — iced 0.14 has no animation driver, so this is the same
/// sleep-and-re-fire idiom the restart timers use.
///
/// Generation-stamped because a ramp restarted mid-flight (toggle the
/// sidebar twice quickly, sweep across two rows) leaves the previous
/// chain in flight: without the stamp both chains keep firing and the
/// ramp advances at double speed, then keeps ticking after it settles.
#[derive(Default)]
struct Anim {
    t: f32,
    stamp: u64,
}

impl Anim {
    /// Restart from zero. The returned generation is what the tick chain
    /// must carry back; anything else is stale by definition.
    fn start(&mut self) -> u64 {
        self.restart_at(0.0)
    }

    /// Restart so the ramp's EASED position lands on `f` — for reversing
    /// a glide in flight without the widget jumping. The ease has to be
    /// inverted to get there: `1 - t` would not do, since
    /// `eased(1 - t) != 1 - eased(t)` (mirroring t=0.5 lands on 0.75, not
    /// the 0.25 the reversal needs).
    fn restart_at(&mut self, f: f32) -> u64 {
        self.t = 1.0 - (1.0 - f.clamp(0.0, 1.0)).sqrt();
        self.stamp += 1;
        self.stamp
    }

    /// Advance one frame. `false` means stop scheduling — either the tick
    /// was stale or the ramp has arrived.
    fn tick(&mut self, generation: u64, span_ms: f32) -> bool {
        if generation != self.stamp || self.t >= 1.0 {
            return false;
        }
        self.t = (self.t + FRAME.as_millis() as f32 / span_ms).min(1.0);
        self.t < 1.0
    }

    fn settled(&self) -> bool {
        self.t >= 1.0
    }

    /// Ease-out (quadratic): quick off the mark, gentle into the seat.
    fn eased(&self) -> f32 {
        let t = self.t.clamp(0.0, 1.0);
        1.0 - (1.0 - t) * (1.0 - t)
    }
}

/// The working-agent sweep: the shared phase driving every card whose
/// agent is producing output — a breath in the card's border ring, and,
/// on the ACTIVE card only, a band of light crossing its wash
/// ([`theme::project_card`]). Same tick-chain idiom as [`Anim`] and
/// generation-stamped for the same reason, but it LOOPS rather than
/// settling — and it is linear, since an ease would make a repeating pass
/// lurch at the seam.
#[derive(Default)]
struct Sweep {
    phase: f32,
    stamp: u64,
    running: bool,
}

impl Sweep {
    /// Begin a chain, or `None` if one is already in flight.
    fn start(&mut self) -> Option<u64> {
        if self.running {
            return None;
        }
        self.running = true;
        self.phase = 0.0;
        self.stamp += 1;
        Some(self.stamp)
    }

    /// Advance one frame. `None` means the tick was stale; otherwise the
    /// bool says whether the phase wrapped — the moment cards that have
    /// gone quiet may drop out, since the band is off-card there and their
    /// light goes out at the edge instead of mid-pass.
    fn tick(&mut self, generation: u64) -> Option<bool> {
        if generation != self.stamp {
            return None;
        }
        self.phase += SWEEP_FRAME.as_millis() as f32 / SWEEP_MS;
        Some(if self.phase >= 1.0 {
            self.phase -= 1.0;
            true
        } else {
            false
        })
    }
}

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
        // The pane carries no toolbar, so the window title says what it
        // used to: which project, and what the selected process calls
        // itself right now (its OSC title, else its configured name).
        // Capped — agents write whole sentences into the OSC title.
        .title(|app: &App| match app.active_project() {
            Some(p) => match p.entries.get(p.selected) {
                Some(entry) => format!(
                    "TuxFlow - {}: {}",
                    p.name,
                    entry.display_title().chars().take(70).collect::<String>()
                ),
                None => format!("TuxFlow - {}", p.name),
            },
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
    /// Working-tree line counts behind the status bar's changes chip.
    /// Separate from `git` because it costs its own round trips — the
    /// porcelain status can't produce `+N −M`.
    diffstat: tuxflow_core::remote::git::DiffStat,
    /// The project's avatar, resolved once at load; None falls back to the
    /// initials square. Already checked to exist — `view` runs every frame
    /// and must never stat the disk.
    icon: Option<PathBuf>,
    /// The pre-merge config list the load produced (tuxflow.toml's authored
    /// processes, or detection). Edit Project resolves its Hidden and
    /// Detected groups from this — GTK keeps its load-time stacks for the
    /// same job, because re-detecting a REMOTE project live would be an ssh
    /// round trip mid-dialog (local projects re-detect live on top of it).
    detected_configs: Vec<ProcessConfig>,
    /// The card is riding the working-agent sweep. Set when an agent here
    /// starts producing output, cleared only at a pass boundary — see
    /// [`Sweep::tick`].
    sweeping: bool,
    /// Whether the project sat in the running TIER at the last look —
    /// [`App::refresh_recent_order`] stamps `last_used` and re-sorts the
    /// sidebar on the flip, GTK's `refresh_project_running_state`. Starts
    /// false, so a project loading with live sessions (reattach) flips and
    /// stamps like a start, while a project loading idle stamps nothing —
    /// stamping at load would re-date every project and wipe the saved
    /// recency order.
    was_running: bool,
}

impl ProjectState {
    fn key(&self) -> String {
        self.location.key()
    }

    fn running(&self) -> usize {
        self.entries.iter().filter(|e| e.is_running()).count()
    }

    /// Whether the card should read as live. Wider than [`running`], which
    /// feeds the counter pill: GTK's `project_has_running` counts
    /// `Restarting` too, and a `Reconnecting` remote process is the same
    /// case — its tmux session is alive on the host, only the link is
    /// down, so the card must not go dark mid-reconnect.
    fn has_running(&self) -> bool {
        self.entries.iter().any(|e| {
            matches!(
                e.status,
                Status::Running | Status::Restarting(_) | Status::Reconnecting(_)
            )
        })
    }

    /// At least one agent in here is producing output right now.
    fn agent_working(&self) -> bool {
        self.entries.iter().any(|e| e.working)
    }
}

struct ProcessForm {
    name: String,
    command: String,
    working_dir: String,
    agent: bool,
    /// The user has edited the name, so an agent-preset pick must stop
    /// rewriting it. Without this, choosing a preset after naming the
    /// process silently discards the name.
    name_touched: bool,
    start_with_project: bool,
    auto_restart: bool,
    open_in_browser: bool,
    /// Some((project id, entry index)) when editing an existing process.
    editing: Option<(u64, usize)>,
    original_category: ProcessCategory,
    /// Why the last submit was refused (duplicate name, empty fields).
    /// Cleared on the next edit of any text field — a refusal with no
    /// message is a dead button, and taking the form down with everything
    /// typed in it is worse.
    error: Option<String>,
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
    /// Git Changes view state; `Some` = the main pane shows it. Like
    /// settings, it takes the pane rather than floating over it — a diff
    /// wants the whole window, and GTK's dialog opens at the parent's
    /// full size for the same reason.
    git_ui: Option<git_view::State>,
    /// Projects with a one-click status-bar sync (fetch + ff-pull + push)
    /// in flight, by project id. The chip's counters hide while its own
    /// project syncs: showing the pre-sync numbers next to a spinner reads
    /// as "the sync did nothing". Keyed rather than a bare bool because a
    /// remote sync takes seconds and the user can switch projects under
    /// it — an unkeyed flag put "syncing…" on project B's chip and hung
    /// B's repo name on A's failure notice.
    git_syncing: std::collections::HashSet<u64>,
    /// Identifies the open Git Changes view's poll chain, so a rapid
    /// close-and-reopen doesn't leave two of them polling one view.
    git_tick_stamp: u64,
    /// Seed for each add-project form's generation stamps, strided so no
    /// two form instances ever share a stamp value — an in-flight listing
    /// or probe outlives its form, and must not be accepted by the next one.
    add_form_epoch: u64,
    /// A (heading, body) message awaiting an OK — GTK's AlertDialog for
    /// things that failed but need no decision.
    notice: Option<(String, String)>,
    composer: String,
    /// Header toggle (GTK: the AdwOverlaySplitView sidebar). Runtime-only,
    /// like GTK — a fresh launch always shows the sidebar.
    sidebar_visible: bool,
    /// The collapse/expand glide toward whatever `sidebar_visible` now
    /// says. Until it settles the sidebar is mid-flight, not at either end.
    sidebar_anim: Anim,
    /// Sidebar filter (GTK: the header search toggle + SearchEntry).
    filter_open: bool,
    filter_query: String,
    filter_input: iced::widget::Id,
    /// Which sidebar row the pointer is on: (project id, process index),
    /// index None = the project header. Drives the cluster slide-in.
    hovered_row: Option<(u64, Option<usize>)>,
    hover_anim: Anim,
    /// Ctrl is down right now, so the rows the digit switcher can reach are
    /// wearing their keycaps. Nothing about the chords depends on this — it
    /// only decides whether the sidebar is currently answering "which one
    /// is 3?", which is why it can be dropped on focus loss without care.
    ctrl_held: bool,
    /// Shared by every card riding it, so their bands stay in step.
    sweep: Sweep,
    /// Last pointer position in window coords — where a right-click's
    /// context menu opens (mouse_area reports no position itself).
    cursor: iced::Point,
    /// An open right-click menu (GTK's sidebar popovers).
    context_menu: Option<MenuTarget>,
    /// A pending destructive action awaiting its GTK-style confirmation.
    confirm: Option<ConfirmAction>,
    search_open: bool,
    search_query: String,
    search_hit: Option<bool>,
    search_input: iced::widget::Id,
    palette_open: bool,
    palette_query: String,
    palette_index: usize,
    palette_input: iced::widget::Id,
    add_project: Option<add_project::State>,
    add_command: Option<ProcessForm>,
    /// Edit Project form; `Some` = the main pane shows it. Mutually
    /// exclusive with the two add forms, like the GTK dialogs they port.
    edit_project: Option<edit_project::State>,
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

/// A right-click's target row: process index, or None for the project
/// header. `at` is the click position (menus open at the pointer).
#[derive(Debug, Clone, Copy)]
struct MenuTarget {
    project: u64,
    index: Option<usize>,
    at: iced::Point,
}

/// Destructive sidebar actions ask first, like GTK's AlertDialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmAction {
    RemoveProject(u64),
    DeleteProcess { project: u64, index: usize },
}

/// Probe success: project name if configured, process configs, live tmux
/// sessions, and the local cache path of a fetched icon. Failure: message +
/// whether it's worth retrying.
type ProbeResult = Result<ProbeOk, (String, bool)>;

/// The probe's payload — a struct rather than a tuple because the icon made
/// it four wide and `p.2` stopped saying anything.
#[derive(Debug, Clone)]
struct ProbeOk {
    name: Option<String>,
    configs: Vec<ProcessConfig>,
    live_sessions: Vec<String>,
    /// Icon pulled into `~/.cache/tuxflow/icons/`, when the project had none
    /// saved and the host had one to give.
    icon: Option<String>,
}

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
    /// Re-request maximize once the window is mapped (the pre-map hint
    /// is dropped on X11).
    RestoreMaximize,
    RestoreMeasured {
        id: iced::window::Id,
        actual: Option<iced::Point>,
    },
    OpenSettings,
    ToggleSidebar,
    ToggleFilter,
    FilterInput(String),
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
    /// Project-header cluster (GTK's hover controls): start every
    /// process marked start_with_project / restart / stop the running.
    StartAll(u64),
    RestartAll(u64),
    StopAll(u64),
    /// Pointer entered/left a sidebar row (index None = project header).
    RowEnter {
        project: u64,
        index: Option<usize>,
    },
    RowExit {
        project: u64,
        index: Option<usize>,
    },
    /// One frame of the cluster slide-in.
    HoverTick(u64),
    /// One frame of the sidebar's collapse/expand glide.
    SidebarTick(u64),
    /// Fixed-interval sample of which agents are producing output.
    ActivityTick,
    /// One frame of the working-agent sweep.
    SweepTick(u64),
    CursorMoved(iced::Point),
    /// Ctrl went down or up — reveals/hides the sidebar's keycaps.
    CtrlHeld(bool),
    /// Right-click on a sidebar row (index None = project header).
    OpenContextMenu {
        project: u64,
        index: Option<usize>,
    },
    CloseContextMenu,
    /// A picked menu item: close the menu, then run the wrapped event.
    MenuAction(Box<Event>),
    /// GTK menu backings without a button elsewhere.
    CopyText(String),
    OpenInEditor(u64),
    ToggleProcessAt {
        project: u64,
        index: usize,
    },
    ResumeAgentAt {
        project: u64,
        index: usize,
    },
    EditProcessAt {
        project: u64,
        index: usize,
    },
    /// Destructive actions route through a confirmation card first.
    ConfirmRequest(ConfirmAction),
    ConfirmCancel,
    ConfirmProceed,
    AddTerminal(u64),
    ToggleExpanded(u64),
    RestartDue {
        project: u64,
        index: usize,
        generation: u64,
    },
    GitTick,
    GitPolled {
        project: u64,
        status: Option<tuxflow_core::remote::git::GitStatus>,
        diffstat: tuxflow_core::remote::git::DiffStat,
    },
    /// Status-bar sync chip: fetch, ff-pull if behind, push if ahead.
    GitSync,
    GitSynced {
        project: u64,
        result: Result<(), String>,
    },
    /// Status-bar changes chip → the Git Changes view.
    OpenGitChanges,
    GitMsg(git_view::Msg),
    /// Status-bar Clear: empty the selected terminal's grid, child intact.
    ClearTerminal,
    NoticeDismiss,
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
        /// The run that asked for the paste — the terminal outlives its
        /// runs, so its id alone no longer says the paste is still wanted.
        run: u64,
        result: Result<Vec<u8>, String>,
    },
    /// The status bar's open-in-browser button: the (tunnel-mapped) URL.
    OpenBadge,
    /// The row context menu's "Open in Browser" — any process, not just
    /// the selected.
    OpenBadgeFor {
        project: u64,
        index: usize,
    },
    OpenAddProject,
    AddProjectMsg(add_project::Msg),
    /// Add a command (or agent) to a project — the form writes into
    /// whatever project is active, so it names the one it was raised on.
    OpenAddCommand {
        project: u64,
        agent: bool,
    },
    OpenEditProcess,
    AddCommandName(String),
    AddCommandCommand(String),
    /// Index into `agents::AGENT_PRESETS`.
    AgentPreset(usize),
    FormWorkingDir(String),
    FormToggleStartWith(bool),
    FormToggleAutoRestart(bool),
    FormToggleOpenBrowser(bool),
    DeleteProcess,
    AddCommandSubmit,
    AddCommandCancel,
    /// The project context menu's Edit Project — GTK's dialog as a
    /// full-pane view.
    OpenEditProject(u64),
    EditProjectMsg(edit_project::Msg),
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
            git_ui: None,
            git_syncing: std::collections::HashSet::new(),
            git_tick_stamp: 0,
            add_form_epoch: 0,
            notice: None,
            composer: String::new(),
            sidebar_visible: true,
            // Settled: a fresh launch shows the sidebar without a glide.
            sidebar_anim: Anim { t: 1.0, stamp: 0 },
            filter_open: std::env::var("TUXFLOW_ICED_UI").as_deref() == Ok("filter"),
            filter_query: String::new(),
            filter_input: iced::widget::Id::unique(),
            hovered_row: None,
            ctrl_held: false,
            hover_anim: Anim::default(),
            sweep: Sweep::default(),
            cursor: iced::Point::ORIGIN,
            context_menu: None,
            confirm: None,
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
            edit_project: None,
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
        // Maximized restore needs a second ask AFTER the WM maps the
        // window: winit's pre-map request is lost on X11 (verified under
        // metacity — the window came up floating at the saved size).
        if app.settings.window.maximized {
            tasks.push(Task::perform(
                tokio::time::sleep(Duration::from_millis(250)),
                |_| Event::RestoreMaximize,
            ));
        }
        tasks.push(Task::done(Event::GitTick));
        tasks.push(Task::done(Event::ActivityTick));

        (app, Task::batch(tasks))
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
            diffstat: tuxflow_core::remote::git::DiffStat::default(),
            icon: None,
            detected_configs: Vec::new(),
            sweeping: false,
            was_running: false,
            location,
        };

        match project.location.clone() {
            ProjectLocation::Local(dir) => {
                let (name, configs) = processes::load_local_configs(&dir);
                if self.saved.get_name(key).is_none() {
                    project.name = name;
                }
                // Local detection is a handful of stats on a directory we are
                // already reading configs from — cheap enough to stay inline,
                // unlike the remote half, which rides the probe worker.
                project.icon = usable_icon(icon_detector::resolve_icon(
                    &mut self.saved,
                    key,
                    Some(&dir),
                    None,
                ));
                project.detected_configs = configs.clone();
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

    /// Is the selected process a live remote AGENT terminal? Decides
    /// whether a plain Ctrl+V that fell through the widget belongs to
    /// the paste bridge (see the Paste rebind in start()).
    fn remote_agent_selected(&self) -> bool {
        self.active_project().is_some_and(|p| {
            p.location.host().is_some()
                && p.entries
                    .get(p.selected)
                    .is_some_and(|e| e.is_running() && e.config.category == ProcessCategory::Agent)
        })
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
        // A project whose saved icon still exists skips the fetch entirely —
        // it is a second ssh round trip, and the saved path wins over it
        // anyway. Existence-aware on purpose: fetched icons live in
        // ~/.cache, and after a cleared cache the fetch is the only way the
        // file comes back.
        let fetch_icon =
            !icon_detector::has_usable_saved_icon(&self.saved, &self.projects[pidx].key());
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
                        // Own ssh permit inside: the probe released its own
                        // on return and the fetch opens channels of its own.
                        let icon = fetch_icon
                            .then(|| {
                                let _permit = remote::ssh_permit();
                                remote::icon::fetch_remote_icon(&host, &dir)
                            })
                            .flatten();
                        ProbeOk {
                            name,
                            configs,
                            live_sessions: p.live_sessions,
                            icon,
                        }
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

    /// Spawn (or respawn/reattach) a process's run.
    ///
    /// An entry keeps ONE terminal for its whole life: a second run of the
    /// same process spawns into the grid the first one printed into, under
    /// a separator, so its output is still there to read (the GTK app gets
    /// this by reusing a single VTE widget per process). Only the first run
    /// builds a terminal, and only that path takes a fresh terminal id —
    /// subscription identity must change per TERMINAL or iced keeps the
    /// dead stream, but a respawn keeps the live stream it already has.
    fn start(&mut self, pidx: usize, index: usize) -> Task<Event> {
        let fresh_id = self.next_term_id;

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
        let remote_agent = project.location.host().is_some()
            && project.entries[index].config.category == ProcessCategory::Agent;
        let entry = &mut project.entries[index];
        let separator_label = run_label(&entry.status);
        // (widget id to focus, terminal id, whether `fresh_id` was taken)
        let spawned: std::io::Result<(iced::widget::Id, u64, bool)> = if entry.terminal.is_some() {
            let id = entry.term_id.unwrap_or(fresh_id);
            let term = entry.terminal.as_mut().expect("terminal, just checked");
            let cols = term.backend().renderable_content().terminal_size.columns();
            let banner = banner::run_separator(cols, separator_label);
            term.respawn(settings.backend, banner.as_bytes())
                .map(|()| (term.widget_id().clone(), id, false))
        } else {
            iced_term::Terminal::new(fresh_id, settings).map(|mut term| {
                // Reserve the app's chords before the first keystroke —
                // the stock bindings would type them into the shell.
                term.handle(iced_term::Command::AddBindings(reservations));
                if remote_agent {
                    // GTK parity (window.rs, "plain Ctrl+V in a remote
                    // agent terminal"): the agent's raw ^V reads the
                    // HOST's clipboard, which is not where the user's
                    // clipboard lives — paste text from here instead.
                    // An image-only clipboard leaves the widget nothing
                    // to paste, so the chord falls through uncaptured to
                    // the Hotkey handler's paste_image bridge.
                    term.handle(iced_term::Command::AddBindings(vec![(
                        iced_term::bindings::Binding {
                            target: iced_term::bindings::InputKind::Char("v".into()),
                            modifiers: iced::keyboard::Modifiers::CTRL,
                            terminal_mode_include: iced_term::TermMode::empty(),
                            terminal_mode_exclude: iced_term::TermMode::empty(),
                        },
                        iced_term::bindings::BindingAction::Paste,
                    )]));
                }
                let widget = term.widget_id().clone();
                entry.terminal = Some(term);
                (widget, fresh_id, true)
            })
        };

        match spawned {
            Ok((widget, id, took_fresh_id)) => {
                entry.term_id = Some(id);
                entry.run_id += 1;
                entry.status = Status::Running;
                entry.last_exit = None;
                entry.stopping = false;
                entry.auto_open_grace = false;
                entry.started_at = Some(Instant::now());
                let name = entry.config.name.clone();
                project.ports.clear(&name);
                project.selected = index;
                self.active = pidx;
                if took_fresh_id {
                    self.next_term_id += 1;
                }
                TerminalView::focus(widget)
            }
            Err(err) => {
                log::error!("failed to spawn {}: {err}", entry.config.name);
                entry.status = Status::Crashed(None);
                Task::none()
            }
        }
    }

    /// Manual start: forgives past failures, cancels pending timers, arms
    /// the one-shot auto-open. NO last_used stamp here — recency is stamped
    /// on running-tier FLIPS by `refresh_recent_order` (GTK parity). A
    /// per-start stamp re-dates a project that is already running, which
    /// moves it WITHIN the running tier the moment a second process starts;
    /// GTK's running tier holds still.
    fn start_fresh(&mut self, pidx: usize, index: usize) -> Task<Event> {
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
    /// spawn clear any survivor. Local: `shutdown()` SIGHUPs the child on
    /// the PTY thread — the same teardown dropping the terminal performed,
    /// minus the part that threw away everything the process printed.
    /// It emits no Exit event, so the status set here is the final word.
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
        if let Some(term) = entry.terminal.as_ref() {
            term.shutdown();
        }
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

    fn open_edit_form(&mut self, pidx: usize, index: usize) {
        let project = &self.projects[pidx];
        let Some(entry) = project.entries.get(index) else {
            return;
        };
        self.add_command = Some(ProcessForm {
            name: entry.config.name.clone(),
            command: entry.config.command.clone(),
            working_dir: entry.config.working_dir.clone().unwrap_or_default(),
            agent: entry.config.category == ProcessCategory::Agent,
            name_touched: true,
            start_with_project: entry.config.start_with_project,
            auto_restart: entry.config.auto_restart,
            open_in_browser: entry.config.open_in_browser,
            editing: Some((project.id, index)),
            original_category: entry.config.category.clone(),
            error: None,
        });
        // The form panes are mutually exclusive, as GTK's modal dialogs
        // are — the main pane can only show one, and a form left standing
        // underneath swallows the first Esc invisibly.
        self.add_project = None;
        self.edit_project = None;
    }

    /// Stop and remove a process: drop its custom-command copy AND record
    /// the deletion — the user's edit of a DETECTED process lives in
    /// custom_commands, and without the deletion record detection
    /// resurrects it on the next load.
    fn delete_process(&mut self, pidx: usize, index: usize) {
        if self.projects[pidx].entries.get(index).is_none() {
            return;
        }
        self.stop(pidx, index);
        let key = self.projects[pidx].key();
        let name = self.projects[pidx].entries[index].config.name.clone();
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
    }

    /// Close a project: local processes die with their PTYs, remote
    /// sessions DETACH (kill only happens on explicit per-process stop) —
    /// the same contract as quitting the app.
    fn close_project(&mut self, pidx: usize) {
        let key = self.projects[pidx].key();
        if let Some(tunnels) = &mut self.projects[pidx].tunnels {
            tunnels.close_all();
        }
        // A fetched remote icon lives in our cache — delete it with the
        // project so removals don't orphan cache files (GTK parity).
        if let Some(icon) = self.saved.get_icon(&key) {
            remote::icon::discard_if_cached(icon);
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
        let host = project.location.host().map(String::from);
        let entry = &mut project.entries[index];

        // The terminal stays — it holds the run's output, which is what the
        // user reaches for when a run ends badly. GTK feeds the same line
        // into its VTE, and it matters most on remote projects: an error
        // printed inside the tmux pane dies with the session, leaving only
        // tmux's bare "[exited]" behind.
        if let Some(code) = entry.last_exit
            && let Some(msg) = banner::exit_banner(
                code,
                connection_loss,
                &entry.config.command,
                host.as_deref(),
            )
            && let Some(term) = entry.terminal.as_mut()
        {
            term.feed(msg.as_bytes());
        }

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
                notify::disconnect(&self.settings.notifications, &project_name, &name);
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
            tokio::task::spawn_blocking(move || {
                let status = tuxflow_core::remote::git::query_status(&location)?;
                // Only worth the extra round trips once we know it IS a
                // repo — the diffstat on a non-repo is two failed calls.
                Some((status, tuxflow_core::remote::git::query_diffstat(&location)))
            }),
            move |joined| {
                let answer = joined.ok().flatten();
                Event::GitPolled {
                    project: id,
                    status: answer.as_ref().map(|(s, _)| s.clone()),
                    diffstat: answer.map(|(_, d)| d).unwrap_or_default(),
                }
            },
        )
    }

    /// The status-bar sync chip: fetch, ff-pull if behind, push if ahead.
    /// One click, and every failure mode lands in a notice rather than
    /// silently leaving the counters wrong.
    fn start_git_sync(&mut self) -> Task<Event> {
        let Some((id, location)) = self.active_project().map(|p| (p.id, p.location.clone())) else {
            return Task::none();
        };
        if !self.git_syncing.insert(id) {
            return Task::none();
        }
        Task::perform(
            tokio::task::spawn_blocking(move || {
                tuxflow_core::remote::git::sync_with_remote(&location)
            }),
            move |joined| Event::GitSynced {
                project: id,
                result: joined.unwrap_or_else(|e| Err(e.to_string())),
            },
        )
    }

    /// GTK opens an AlertDialog here; the notice card is this shell's
    /// equivalent — one message, one OK. Returns the modal grab (see
    /// `ConfirmRequest`) — run it, or keys keep reaching the terminal
    /// under the card.
    fn notify_git_failure(&mut self, heading: &str, detail: &str) -> Task<Event> {
        self.notice = Some((
            heading.to_string(),
            format!("{detail}\n\nOpen Git Changes to resolve it manually."),
        ));
        TerminalView::unfocus()
    }

    /// Open the Git Changes view on the active project, seeded from what
    /// the status-bar poll already knows so it renders complete instead of
    /// blank for a round trip.
    fn open_git_changes(&mut self) -> Task<Event> {
        let Some(project) = self.active_project() else {
            return Task::none();
        };
        let seed = git_view::Seed {
            ahead: project.git.as_ref().map_or(0, |g| g.ahead as usize),
            behind: project.git.as_ref().map_or(0, |g| g.behind as usize),
            branch: project.git.as_ref().map(|g| g.branch.clone()),
        };
        let (id, location) = (project.id, project.location.clone());
        self.git_tick_stamp += 1;
        let stamp = self.git_tick_stamp;
        self.git_ui = Some(git_view::State::new(id, location, seed, stamp));
        // Closing the other full-pane views keeps "what is the main area
        // showing?" a single answer.
        self.settings_ui = None;
        Task::batch([
            self.git_load_files(),
            self.git_refresh_sync(true),
            Task::perform(tokio::time::sleep(Duration::from_secs(2)), move |_| {
                Event::GitMsg(git_view::Msg::Tick(stamp))
            }),
        ])
    }

    /// Reload the changed-file list. Clears the selection, because the
    /// index it holds is about to mean a different file.
    fn git_load_files(&mut self) -> Task<Event> {
        let Some(state) = &mut self.git_ui else {
            return Task::none();
        };
        let generation = state.bump();
        state.loading = state.files.is_empty();
        let location = state.location.clone();
        Task::perform(
            tokio::task::spawn_blocking(move || {
                tuxflow_core::remote::git::changed_files(&location)
            }),
            move |joined| {
                Event::GitMsg(git_view::Msg::Files {
                    generation,
                    files: joined.unwrap_or_default(),
                })
            },
        )
    }

    /// Load the selected file's diff. Shares the file list's generation:
    /// a reload invalidates a diff still in flight for the old list.
    fn git_load_diff(&mut self) -> Task<Event> {
        let Some(state) = &mut self.git_ui else {
            return Task::none();
        };
        let Some(file) = state.selected_file().cloned() else {
            state.diff = None;
            return Task::none();
        };
        let generation = state.generation;
        state.diff_loading = true;
        let location = state.location.clone();
        let path = file.path.clone();
        Task::perform(
            tokio::task::spawn_blocking(move || {
                tuxflow_core::remote::git::load_diff(&location, &file)
            }),
            move |joined| {
                Event::GitMsg(git_view::Msg::Diff {
                    generation,
                    path: path.clone(),
                    diff: Box::new(joined.unwrap_or_default()),
                })
            },
        )
    }

    /// Branch + ahead/behind + the porcelain hash the poll compares on.
    /// `fetch` costs a network round trip, so it runs on open and every
    /// ~30 s, not on every 2 s tick.
    fn git_refresh_sync(&mut self, fetch: bool) -> Task<Event> {
        let Some(state) = &self.git_ui else {
            return Task::none();
        };
        let generation = state.generation;
        let location = state.location.clone();
        Task::perform(
            tokio::task::spawn_blocking(move || {
                use tuxflow_core::remote::git as g;
                if fetch {
                    g::fetch(&location);
                }
                (
                    g::commits_ahead(&location),
                    g::commits_behind(&location),
                    g::current_branch(&location),
                    g::status_hash(&location),
                )
            }),
            move |joined| {
                let (ahead, behind, branch, hash) = joined.unwrap_or((0, 0, None, 0));
                Event::GitMsg(git_view::Msg::Sync {
                    generation,
                    ahead,
                    behind,
                    branch,
                    hash,
                })
            },
        )
    }

    /// A commit / push / pull from the view, all shaped the same: mark
    /// busy, run it on a worker, report back under the current generation.
    fn git_run(&mut self, action: git_view::Busy) -> Task<Event> {
        let Some(state) = &mut self.git_ui else {
            return Task::none();
        };
        if state.busy.is_some() {
            return Task::none();
        }
        let message = state.commit_message();
        if action == git_view::Busy::Commit && message.is_empty() {
            return Task::none();
        }
        state.busy = Some(action);
        state.error = None;
        let location = state.location.clone();
        Task::perform(
            tokio::task::spawn_blocking(move || {
                use tuxflow_core::remote::git as g;
                match action {
                    git_view::Busy::Commit => g::commit_all(&location, &message),
                    git_view::Busy::Push => g::push(&location),
                    git_view::Busy::Pull => g::pull(&location),
                }
            }),
            move |joined| {
                Event::GitMsg(git_view::Msg::Done {
                    action,
                    result: joined.unwrap_or_else(|e| Err(e.to_string())),
                })
            },
        )
    }

    /// The add-project flow's workspace-touching half — duplicate checks,
    /// the detection workers, and the persistence that GTK does inside
    /// `Workspace::finalize_project`.
    fn update_add_project(&mut self, msg: add_project::Msg) -> Task<Event> {
        use add_project::Msg;
        let Some(state) = &mut self.add_project else {
            return Task::none();
        };
        // A verify/detect captures the fields at submit and proceeds with
        // those. Editing them underneath it would leave the next stage
        // configuring the path that was submitted rather than the one on
        // screen, so the inputs are frozen for its duration. The view greys
        // the two text fields to match; this guard is what also covers the
        // host picker, which iced gives no way to disable.
        if state.busy.is_some()
            && matches!(
                msg,
                Msg::HostChoice(_) | Msg::HostInput(_) | Msg::PathInput(_) | Msg::UseSuggestion(_)
            )
        {
            return Task::none();
        }
        match msg {
            Msg::Close => {
                self.add_project = None;
                self.focus_selected_terminal()
            }
            Msg::Back => {
                match state.stage {
                    // Back out of Configure to the picker that produced it,
                    // keeping what was typed — the detection is cheap to redo
                    // and a typo in the path is the reason to come back.
                    add_project::Stage::Configure => {
                        state.stage = add_project::Stage::Locate;
                        state.configure = None;
                        state.error = None;
                    }
                    _ => {
                        state.stage = add_project::Stage::Choose;
                        state.suggestions.clear();
                        state.error = None;
                        state.busy = None;
                    }
                }
                // Whichever way we went, a probe or a listing requested from
                // the stage we just left must not land on the one we are now
                // on.
                state.probe_stamp += 1;
                state.stamp += 1;
                Task::none()
            }
            Msg::Pick(kind) => {
                state.enter(kind);
                Task::none()
            }
            Msg::HostChoice(label) => {
                // Selecting an alias fills the host field with the ALIAS, so
                // ssh resolves ProxyJump/IdentityFile/User itself.
                state.host_choice = label.clone();
                state.host = match label == add_project::CUSTOM_HOST {
                    true => String::new(),
                    false => label,
                };
                state.suggestions.clear();
                state.error = None;
                self.complete_path()
            }
            Msg::HostInput(value) => {
                state.host = value;
                // A hand-typed host no longer corresponds to the picked entry.
                state.host_choice = add_project::CUSTOM_HOST.to_string();
                state.error = None;
                self.complete_path()
            }
            Msg::PathInput(value) => {
                state.path = value;
                state.error = None;
                self.complete_path()
            }
            Msg::UseSuggestion(dir) => {
                // Filling the field re-triggers completion one level deeper,
                // which is what makes the list a browser rather than a
                // one-shot guess.
                state.path = dir;
                state.error = None;
                self.complete_path()
            }
            Msg::Suggestions { stamp, dirs } => {
                // Drop a listing whose keystroke has already been superseded.
                if stamp == state.stamp {
                    state.suggestions = dirs;
                }
                Task::none()
            }
            Msg::Locate => self.locate_project(),
            Msg::Detected(found) => {
                state.configured(*found);
                Task::none()
            }
            Msg::Failed { stamp, error } => {
                if stamp == state.probe_stamp {
                    state.busy = None;
                    state.error = Some(error);
                }
                Task::none()
            }
            Msg::NameInput(value) => {
                if let Some(c) = &mut state.configure {
                    c.name = value;
                }
                Task::none()
            }
            Msg::Toggle(index, on) => {
                if let Some(c) = &mut state.configure
                    && let Some(slot) = c.selected.get_mut(index)
                {
                    *slot = on;
                }
                Task::none()
            }
            Msg::SetAll(on) => {
                if let Some(c) = &mut state.configure {
                    c.selected.iter_mut().for_each(|s| *s = on);
                }
                Task::none()
            }
            Msg::Confirm => self.finish_add_project(),
        }
    }

    /// Ask for directory completions for what is in the path field now.
    ///
    /// The stamp is bumped per keystroke and checked on arrival, so a slow
    /// listing can't paint over what is being typed now. The remote half is
    /// debounced because each probe is an ssh round trip; the local half is a
    /// `read_dir` and answers immediately.
    fn complete_path(&mut self) -> Task<Event> {
        let Some(state) = &mut self.add_project else {
            return Task::none();
        };
        state.stamp += 1;
        let stamp = state.stamp;
        if !state.can_complete() {
            state.suggestions.clear();
            return Task::none();
        }
        let host = state.probe_host();
        let prefix = state.path.trim().to_string();
        let debounce = host.is_some();
        Task::perform(
            async move {
                if debounce {
                    tokio::time::sleep(SUGGEST_DEBOUNCE).await;
                }
                tokio::task::spawn_blocking(move || remote::fs::list_dirs(host.as_deref(), &prefix))
                    .await
                    .unwrap_or_default()
            },
            move |dirs| Event::AddProjectMsg(add_project::Msg::Suggestions { stamp, dirs }),
        )
    }

    /// Commit the Locate stage: reject a duplicate, then detect. The remote
    /// half verifies over ssh first (BatchMode, so it can never hang on an
    /// auth prompt) — `probe_remote` does that as its own first step.
    fn locate_project(&mut self) -> Task<Event> {
        let Some(state) = &self.add_project else {
            return Task::none();
        };
        if !state.can_locate() {
            return Task::none();
        }
        let dir = state.path.trim().trim_end_matches('/').to_string();
        let location = match state.probe_host() {
            Some(host) => ProjectLocation::Ssh {
                host,
                dir: dir.clone(),
            },
            // Canonicalize so `/srv/app/.` and a symlinked path can't open
            // the same project twice under two keys.
            None => ProjectLocation::Local(
                PathBuf::from(&dir)
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(&dir)),
            ),
        };
        let key = location.key();

        // GTK silently drops a duplicate here, which reads as a dead button.
        // Say so instead. Resolved before re-borrowing the form.
        let duplicate = self
            .projects
            .iter()
            .find(|p| p.key() == key)
            .map(|p| p.name.clone());
        let Some(state) = &mut self.add_project else {
            return Task::none();
        };
        if let Some(name) = duplicate {
            state.error = Some(format!("Already open: {name}"));
            return Task::none();
        }
        state.probe_stamp += 1;
        let stamp = state.probe_stamp;

        match location.clone() {
            ProjectLocation::Local(path) => {
                if !path.is_dir() {
                    state.error = Some(format!("No such directory: {}", path.display()));
                    return Task::none();
                }
                // Local detection is a handful of stats on one directory —
                // the same call the startup path makes inline.
                let (name, stacks, config_loaded) = detect_for_add(&path);
                let found = add_project::Detected {
                    stamp,
                    key,
                    location,
                    name,
                    stacks,
                    config_loaded,
                };
                Task::done(Event::AddProjectMsg(add_project::Msg::Detected(Box::new(
                    found,
                ))))
            }
            ProjectLocation::Ssh { host, dir } => {
                state.busy = Some(format!("Connecting to {host}\u{2026}"));
                state.suggestions.clear();
                let probe_host = host.clone();
                Task::perform(
                    tokio::task::spawn_blocking(move || {
                        // conservative = false: the add flow offers everything
                        // detection can find, and persists the extras as
                        // custom commands so they survive the next launch.
                        remote::probe::probe_remote(&probe_host, &dir, false)
                    }),
                    move |joined| {
                        let msg = match joined {
                            Ok(Ok(probe)) => {
                                let name = probe
                                    .config
                                    .as_ref()
                                    .map(|c| c.project.name.clone())
                                    .unwrap_or_else(|| location.base_name());
                                let config_loaded = probe.config.is_some();
                                let stacks = match config_loaded {
                                    // An authored process list isn't a
                                    // detected stack — nothing to choose.
                                    true => Vec::new(),
                                    false => probe.stacks,
                                };
                                add_project::Msg::Detected(Box::new(add_project::Detected {
                                    stamp,
                                    key: key.clone(),
                                    location: location.clone(),
                                    name,
                                    stacks,
                                    config_loaded,
                                }))
                            }
                            Ok(Err(e)) => add_project::Msg::Failed {
                                stamp,
                                error: connect_hint(&host, &e),
                            },
                            Err(e) => add_project::Msg::Failed {
                                stamp,
                                error: e.to_string(),
                            },
                        };
                        Event::AddProjectMsg(msg)
                    },
                )
            }
        }
    }

    /// Commit the Configure stage — where the project actually joins the
    /// workspace.
    ///
    /// The persistence mirrors GTK's `finalize_project` exactly, and the two
    /// halves of it are not symmetric. A DESELECTED process is marked deleted
    /// so the loader keeps filtering it out. A SELECTED one usually needs
    /// nothing — `open_project` re-detects and finds it again — EXCEPT when
    /// it sits outside the conservative subset the startup loader re-detects,
    /// in which case it has no source to come back from and is persisted as a
    /// custom command instead.
    fn finish_add_project(&mut self) -> Task<Event> {
        let Some(state) = &mut self.add_project else {
            return Task::none();
        };
        let Some(c) = &state.configure else {
            return Task::none();
        };
        let name = c.name.trim().to_string();
        if name.is_empty() {
            return Task::none();
        }
        let key = c.key.clone();
        let default_dir = c.location.dir_str();
        let conservative = detector::conservative_names(&c.stacks);

        self.saved.add(&key);
        if name != c.detected_name {
            self.saved.set_name(&key, &name);
        }
        for (proc, keep) in c.flat().zip(c.selected.iter().copied()) {
            if !keep {
                self.saved.add_deleted_process(&key, &proc.name);
                continue;
            }
            if !c.config_loaded && !conservative.contains(&proc.name) {
                let mut pc = proc.clone();
                if pc.working_dir.is_none() {
                    pc.working_dir = Some(default_dir.clone());
                }
                self.saved.add_custom_command(&key, pc);
            }
        }
        self.saved.save();

        self.add_project = None;
        let task = self.open_project(&key);
        self.active = self.projects.len() - 1;
        Task::batch([task, self.poll_git()])
    }

    /// Raise the Edit Project view for a card — GTK's dialog. Ready
    /// projects only: the union below needs the entries and the detection
    /// list the load produced.
    fn open_edit_project(&mut self, project: u64) -> Task<Event> {
        let Some(pidx) = self.project_index(project) else {
            return Task::none();
        };
        if !matches!(self.projects[pidx].phase, Phase::Ready) {
            return Task::none();
        }
        // The form writes into the project it names; raising it from
        // another card switches there first, as OpenAddCommand does.
        let switched = self.active != pidx;
        self.active = pidx;
        let p = &self.projects[pidx];
        let key = p.key();

        // The pool the Hidden/Detected groups resolve from: the load-time
        // config list, plus — locally — a LIVE full detection, so commands
        // added to the project since load appear (GTK's dialog behavior;
        // its remote fallback to load-time stacks is this same trade).
        let mut pool = p.detected_configs.clone();
        if let ProjectLocation::Local(dir) = &p.location {
            for config in detector::detect_stacks(dir)
                .into_iter()
                .flat_map(|s| s.suggested_processes)
            {
                if !pool.iter().any(|c| c.name == config.name) {
                    pool.push(config);
                }
            }
        }
        let active: Vec<ProcessConfig> = p.entries.iter().map(|e| e.config.clone()).collect();
        let deleted = self
            .saved
            .deleted_processes
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let custom = self
            .saved
            .get_custom_commands(&key)
            .cloned()
            .unwrap_or_default();
        let commands = edit_project::toggle_entries(&active, &deleted, &custom, &pool);

        // Same epoch stride as the add forms: an icon fetch or listing in
        // flight when this form closes must not land in the next one.
        self.add_form_epoch += 1 << 32;
        self.edit_project = Some(edit_project::State {
            project,
            name: p.name.clone(),
            key,
            remote: p.location.is_remote(),
            icon: p
                .icon
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            icon_path: String::new(),
            suggestions: Vec::new(),
            stamp: self.add_form_epoch,
            fetch_stamp: self.add_form_epoch,
            commands,
            busy: None,
            error: None,
        });
        self.add_command = None;
        self.add_project = None;
        // Symmetric with OpenSettings: a hidden Git view would keep its
        // 2 s (possibly ssh) poll running blind underneath.
        self.git_ui = None;
        if switched {
            self.poll_git()
        } else {
            Task::none()
        }
    }

    fn update_edit_project(&mut self, msg: edit_project::Msg) -> Task<Event> {
        use edit_project::Msg;
        let Some(state) = &mut self.edit_project else {
            return Task::none();
        };
        match msg {
            Msg::Close => {
                self.edit_project = None;
                self.focus_selected_terminal()
            }
            Msg::NameInput(value) => {
                state.name = value;
                state.error = None;
                Task::none()
            }
            Msg::Toggle(index, on) => {
                if let Some(entry) = state.commands.get_mut(index) {
                    entry.on = on;
                }
                Task::none()
            }
            Msg::IconPathInput(value) => {
                state.icon_path = value;
                state.error = None;
                self.complete_icon_path()
            }
            Msg::IconSuggestions { stamp, paths } => {
                // Drop a listing whose keystroke has been superseded.
                if stamp == state.stamp {
                    state.suggestions = paths;
                }
                Task::none()
            }
            Msg::UseIconSuggestion(path) => {
                let descend = path.ends_with('/');
                state.icon_path = path;
                state.error = None;
                if descend {
                    // A directory lists one level deeper — the browser
                    // behavior, as in the add-project path field.
                    self.complete_icon_path()
                } else {
                    self.commit_icon_path()
                }
            }
            Msg::CommitIconPath => self.commit_icon_path(),
            Msg::IconAutoDetect => self.icon_auto_detect(),
            Msg::IconFetched { stamp, path } => {
                if stamp != state.fetch_stamp {
                    return Task::none();
                }
                state.busy = None;
                match path {
                    Some(local) => {
                        state.icon = Some(local);
                        state.error = None;
                    }
                    None => state.error = Some(String::from("No usable image found.")),
                }
                Task::none()
            }
            Msg::IconClear => {
                state.icon = None;
                Task::none()
            }
            Msg::CopyPath => {
                let id = state.project;
                match self.project_index(id) {
                    Some(pidx) => {
                        iced::clipboard::write(copyable_path(&self.projects[pidx].location))
                    }
                    None => Task::none(),
                }
            }
            Msg::OpenEditor => {
                let id = state.project;
                self.update(Event::OpenInEditor(id))
            }
            Msg::Save => self.save_edit_project(),
            Msg::RemoveProject => {
                let id = state.project;
                self.update(Event::ConfirmRequest(ConfirmAction::RemoveProject(id)))
            }
        }
    }

    /// Ask for icon completions (directories to descend + image files) for
    /// what is in the Edit Project path field now — the add-project
    /// completion idiom over `list_icon_paths`, stamped and debounced the
    /// same way.
    fn complete_icon_path(&mut self) -> Task<Event> {
        let Some(state) = &mut self.edit_project else {
            return Task::none();
        };
        state.stamp += 1;
        let stamp = state.stamp;
        let prefix = state.icon_path.trim().to_string();
        let id = state.project;
        if !prefix.starts_with('/') {
            state.suggestions.clear();
            return Task::none();
        }
        let host = self
            .project_index(id)
            .and_then(|pidx| self.projects[pidx].location.host().map(String::from));
        let debounce = host.is_some();
        Task::perform(
            async move {
                if debounce {
                    tokio::time::sleep(SUGGEST_DEBOUNCE).await;
                }
                tokio::task::spawn_blocking(move || {
                    remote::fs::list_icon_paths(host.as_deref(), &prefix)
                })
                .await
                .unwrap_or_default()
            },
            move |paths| Event::EditProjectMsg(edit_project::Msg::IconSuggestions { stamp, paths }),
        )
    }

    /// Commit the icon field: a local path is checked and adopted as-is; a
    /// remote one is pulled into the icon cache on a worker first — every
    /// saved icon is a local file, which is what keeps Save synchronous
    /// (GTK's picker runs the same `cache_remote_icon` at pick time).
    fn commit_icon_path(&mut self) -> Task<Event> {
        let (id, key) = match &self.edit_project {
            Some(state) => (state.project, state.key.clone()),
            None => return Task::none(),
        };
        let host = self
            .project_index(id)
            .and_then(|pidx| self.projects[pidx].location.host().map(String::from));
        let Some(state) = &mut self.edit_project else {
            return Task::none();
        };
        let path = state.icon_path.trim().to_string();
        if !path.starts_with('/') || path.ends_with('/') {
            state.error = Some(String::from("Enter the absolute path of an image file."));
            return Task::none();
        }
        state.suggestions.clear();
        match host {
            None => {
                if std::path::Path::new(&path).is_file() {
                    state.icon = Some(path);
                    state.error = None;
                } else {
                    state.error = Some(format!("No such file: {path}"));
                }
                Task::none()
            }
            Some(host) => {
                state.fetch_stamp += 1;
                let stamp = state.fetch_stamp;
                state.busy = Some(format!("Fetching from {host}\u{2026}"));
                Task::perform(
                    tokio::task::spawn_blocking(move || {
                        // Own ssh permit, as the probe's fetch takes one.
                        let _permit = remote::ssh_permit();
                        remote::icon::cache_remote_icon(&host, &path, &key)
                    }),
                    move |joined| {
                        Event::EditProjectMsg(edit_project::Msg::IconFetched {
                            stamp,
                            path: joined.ok().flatten(),
                        })
                    },
                )
            }
        }
    }

    /// The icon row's Auto-detect: a local project scans its own disk
    /// inline; a remote one reruns the probe's icon fetch on a worker —
    /// which GTK's dialog never offered remotely (its scan is local-only),
    /// the add-agent kind of deliberate improvement rather than a port.
    fn icon_auto_detect(&mut self) -> Task<Event> {
        let Some(id) = self.edit_project.as_ref().map(|s| s.project) else {
            return Task::none();
        };
        let Some(pidx) = self.project_index(id) else {
            return Task::none();
        };
        let location = self.projects[pidx].location.clone();
        let Some(state) = &mut self.edit_project else {
            return Task::none();
        };
        match location {
            ProjectLocation::Local(dir) => {
                match icon_detector::detect_icon(&dir) {
                    Some(found) => {
                        state.icon = Some(found);
                        state.error = None;
                    }
                    None => state.error = Some(String::from("No icon found in the project.")),
                }
                Task::none()
            }
            ProjectLocation::Ssh { host, dir } => {
                state.fetch_stamp += 1;
                let stamp = state.fetch_stamp;
                state.busy = Some(format!("Looking for an icon on {host}\u{2026}"));
                Task::perform(
                    tokio::task::spawn_blocking(move || {
                        let _permit = remote::ssh_permit();
                        remote::icon::fetch_remote_icon(&host, &dir)
                    }),
                    move |joined| {
                        Event::EditProjectMsg(edit_project::Msg::IconFetched {
                            stamp,
                            path: joined.ok().flatten(),
                        })
                    },
                )
            }
        }
    }

    /// Apply the Edit Project form — GTK's `EditProjectResult` handler in
    /// `project_list.rs`, in iced terms: rename, icon, then the command
    /// toggles.
    fn save_edit_project(&mut self) -> Task<Event> {
        let Some(state) = self.edit_project.take() else {
            return Task::none();
        };
        let Some(pidx) = self.project_index(state.project) else {
            return Task::none();
        };
        let name = state.name.trim().to_string();
        if name.is_empty() {
            // Refuse but KEEP the form, the add-command idiom: taking it
            // down with everything set is worse than a red line.
            let mut state = state;
            state.error = Some(String::from("A name is required."));
            self.edit_project = Some(state);
            return Task::none();
        }
        let key = self.projects[pidx].key();

        // Rename only when it happened — writing an unchanged name would
        // pin a detected name as an override, and a later tuxflow.toml
        // edit would then never show (the add flow's detected_name rule).
        if name != self.projects[pidx].name {
            self.projects[pidx].name = name.clone();
            self.saved.set_name(&key, &name);
        }

        // The icon pick, mirrored into the card the way the load resolves
        // it. `set_icon(None)` clears the entry — Reset to Initials.
        self.saved.set_icon(&key, state.icon.clone());
        self.projects[pidx].icon = usable_icon(state.icon.clone());

        // Disables first, GTK's order — each is the full deletion the
        // context menu's Delete Command performs (stop, drop the custom
        // copy, record the deletion, remove the entry), found by NAME:
        // every removal shifts the indices under the rest.
        let (enabled, disabled) = edit_project::diff(&state.commands);
        for gone in &disabled {
            if let Some(index) = self.projects[pidx]
                .entries
                .iter()
                .position(|e| &e.config.name == gone)
            {
                self.delete_process(pidx, index);
            }
        }

        // Enables: unmark the deletion, persist as the custom command that
        // overrides same-named detection on every future load (GTK saves
        // every enable), and join the sidebar STOPPED — enabling is not
        // starting.
        let default_dir = self.projects[pidx].location.dir_str();
        for mut config in enabled {
            if self.projects[pidx]
                .entries
                .iter()
                .any(|e| e.config.name == config.name)
            {
                continue;
            }
            if config.working_dir.is_none() {
                config.working_dir = Some(default_dir.clone());
            }
            self.saved.unmark_process_deleted(&key, &config.name);
            self.saved.add_custom_command(&key, config.clone());
            self.projects[pidx].entries.push(ProcessEntry::new(config));
        }
        self.focus_selected_terminal()
    }

    fn update_git_view(&mut self, msg: git_view::Msg) -> Task<Event> {
        use git_view::Msg;
        match msg {
            Msg::Close => {
                self.git_ui = None;
                self.focus_selected_terminal()
            }
            Msg::Refresh => Task::batch([self.git_load_files(), self.git_refresh_sync(true)]),
            Msg::SelectFile(index) => {
                let Some(state) = &mut self.git_ui else {
                    return Task::none();
                };
                state.selected = Some(index);
                self.git_load_diff()
            }
            Msg::MessageAction(action) => {
                if let Some(state) = &mut self.git_ui {
                    state.message.perform(action);
                }
                Task::none()
            }
            Msg::Commit => self.git_run(git_view::Busy::Commit),
            Msg::Push => self.git_run(git_view::Busy::Push),
            Msg::Pull => self.git_run(git_view::Busy::Pull),
            Msg::DismissError => {
                if let Some(state) = &mut self.git_ui {
                    state.error = None;
                }
                Task::none()
            }
            Msg::Tick(stamp) => {
                let Some(state) = &self.git_ui else {
                    return Task::none();
                };
                // A stale chain from a previous open: let it die.
                if state.stamp != stamp {
                    return Task::none();
                }
                let ticks = state.ticks;
                if let Some(state) = &mut self.git_ui {
                    state.ticks = ticks.wrapping_add(1);
                }
                let next = Task::perform(tokio::time::sleep(Duration::from_secs(2)), move |_| {
                    Event::GitMsg(Msg::Tick(stamp))
                });
                Task::batch([self.git_refresh_sync(ticks % 15 == 0), next])
            }
            Msg::Files { generation, files } => {
                let Some(state) = &mut self.git_ui else {
                    return Task::none();
                };
                if state.generation != generation {
                    return Task::none();
                }
                state.loading = false;
                // Hold the selection on the same PATH, not the same index:
                // a file leaving the list above the selected one would
                // otherwise silently move the selection onto its neighbour.
                let held = state.selected_file().map(|f| f.path.clone());
                state.files = files;
                state.selected = held
                    .and_then(|path| state.files.iter().position(|f| f.path == path))
                    .or(if state.files.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                self.git_load_diff()
            }
            Msg::Diff {
                generation,
                path,
                diff,
            } => {
                // Generation gates cross-reload staleness (same file, older
                // content); the path gates same-generation staleness (two
                // quick clicks — SelectFile shares the list's generation).
                // A dropped arrival leaves diff_loading alone: whichever
                // newer load superseded this one is still on its way.
                if let Some(state) = &mut self.git_ui
                    && state.generation == generation
                    && state.selected_file().map(|f| f.path.as_str()) == Some(path.as_str())
                {
                    state.diff_loading = false;
                    state.diff = Some(*diff);
                }
                Task::none()
            }
            Msg::Sync {
                generation,
                ahead,
                behind,
                branch,
                hash,
            } => {
                let Some(state) = &mut self.git_ui else {
                    return Task::none();
                };
                if state.generation != generation {
                    return Task::none();
                }
                // A write action owns its own counter until it reports —
                // repainting the pre-push number over a push in flight
                // reads as "the push did nothing".
                if state.busy.is_none() {
                    state.ahead = ahead;
                    state.behind = behind;
                }
                if branch.is_some() {
                    state.branch = branch;
                }
                let changed = state.last_hash != 0 && state.last_hash != hash;
                state.last_hash = hash;
                if changed {
                    self.git_load_files()
                } else {
                    Task::none()
                }
            }
            Msg::Done { action, result } => {
                let Some(state) = &mut self.git_ui else {
                    return Task::none();
                };
                // Gate on the busy flag, not the generation: only one write
                // action is ever in flight, so "the action I'm waiting for"
                // is the exact question — while a generation gate let ANY
                // list reload (Refresh, or a mid-commit hash change on the
                // 2 s tick) orphan `busy` and wedge the view in "Pushing…"
                // with every button disabled. A Done surviving from a
                // closed-and-reopened view finds busy == None and drops.
                if state.busy != Some(action) {
                    return Task::none();
                }
                state.busy = None;
                match result {
                    Ok(()) => {
                        if action == git_view::Busy::Commit {
                            state.message = iced::widget::text_editor::Content::new();
                        }
                        // The counters and the file list both moved; a
                        // fetch isn't needed since we just talked to the
                        // remote ourselves.
                        Task::batch([
                            self.git_load_files(),
                            self.git_refresh_sync(false),
                            // The status bar's chip is showing the numbers
                            // from before this action.
                            self.poll_git(),
                        ])
                    }
                    Err(detail) => {
                        state.error = Some((action.failure_heading().to_string(), detail));
                        Task::none()
                    }
                }
            }
        }
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
        // GTK's skip_port_detection: agent and ssh terminals are prose
        // surfaces, never scanned — a URL the model *mentions* is not an
        // address this process serves.
        if !port_detector::scans_ports(&project.entries[index].config.category) {
            return Task::none();
        }
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
            AppAction::ToggleSidebar => self.set_sidebar(!self.sidebar_visible),
            AppAction::FilterSidebar => self.toggle_filter(),
            AppAction::SelectProcessN(n) => {
                // Routed through SelectProcess rather than assigning here:
                // crossing to another project has to refresh the git chip,
                // and that rule belongs in one place.
                match self.switch_targets().get(n as usize - 1) {
                    Some(&(pidx, index)) => {
                        let project = self.projects[pidx].id;
                        self.update(Event::SelectProcess { project, index })
                    }
                    None => Task::none(),
                }
            }
            AppAction::SelectProjectN(n) => {
                let idx = n as usize - 1;
                if idx < self.projects.len() && idx != self.active {
                    self.active = idx;
                    return Task::batch([self.focus_selected_terminal(), self.poll_git()]);
                }
                Task::none()
            }
        }
    }

    /// Flip the sidebar and start its glide. A no-op when it is already
    /// there OR already heading there — a rail button that reopens a
    /// mid-expand sidebar must not restart the ramp and snap it back.
    fn set_sidebar(&mut self, visible: bool) -> Task<Event> {
        if self.sidebar_visible == visible {
            return Task::none();
        }
        // Toggled mid-glide, the new ramp picks up where this one stands:
        // reversed, the eased position `f` becomes `1 - f`, whichever way
        // it was going (the two cases work out the same).
        let resume = if self.sidebar_anim.settled() {
            0.0
        } else {
            1.0 - self.sidebar_anim.eased()
        };
        self.sidebar_visible = visible;
        let generation = self.sidebar_anim.restart_at(resume);
        Task::perform(tokio::time::sleep(FRAME), move |_| {
            Event::SidebarTick(generation)
        })
    }

    /// GTK's search toggle: opening focuses the entry, closing clears the
    /// query (the sidebar un-narrows) and hands focus back to the terminal.
    fn toggle_filter(&mut self) -> Task<Event> {
        self.filter_open = !self.filter_open;
        if self.filter_open {
            // The entry lives in the sidebar — filtering a collapsed
            // sidebar reopens it (rail button / Ctrl+F while hidden).
            let open = self.set_sidebar(true);
            Task::batch([
                open,
                iced::widget::operation::focus(self.filter_input.clone()),
            ])
        } else {
            self.filter_query.clear();
            self.focus_selected_terminal()
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

    /// What Ctrl+1..9 reaches, in the order the sidebar shows it: every
    /// project in workspace order, each project's rows in category order,
    /// RUNNING processes only — GTK's `switch_to_nth_global`.
    ///
    /// Three properties come from GTK and all three matter. The sequence is
    /// GLOBAL, so the chords address the whole sidebar rather than restarting
    /// per card; it is drawn order, not `entries` order, so the number on a
    /// row matches counting rows down the screen (an agent sorts to the top
    /// of its card whatever its saved index); and it skips everything not
    /// running, because the switcher's job is to reach a live terminal and a
    /// stopped row has none to focus. The same list draws the hints, so a
    /// row can never advertise a chord that goes elsewhere.
    fn switch_targets(&self) -> Vec<(usize, usize)> {
        switch_targets_of(self.projects.iter().map(|p| p.entries.as_slice()))
    }

    /// (project id, entry index) pairs matching the palette query, in
    /// sidebar order. Case-insensitive substring over "project process".
    fn palette_matches(&self) -> Vec<(u64, usize)> {
        let needle = self.palette_query.to_lowercase();
        let mut out = Vec::new();
        for project in &self.projects {
            for i in sidebar_order(&project.entries) {
                let hay =
                    format!("{} {}", project.name, project.entries[i].config.name).to_lowercase();
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
        let (Some(term), true) = (entry.term_id, entry.is_running()) else {
            return Task::none();
        };
        let run = entry.run_id;
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
                run,
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
        // The Git Changes view belongs to ONE project. `self.active` moves
        // from eight different places (row click, palette, digit switcher,
        // close, reorder…), so this is checked here rather than chased
        // through each of them — a stale view would otherwise sit in the
        // main pane showing another project's repo while the sidebar and
        // the status bar both say something else, and keep polling it.
        if let Some(state) = &self.git_ui
            && self.active_project().map(|p| p.id) != Some(state.project)
        {
            self.git_ui = None;
        }
        // Same contract for the Edit Project form: it edits ONE project,
        // and a removal or a sidebar switch underneath it would leave a
        // form whose Save writes into the wrong card.
        if let Some(state) = &self.edit_project
            && self.active_project().map(|p| p.id) != Some(state.project)
        {
            self.edit_project = None;
        }
        // Running-tier flips stamp last_used and re-sort the sidebar —
        // checked here for the same reason as the git view above: the
        // statuses move from many places (clicks, async exits, reattach).
        self.refresh_recent_order();
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
            Event::RestoreMaximize => {
                iced::window::latest().and_then(|id| iced::window::maximize(id, true))
            }
            Event::RestoreMeasured { id, actual } => {
                let w = &self.settings.window;
                log::info!(
                    "restore measure: actual {actual:?} saved ({:?},{:?})",
                    w.x,
                    w.y
                );
                if let (Some(sx), Some(sy), Some(actual), false) = (w.x, w.y, actual, w.maximized) {
                    let (dx, dy) = (actual.x - sx as f32, actual.y - sy as f32);
                    // Only correct frame-sized deltas. A large one means the
                    // WM CLAMPED the restore (saved position from a monitor
                    // that is gone, or a settings file that traveled between
                    // machines) or the user is already dragging — mirroring
                    // that delta would shove the window off-screen in the
                    // opposite direction, and WMs honor explicit moves.
                    let frame_sized = dx.abs() <= 100.0 && dy.abs() <= 100.0;
                    if (dx != 0.0 || dy != 0.0) && frame_sized {
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
                // Symmetric with `open_git_changes` clearing settings_ui:
                // "what is the main area showing?" stays a single answer,
                // and a hidden Git view would keep its 2 s (possibly ssh)
                // poll chain running blind underneath.
                self.git_ui = None;
                Task::none()
            }
            Event::ToggleSidebar => self.set_sidebar(!self.sidebar_visible),
            Event::ToggleFilter => self.toggle_filter(),
            Event::FilterInput(query) => {
                self.filter_query = query;
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
                    Ok(ProbeOk {
                        name,
                        configs,
                        live_sessions,
                        icon,
                    }) => {
                        let key = self.projects[pidx].key();
                        if self.saved.get_name(&key).is_none()
                            && let Some(name) = name
                        {
                            self.projects[pidx].name = name;
                        }
                        // Remote projects have no local dir to scan — the icon
                        // arrives already fetched into the cache by the probe.
                        self.projects[pidx].icon = usable_icon(icon_detector::resolve_icon(
                            &mut self.saved,
                            &key,
                            None,
                            icon,
                        ));
                        self.projects[pidx].detected_configs = configs.clone();
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
                if self.projects[pidx].entries[index].is_running() {
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
            Event::StartAll(project) => {
                // GTK's spawn_project_group: the marked processes only.
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                let mut tasks = Vec::new();
                for index in 0..self.projects[pidx].entries.len() {
                    let entry = &self.projects[pidx].entries[index];
                    let idle = matches!(entry.status, Status::Stopped | Status::Crashed(_));
                    if entry.config.start_with_project && idle {
                        if self.projects[pidx].entries[index].is_running() {
                            self.stop(pidx, index);
                        }
                        tasks.push(self.start_fresh(pidx, index));
                    }
                }
                Task::batch(tasks)
            }
            Event::RestartAll(project) => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                let mut tasks = Vec::new();
                for index in 0..self.projects[pidx].entries.len() {
                    if self.projects[pidx].entries[index].is_running() {
                        self.stop(pidx, index);
                        tasks.push(self.start_fresh(pidx, index));
                    }
                }
                Task::batch(tasks)
            }
            Event::StopAll(project) => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                for index in 0..self.projects[pidx].entries.len() {
                    // Running, restarting and reconnecting alike — "stop
                    // all" also cancels pending comebacks.
                    let active = !matches!(
                        self.projects[pidx].entries[index].status,
                        Status::Stopped | Status::Crashed(_)
                    );
                    if active {
                        self.stop(pidx, index);
                    }
                }
                Task::none()
            }
            Event::RowEnter { project, index } => {
                let target = Some((project, index));
                if self.hovered_row == target {
                    return Task::none();
                }
                self.hovered_row = target;
                let generation = self.hover_anim.start();
                Task::perform(tokio::time::sleep(FRAME), move |_| {
                    Event::HoverTick(generation)
                })
            }
            Event::RowExit { project, index } => {
                // Enter of the next row may already have retargeted us —
                // only clear if this exit still owns the state.
                if self.hovered_row == Some((project, index)) {
                    self.hovered_row = None;
                }
                Task::none()
            }
            Event::CursorMoved(position) => {
                self.cursor = position;
                Task::none()
            }
            Event::CtrlHeld(down) => {
                self.ctrl_held = down;
                Task::none()
            }
            Event::OpenContextMenu { project, index } => {
                self.context_menu = Some(MenuTarget {
                    project,
                    index,
                    at: self.cursor,
                });
                // A modal grab, done the only way a Stack layer can: layers
                // don't capture keyboard events, so a focused terminal
                // underneath would keep eating them — Esc meant for the
                // menu reaches a running agent as "interrupt". Dismissal
                // refocuses.
                TerminalView::unfocus()
            }
            Event::CloseContextMenu => {
                self.context_menu = None;
                self.focus_selected_terminal()
            }
            Event::MenuAction(inner) => {
                self.context_menu = None;
                let task = self.update(*inner);
                // Hand focus back unless the action raised the next modal
                // layer itself (Remove Project / Delete Command open the
                // confirm card) — refocusing under THAT would re-open the
                // key leak the unfocus exists to stop.
                if self.confirm.is_none() && self.notice.is_none() {
                    Task::batch([task, self.focus_selected_terminal()])
                } else {
                    task
                }
            }
            Event::CopyText(text) => iced::clipboard::write(text),
            Event::OpenInEditor(project) => {
                if let Some(pidx) = self.project_index(project) {
                    tuxflow_core::util::editor::open_in_editor(&self.projects[pidx].location);
                }
                Task::none()
            }
            Event::ToggleProcessAt { project, index } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                let Some(entry) = self.projects[pidx].entries.get(index) else {
                    return Task::none();
                };
                if entry.is_running() {
                    self.stop(pidx, index);
                    Task::none()
                } else {
                    if self.projects[pidx].entries[index].is_running() {
                        self.stop(pidx, index);
                    }
                    self.start_fresh(pidx, index)
                }
            }
            Event::ResumeAgentAt { project, index } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                let Some(entry) = self.projects[pidx].entries.get(index) else {
                    return Task::none();
                };
                let Some(resume) = resume_command_for(&entry.config.command) else {
                    return Task::none();
                };
                // GTK's spawn_with_command_override: a running session is
                // replaced, and the override lasts one spawn — a later
                // crash restarts the configured command.
                if self.projects[pidx].entries[index].is_running() {
                    self.stop(pidx, index);
                }
                self.projects[pidx].entries[index].command_override = Some(resume);
                self.projects[pidx].selected = index;
                self.active = pidx;
                self.start_fresh(pidx, index)
            }
            Event::EditProcessAt { project, index } => {
                if let Some(pidx) = self.project_index(project) {
                    self.active = pidx;
                    self.projects[pidx].selected = index;
                    self.open_edit_form(pidx, index);
                }
                Task::none()
            }
            Event::ConfirmRequest(action) => {
                self.confirm = Some(action);
                // Same modal grab as the context menu: keys must answer the
                // card, not the shell at the prompt underneath it.
                TerminalView::unfocus()
            }
            Event::ConfirmCancel => {
                self.confirm = None;
                self.focus_selected_terminal()
            }
            Event::ConfirmProceed => {
                match self.confirm.take() {
                    Some(ConfirmAction::RemoveProject(project)) => {
                        if let Some(pidx) = self.project_index(project) {
                            self.close_project(pidx);
                        }
                    }
                    Some(ConfirmAction::DeleteProcess { project, index }) => {
                        if let Some(pidx) = self.project_index(project) {
                            self.delete_process(pidx, index);
                        }
                    }
                    None => {}
                }
                self.focus_selected_terminal()
            }
            Event::HoverTick(generation) => {
                if self.hovered_row.is_none() || !self.hover_anim.tick(generation, HOVER_SLIDE_MS) {
                    return Task::none();
                }
                Task::perform(tokio::time::sleep(FRAME), move |_| {
                    Event::HoverTick(generation)
                })
            }
            Event::SidebarTick(generation) => {
                if !self.sidebar_anim.tick(generation, SIDEBAR_SLIDE_MS) {
                    return Task::none();
                }
                Task::perform(tokio::time::sleep(FRAME), move |_| {
                    Event::SidebarTick(generation)
                })
            }
            Event::ActivityTick => {
                // Which agents are busy, on core's shared hysteresis. A
                // card joins the sweep here; it only leaves at a pass
                // boundary, so nothing blinks out mid-card.
                for project in &mut self.projects {
                    let working = project
                        .entries
                        .iter_mut()
                        .fold(false, |any, entry| entry.sample_activity() | any);
                    project.sweeping |= working;
                }
                let start = self
                    .projects
                    .iter()
                    .any(|p| p.sweeping)
                    .then(|| self.sweep.start())
                    .flatten();
                let next = Task::perform(tokio::time::sleep(activity::SAMPLE_INTERVAL), |_| {
                    Event::ActivityTick
                });
                match start {
                    Some(generation) => {
                        Task::batch([next, Task::done(Event::SweepTick(generation))])
                    }
                    None => next,
                }
            }
            Event::SweepTick(generation) => {
                let Some(wrapped) = self.sweep.tick(generation) else {
                    return Task::none();
                };
                if wrapped {
                    for project in &mut self.projects {
                        project.sweeping = project.agent_working();
                    }
                    if !self.projects.iter().any(|p| p.sweeping) {
                        self.sweep.running = false;
                        return Task::none();
                    }
                }
                Task::perform(tokio::time::sleep(SWEEP_FRAME), move |_| {
                    Event::SweepTick(generation)
                })
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
            Event::GitPolled {
                project,
                status,
                diffstat,
            } => {
                if let Some(pidx) = self.project_index(project) {
                    self.projects[pidx].git = status;
                    self.projects[pidx].diffstat = diffstat;
                }
                Task::none()
            }
            Event::GitSync => self.start_git_sync(),
            Event::GitSynced { project, result } => {
                self.git_syncing.remove(&project);
                let active = self.active_project().map(|p| p.id) == Some(project);
                let mut tasks = Vec::new();
                if let Err(detail) = result {
                    // A failure surfaces even if the user has moved on, but
                    // then it must say WHOSE sync failed — an unattributed
                    // notice reads as the active project's, and its "resolve
                    // it manually" hint would open the wrong repo.
                    let heading = match (active, self.project_index(project)) {
                        (false, Some(pidx)) => {
                            format!("Sync Failed \u{2014} {}", self.projects[pidx].name)
                        }
                        _ => String::from("Sync Failed"),
                    };
                    tasks.push(self.notify_git_failure(&heading, &detail));
                }
                // Repaint the counters from what the sync actually left
                // behind, not from what we assumed it would. poll_git reads
                // the ACTIVE project, so only fire it while that is still
                // the synced one — otherwise the on-switch poll covers it.
                if active {
                    tasks.push(self.poll_git());
                }
                Task::batch(tasks)
            }
            Event::OpenGitChanges => self.open_git_changes(),
            Event::NoticeDismiss => {
                self.notice = None;
                self.focus_selected_terminal()
            }
            Event::GitMsg(msg) => self.update_git_view(msg),
            Event::ClearTerminal => {
                if let Some(project) = self.projects.get_mut(self.active)
                    && let Some(entry) = project.entries.get_mut(project.selected)
                    && let Some(terminal) = entry.terminal.as_mut()
                {
                    terminal.clear();
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
                // Esc peels overlays top-down: notice, confirmation card,
                // context menu, then the full-pane views like the other
                // panels.
                if matches!(
                    key.as_ref(),
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                ) {
                    if self.notice.is_some() {
                        self.notice = None;
                        return self.focus_selected_terminal();
                    }
                    if self.confirm.is_some() {
                        self.confirm = None;
                        return self.focus_selected_terminal();
                    }
                    if self.context_menu.is_some() {
                        self.context_menu = None;
                        return self.focus_selected_terminal();
                    }
                    if self.settings_ui.is_some() {
                        self.settings_ui = None;
                        return Task::none();
                    }
                    // The commit box is a text_editor, which — like the
                    // filter's text_input — eats the first Esc to unfocus
                    // itself. The second reaches here and closes the view.
                    if self.git_ui.is_some() {
                        self.git_ui = None;
                        return self.focus_selected_terminal();
                    }
                    // The two add forms are full-pane views like the ones
                    // above, so Esc closes them the same way. Their text
                    // fields eat the first Esc to unfocus, as everywhere
                    // else in this shell.
                    if self.add_project.is_some() {
                        self.add_project = None;
                        return self.focus_selected_terminal();
                    }
                    if self.add_command.is_some() {
                        self.add_command = None;
                        return self.focus_selected_terminal();
                    }
                    if self.edit_project.is_some() {
                        self.edit_project = None;
                        return self.focus_selected_terminal();
                    }
                }
                // While a modal layer is up, the remaining chords stay
                // dead: they act on the SELECTION, and reordering or
                // closing processes under a "Delete 'dev'?" card silently
                // retargets what Proceed is about to delete. GTK's popover
                // and AlertDialog are grabs; this is that grab's keyboard
                // half (the unfocus on raise is the terminal half).
                if self.notice.is_some() || self.confirm.is_some() || self.context_menu.is_some() {
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
                if self.filter_open
                    && matches!(
                        key.as_ref(),
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                    )
                {
                    return self.toggle_filter();
                }
                if let Some(action) = self.app_keys.action_for(&key, modifiers) {
                    return self.apply_action(action);
                }
                match key.as_ref() {
                    // Reaching here means the widget found no TEXT to paste
                    // — the clipboard holds an image (or nothing). Plain
                    // Ctrl+V only falls through on remote agent terminals,
                    // where start() rebinds it to Paste (GTK's hardcoded
                    // branch); the guard keeps a stray unfocused chord from
                    // typing into a terminal it was never aimed at.
                    iced::keyboard::Key::Character(c)
                        if c.eq_ignore_ascii_case("v")
                            && modifiers.control()
                            && (modifiers.shift() || self.remote_agent_selected()) =>
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
                run,
                result,
            } => {
                match result {
                    Ok(bytes) => {
                        // Only if the run that asked is still the one on the
                        // other end — a restart between paste and upload
                        // must not type a stale path into the fresh run.
                        let target = self.project_index(project).and_then(|pidx| {
                            self.projects[pidx]
                                .entries
                                .iter_mut()
                                .find(|e| e.term_id == Some(term) && e.run_id == run)
                                .filter(|e| e.is_running())
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
                {
                    open_badge(project, entry);
                }
                Task::none()
            }
            Event::OpenBadgeFor { project, index } => {
                if let Some(pidx) = self.project_index(project)
                    && let Some(entry) = self.projects[pidx].entries.get(index)
                {
                    open_badge(&self.projects[pidx], entry);
                }
                Task::none()
            }
            Event::OpenAddProject => {
                // The host list is read once per raise, not per frame — it is
                // a file read, and the picker is rebuilt on every keystroke.
                //
                // The epoch stride keeps this instance's stamps disjoint from
                // every earlier one's: in-flight listings and probes outlive
                // a closed form, and with counters restarting at 0 a reopened
                // form would accept the abandoned instance's replies as its
                // own — up to and including a Configure stage for the
                // previously typed project.
                self.add_form_epoch += 1 << 32;
                self.add_project = Some(add_project::State::new(
                    ssh::parse_ssh_config(),
                    self.add_form_epoch,
                ));
                self.add_command = None;
                self.edit_project = None;
                Task::none()
            }
            Event::AddProjectMsg(msg) => self.update_add_project(msg),
            Event::OpenAddCommand { project, agent } => {
                let Some(pidx) = self.project_index(project) else {
                    return Task::none();
                };
                // Submit adds to the ACTIVE project, so raising the form on
                // another card has to switch to it first (as selecting one
                // of its processes would).
                let switched = self.active != pidx;
                self.active = pidx;
                self.add_command = Some(ProcessForm {
                    name: String::new(),
                    command: String::new(),
                    working_dir: String::new(),
                    agent,
                    name_touched: false,
                    start_with_project: false,
                    auto_restart: false,
                    open_in_browser: false,
                    editing: None,
                    original_category: if agent {
                        ProcessCategory::Agent
                    } else {
                        ProcessCategory::Command
                    },
                    error: None,
                });
                // Mutually exclusive with the other form panes (see
                // `open_edit_form`).
                self.add_project = None;
                self.edit_project = None;
                if switched {
                    self.poll_git()
                } else {
                    Task::none()
                }
            }
            Event::OpenEditProcess => {
                if let Some(project) = self.active_project() {
                    let (pidx, index) = (self.active, project.selected);
                    self.open_edit_form(pidx, index);
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
                self.delete_process(pidx, index);
                Task::none()
            }
            Event::AddCommandName(value) => {
                if let Some(form) = &mut self.add_command {
                    // An emptied field goes back to being the preset's to
                    // fill, so clearing it and picking another agent works.
                    form.name_touched = !value.trim().is_empty();
                    form.name = value;
                    form.error = None;
                }
                Task::none()
            }
            Event::AgentPreset(index) => {
                let taken: Vec<String> = self
                    .active_project()
                    .map(|p| p.entries.iter().map(|e| e.config.name.clone()).collect())
                    .unwrap_or_default();
                if let Some(form) = &mut self.add_command
                    && let Some(preset) = agents::AGENT_PRESETS.get(index)
                {
                    form.command = preset.command.to_string();
                    if !form.name_touched {
                        form.name = agents::unique_agent_name(&taken, preset.slug);
                    }
                }
                Task::none()
            }
            Event::AddCommandCommand(value) => {
                if let Some(form) = &mut self.add_command {
                    form.command = value;
                    form.error = None;
                }
                Task::none()
            }
            Event::AddCommandCancel => {
                self.add_command = None;
                Task::none()
            }
            Event::OpenEditProject(project) => self.open_edit_project(project),
            Event::EditProjectMsg(msg) => self.update_edit_project(msg),
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
                        let mut form = form;
                        form.error = Some(String::from("A command is required."));
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
                    let mut form = form;
                    form.error = Some(String::from("A name and a command are both required."));
                    self.add_command = Some(form);
                    return Task::none();
                }
                let pidx = self.active;
                let Some(project) = self.projects.get(pidx) else {
                    // No project to add to — nothing the form can do.
                    return Task::none();
                };
                if project.entries.iter().any(|e| e.config.name == name) {
                    // Refuse, but KEEP the form: taking it down here threw
                    // away everything typed, with no hint why.
                    let mut form = form;
                    form.error = Some(format!(
                        "A process named \u{201c}{name}\u{201d} already exists in this project."
                    ));
                    self.add_command = Some(form);
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
                if let BackendCommand::ProcessAlacrittyEvent(run, ev) = &cmd {
                    // A terminal spans runs, so the queue can still hold a
                    // PREVIOUS run's events — most damagingly its ChildExit/
                    // Exit, parked there when a child died right as the user
                    // hit restart. Unstamped, that flipped the fresh run to
                    // Crashed and fed a crash banner into its running grid.
                    let current = self.projects[pidx].entries[index]
                        .terminal
                        .as_ref()
                        .map(|t| t.backend().run_generation());
                    if current != Some(*run) {
                        return Task::none();
                    }
                    match ev {
                        AEvent::Wakeup => {
                            rescan = true;
                            // The repaint signal the working-agent sweep
                            // reads — VTE's contents-changed on GTK.
                            let entry = &mut self.projects[pidx].entries[index];
                            entry.activity_burst = entry.activity_burst.saturating_add(1);
                            entry.last_activity = Some(Instant::now());
                        }
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
        let mut body = row![].height(Length::Fill);
        if self.sidebar_visible || !self.sidebar_anim.settled() {
            // Mid-glide the sidebar keeps its own content, clipped to the
            // animated width. The rail is a different layout — the same
            // four icons stacked instead of in a row — so swapping to it
            // before the collapse lands reads as a flicker, not a slide.
            body = body.push(self.view_sidebar()).push(vline());
        } else {
            // Collapsed: the cluster survives as a slim icon rail, so the
            // toggle (and everything else) stays reachable by mouse.
            body = body.push(self.view_rail()).push(vline());
        }
        body = body.push(
            container(self.view_main())
                .width(Length::Fill)
                .height(Length::Fill),
        );

        let base: Element<'_, Event> = column![body, hline(), self.view_status_bar()].into();

        // Overlay order: palette, then a context menu, then a pending
        // confirmation, then a notice on the very top — a notice reports
        // something that already happened, so it outranks a question.
        let mut layers = vec![base];
        if self.palette_open {
            layers.push(self.view_palette());
        }
        if self.context_menu.is_some() {
            layers.push(self.view_context_menu());
        }
        if self.confirm.is_some() {
            layers.push(self.view_confirm());
        }
        if let Some((heading, body)) = &self.notice {
            layers.push(self.view_notice(heading, body));
        }
        // ALWAYS a Stack, even with nothing over the base. Returning the base
        // directly when there are no overlays would change the ROOT widget's
        // type as soon as one opens (Column -> Stack), and iced diffs the
        // tree by widget tag: a tag mismatch at the root discards the whole
        // subtree's state and rebuilds it. Every scrollable under it snaps
        // back to the top — right-clicking a project at the bottom of a long
        // sidebar scrolled it to the first card. A one-child Stack lays out
        // identically and costs nothing, and it keeps the base at
        // `children[0]` whether or not a layer sits above it.
        iced::widget::Stack::with_children(layers).into()
    }

    /// The GTK sidebar popovers, rebuilt: a click-away backdrop plus an
    /// item card at the right-click position.
    fn view_context_menu(&'_ self) -> Element<'_, Event> {
        // None = separator; (label, event, destructive) otherwise.
        type Item = Option<(&'static str, Event, bool)>;

        let target = match self.context_menu {
            Some(t) => t,
            None => return column![].into(),
        };
        let mut items: Vec<Item> = Vec::new();
        if let Some(pidx) = self.projects.iter().position(|p| p.id == target.project) {
            let project = &self.projects[pidx];
            match target.index {
                // Project header — mirrors GTK's project_row menu.
                None => {
                    items.push(Some(("Start All", Event::StartAll(project.id), false)));
                    items.push(Some(("Stop All", Event::StopAll(project.id), false)));
                    items.push(Some(("Restart All", Event::RestartAll(project.id), false)));
                    items.push(None);
                    items.push(Some((
                        "New Terminal",
                        Event::AddTerminal(project.id),
                        false,
                    )));
                    // GTK keeps these two in the command palette; here the
                    // pane toolbar was their only home, so they join the
                    // other creator on the project's own menu.
                    items.push(Some((
                        "New Command",
                        Event::OpenAddCommand {
                            project: project.id,
                            agent: false,
                        },
                        false,
                    )));
                    items.push(Some((
                        "New Agent",
                        Event::OpenAddCommand {
                            project: project.id,
                            agent: true,
                        },
                        false,
                    )));
                    items.push(Some((
                        "Open in Editor",
                        Event::OpenInEditor(project.id),
                        false,
                    )));
                    // GTK's slot, between the editor and Copy Path. Only
                    // once the probe delivered — the form's command union
                    // needs the entries and the detection list.
                    if matches!(project.phase, Phase::Ready) {
                        items.push(Some((
                            "Edit Project",
                            Event::OpenEditProject(project.id),
                            false,
                        )));
                    }
                    items.push(Some((
                        "Copy Path",
                        Event::CopyText(copyable_path(&project.location)),
                        false,
                    )));
                    items.push(None);
                    items.push(Some((
                        "Remove Project",
                        Event::ConfirmRequest(ConfirmAction::RemoveProject(project.id)),
                        true,
                    )));
                }
                // Process row — mirrors GTK's process_row menu (minus
                // Clear Output / Redraw Terminal: no backend command yet).
                Some(index) => {
                    if let Some(entry) = project.entries.get(index) {
                        items.push(Some((
                            "Start / Stop",
                            Event::ToggleProcessAt {
                                project: project.id,
                                index,
                            },
                            false,
                        )));
                        items.push(Some((
                            "Restart",
                            Event::Restart {
                                project: project.id,
                                index,
                            },
                            false,
                        )));
                        if entry.config.category == ProcessCategory::Agent
                            && resume_command_for(&entry.config.command).is_some()
                        {
                            items.push(Some((
                                "Resume Session",
                                Event::ResumeAgentAt {
                                    project: project.id,
                                    index,
                                },
                                false,
                            )));
                        }
                        if browser_url(project, &entry.config.name).is_some() {
                            items.push(None);
                            items.push(Some((
                                "Open in Browser",
                                Event::OpenBadgeFor {
                                    project: project.id,
                                    index,
                                },
                                false,
                            )));
                        }
                        items.push(None);
                        items.push(Some((
                            "Edit Command",
                            Event::EditProcessAt {
                                project: project.id,
                                index,
                            },
                            false,
                        )));
                        items.push(Some((
                            "Copy Command",
                            Event::CopyText(entry.config.command.clone()),
                            false,
                        )));
                        items.push(None);
                        items.push(Some((
                            "Delete Command",
                            Event::ConfirmRequest(ConfirmAction::DeleteProcess {
                                project: project.id,
                                index,
                            }),
                            true,
                        )));
                    }
                }
            }
        }

        const MENU_WIDTH: f32 = 190.0;
        let mut height = 8.0; // card padding
        let mut col = column![].width(Length::Fill);
        for item in items {
            match item {
                Some((label, event, destructive)) => {
                    height += 27.0;
                    col = col.push(
                        button(text(label).size(12.5))
                            .width(Length::Fill)
                            .padding([5, 12])
                            .style(theme::menu_item(destructive))
                            .on_press(Event::MenuAction(Box::new(event))),
                    );
                }
                None => {
                    height += 7.0;
                    col = col.push(container(hline()).padding([3, 6]));
                }
            }
        }
        let menu = container(col)
            .width(MENU_WIDTH)
            .padding(4)
            .style(theme::menu_card);

        // Open at the pointer, nudged inside the window edges.
        let x = target
            .at
            .x
            .min(self.window_size.width - MENU_WIDTH - 8.0)
            .max(0.0);
        let y = target
            .at
            .y
            .min(self.window_size.height - height - 8.0)
            .max(0.0);
        let placed = container(menu)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(iced::Padding {
                top: y,
                left: x,
                right: 0.0,
                bottom: 0.0,
            });

        let backdrop = iced::widget::mouse_area(
            container(column![])
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_press(Event::CloseContextMenu)
        .on_right_press(Event::CloseContextMenu);

        iced::widget::stack![backdrop, placed].into()
    }

    /// GTK's AlertDialog for destructive sidebar actions: dimmed ground,
    /// centered card, Cancel default.
    fn view_confirm(&'_ self) -> Element<'_, Event> {
        let Some(action) = self.confirm else {
            return column![].into();
        };
        let (heading, body, verb) = match action {
            ConfirmAction::RemoveProject(project) => {
                let name = self
                    .projects
                    .iter()
                    .find(|p| p.id == project)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                (
                    format!("Remove '{name}'?"),
                    "This will remove the project and all its processes from the sidebar.",
                    "Remove",
                )
            }
            ConfirmAction::DeleteProcess { project, index } => {
                let name = self
                    .projects
                    .iter()
                    .find(|p| p.id == project)
                    .and_then(|p| p.entries.get(index))
                    .map(|e| e.config.name.clone())
                    .unwrap_or_default();
                (
                    format!("Delete '{name}'?"),
                    "This will stop the process and remove it from the sidebar.",
                    "Delete",
                )
            }
        };

        let card = container(
            column![
                text(heading).size(15).font(bold()).color(TEXT),
                text(body).size(12).color(TEXT_SECONDARY),
                container(
                    row![
                        button(text("Cancel").size(12))
                            .padding([6, 16])
                            .style(theme::pill_button(LOCAL_ACCENT))
                            .on_press(Event::ConfirmCancel),
                        button(text(verb).size(12))
                            .padding([6, 16])
                            .style(theme::danger())
                            .on_press(Event::ConfirmProceed),
                    ]
                    .spacing(8),
                )
                .width(Length::Fill)
                .align_x(iced::Alignment::End),
            ]
            .spacing(12),
        )
        .padding(18)
        .width(380)
        .style(theme::form_card);

        let backdrop = iced::widget::mouse_area(
            container(column![])
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| iced::widget::container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.0, 0.0, 0.0, 0.4,
                    ))),
                    ..Default::default()
                }),
        )
        .on_press(Event::ConfirmCancel);

        let placed = container(card)
            .center_x(Length::Fill)
            .center_y(Length::Fill);

        iced::widget::stack![backdrop, placed].into()
    }

    /// Report-only card: what GTK opens an AlertDialog for when something
    /// failed and there is nothing to decide. Same furniture as the
    /// confirmation, one button.
    fn view_notice<'a>(&'a self, heading: &'a str, body: &'a str) -> Element<'a, Event> {
        let card = container(
            column![
                text(heading).size(15).font(bold()).color(TEXT),
                text(body).size(12).color(TEXT_SECONDARY),
                container(
                    button(text("OK").size(12))
                        .padding([6, 16])
                        .style(theme::pill_button(LOCAL_ACCENT))
                        .on_press(Event::NoticeDismiss),
                )
                .width(Length::Fill)
                .align_x(iced::Alignment::End),
            ]
            .spacing(12),
        )
        .padding(18)
        .width(380)
        .style(theme::form_card);

        let backdrop = iced::widget::mouse_area(
            container(column![])
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| iced::widget::container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.0, 0.0, 0.0, 0.4,
                    ))),
                    ..Default::default()
                }),
        )
        .on_press(Event::NoticeDismiss);

        iced::widget::stack![
            backdrop,
            container(card)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
        ]
        .into()
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
                text_input("Jump to a process\u{2026}", &self.palette_query)
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

    /// The GTK header bar's button cluster: sidebar toggle, sidebar
    /// filter, settings, add — the same four Adwaita symbolic icons in the
    /// same order, flat until hovered, washed while toggled on. Lives at
    /// the top of the sidebar; `vertical` renders the collapsed rail.
    fn header_cluster(&'_ self, vertical: bool) -> Element<'_, Event> {
        let kb = &self.settings.keybindings;
        // A collapsed rail's tooltips open away from the edge they hug.
        let position = if vertical {
            iced::widget::tooltip::Position::Right
        } else {
            iced::widget::tooltip::Position::Bottom
        };
        let btn = |icon: &'static [u8], active: bool, tip: String, event: Event| {
            iced::widget::tooltip(
                button(symbolic(icon, 16.0, TEXT))
                    .padding(7)
                    .style(theme::toolbar_icon(active))
                    .on_press(event),
                text(tip).size(11),
                position,
            )
            .gap(4)
            .padding(7)
            .style(theme::tooltip)
        };
        let buttons = [
            btn(
                ICON_SIDEBAR,
                self.sidebar_visible,
                format!("Toggle Sidebar ({})", kb.toggle_sidebar),
                Event::ToggleSidebar,
            ),
            btn(
                ICON_FIND,
                self.filter_open,
                format!("Filter Sidebar ({})", kb.filter_processes),
                Event::ToggleFilter,
            ),
            btn(
                ICON_GEAR,
                self.settings_ui.is_some(),
                format!("Settings ({})", kb.settings),
                Event::OpenSettings,
            ),
            btn(
                ICON_ADD,
                self.add_project.is_some(),
                String::from("Add Project"),
                Event::OpenAddProject,
            ),
        ];
        if vertical {
            let mut col = column![].spacing(2).align_x(iced::Alignment::Center);
            for b in buttons {
                col = col.push(b);
            }
            col.into()
        } else {
            let mut r = row![].spacing(4).align_y(iced::Alignment::Center);
            for b in buttons {
                r = r.push(b);
            }
            r.into()
        }
    }

    /// The hidden-sidebar stand-in: the same four buttons as a slim
    /// vertical rail on the window edge.
    fn view_rail(&'_ self) -> Element<'_, Event> {
        // Pinned to SIDEBAR_RAIL rather than left to its content: that
        // constant is the floor the collapse glide aims at, and a rail
        // even a pixel off it would jump on arrival.
        container(self.header_cluster(true))
            .width(SIDEBAR_RAIL)
            .height(Length::Fill)
            .padding([6, 4])
            .style(theme::ground)
            .into()
    }

    /// The sidebar's full width — GTK's quarter-of-the-window rule.
    fn sidebar_width(&self) -> f32 {
        (self.window_size.width * SIDEBAR_FRACTION).clamp(SIDEBAR_MIN, SIDEBAR_MAX)
    }

    /// How wide the sidebar column is drawn right now: rail width at one
    /// end of the glide, full width at the other.
    fn sidebar_extent(&self) -> f32 {
        let f = self.sidebar_anim.eased();
        let open = if self.sidebar_visible { f } else { 1.0 - f };
        SIDEBAR_RAIL + (self.sidebar_width() - SIDEBAR_RAIL) * open
    }

    fn view_sidebar(&'_ self) -> Element<'_, Event> {
        // The filter narrows the whole sidebar (GTK semantics): a project
        // matching by name keeps all its rows; otherwise only matching
        // process rows stay, and a project with no match hides entirely.
        let query = self.filter_query.trim().to_lowercase();
        let filter = (self.filter_open && !query.is_empty()).then_some(query.as_str());

        // Numbered over the whole workspace, not per visible card: a filter
        // hides rows but does not rebind the chords, so the hint a row keeps
        // is still the one that reaches it.
        let targets = self.switch_targets();

        let mut col = column![].spacing(10).padding([12, 10]);
        for (pidx, project) in self.projects.iter().enumerate() {
            if let Some(q) = filter {
                let name_match = project.name.to_lowercase().contains(q);
                let process_match = project
                    .entries
                    .iter()
                    .any(|e| e.config.name.to_lowercase().contains(q));
                if !name_match && !process_match {
                    continue;
                }
            }
            col = col.push(self.view_project_block(pidx, project, filter, &targets));
        }

        let mut inner = column![
            container(self.header_cluster(false)).padding(iced::Padding {
                top: 6.0,
                right: 8.0,
                bottom: 2.0,
                left: 8.0,
            })
        ];
        if self.filter_open {
            inner = inner.push(
                container(
                    text_input("Filter projects & processes\u{2026}", &self.filter_query)
                        .id(self.filter_input.clone())
                        .on_input(Event::FilterInput)
                        .style(theme::input(LOCAL_ACCENT))
                        .padding([6, 12])
                        .size(12),
                )
                .padding(iced::Padding {
                    top: 6.0,
                    right: 10.0,
                    bottom: 0.0,
                    left: 10.0,
                }),
            );
        }
        inner = inner.push(
            scrollable(col)
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new().width(4).scroller_width(4),
                ))
                .style(theme::overlay_scrollbar)
                .height(Length::Fill)
                .width(Length::Fill),
        );

        // Two boxes on purpose: the inner one holds the content at the
        // sidebar's FULL width so the glide reveals a finished layout,
        // while the outer one carries the animated width and clips it.
        // Laying the content out at the animated width instead would
        // re-wrap every clipped label and re-flow every card each frame —
        // the cards would visibly rearrange themselves mid-slide.
        container(container(inner).width(self.sidebar_width()))
            .width(self.sidebar_extent())
            .height(Length::Fill)
            .clip(true)
            .style(theme::ground)
            .into()
    }

    /// One floating project card; the active one is lit by its accent
    /// gradient. A filter query forces the card open on its matching rows
    /// (all of them when the project matched by name).
    fn view_project_block<'a>(
        &'a self,
        pidx: usize,
        project: &'a ProjectState,
        filter: Option<&str>,
        targets: &[(usize, usize)],
    ) -> Element<'a, Event> {
        let remote = project.location.is_remote();
        let accent = accent_for(remote);
        let active = pidx == self.active;

        // 26px avatar: the project's own artwork, or an initials square —
        // the shared drawing, so the Edit Project preview can't disagree.
        let icon: Element<'a, Event> =
            widgets::avatar(project.icon.as_deref(), &project.name, accent, remote, 26.0);

        // GTK's hover controls on the project row: start the marked set,
        // restart the running, stop everything. They take the counter
        // pill's seat while the pointer is on the header.
        // GTK's `.project-has-running .project-name`: the title lights up in
        // the project's accent while anything inside is up, alongside the
        // card's border ring. Idle cards keep the plain title.
        let running = project.has_running();
        let hovered = self.hovered_row == Some((project.id, None));
        let mut title = row![
            icon,
            clipped_label(text(&project.name).size(13).font(bold()).color(if running {
                accent
            } else {
                TEXT
            })),
        ]
        .spacing(9)
        .align_y(iced::Alignment::Center);
        if !hovered {
            let counter = format!("{}/{}", project.running(), project.entries.len());
            title = title.push(
                container(text(counter).size(10))
                    .padding([2, 8])
                    .style(theme::pill),
            );
        }
        let mut header = row![
            button(title)
                .width(Length::Fill)
                .padding([2, 4])
                .style(theme::header_title)
                .on_press(Event::ToggleExpanded(project.id)),
        ]
        .spacing(2)
        .align_y(iced::Alignment::Center);
        if hovered {
            let p = self.hover_progress();
            let cluster = row![
                row_action(
                    ICON_PLAY,
                    theme::alpha(LOCAL_ACCENT, p),
                    String::from("Start all marked processes"),
                    Event::StartAll(project.id),
                ),
                row_action(
                    ICON_RESTART,
                    theme::alpha(TEXT_SECONDARY, p),
                    String::from("Restart all running processes"),
                    Event::RestartAll(project.id),
                ),
                row_action(
                    ICON_STOP,
                    theme::alpha(CRASHED, p),
                    String::from("Stop all"),
                    Event::StopAll(project.id),
                ),
            ]
            .spacing(1)
            .align_y(iced::Alignment::Center);
            header = header.push(self.slide_in(cluster));
        }
        // No ✕ here: removing a project lives in the right-click menu
        // behind a confirmation, like GTK.
        let header = container(header)
            .width(Length::Fill)
            .style(theme::project_header(accent, hovered));
        let header = iced::widget::mouse_area(header)
            .on_enter(Event::RowEnter {
                project: project.id,
                index: None,
            })
            .on_exit(Event::RowExit {
                project: project.id,
                index: None,
            })
            .on_right_press(Event::OpenContextMenu {
                project: project.id,
                index: None,
            });

        let mut block = column![header].spacing(2);

        let name_match = filter.is_some_and(|q| project.name.to_lowercase().contains(q));
        if project.expanded || filter.is_some() {
            // The header is a group of its own, so it gets the same gap
            // under it that separates the categories below. Without it the
            // title sits tighter to the first row than the rows sit to each
            // other (the header's button padding is 2 against their 5), and
            // a hovered header's tint touches the selected row's.
            block = block.push(group_gap());
            match &project.phase {
                Phase::Loading => {
                    block = block.push(
                        container(text("Connecting\u{2026}").size(11).color(DIM)).padding([3, 10]),
                    );
                }
                Phase::Failed(_, retryable) => {
                    let mut r = row![text("Unreachable").size(11).color(CRASHED)].spacing(8);
                    if *retryable {
                        r = r.push(
                            button(text("Retry").size(10))
                                .padding([1, 8])
                                .style(theme::pill_button(accent))
                                .on_press(Event::RetryProbe(project.id)),
                        );
                    }
                    block = block.push(container(r).padding([3, 10]));
                }
                Phase::Ready => {
                    // The gap leads each category rather than trailing it:
                    // a separator after the last one is padding, and it made
                    // the card bottom-heavy against the 8px above the header.
                    let mut first = true;
                    for category in SIDEBAR_CATEGORIES {
                        let members: Vec<usize> = (0..project.entries.len())
                            .filter(|&i| project.entries[i].config.category == category)
                            .filter(|&i| match filter {
                                Some(q) if !name_match => {
                                    project.entries[i].config.name.to_lowercase().contains(q)
                                }
                                _ => true,
                            })
                            .collect();
                        if members.is_empty() {
                            continue;
                        }
                        if !first {
                            block = block.push(group_gap());
                        }
                        first = false;
                        for i in members {
                            block = block.push(self.view_row(pidx, i, targets));
                        }
                    }
                }
            }
        }

        let sweep = project.sweeping.then_some(self.sweep.phase);
        container(block)
            .width(Length::Fill)
            .padding(8)
            .style(theme::project_card(accent, running, active, sweep))
            .into()
    }

    fn view_row(
        &'_ self,
        pidx: usize,
        index: usize,
        targets: &[(usize, usize)],
    ) -> Element<'_, Event> {
        let project = &self.projects[pidx];
        let remote = project.location.is_remote();
        let accent = accent_for(remote);
        let entry = &project.entries[index];

        let dot_color = match entry.status {
            Status::Running => accent,
            Status::Stopped => STOPPED,
            Status::Crashed(_) => CRASHED,
            Status::Restarting(_) | Status::Reconnecting(_) => RESTARTING,
        };
        // The light rides the card sweep's phase instead of keeping one of
        // its own: every working row turns in step, and a busy agent still
        // costs exactly one timer no matter how many rows are lit.
        let working = entry.working.then_some(self.sweep.phase);
        let name = entry
            .config
            .display_name
            .as_deref()
            .unwrap_or(&entry.config.name);

        let hovered = self.hovered_row == Some((project.id, Some(index)));

        // GTK hangs the command off the whole row (`set_tooltip_text` in
        // sidebar/process_row.rs); here it rides the NAME instead, because
        // iced opens a parent's card *and* a child's at once where GTK
        // lets the innermost win — over the row it would collide with the
        // lifecycle glyphs, whose tooltips sit at the same edge.
        // The name is the row's Fill element, so that is everything but
        // the dot and the trailing pills anyway.
        let label = match entry.config.command.trim() {
            // A plain terminal carries no command (it spawns a login
            // shell); an empty card is worse than no card.
            "" => clipped_label(text(name).size(12.5)),
            command => tip_after(
                clipped_label(text(name).size(12.5)),
                command.to_string(),
                iced::widget::tooltip::Position::Bottom,
                // Instant is right for a glyph you had to aim at; this
                // one covers most of the row, so without the delay it
                // fires on every row the pointer crosses on its way
                // somewhere else. GTK's own tooltip timeout, near enough.
                Duration::from_millis(500),
            ),
        };

        // Fixed content height: hover swaps elements in and out (hint ↔
        // glyph cluster), and the row must measure the same with any of
        // them or the rows below shift while the pointer moves.
        let mut content = row![status_dot(dot_color, working), label]
            .spacing(8)
            .height(17)
            .align_y(iced::Alignment::Center);

        // Ctrl+1..9 keycaps, revealed only while Ctrl is actually down.
        //
        // Numbered off the very list the switcher indexes into — the label
        // is a lookup, never a count of its own, so the two cannot drift.
        // Standing hints were the wrong shape for what this became: the
        // sequence is global and skips stopped rows, so it is at most nine
        // marks scattered over the whole sidebar, and holding a slot on
        // every row to show them was paying full chrome for a sparse
        // overlay. On the modifier the sparseness is the point — only the
        // reachable rows answer.
        //
        // The cap carries the digit alone. `⌃` is the half of the chord
        // that never varies, it is illegible at this size, and while the
        // reveal is running the modifier is being held anyway.
        //
        // Still yields to the lifecycle glyphs under the pointer: they
        // share this slot, and the row's fixed height is what keeps that
        // swap from re-flowing the rows below.
        if self.ctrl_held
            && !hovered
            && self.settings.sidebar.show_keybind_hints
            && let Some(slot) = targets.iter().position(|&t| t == (pidx, index))
        {
            content = content.push(
                container(text(slot + 1).size(10))
                    .padding([0, 4])
                    .style(theme::keycap),
            );
        }

        match entry.status {
            Status::Restarting(attempt) => {
                content = content.push(
                    text(format!(
                        "Retry {attempt}/{}",
                        processes::MAX_RESTART_ATTEMPTS
                    ))
                    .size(9)
                    .color(RESTARTING),
                );
            }
            Status::Reconnecting(attempt) => {
                content = content.push(
                    text(format!("Reconnect {attempt}"))
                        .size(9)
                        .color(RESTARTING),
                );
            }
            _ => {}
        }

        // The lifecycle glyphs (design round F): bare icons sliding in
        // while the pointer is on the row — play when idle, restart+stop
        // when running, stop-as-cancel while coming back.
        if hovered {
            let p = self.hover_progress();
            let mut cluster = row![].spacing(1).align_y(iced::Alignment::Center);
            match entry.status {
                Status::Stopped | Status::Crashed(_) => {
                    cluster = cluster.push(row_action(
                        ICON_PLAY,
                        theme::alpha(LOCAL_ACCENT, p),
                        entry.config.command.clone(),
                        Event::Start {
                            project: project.id,
                            index,
                        },
                    ));
                }
                Status::Running => {
                    cluster = cluster
                        .push(row_action(
                            ICON_RESTART,
                            theme::alpha(TEXT_SECONDARY, p),
                            String::from("Restart"),
                            Event::Restart {
                                project: project.id,
                                index,
                            },
                        ))
                        .push(row_action(
                            ICON_STOP,
                            theme::alpha(CRASHED, p),
                            String::from("Stop"),
                            Event::Stop {
                                project: project.id,
                                index,
                            },
                        ));
                }
                Status::Restarting(_) | Status::Reconnecting(_) => {
                    cluster = cluster.push(row_action(
                        ICON_STOP,
                        theme::alpha(CRASHED, p),
                        String::from("Cancel"),
                        Event::Stop {
                            project: project.id,
                            index,
                        },
                    ));
                }
            }
            content = content.push(self.slide_in(cluster));
        }

        let selected = pidx == self.active && index == project.selected;
        let base = button(content)
            .width(Length::Fill)
            .padding([5, 9])
            .style(theme::process_row(accent, selected))
            .on_press(Event::SelectProcess {
                project: project.id,
                index,
            });

        iced::widget::mouse_area(base)
            .on_enter(Event::RowEnter {
                project: project.id,
                index: Some(index),
            })
            .on_exit(Event::RowExit {
                project: project.id,
                index: Some(index),
            })
            .on_right_press(Event::OpenContextMenu {
                project: project.id,
                index: Some(index),
            })
            .into()
    }

    /// Eased slide progress (0..1) of the current hover reveal.
    fn hover_progress(&self) -> f32 {
        self.hover_anim.eased()
    }

    /// F's glide: the cluster starts 6px right of its seat and settles,
    /// swapping padding side for side so total width never changes (a
    /// varying width would wobble the clipped name next to it).
    fn slide_in<'a>(&self, cluster: iced::widget::Row<'a, Event>) -> Element<'a, Event> {
        let p = self.hover_progress();
        container(cluster)
            .padding(iced::Padding {
                top: 0.0,
                right: 6.0 * p,
                bottom: 0.0,
                left: 6.0 * (1.0 - p),
            })
            .into()
    }

    fn view_main(&'_ self) -> Element<'_, Event> {
        if let Some(state) = &self.settings_ui {
            return settings_ui::view(state, &self.settings).map(Event::SettingsMsg);
        }
        if let Some(state) = &self.git_ui {
            return git_view::view(state).map(Event::GitMsg);
        }
        if let Some(form) = &self.add_command {
            return self.view_add_command(form);
        }
        if let Some(state) = &self.add_project {
            return add_project::view(state).map(Event::AddProjectMsg);
        }
        if let Some(state) = &self.edit_project {
            return edit_project::view(state).map(Event::EditProjectMsg);
        }

        let Some(project) = self.active_project() else {
            return container(
                column![
                    text("No projects yet").size(14).color(DIM),
                    button(text("+ Add Project").size(13))
                        .padding([7, 16])
                        .style(theme::primary(LOCAL_ACCENT))
                        .on_press(Event::OpenAddProject),
                ]
                .spacing(14)
                .align_x(iced::Alignment::Center),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(theme::pane)
            .into();
        };
        let remote = project.location.is_remote();
        let accent = accent_for(remote);

        match &project.phase {
            Phase::Loading => {
                return container(
                    text(format!("Connecting to {}\u{2026}", project.key()))
                        .size(14)
                        .color(DIM),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(theme::pane)
                .into();
            }
            Phase::Failed(message, retryable) => {
                let mut col = column![text(message).size(14).color(CRASHED)]
                    .spacing(14)
                    .align_x(iced::Alignment::Center);
                if *retryable {
                    col = col.push(
                        button(text("\u{27f3} Retry").size(12))
                            .padding([6, 14])
                            .style(theme::primary(accent))
                            .on_press(Event::RetryProbe(project.id)),
                    );
                }
                return container(col)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(theme::pane)
                    .into();
            }
            Phase::Ready => {}
        }

        let Some(entry) = project.entries.get(project.selected) else {
            return container(
                column![
                    text("No processes").size(14).color(DIM),
                    row![
                        button(text("+ Command").size(12))
                            .padding([6, 14])
                            .style(theme::primary(accent))
                            .on_press(Event::OpenAddCommand {
                                project: project.id,
                                agent: false,
                            }),
                        button(text("+ Agent").size(12))
                            .padding([6, 14])
                            .style(theme::primary(accent))
                            .on_press(Event::OpenAddCommand {
                                project: project.id,
                                agent: true,
                            }),
                    ]
                    .spacing(8),
                ]
                .spacing(14)
                .align_x(iced::Alignment::Center),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(theme::pane)
            .into();
        };

        let body: Element<'_, Event> = match &entry.terminal {
            // Flush, like VTE in the GTK shell — padding here frames any
            // program that repaints its own background (the grid carries
            // the color, the padding keeps the pane's) in a visible band.
            Some(term) => container(TerminalView::show(term).map(Event::Terminal))
                .style(theme::terminal_pane(
                    &self.settings.appearance.terminal_theme,
                ))
                .into(),
            // Only before a process's FIRST run: from then on its terminal
            // stays for good, showing what the last run printed — and its
            // exit banner, which is where the status is spelled out.
            None => {
                let label = match entry.status {
                    Status::Crashed(_) => {
                        String::from("Could not start \u{2014} check the command")
                    }
                    _ => String::from("Not running"),
                };
                container(text(label).size(13).color(DIM))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(theme::pane)
                    .into()
            }
        };

        // No strip above the terminal: the process name, its OSC title and
        // its status live in the window title bar and the sidebar row, and
        // the actions that were pills here are on the row's hover cluster
        // and its context menu.
        let mut col = column![];

        if self.search_open {
            let hint: Element<'_, Event> = match self.search_hit {
                Some(false) => text("No matches").size(11).color(CRASHED).into(),
                _ => text("").size(11).into(),
            };
            col = col
                .push(
                    container(
                        row![
                            text_input("Search scrollback (regex)\u{2026}", &self.search_query)
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
            && entry.is_running()
            && self.settings.tools.agent_composer
        {
            let placeholder = format!("Message to {}\u{2026}", entry.config.name);
            col = col.push(hline()).push(
                container(
                    row![
                        text_input(&placeholder, &self.composer)
                            .on_input(Event::ComposerChanged)
                            .on_submit(Event::ComposerSend)
                            .style(theme::input(accent))
                            .padding([7, 14])
                            .size(13),
                        button(text("Send").size(12).font(bold()))
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
            (true, _) => "Edit Process",
            (false, true) => "Add Agent",
            (false, false) => "Add Command",
        };

        let name_row: Element<'_, Event> = if editing {
            row![
                text(&form.name).size(15).font(bold()),
                text(match form.original_category {
                    ProcessCategory::Agent => "Agent",
                    ProcessCategory::Command => "Command",
                    ProcessCategory::Terminal => "Terminal",
                    ProcessCategory::SSH => "SSH",
                })
                .size(11)
                .color(DIM),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            text_input("Name \u{2014} e.g. web", &form.name)
                .on_input(Event::AddCommandName)
                .style(theme::input(accent))
                .padding([8, 14])
                .size(13)
                .into()
        };

        let mut col = column![text(title).size(16).font(bold())].spacing(14);

        // Picking an agent is choosing WHICH one first; the fields below are
        // the starting point it fills in, all still editable. Only offered
        // when creating — an existing process's command is the thing being
        // edited, and a preset row would silently overwrite it.
        if form.agent && !editing {
            let typed = form.command.trim();
            let mut list = column![].spacing(0);
            for (i, preset) in agents::AGENT_PRESETS.iter().enumerate() {
                let picked = typed == preset.command;
                list = list.push(
                    button(
                        row![
                            text(preset.label).size(12.5).color(TEXT),
                            iced::widget::space::horizontal(),
                            text(preset.command).size(11).color(DIM),
                        ]
                        .align_y(iced::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([6, 12])
                    .style(theme::process_row(accent, picked))
                    .on_press(Event::AgentPreset(i)),
                );
            }
            col = col.push(
                column![
                    text("Agent").size(11.5).font(bold()).color(TEXT_SECONDARY),
                    container(list)
                        .padding([4, 0])
                        .style(theme::settings_card)
                        .width(Length::Fill),
                ]
                .spacing(6),
            );
        }

        col = col.push(name_row).push(
            text_input(
                match form.agent {
                    true => "Command \u{2014} e.g. claude --model opus",
                    false => "Command \u{2014} e.g. npm run dev",
                },
                &form.command,
            )
            .on_input(Event::AddCommandCommand)
            .on_submit(Event::AddCommandSubmit)
            .style(theme::input(accent))
            .padding([8, 14])
            .size(13),
        );
        col = col.push(
            text_input(
                "Working directory \u{2014} optional, defaults to the project",
                &form.working_dir,
            )
            .on_input(Event::FormWorkingDir)
            .style(theme::input(accent))
            .padding([8, 14])
            .size(13),
        );
        col = col
            .push(
                iced::widget::checkbox(form.start_with_project)
                    .label("Start with project")
                    .on_toggle(Event::FormToggleStartWith)
                    .size(16)
                    .text_size(12.5),
            )
            .push(
                iced::widget::checkbox(form.auto_restart)
                    .label("Restart on crash")
                    .on_toggle(Event::FormToggleAutoRestart)
                    .size(16)
                    .text_size(12.5),
            );
        // An agent serves no port, so "open in browser when a port appears"
        // is dead weight on this form. The stored flag is left alone rather
        // than forced off, so nothing changes under an existing process.
        if !form.agent {
            col = col.push(
                iced::widget::checkbox(form.open_in_browser)
                    .label("Open in browser when a port appears")
                    .on_toggle(Event::FormToggleOpenBrowser)
                    .size(16)
                    .text_size(12.5),
            );
        }
        if let Some(error) = &form.error {
            col = col.push(text(error).size(12).color(CRASHED));
        }

        let mut buttons = row![
            button(
                text(if editing { "Save" } else { "Add & Start" })
                    .size(12)
                    .font(bold()),
            )
            .padding([7, 16])
            .style(theme::primary(accent))
            .on_press(Event::AddCommandSubmit),
            button(text("Cancel").size(12))
                .padding([7, 16])
                .style(theme::pill_button(accent))
                .on_press(Event::AddCommandCancel),
        ]
        .spacing(8);
        if editing {
            buttons = buttons.push(iced::widget::space::horizontal()).push(
                button(text("Delete Process").size(12))
                    .padding([7, 16])
                    .style(theme::pill_intent(accent, CRASHED))
                    .on_press(Event::DeleteProcess),
            );
        }
        col = col.push(buttons);

        form_card(col)
    }

    /// The bottom bar, GTK's `status_bar.rs` laid out in the same order:
    /// remote hint + per-project counter + across-projects total on the
    /// left, the git chips and the action buttons on the right.
    fn view_status_bar(&'_ self) -> Element<'_, Event> {
        let project = self.active_project();

        let mut bar = row![].spacing(8).align_y(iced::Alignment::Center);

        // ── Left: where and how much is running ─────────────────────────
        if let Some(hint) = project.and_then(|p| remote_hint(&p.location)) {
            bar = bar.push(tip(
                symbolic(ICON_REMOTE, 13.0, DIM).into(),
                hint,
                iced::widget::tooltip::Position::Top,
            ));
        }
        if let Some(p) = project {
            let label = if p.entries.is_empty() {
                p.name.clone()
            } else {
                format!("{} {}/{}", p.name, p.running(), p.entries.len())
            };
            bar = bar.push(text(label).size(11).color(TEXT_SECONDARY));
        }

        // Across every open project — the counter that says something is
        // still running in a project you aren't looking at.
        let total: usize = self.projects.iter().map(|p| p.entries.len()).sum();
        if total > 0 {
            let running: usize = self.projects.iter().map(|p| p.running()).sum();
            if project.is_some() {
                bar = bar.push(text("\u{00b7}").size(11).color(DIM));
            }
            let counter = text(format!("Total {running}/{total}")).size(11).color(DIM);
            bar = match self.running_summary() {
                Some(summary) => bar.push(tip(
                    counter.into(),
                    summary,
                    iced::widget::tooltip::Position::Top,
                )),
                None => bar.push(counter),
            };
        }

        bar = bar.push(iced::widget::space::horizontal());

        // ── Right: git chips, then the actions ──────────────────────────
        if let Some(chip) = self.view_git_sync_chip() {
            bar = bar.push(chip);
        }
        if let Some(chip) = self.view_git_changes_chip() {
            bar = bar.push(chip);
        }

        // GTK's browser button: icon-only, the URL on hover (its `Open
        // {url}` tooltip verbatim). A text chip sat here first — a real
        // URL is wide enough to crowd out the actions beside it. Ahead of
        // Focus (GTK puts it after) on Nikola's ask: it comes and goes
        // with the badge, and appearing between two standing buttons made
        // the whole right end jump.
        let url = project.and_then(|p| {
            p.entries
                .get(p.selected)
                .and_then(|e| browser_url(p, &e.config.name))
        });
        if let Some(url) = url {
            bar = bar.push(tip(
                button(symbolic(ICON_EXTERNAL, 13.0, TEXT_SECONDARY))
                    .padding([3, 7])
                    .style(theme::toolbar_icon(false))
                    .on_press(Event::OpenBadge)
                    .into(),
                format!("Open {url}"),
                iced::widget::tooltip::Position::Top,
            ));
        }

        bar = bar.push(tip(
            button(symbolic(ICON_FOCUS, 13.0, TEXT_SECONDARY))
                .padding([3, 7])
                .style(theme::toolbar_icon(!self.sidebar_visible))
                .on_press(Event::ToggleSidebar)
                .into(),
            String::from("Focus"),
            iced::widget::tooltip::Position::Top,
        ));

        // Clear / Stop / Restart act on the SELECTED process, so they are
        // dead without one. Stop is hidden rather than disabled when it
        // isn't running — you can't stop what isn't going (GTK parity).
        let selected = project.and_then(|p| {
            p.entries
                .get(p.selected)
                .map(|e| (p.id, p.selected, e.is_running()))
        });
        if let Some((id, index, running)) = selected {
            bar = bar.push(tip(
                button(symbolic(ICON_CLEAR, 13.0, TEXT_SECONDARY))
                    .padding([3, 7])
                    .style(theme::toolbar_icon(false))
                    .on_press(Event::ClearTerminal)
                    .into(),
                String::from("Clear"),
                iced::widget::tooltip::Position::Top,
            ));
            if running {
                bar = bar.push(tip(
                    button(symbolic(ICON_STOP, 13.0, CRASHED))
                        .padding([3, 7])
                        .style(theme::toolbar_icon(false))
                        .on_press(Event::Stop { project: id, index })
                        .into(),
                    String::from("Stop"),
                    iced::widget::tooltip::Position::Top,
                ));
            }
            bar = bar.push(tip(
                button(symbolic(ICON_RESTART, 13.0, TEXT_SECONDARY))
                    .padding([3, 7])
                    .style(theme::toolbar_icon(false))
                    .on_press(Event::Restart { project: id, index })
                    .into(),
                String::from("Restart"),
                iced::widget::tooltip::Position::Top,
            ));
        }

        container(bar)
            .padding([5, 12])
            .width(Length::Fill)
            .style(theme::chrome)
            .into()
    }

    /// "project: proc, proc" per project, for the total counter's tooltip.
    /// None when nothing is running — an empty tooltip is worse than none.
    fn running_summary(&self) -> Option<String> {
        let lines: Vec<String> = self
            .projects
            .iter()
            .filter_map(|p| {
                let names: Vec<&str> = p
                    .entries
                    .iter()
                    .filter(|e| e.is_running())
                    .map(|e| e.config.name.as_str())
                    .collect();
                (!names.is_empty()).then(|| format!("{}: {}", p.name, names.join(", ")))
            })
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// Branch + ↓ to pull / ↑ to push; one click syncs both ways. Needs a
    /// branch as well as a repo — a detached HEAD has nothing to pull to
    /// or push from, so the whole chip goes away.
    fn view_git_sync_chip(&'_ self) -> Option<Element<'_, Event>> {
        let project = self.active_project()?;
        let git = project.git.as_ref()?;
        if git.branch.is_empty() || git.branch == "(detached)" {
            return None;
        }
        // THIS project's sync, not anyone's: a sync started on another card
        // must not dress this chip in a spinner it didn't ask for.
        let syncing = self.git_syncing.contains(&project.id);

        let mut content = row![text(format!("\u{2387} {}", git.branch)).size(10.5)]
            .spacing(5)
            .align_y(iced::Alignment::Center);
        // While a sync runs the counters stand down: the pre-sync numbers
        // sitting next to "syncing…" read as "the sync did nothing".
        if syncing {
            content = content.push(text("Syncing\u{2026}").size(10.5).color(DIM));
        } else {
            if git.behind > 0 {
                content = content.push(
                    text(format!("\u{2193}{}", git.behind))
                        .size(10.5)
                        .color(GIT_BEHIND),
                );
            }
            if git.ahead > 0 {
                content = content.push(
                    text(format!("\u{2191}{}", git.ahead))
                        .size(10.5)
                        .color(GIT_ADDED),
                );
            }
        }

        let hint = match (git.ahead, git.behind) {
            _ if syncing => String::from("Syncing\u{2026}"),
            (0, 0) => String::from("Pull & Push (in sync \u{2014} click to fetch)"),
            (a, 0) => format!("Pull & Push ({a} to push)"),
            (0, b) => format!("Pull & Push ({b} to pull)"),
            (a, b) => format!("Pull & Push ({a} to push, {b} to pull)"),
        };

        let mut chip = button(content)
            .padding([3, 9])
            .style(theme::pill_button(TEXT_SECONDARY));
        if !syncing {
            chip = chip.on_press(Event::GitSync);
        }
        Some(tip(chip.into(), hint, iced::widget::tooltip::Position::Top))
    }

    /// Working-tree `+N −M`; click opens the Git Changes view. Shown for
    /// any repo, counters and all, even clean — the way in has to be there
    /// before there is anything to see.
    fn view_git_changes_chip(&'_ self) -> Option<Element<'_, Event>> {
        let project = self.active_project()?;
        project.git.as_ref()?;
        let stat = project.diffstat;

        let mut content = row![symbolic(ICON_CHANGES, 12.0, TEXT_SECONDARY)]
            .spacing(5)
            .align_y(iced::Alignment::Center);
        if stat.added > 0 {
            content = content.push(
                text(format!("+{}", git_view::compact_count(stat.added)))
                    .size(10.5)
                    .color(GIT_ADDED),
            );
        }
        if stat.removed > 0 {
            content = content.push(
                text(format!("\u{2212}{}", git_view::compact_count(stat.removed)))
                    .size(10.5)
                    .color(GIT_REMOVED),
            );
        }

        // Exact numbers live in the tooltip; the chip carries the compact
        // ones. Untracked files can't show in the line counts — git diff
        // doesn't see them — so they are named here instead.
        let mut parts = Vec::new();
        if stat.files > 0 {
            parts.push(format!(
                "{} files: +{} \u{2212}{}",
                stat.files, stat.added, stat.removed
            ));
        }
        if stat.untracked > 0 {
            parts.push(format!("{} untracked", stat.untracked));
        }
        let hint = if parts.is_empty() {
            String::from("Git Changes")
        } else {
            format!("Git Changes ({})", parts.join(", "))
        };

        Some(tip(
            button(content)
                .padding([3, 9])
                .style(theme::pill_button(TEXT_SECONDARY))
                .on_press(Event::OpenGitChanges)
                .into(),
            hint,
            iced::widget::tooltip::Position::Top,
        ))
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
                // Both directions re-sort — GTK's set_recent_first applies
                // the manual order when switched OFF, not a freeze of
                // whatever recency had produced.
                if v {
                    self.sort_projects_recent_first();
                } else {
                    self.sort_projects_manual();
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

    /// GTK's sidebar order (`sort_project_rows` in project_list.rs), not a
    /// flat recency sort: TWO TIERS. Projects with something running sit on
    /// top in stable start order (`last_used` ASCENDING — a newly started
    /// project appends BELOW the already-running, so their positions hold
    /// still), stopped ones below it most-recently-used first, and
    /// never-used projects tie at zero and keep the manual order at the
    /// bottom. "Used" is the last tier FLIP, start or stop — see
    /// [`Self::refresh_recent_order`]. Project ids keep timers safe — only
    /// the vec order changes.
    fn sort_projects_recent_first(&mut self) {
        let active_id = self.projects.get(self.active).map(|p| p.id);
        let saved = &self.saved;
        let keys: HashMap<u64, (std::cmp::Reverse<bool>, i64, usize)> = self
            .projects
            .iter()
            .map(|p| {
                let key = p.key();
                let manual = saved
                    .directories
                    .iter()
                    .position(|d| d == &key)
                    .unwrap_or(usize::MAX);
                (
                    p.id,
                    recent_order_key(p.has_running(), saved.get_last_used(&key), manual),
                )
            })
            .collect();
        self.projects.sort_by_key(|p| keys[&p.id]);
        if let Some(id) = active_id
            && let Some(idx) = self.projects.iter().position(|p| p.id == id)
        {
            self.active = idx;
        }
    }

    /// The order with recent-first OFF: the manual (saved) one — GTK's
    /// `desired = order` branch. Turning the setting off must actually go
    /// back, not freeze whatever recency had produced.
    fn sort_projects_manual(&mut self) {
        let active_id = self.projects.get(self.active).map(|p| p.id);
        let saved = &self.saved;
        let keys: HashMap<u64, usize> = self
            .projects
            .iter()
            .map(|p| {
                let key = p.key();
                let manual = saved
                    .directories
                    .iter()
                    .position(|d| d == &key)
                    .unwrap_or(usize::MAX);
                (p.id, manual)
            })
            .collect();
        self.projects.sort_by_key(|p| keys[&p.id]);
        if let Some(id) = active_id
            && let Some(idx) = self.projects.iter().position(|p| p.id == id)
        {
            self.active = idx;
        }
    }

    /// Stamp and re-sort on running-tier FLIPS — GTK's
    /// `refresh_project_running_state`, centralized: both edges stamp
    /// `last_used` (starting floats the project to the bottom of the
    /// running tier, stopping slots it at the TOP of the stopped tier —
    /// stamping only on start would order stopped projects by when they
    /// were last STARTED, so stopping the one you started first would drop
    /// it below projects you had already stopped). Runs at the top of
    /// `update()`, the git-view invalidation idiom, so no status-mutation
    /// site can be missed — exits arrive async, and stop/start/restart are
    /// eight call sites. The event AFTER the mutating one applies the
    /// order (a release always follows a press), which is also GTK's
    /// deferred-to-idle timing.
    fn refresh_recent_order(&mut self) {
        let mut flipped = false;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        for pidx in 0..self.projects.len() {
            let running = self.projects[pidx].has_running();
            if self.projects[pidx].was_running == running {
                continue;
            }
            self.projects[pidx].was_running = running;
            let key = self.projects[pidx].key();
            self.saved.set_last_used(&key, now);
            flipped = true;
        }
        if flipped && self.settings.sidebar.recent_first {
            self.sort_projects_recent_first();
        }
    }

    /// GTK-parity close save: reload from disk first (the GTK app may have
    /// saved while we ran — don't clobber its edits), then write only the
    /// window geometry. A maximized close keeps the last normal size and
    /// position on disk, so unmaximizing after relaunch restores them.
    fn save_window_state(&mut self, maximized: bool, position: Option<iced::Point>) {
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
        // Mirror into the LIVE settings too. Every settings toggle and font
        // change saves the whole struct, and `self.settings.window` was
        // populated once at launch — without the mirror, the first toggle
        // after a move/resize wrote the launch-time geometry back over what
        // the debounced saves had recorded, exactly the loss the debounce
        // exists to prevent (cargo watch kills without a close).
        self.settings.window = settings.window.clone();
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
                // Tracked continuously because a right-press event carries
                // no position — this is where its context menu opens.
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Event::CursorMoved(position))
                }
                // The keycap reveal. Taken from ModifiersChanged rather than
                // KeyPressed because a bare modifier is not a key press —
                // and read regardless of capture status, since the terminal
                // swallows the keyboard whenever it has focus, which is
                // exactly when the hint is wanted.
                iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => {
                    Some(Event::CtrlHeld(m.control()))
                }
                // Ctrl+Tab away and the release never arrives; without this
                // the sidebar comes back still wearing its keycaps.
                iced::Event::Window(iced::window::Event::Unfocused) => Some(Event::CtrlHeld(false)),
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

/// Centered elevated card on the main-pane surface.
fn form_card(content: iced::widget::Column<'_, Event>) -> Element<'_, Event> {
    container(
        container(content.width(420))
            .padding(24)
            .style(theme::form_card),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .style(theme::pane)
    .into()
}

/// Debounce before a remote path completion goes out — long enough to skip
/// probing on every keystroke, short enough to feel live over a warm
/// ControlMaster connection. GTK's `SUGGEST_DEBOUNCE_MS`.
const SUGGEST_DEBOUNCE: Duration = Duration::from_millis(250);

/// Detection for the ADD flow, mirroring GTK's `prepare_project_inner(dir,
/// conservative: false)`: an authored `tuxflow.toml` wins outright and leaves
/// nothing to choose between, otherwise the FULL detector runs — the add
/// dialog offers everything, and `finish_add_project` persists whatever the
/// startup loader wouldn't re-detect.
fn detect_for_add(dir: &std::path::Path) -> (String, Vec<detector::DetectedStack>, bool) {
    use tuxflow_core::config::loader;
    let base = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("project"));
    match loader::find_config(dir).and_then(|p| loader::load_config(&p).ok()) {
        Some(config) => (config.project.name, Vec::new(), true),
        None => (base, detector::detect_stacks(dir), false),
    }
}

/// GTK's advice verbatim: BatchMode can't answer a password or a first-time
/// host-key prompt, so the fix is always to connect once by hand.
fn connect_hint(host: &str, err: &ProbeError) -> String {
    match err {
        ProbeError::Invalid(msg) => msg.clone(),
        ProbeError::Unreachable(msg) => format!(
            "{msg}\n\nIf this host needs a password or first-time host-key \
             confirmation, connect once in a terminal (ssh {host}), then retry."
        ),
    }
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

/// One small icon button inside a sidebar hover cluster.
fn row_action(
    icon: &'static [u8],
    tint: iced::Color,
    label: String,
    event: Event,
) -> Element<'static, Event> {
    tip(
        // 12px icon + 2px vertical padding = 16px, safely under the
        // row's text line — a taller button would grow the row on hover
        // and shove everything below it down a few pixels.
        button(symbolic(icon, 12.0, tint))
            .padding([2, 4])
            .style(theme::toolbar_icon(false))
            .on_press(event)
            .into(),
        label,
        iced::widget::tooltip::Position::Bottom,
    )
}

fn open_badge(project: &ProjectState, entry: &ProcessEntry) {
    if let Some(url) = browser_url(project, &entry.config.name) {
        log::info!("open badge {url}");
        if let Err(e) = open::that(&url) {
            log::warn!("open {url} failed: {e}");
        }
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

/// The recent-first comparison key for one project, sorted ascending:
/// running tier first, then within it `last_used` ASCENDING (stable start
/// order — GTK appends a newly started project BELOW the already-running),
/// while the stopped tier runs DESCENDING (most recently used first); the
/// sign flip encodes the direction switch into one key. Never-used stopped
/// projects tie at zero and fall to the manual position.
fn recent_order_key(
    running: bool,
    last_used: u64,
    manual: usize,
) -> (std::cmp::Reverse<bool>, i64, usize) {
    let tier_key = match running {
        true => last_used as i64,
        false => -(last_used as i64),
    };
    (std::cmp::Reverse(running), tier_key, manual)
}

/// An avatar path we can actually draw, or None for the initials square.
/// Existence is checked HERE, once at load, and never again: a saved icon
/// can outlive the file it names (a deleted logo, a cleared cache), and iced
/// draws a missing image as an empty hole where GTK's initials would be —
/// but `view` runs every frame and has no business touching the disk.
fn usable_icon(path: Option<String>) -> Option<PathBuf> {
    let path = PathBuf::from(path?);
    path.is_file().then_some(path)
}

/// What Copy Path puts on the clipboard: the local path, or the scp-style
/// `host:dir` a remote project pastes straight into scp/rsync.
fn copyable_path(location: &ProjectLocation) -> String {
    match location {
        ProjectLocation::Local(p) => p.to_string_lossy().into_owned(),
        ProjectLocation::Ssh { host, dir } => format!("{host}:{dir}"),
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

/// The sidebar card's group separator: the air under the project header
/// and between one category's rows and the next.
fn group_gap() -> Element<'static, Event> {
    container(column![]).height(3).into()
}

/// 1px hairline — vertical.
fn vline() -> Element<'static, Event> {
    container(column![])
        .width(1)
        .height(Length::Fill)
        .style(theme::hairline)
        .into()
}

// The GTK sidebar's icons, vendored from adwaita-icon-theme (see
// assets/icons/README.md) so the shells share the exact glyphs.
const ICON_SIDEBAR: &[u8] = include_bytes!("../assets/icons/sidebar-show-symbolic.svg");
const ICON_FIND: &[u8] = include_bytes!("../assets/icons/edit-find-symbolic.svg");
const ICON_GEAR: &[u8] = include_bytes!("../assets/icons/emblem-system-symbolic.svg");
const ICON_ADD: &[u8] = include_bytes!("../assets/icons/list-add-symbolic.svg");
const ICON_PLAY: &[u8] = include_bytes!("../assets/icons/media-playback-start-symbolic.svg");
const ICON_STOP: &[u8] = include_bytes!("../assets/icons/media-playback-stop-symbolic.svg");
const ICON_RESTART: &[u8] = include_bytes!("../assets/icons/view-refresh-symbolic.svg");
const ICON_CHANGES: &[u8] = include_bytes!("../assets/icons/send-to-symbolic.svg");
const ICON_FOCUS: &[u8] = include_bytes!("../assets/icons/focus-windows-symbolic.svg");
const ICON_CLEAR: &[u8] = include_bytes!("../assets/icons/edit-clear-symbolic.svg");
const ICON_EXTERNAL: &[u8] = include_bytes!("../assets/icons/external-link-symbolic.svg");
const ICON_REMOTE: &[u8] = include_bytes!("../assets/icons/tuxflow-remote-symbolic.svg");

/// A symbolic icon: the baked-in fill is overridden by the tint, which
/// is what makes these behave like GTK's -symbolic icons.
fn symbolic(bytes: &'static [u8], px: f32, tint: iced::Color) -> iced::widget::svg::Svg<'static> {
    iced::widget::svg(iced::widget::svg::Handle::from_memory(bytes))
        .width(px)
        .height(px)
        .style(move |_, _| iced::widget::svg::Style { color: Some(tint) })
}

/// Hang a tooltip on anything. GTK's icon-only chips are unreadable
/// without one, so every glyph-only control in the app goes through here.
fn tip<'a>(
    content: Element<'a, Event>,
    label: String,
    position: iced::widget::tooltip::Position,
) -> Element<'a, Event> {
    tip_after(content, label, position, Duration::ZERO)
}

/// `tip` that waits before opening — for hints hung on something the
/// pointer crosses on its way elsewhere (a whole sidebar row), rather
/// than on a control it was aimed at.
fn tip_after<'a>(
    content: Element<'a, Event>,
    label: String,
    position: iced::widget::tooltip::Position,
    delay: Duration,
) -> Element<'a, Event> {
    iced::widget::tooltip(content, text(label).size(11), position)
        .gap(4)
        .padding(7)
        .delay(delay)
        .style(theme::tooltip)
        .into()
}

/// `host:dir` for a remote project — what the status bar's remote glyph
/// says on hover. None for a local one: the icon isn't shown at all.
fn remote_hint(location: &ProjectLocation) -> Option<String> {
    match location {
        ProjectLocation::Local(_) => None,
        ProjectLocation::Ssh { host, dir } => Some(format!("{host}:{dir}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPAN: f32 = 160.0;

    #[test]
    fn ramp_advances_then_settles() {
        let mut a = Anim::default();
        let g = a.start();
        assert_eq!(a.eased(), 0.0);
        // Ten 16ms frames cover a 160ms span exactly.
        for _ in 0..9 {
            assert!(a.tick(g, SPAN), "ramp stopped early");
        }
        assert!(!a.tick(g, SPAN), "ramp should have arrived");
        assert!(a.settled());
        assert_eq!(a.eased(), 1.0);
    }

    #[test]
    fn stale_ticks_are_dropped() {
        let mut a = Anim::default();
        let old = a.start();
        a.tick(old, SPAN);
        let new = a.start();
        assert_ne!(old, new);
        // The abandoned chain must not advance the restarted ramp — two
        // live chains would run it at double speed.
        assert!(!a.tick(old, SPAN));
        assert_eq!(a.eased(), 0.0);
    }

    #[test]
    fn reversal_is_continuous() {
        let mut a = Anim::default();
        let g = a.start();
        for _ in 0..3 {
            a.tick(g, SPAN);
        }
        // Reversing mid-glide: the widget's position is `1 - eased`, and
        // the new ramp has to start exactly there or it visibly jumps.
        let mirrored = 1.0 - a.eased();
        a.restart_at(mirrored);
        assert!(
            (a.eased() - mirrored).abs() < 1e-5,
            "{} != {mirrored}",
            a.eased()
        );
    }

    /// `(name, category)` → a running entry.
    fn up(name: &str, category: ProcessCategory) -> ProcessEntry {
        let mut e = ProcessEntry::new(ProcessConfig {
            name: name.into(),
            command: format!("echo {name}"),
            working_dir: None,
            start_with_project: false,
            auto_restart: false,
            open_in_browser: false,
            restart_when_changed: Vec::new(),
            env: Default::default(),
            category,
            auto_named: false,
            display_name: None,
        });
        e.status = Status::Running;
        e
    }

    fn down(name: &str, category: ProcessCategory) -> ProcessEntry {
        let mut e = up(name, category);
        e.status = Status::Stopped;
        e
    }

    #[test]
    fn slots_follow_drawn_order_not_entry_order() {
        // The reported card: four commands with an agent saved third. The
        // agent draws at the TOP of the card, so it must own Ctrl+1 — the
        // old code numbered by entry index and labelled it Ctrl+3, which is
        // what made the sidebar read 3,1,2,4,5 down the screen.
        use ProcessCategory::{Agent, Command};
        let entries = vec![
            up("dev", Command),
            up("build", Command),
            up("shopify app launch status", Agent),
            up("deploy", Command),
            up("deploy shopify config", Command),
        ];
        let targets = switch_targets_of([entries.as_slice()]);
        assert_eq!(targets, vec![(0, 2), (0, 0), (0, 1), (0, 3), (0, 4)]);
    }

    #[test]
    fn categories_draw_agents_commands_terminals_ssh() {
        // Must match GTK's `running_names_in_sidebar_order`; the iced port
        // had Terminal and SSH the other way round.
        use ProcessCategory::{Agent, Command, SSH, Terminal};
        let entries = vec![
            up("s", SSH),
            up("t", Terminal),
            up("c", Command),
            up("a", Agent),
        ];
        let order: Vec<usize> = sidebar_order(&entries).collect();
        assert_eq!(order, vec![3, 2, 1, 0]);
    }

    /// GTK's sort_project_rows contract: running projects on top in START
    /// order (ascending stamps — the newest start sits at the BOTTOM of
    /// the tier, so the already-running rows hold still), stopped ones
    /// below it most-recently-used first, never-used last in manual order.
    #[test]
    fn recent_first_orders_in_two_tiers() {
        let mut rows = vec![
            ("stopped-recent", recent_order_key(false, 200, 0)),
            ("running-old", recent_order_key(true, 50, 1)),
            ("never-used-b", recent_order_key(false, 0, 3)),
            ("running-new", recent_order_key(true, 90, 4)),
            ("stopped-older", recent_order_key(false, 100, 5)),
            ("never-used-a", recent_order_key(false, 0, 2)),
        ];
        rows.sort_by_key(|(_, k)| *k);
        let order: Vec<&str> = rows.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            order,
            vec![
                "running-old",
                "running-new",
                "stopped-recent",
                "stopped-older",
                "never-used-a",
                "never-used-b",
            ]
        );
    }

    #[test]
    fn stopped_rows_take_no_slot() {
        // GTK only numbers running processes: the chord's whole job is to
        // focus a terminal, and a stopped row has none.
        use ProcessCategory::Command;
        let entries = vec![
            down("a", Command),
            up("b", Command),
            down("c", Command),
            up("d", Command),
        ];
        assert_eq!(
            switch_targets_of([entries.as_slice()]),
            vec![(0, 1), (0, 3)]
        );
    }

    #[test]
    fn numbering_runs_across_projects() {
        // Global, not per-card — Ctrl+N addresses the whole sidebar.
        use ProcessCategory::Command;
        let a = vec![up("a1", Command), up("a2", Command)];
        let b = vec![up("b1", Command)];
        let targets = switch_targets_of([a.as_slice(), b.as_slice()]);
        assert_eq!(targets, vec![(0, 0), (0, 1), (1, 0)]);
    }

    #[test]
    fn numbering_stops_at_nine() {
        use ProcessCategory::Command;
        let entries: Vec<ProcessEntry> = (0..12).map(|i| up(&i.to_string(), Command)).collect();
        assert_eq!(switch_targets_of([entries.as_slice()]).len(), SWITCH_SLOTS);
    }
}
