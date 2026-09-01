//! Adding a project — the iced port of GTK's dialog chain.
//!
//! GTK spreads this over a native folder picker, `add_remote_project_dialog`,
//! and then one of two follow-up dialogs (`show_confirm_project_dialog` for a
//! plain rename, `select_commands_dialog` when detection found more than a
//! handful). Here it is one full-pane view walking the same stages, so the
//! local and remote halves share the path field, the completion list and the
//! whole tail of the flow instead of being two separate dialogs that happen
//! to end in the same place.
//!
//! The one deliberate difference from GTK: there is no native folder picker.
//! A portal dialog can only browse THIS machine, and the remote half needs an
//! in-app browser regardless — so both halves complete paths through the same
//! widget, over `remote::fs::list_dirs`.
//!
//! View + state only. Everything that touches the workspace (duplicate
//! checks, detection workers, persistence) lives in `App::update_add_project`,
//! the way `git_view` splits against `App::update_git_view`.

use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Length};
use tuxflow_core::config::schema::ProcessConfig;
use tuxflow_core::config::ssh::SshHost;
use tuxflow_core::detect::detector::{self, DetectedStack};
use tuxflow_core::remote::ProjectLocation;

use crate::theme::{self, CRASHED, DIM, TEXT, TEXT_SECONDARY, bold};
use crate::widgets::{group, switch_row_owned};

/// The label standing in for "no ~/.ssh/config host" in the picker —
/// GTK's `ComboRow` entry, verbatim.
pub const CUSTOM_HOST: &str = "Custom...";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Local,
    Remote,
}

impl Kind {
    pub fn is_remote(self) -> bool {
        matches!(self, Kind::Remote)
    }
}

/// Where the flow is. `Choose` is the fork GTK offers as two command-palette
/// items; `Locate` is whichever of the two pickers that choice led to;
/// `Configure` is the shared tail both of them feed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Choose,
    Locate,
    Configure,
}

pub struct State {
    pub stage: Stage,
    pub kind: Kind,
    /// Host field — an alias from ~/.ssh/config or a literal `user@host`.
    pub host: String,
    /// The directory being typed, on whichever machine `kind` names.
    pub path: String,
    pub hosts: Vec<SshHost>,
    pub host_choice: String,
    /// Completions for `path`, and the stamp of the keystroke that asked for
    /// them — a listing that lands after a newer keystroke is dropped rather
    /// than painted over what is being typed now.
    pub suggestions: Vec<String>,
    pub stamp: u64,
    /// Stamp of the in-flight DETECTION, bumped whenever the flow moves under
    /// one. A remote probe is seconds long and the form stays usable during
    /// it, so a reply that lands after the user pressed Back would otherwise
    /// throw them into the Configure stage for a project they abandoned.
    /// Separate from `stamp`, which every keystroke bumps — typing while a
    /// probe runs must not cancel it.
    pub probe_stamp: u64,
    /// A blocking step is in flight (verifying, detecting). Holds the line
    /// shown in place of the error, and disables the commit button.
    pub busy: Option<String>,
    pub error: Option<String>,
    pub configure: Option<Configure>,
}

/// The tail of the flow: name the project, and — when detection found enough
/// to be worth choosing between — pick which commands to keep.
pub struct Configure {
    pub key: String,
    pub location: ProjectLocation,
    pub name: String,
    /// What detection called it, so a name is persisted as an override only
    /// when the user actually changed it — GTK compares against the DETECTED
    /// name, which for a `tuxflow.toml` project is the authored one, not the
    /// directory's basename. Saving an unchanged name would pin it, and a
    /// later edit of tuxflow.toml would then never show.
    pub detected_name: String,
    pub stacks: Vec<DetectedStack>,
    /// Parallel to the stacks flattened in order; `select` decides whether
    /// the user ever sees it.
    pub selected: Vec<bool>,
    pub config_loaded: bool,
    /// Show the command list, or just ask for a name (GTK's two dialogs).
    pub select: bool,
}

impl Configure {
    /// Detected processes flattened in render order, so `selected` can be a
    /// flat parallel vec while the view still groups by stack.
    pub fn flat(&self) -> impl Iterator<Item = &ProcessConfig> {
        self.stacks
            .iter()
            .flat_map(|s| s.suggested_processes.iter())
    }

    pub fn total(&self) -> usize {
        self.selected.len()
    }

    pub fn chosen(&self) -> usize {
        self.selected.iter().filter(|s| **s).count()
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Pick(Kind),
    Back,
    Close,
    HostChoice(String),
    HostInput(String),
    PathInput(String),
    Suggestions {
        stamp: u64,
        dirs: Vec<String>,
    },
    UseSuggestion(String),
    /// Commit the Locate stage: verify (remote) and run detection.
    Locate,
    Detected(Box<Detected>),
    Failed {
        stamp: u64,
        error: String,
    },
    NameInput(String),
    Toggle(usize, bool),
    SetAll(bool),
    /// Commit the Configure stage — the project is added here.
    Confirm,
}

/// A finished detection pass, ready for the Configure stage.
#[derive(Debug, Clone)]
pub struct Detected {
    /// The `probe_stamp` this detection was requested under.
    pub stamp: u64,
    pub key: String,
    pub location: ProjectLocation,
    pub name: String,
    pub stacks: Vec<DetectedStack>,
    pub config_loaded: bool,
}

impl State {
    /// `epoch` seeds both generation counters. It must be disjoint from
    /// every previous form instance's range (the app strides it per open):
    /// listings and probes in flight when a form closes still arrive, get
    /// routed to whatever form exists then, and with counters restarting
    /// at 0 a reopened form would adopt the abandoned instance's replies —
    /// up to a Configure stage for the previously typed project.
    pub fn new(hosts: Vec<SshHost>, epoch: u64) -> Self {
        Self {
            stage: Stage::Choose,
            kind: Kind::Local,
            host: String::new(),
            path: String::new(),
            hosts,
            host_choice: CUSTOM_HOST.to_string(),
            suggestions: Vec::new(),
            stamp: epoch,
            probe_stamp: epoch,
            busy: None,
            error: None,
            configure: None,
        }
    }

    /// The host to complete paths against — `None` for the local half.
    pub fn probe_host(&self) -> Option<String> {
        let host = self.host.trim();
        match self.kind.is_remote() && !host.is_empty() {
            true => Some(host.to_string()),
            false => None,
        }
    }

    /// Can the Locate stage be committed? GTK's rule: a host (remote only)
    /// and an absolute path.
    pub fn can_locate(&self) -> bool {
        let path_ok = self.path.trim().starts_with('/');
        let host_ok = !self.kind.is_remote() || !self.host.trim().is_empty();
        path_ok && host_ok && self.busy.is_none()
    }

    /// Completions only make sense once there is somewhere to look and an
    /// absolute path to look for — the same gate GTK's `make_job` applies.
    pub fn can_complete(&self) -> bool {
        self.path.trim_start().starts_with('/')
            && (!self.kind.is_remote() || !self.host.trim().is_empty())
    }

    pub fn enter(&mut self, kind: Kind) {
        self.kind = kind;
        self.stage = Stage::Locate;
        self.probe_stamp += 1;
        // The suggestion stamp too: a listing still in flight for the OTHER
        // kind (a slow remote ls, say) must not paint the VPS's directories
        // under a local path field the moment it lands.
        self.stamp += 1;
        self.suggestions.clear();
        self.error = None;
        self.busy = None;
    }

    /// Set up the Configure stage from a finished detection, unless the flow
    /// moved on while it was in flight.
    pub fn configured(&mut self, d: Detected) {
        if d.stamp != self.probe_stamp {
            return;
        }
        let select = detector::needs_command_selection(d.config_loaded, &d.stacks);
        let total: usize = d.stacks.iter().map(|s| s.suggested_processes.len()).sum();
        self.configure = Some(Configure {
            key: d.key,
            location: d.location,
            detected_name: d.name.clone(),
            name: d.name,
            stacks: d.stacks,
            selected: vec![true; total],
            config_loaded: d.config_loaded,
            select,
        });
        self.stage = Stage::Configure;
        self.busy = None;
        self.error = None;
    }
}

// ── View ────────────────────────────────────────────────────────────────

pub fn view(state: &'_ State) -> Element<'_, Msg> {
    let accent = theme::accent_for(state.kind.is_remote() && state.stage != Stage::Choose);

    let body = match state.stage {
        Stage::Choose => view_choose(),
        Stage::Locate => view_locate(state, accent),
        Stage::Configure => match &state.configure {
            Some(c) => view_configure(state, c, accent),
            None => column![].into(),
        },
    };

    let header = row![
        text(title_for(state)).size(16).font(bold()),
        iced::widget::space::horizontal(),
        button(text("\u{00d7} Close").size(12))
            .padding([5, 12])
            .style(theme::pill_button(TEXT_SECONDARY))
            .on_press(Msg::Close),
    ]
    .align_y(iced::Alignment::Center);

    container(
        column![
            header,
            scrollable(container(body).width(560).padding([0, 4]))
                .height(Length::Fill)
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new().width(4).scroller_width(4),
                ))
                .style(theme::overlay_scrollbar)
                .width(Length::Fill),
        ]
        .spacing(16),
    )
    .padding(20)
    .style(theme::pane)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn title_for(state: &State) -> &'static str {
    match (state.stage, state.kind) {
        (Stage::Choose, _) => "Add Project",
        (Stage::Locate, Kind::Local) => "Open Project Directory",
        (Stage::Locate, Kind::Remote) => "Add Remote Project",
        (Stage::Configure, _) => match state.configure.as_ref().is_some_and(|c| c.select) {
            true => "Select Commands",
            false => "Add Project",
        },
    }
}

/// The fork GTK offers as two command-palette items.
fn view_choose<'a>() -> Element<'a, Msg> {
    let card = |title: &'static str, subtitle: &'static str, kind: Kind| {
        button(
            container(
                column![
                    text(title).size(14).font(bold()).color(TEXT),
                    text(subtitle).size(11).color(DIM),
                ]
                .spacing(5),
            )
            .padding([12, 16])
            .width(Length::Fill),
        )
        .padding(0)
        .width(Length::Fill)
        .style(theme::choice_card(theme::accent_for(kind.is_remote())))
        .on_press(Msg::Pick(kind))
    };

    column![
        card(
            "New project (open directory)",
            "A directory on this machine",
            Kind::Local,
        ),
        card(
            "New remote project (over SSH)",
            "A directory on a host from ~/.ssh/config",
            Kind::Remote,
        ),
    ]
    .spacing(8)
    .into()
}

/// The path picker — GTK's folder chooser and its remote dialog, merged.
fn view_locate<'a>(state: &'a State, accent: iced::Color) -> Element<'a, Msg> {
    let mut fields: Vec<Element<'a, Msg>> = Vec::new();
    // A verify captures the host/path at submit and proceeds with THOSE, as
    // GTK's dialog does. GTK leaves its entries live regardless, which lets
    // you retype under an in-flight probe and then land in the next stage
    // configuring the path you submitted rather than the one on screen — so
    // the fields freeze with the button instead of only the button.
    let busy = state.busy.is_some();

    if state.kind.is_remote() {
        // The ~/.ssh/config picker. Selecting an alias fills the host field
        // with the ALIAS, never its resolved hostname — ssh resolves it
        // itself, which is what preserves ProxyJump/IdentityFile/User.
        let mut options: Vec<String> = vec![CUSTOM_HOST.to_string()];
        options.extend(state.hosts.iter().map(|h| h.name.clone()));
        fields.push(crate::widgets::row_base(
            "SSH Config Host",
            "Pick from ~/.ssh/config or enter custom",
            pick_list(options, Some(state.host_choice.clone()), Msg::HostChoice)
                .text_size(12)
                .padding([4, 10])
                .into(),
        ));
        let mut host_field = text_input("Hostname", &state.host)
            .style(theme::input(accent))
            .padding([6, 10])
            .size(12.5)
            .width(260);
        if !busy {
            host_field = host_field.on_input(Msg::HostInput);
        }
        fields.push(crate::widgets::row_base(
            "Host (alias or user@host)",
            "",
            host_field.into(),
        ));
    }

    let path_title = match state.kind {
        Kind::Local => "Project directory (absolute path)",
        Kind::Remote => "Remote directory (absolute path)",
    };
    let mut path_field = text_input("/path/to/project", &state.path)
        .style(theme::input(accent))
        .padding([6, 10])
        .size(12.5)
        .width(260);
    if !busy {
        path_field = path_field.on_input(Msg::PathInput).on_submit(Msg::Locate);
    }
    fields.push(crate::widgets::row_base(path_title, "", path_field.into()));

    let mut content = column![group("", fields)].spacing(14);

    // Completions. Absent entirely when there is nothing to show, so an
    // empty box never sits under the field.
    if !state.suggestions.is_empty() {
        content = content.push(crate::widgets::suggestion_list(
            &state.suggestions,
            Msg::UseSuggestion,
        ));
    }

    if let Some(line) = status_line(state) {
        content = content.push(line);
    }

    content = content.push(
        row![
            button(text("Back").size(12))
                .padding([7, 16])
                .style(theme::pill_button(TEXT_SECONDARY))
                .on_press(Msg::Back),
            iced::widget::space::horizontal(),
            commit_button(
                match state.kind {
                    Kind::Local => "Open",
                    Kind::Remote => "Add",
                },
                accent,
                state.can_locate().then_some(Msg::Locate),
            ),
        ]
        .align_y(iced::Alignment::Center),
    );

    content.into()
}

/// Name the project, and choose its commands when there are enough to choose.
fn view_configure<'a>(state: &'a State, c: &'a Configure, accent: iced::Color) -> Element<'a, Msg> {
    let mut content = column![crate::widgets::row_owned(
        "Project Name".to_string(),
        c.location.key(),
        text_input("Name", &c.name)
            .on_input(Msg::NameInput)
            .on_submit(Msg::Confirm)
            .style(theme::input(accent))
            .padding([6, 10])
            .size(12.5)
            .width(260)
            .into(),
    )]
    .spacing(14);

    // Wrap the name row in a card of its own, matching GTK's group.
    content = column![group("", vec![content.into()])].spacing(14);

    if c.select {
        content = content.push(
            text(format!(
                "{} commands detected. Select which to add:",
                c.total()
            ))
            .size(12)
            .color(DIM),
        );
        content = content.push(
            row![
                button(text("Select All").size(12))
                    .padding([5, 12])
                    .style(theme::pill_button(TEXT_SECONDARY))
                    .on_press(Msg::SetAll(true)),
                button(text("Deselect All").size(12))
                    .padding([5, 12])
                    .style(theme::pill_button(TEXT_SECONDARY))
                    .on_press(Msg::SetAll(false)),
            ]
            .spacing(8),
        );

        // One group per detected stack, switches in flat index order so the
        // `selected` vec stays parallel to what is rendered.
        let mut idx = 0usize;
        for stack in &c.stacks {
            let mut rows: Vec<Element<'a, Msg>> = Vec::new();
            for proc in &stack.suggested_processes {
                let at = idx;
                idx += 1;
                rows.push(switch_row_owned(
                    proc.name.clone(),
                    proc.command.clone(),
                    c.selected.get(at).copied().unwrap_or(false),
                    accent,
                    move |on| Msg::Toggle(at, on),
                ));
            }
            content = content.push(group(&stack.name, rows));
        }
    }

    if let Some(line) = status_line(state) {
        content = content.push(line);
    }

    let label = match c.select {
        true => format!("Add {} Commands", c.chosen()),
        false => "Add Project".to_string(),
    };
    content = content.push(
        row![
            button(text("Back").size(12))
                .padding([7, 16])
                .style(theme::pill_button(TEXT_SECONDARY))
                .on_press(Msg::Back),
            iced::widget::space::horizontal(),
            commit_button_owned(
                label,
                accent,
                (!c.name.trim().is_empty() && state.busy.is_none()).then_some(Msg::Confirm),
            ),
        ]
        .align_y(iced::Alignment::Center),
    );

    content.into()
}

/// The busy/error line GTK renders as its status label. Busy wins: while a
/// check is in flight the previous failure is stale.
fn status_line<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    match (&state.busy, &state.error) {
        (Some(msg), _) => Some(text(msg.as_str()).size(11.5).color(TEXT_SECONDARY).into()),
        (None, Some(err)) => Some(text(err.as_str()).size(11.5).color(CRASHED).into()),
        _ => None,
    }
}

fn commit_button<'a>(label: &'a str, accent: iced::Color, msg: Option<Msg>) -> Element<'a, Msg> {
    let mut b = button(text(label).size(12).font(bold()))
        .padding([7, 16])
        .style(theme::primary(accent));
    if let Some(msg) = msg {
        b = b.on_press(msg);
    }
    b.into()
}

fn commit_button_owned<'a>(
    label: String,
    accent: iced::Color,
    msg: Option<Msg>,
) -> Element<'a, Msg> {
    let mut b = button(text(label).size(12).font(bold()))
        .padding([7, 16])
        .style(theme::primary(accent));
    if let Some(msg) = msg {
        b = b.on_press(msg);
    }
    b.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(kind: Kind, host: &str, path: &str) -> State {
        let mut s = State::new(Vec::new(), 0);
        s.kind = kind;
        s.host = host.to_string();
        s.path = path.to_string();
        s
    }

    /// The local half must not demand a host, and the remote half must.
    #[test]
    fn locate_gate_follows_gtk() {
        assert!(state_with(Kind::Local, "", "/srv/app").can_locate());
        assert!(!state_with(Kind::Local, "", "relative").can_locate());
        assert!(!state_with(Kind::Remote, "", "/srv/app").can_locate());
        assert!(state_with(Kind::Remote, "vps", "/srv/app").can_locate());
    }

    /// A blocking step in flight disables the commit button, so a second
    /// Enter can't launch a second verify over the first.
    #[test]
    fn busy_blocks_commit() {
        let mut s = state_with(Kind::Remote, "vps", "/srv/app");
        assert!(s.can_locate());
        s.busy = Some("Connecting to vps…".into());
        assert!(!s.can_locate());
    }

    /// Completion needs an absolute path, and on the remote half a host too
    /// — otherwise every keystroke would open an ssh connection to "".
    #[test]
    fn completion_gate() {
        assert!(state_with(Kind::Local, "", "/sr").can_complete());
        assert!(!state_with(Kind::Local, "", "sr").can_complete());
        assert!(!state_with(Kind::Remote, "", "/sr").can_complete());
        assert!(state_with(Kind::Remote, "vps", "/sr").can_complete());
    }

    /// `selected` must line up with the flattened render order, or a toggle
    /// on the last stack would flip a process in the first.
    #[test]
    fn selection_is_parallel_to_flattened_stacks() {
        let mk = |n: &str| ProcessConfig {
            name: n.to_string(),
            command: format!("run {n}"),
            working_dir: None,
            start_with_project: false,
            auto_restart: false,
            open_in_browser: false,
            restart_when_changed: Vec::new(),
            env: Default::default(),
            category: Default::default(),
            auto_named: false,
            display_name: None,
        };
        let mut s = State::new(Vec::new(), 0);
        s.configured(Detected {
            stamp: s.probe_stamp,
            key: "/srv/app".into(),
            location: ProjectLocation::Local("/srv/app".into()),
            name: "app".into(),
            stacks: vec![
                DetectedStack {
                    name: "Node.js".into(),
                    suggested_processes: vec![mk("dev"), mk("build")],
                },
                DetectedStack {
                    name: "PHP".into(),
                    suggested_processes: vec![mk("serve")],
                },
            ],
            config_loaded: false,
        });
        let c = s.configure.as_ref().unwrap();
        assert_eq!(c.total(), 3);
        assert_eq!(c.chosen(), 3, "everything starts selected, as GTK's do");
        let names: Vec<&str> = c.flat().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["dev", "build", "serve"]);
    }

    /// A remote probe is seconds long and the form stays usable under it, so
    /// a reply for a stage the user has already left must be dropped — or
    /// pressing Back drops them into Configure for an abandoned project.
    #[test]
    fn a_probe_that_lands_after_back_is_dropped() {
        let mut s = State::new(Vec::new(), 0);
        s.enter(Kind::Remote);
        let stale = s.probe_stamp;
        // The user backs out while the probe is in flight.
        s.probe_stamp += 1;
        s.configured(Detected {
            stamp: stale,
            key: "ssh://vps/srv/app".into(),
            location: ProjectLocation::Ssh {
                host: "vps".into(),
                dir: "/srv/app".into(),
            },
            name: "app".into(),
            stacks: Vec::new(),
            config_loaded: true,
        });
        assert!(s.configure.is_none(), "stale probe opened Configure");
        assert_eq!(s.stage, Stage::Locate, "stale probe moved the stage");
    }

    /// A `tuxflow.toml` project has an authored list — nothing to choose
    /// between, so it gets the plain rename step however many it defines.
    #[test]
    fn config_projects_skip_selection() {
        let mut s = State::new(Vec::new(), 0);
        s.configured(Detected {
            stamp: s.probe_stamp,
            key: "/srv/app".into(),
            location: ProjectLocation::Local("/srv/app".into()),
            name: "app".into(),
            stacks: Vec::new(),
            config_loaded: true,
        });
        assert!(!s.configure.as_ref().unwrap().select);
    }
}
