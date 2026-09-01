//! Editing a project — the iced port of GTK's `edit_project_dialog`.
//!
//! Same split as `add_project`: view + pure state here, everything that
//! touches the workspace (the command union at open, the icon workers,
//! save) in `App::update_edit_project`. A full-pane view rather than GTK's
//! modal: the command list wants the height, and the on-host icon browser —
//! a dialog of its own in GTK only because the preferences page left it no
//! room — fits inline here as the same completion list the add-project
//! path field uses.

use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Length};
use tuxflow_core::config::schema::{ProcessCategory, ProcessConfig};

use crate::theme::{self, CRASHED, DIM, TEXT, TEXT_SECONDARY, bold};
use crate::widgets::{avatar, group, group_described, row_base, suggestion_list, switch_row_owned};

/// Which group renders a toggle row — GTK's `source_label`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Active,
    Hidden,
    New,
}

/// One switch row: a process the project has, had, or could have.
pub struct ToggleEntry {
    pub config: ProcessConfig,
    pub on: bool,
    pub initial_on: bool,
    pub source: Source,
}

pub struct State {
    /// The project this form edits. The form closes centrally when the
    /// active project stops matching, like the Git Changes view.
    pub project: u64,
    pub name: String,
    /// The project key (`/dir` or `ssh://host/dir`) — GTK's Directory row,
    /// and the cache key a remote icon pick is stored under.
    pub key: String,
    pub remote: bool,
    /// Pending icon, always a LOCAL path (remote picks are fetched into
    /// the cache at pick time, so Save stays synchronous). None = initials.
    pub icon: Option<String>,
    /// The image path being typed/browsed, on whichever machine the
    /// project lives.
    pub icon_path: String,
    /// Completions for `icon_path`, and the stamp of the keystroke that
    /// asked for them — the add-project idiom, seeded from the same
    /// striding epoch so no two form instances ever share a stamp.
    pub suggestions: Vec<String>,
    pub stamp: u64,
    /// Stamp of the in-flight icon fetch/detect worker, bumped per
    /// request: a remote fetch is seconds long and the form stays usable
    /// under it, so a reply must not overwrite a newer pick.
    pub fetch_stamp: u64,
    pub commands: Vec<ToggleEntry>,
    /// A blocking icon step in flight (fetching from the host). Shown in
    /// place of the error; disables Save and the icon actions.
    pub busy: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Close,
    NameInput(String),
    Toggle(usize, bool),
    IconPathInput(String),
    IconSuggestions {
        stamp: u64,
        paths: Vec<String>,
    },
    UseIconSuggestion(String),
    /// Enter in the path field — commit what is typed as the icon.
    CommitIconPath,
    IconAutoDetect,
    /// A fetch/detect worker came back with a local path (or nothing).
    IconFetched {
        stamp: u64,
        path: Option<String>,
    },
    IconClear,
    CopyPath,
    OpenEditor,
    Save,
    RemoveProject,
}

/// The union GTK's `list_toggleable_commands` builds: active processes
/// (ON), hidden ones resolved from the custom commands or the detection
/// pool (OFF), then the pool's leftovers as newly detected (OFF). Deduped
/// by name with active > hidden > new priority; Terminal and SSH
/// categories are excluded throughout — those are created, not detected,
/// and a "terminal 2" switch row would read as a way to close one.
pub fn toggle_entries(
    active: &[ProcessConfig],
    deleted: &[String],
    custom: &[ProcessConfig],
    pool: &[ProcessConfig],
) -> Vec<ToggleEntry> {
    let excluded =
        |c: &ProcessConfig| matches!(c.category, ProcessCategory::Terminal | ProcessCategory::SSH);
    let mut seen: Vec<&str> = Vec::new();
    let mut entries = Vec::new();

    for config in active {
        if excluded(config) || seen.contains(&config.name.as_str()) {
            continue;
        }
        seen.push(&config.name);
        entries.push(ToggleEntry {
            config: config.clone(),
            on: true,
            initial_on: true,
            source: Source::Active,
        });
    }

    // A hidden name resolves to a config or it can't be offered back: the
    // user's own copy first (their edit of the process), detection second.
    for name in deleted {
        if seen.contains(&name.as_str()) {
            continue;
        }
        let Some(config) = custom
            .iter()
            .find(|c| &c.name == name)
            .or_else(|| pool.iter().find(|c| &c.name == name))
        else {
            continue;
        };
        if excluded(config) {
            continue;
        }
        seen.push(name);
        entries.push(ToggleEntry {
            config: config.clone(),
            on: false,
            initial_on: false,
            source: Source::Hidden,
        });
    }

    for config in pool {
        if excluded(config) || seen.contains(&config.name.as_str()) {
            continue;
        }
        seen.push(&config.name);
        entries.push(ToggleEntry {
            config: config.clone(),
            on: false,
            initial_on: false,
            source: Source::New,
        });
    }

    entries
}

/// What Save must apply: configs switched ON, names switched OFF.
pub fn diff(entries: &[ToggleEntry]) -> (Vec<ProcessConfig>, Vec<String>) {
    let enabled = entries
        .iter()
        .filter(|e| e.on && !e.initial_on)
        .map(|e| e.config.clone())
        .collect();
    let disabled = entries
        .iter()
        .filter(|e| !e.on && e.initial_on)
        .map(|e| e.config.name.clone())
        .collect();
    (enabled, disabled)
}

// ── View ────────────────────────────────────────────────────────────────

pub fn view(state: &'_ State) -> Element<'_, Msg> {
    let accent = theme::accent_for(state.remote);
    let busy = state.busy.is_some();

    // ── Project: name, and the directory with its quick actions ─────────
    let name_input = text_input("Name", &state.name)
        .on_input(Msg::NameInput)
        .style(theme::input(accent))
        .padding([6, 10])
        .size(12.5)
        .width(260);
    let dir_actions = row![
        pill("Copy Path", Some(Msg::CopyPath)),
        pill("Open in Editor", Some(Msg::OpenEditor)),
    ]
    .spacing(6);
    let mut content = column![group(
        "Project",
        vec![
            row_base("Name", "", name_input.into()),
            row_base("Directory", &state.key, dir_actions.into()),
        ],
    )]
    .spacing(14);

    // ── Icon: preview + auto-detect/reset, and the path browser ─────────
    let preview = avatar(
        state.icon.as_deref().map(std::path::Path::new),
        &state.name,
        accent,
        state.remote,
        32.0,
    );
    let icon_state = match state.icon {
        Some(_) => "Custom image",
        None => "Default initials",
    };
    let icon_row = container(
        row![
            preview,
            column![
                text("Icon").size(13).color(TEXT),
                text(icon_state).size(10.5).color(DIM),
            ]
            .spacing(3)
            .width(Length::Fill),
            pill("Auto-detect", (!busy).then_some(Msg::IconAutoDetect)),
            pill("Reset to Initials", (!busy).then_some(Msg::IconClear)),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center),
    )
    .padding([7, 14])
    .into();

    let mut path_field = text_input(
        // GTK's remote icon picker placeholder, shared by the local half —
        // "this machine" is the host a local project lives on.
        match state.remote {
            true => "Path to an image on the host\u{2026}",
            false => "Path to an image\u{2026}",
        },
        &state.icon_path,
    )
    .style(theme::input(accent))
    .padding([6, 10])
    .size(12.5)
    .width(260);
    if !busy {
        path_field = path_field
            .on_input(Msg::IconPathInput)
            .on_submit(Msg::CommitIconPath);
    }
    content = content.push(group(
        "",
        vec![icon_row, row_base("Custom image", "", path_field.into())],
    ));
    if !state.suggestions.is_empty() {
        content = content.push(suggestion_list(&state.suggestions, Msg::UseIconSuggestion));
    }

    // ── Commands: the Active / Hidden / Detected switch groups ──────────
    let mut sections: [(Source, String, &str, Vec<Element<'_, Msg>>); 3] = [
        (
            Source::Active,
            String::from("Active"),
            "Currently part of this project. Toggle off to stop and hide.",
            Vec::new(),
        ),
        (
            Source::Hidden,
            String::from("Hidden"),
            "Previously removed. Toggle on to restore.",
            Vec::new(),
        ),
        (
            Source::New,
            String::from("Detected"),
            "Found in this project but not yet added. Toggle on to include.",
            Vec::new(),
        ),
    ];
    for (index, entry) in state.commands.iter().enumerate() {
        let title = entry
            .config
            .display_name
            .clone()
            .unwrap_or_else(|| entry.config.name.clone());
        let switch = switch_row_owned(
            title,
            entry.config.command.clone(),
            entry.on,
            accent,
            move |on| Msg::Toggle(index, on),
        );
        if let Some(section) = sections.iter_mut().find(|s| s.0 == entry.source) {
            section.3.push(switch);
        }
    }
    for (_, title, description, rows) in sections {
        if !rows.is_empty() {
            let count = rows.len();
            content = content.push(group_described(
                format!("{title} ({count})"),
                description,
                rows,
            ));
        }
    }

    // Busy wins over a stale error, as in add_project.
    match (&state.busy, &state.error) {
        (Some(msg), _) => {
            content = content.push(text(msg.as_str()).size(11.5).color(TEXT_SECONDARY));
        }
        (None, Some(err)) => {
            content = content.push(text(err.as_str()).size(11.5).color(CRASHED));
        }
        _ => {}
    }

    // Save leads, the destructive action sits apart on the right — the
    // GTK dialog's headerbar Save and overflow Remove, flattened.
    let mut save = button(text("Save").size(12).font(bold()))
        .padding([7, 16])
        .style(theme::primary(accent));
    if !busy && !state.name.trim().is_empty() {
        save = save.on_press(Msg::Save);
    }
    content = content.push(
        row![
            save,
            iced::widget::space::horizontal(),
            button(text("Remove Project").size(12))
                .padding([7, 16])
                .style(theme::pill_intent(accent, CRASHED))
                .on_press(Msg::RemoveProject),
        ]
        .align_y(iced::Alignment::Center),
    );

    let header = row![
        text("Edit Project").size(16).font(bold()),
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
            iced::widget::scrollable(container(content).width(560).padding([0, 4]))
                .height(Length::Fill)
                .direction(iced::widget::scrollable::Direction::Vertical(
                    iced::widget::scrollable::Scrollbar::new()
                        .width(4)
                        .scroller_width(4),
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

fn pill(label: &str, msg: Option<Msg>) -> Element<'_, Msg> {
    let mut b = button(text(label).size(11.5))
        .padding([4, 10])
        .style(theme::pill_button(TEXT_SECONDARY));
    if let Some(msg) = msg {
        b = b.on_press(msg);
    }
    b.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(name: &str) -> ProcessConfig {
        ProcessConfig {
            name: name.into(),
            command: format!("run {name}"),
            working_dir: None,
            start_with_project: false,
            auto_restart: false,
            open_in_browser: false,
            restart_when_changed: Vec::new(),
            env: Default::default(),
            category: ProcessCategory::Command,
            auto_named: false,
            display_name: None,
        }
    }

    fn names_by_source(entries: &[ToggleEntry], source: Source) -> Vec<&str> {
        entries
            .iter()
            .filter(|e| e.source == source)
            .map(|e| e.config.name.as_str())
            .collect()
    }

    /// The GTK union: active rows ON, deleted names resolved into Hidden,
    /// the detection pool's leftovers as Detected — deduped by name with
    /// active > hidden > new priority.
    #[test]
    fn union_groups_active_hidden_and_new() {
        let active = [pc("web"), pc("api")];
        let deleted = [String::from("worker"), String::from("api")];
        let custom = [pc("worker")];
        let pool = [pc("web"), pc("worker"), pc("lint")];

        let entries = toggle_entries(&active, &deleted, &custom, &pool);
        assert_eq!(names_by_source(&entries, Source::Active), ["web", "api"]);
        // "api" is deleted-listed but active in the running project — the
        // active row wins, exactly one row per name.
        assert_eq!(names_by_source(&entries, Source::Hidden), ["worker"]);
        assert_eq!(names_by_source(&entries, Source::New), ["lint"]);
        assert!(entries.iter().all(|e| e.on == (e.source == Source::Active)));
    }

    /// A hidden name resolves from the user's custom copy FIRST — their
    /// edit of the process, not detection's idea of it.
    #[test]
    fn hidden_prefers_the_custom_copy() {
        let mut edited = pc("worker");
        edited.command = String::from("bun run worker --queue high");
        let deleted = [String::from("worker")];
        let entries = toggle_entries(&[], &deleted, &[edited], &[pc("worker")]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].config.command, "bun run worker --queue high");
    }

    /// A deleted name nothing can resolve (its stack left the project) is
    /// dropped rather than offered as an empty row — GTK's `continue`.
    #[test]
    fn unresolvable_hidden_names_are_dropped() {
        let deleted = [String::from("gone")];
        let entries = toggle_entries(&[], &deleted, &[], &[pc("web")]);
        assert_eq!(names_by_source(&entries, Source::New), ["web"]);
        assert_eq!(entries.len(), 1);
    }

    /// Terminals and SSH rows never appear in any group, wherever they
    /// come from — they are created, not detected.
    #[test]
    fn terminal_and_ssh_categories_are_excluded() {
        let mut term = pc("terminal 1");
        term.category = ProcessCategory::Terminal;
        let mut ssh = pc("vps");
        ssh.category = ProcessCategory::SSH;
        let entries = toggle_entries(
            &[term.clone(), pc("web")],
            &[String::from("vps")],
            &[ssh],
            &[term],
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].config.name, "web");
    }

    /// Save applies exactly the switches the user flipped: ON→OFF names to
    /// disable, OFF→ON configs to enable, untouched rows nothing at all.
    #[test]
    fn diff_reports_only_flipped_switches() {
        let mut entries = toggle_entries(
            &[pc("web"), pc("api")],
            &[String::from("worker")],
            &[pc("worker")],
            &[pc("lint")],
        );
        entries[1].on = false; // disable "api"
        entries[2].on = true; // restore "worker"

        let (enabled, disabled) = diff(&entries);
        let enabled: Vec<&str> = enabled.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(enabled, ["worker"]);
        assert_eq!(disabled, ["api"]);
    }

    /// Flipping a switch twice is a no-op, not an enable+disable.
    #[test]
    fn a_switch_flipped_back_reports_nothing() {
        let mut entries = toggle_entries(&[pc("web")], &[], &[], &[]);
        entries[0].on = false;
        entries[0].on = true;
        let (enabled, disabled) = diff(&entries);
        assert!(enabled.is_empty() && disabled.is_empty());
    }
}
