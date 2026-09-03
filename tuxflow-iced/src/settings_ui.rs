//! The settings window, iced half — a full port of the GTK
//! `AdwPreferencesDialog`: Appearance, Sidebar, Notifications, Hotkeys,
//! Tools, Integrations, About. Rendered as a full-pane view in the main
//! area (this shell's forms are inline, not modal), with a page rail on
//! the left standing in for Adwaita's page switcher.
//!
//! This module is view-only: it renders `AppSettings` and emits `Msg`.
//! Mutation, saving and live-apply happen in `App::handle_settings` —
//! every change saves immediately, like the GTK dialog's per-row saves.
//! Settings whose iced consumer doesn't exist yet (color scheme, MCP,
//! file-watch…) still edit the shared settings.toml and say so in
//! their subtitle rather than pretending.

use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Length};
use tuxflow_core::config::keybindings::{ShortcutAction, action_metadata};
use tuxflow_core::config::palette;
use tuxflow_core::config::settings::{AppSettings, EDITOR_CHOICES, TERMINAL_CHOICES};
use tuxflow_core::mcp::setup;
use tuxflow_core::util::sounds::BUNDLED_SOUNDS;

use crate::theme::{self, CRASHED, TEXT_SECONDARY, bold};
use crate::widgets::{group, label_row, row_base, switch_row};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Appearance,
    Sidebar,
    Notifications,
    Hotkeys,
    Tools,
    Integrations,
    About,
}

const PAGES: &[(Page, &str)] = &[
    (Page::Appearance, "Appearance"),
    (Page::Sidebar, "Sidebar"),
    (Page::Notifications, "Notifications"),
    (Page::Hotkeys, "Hotkeys"),
    (Page::Tools, "Tools"),
    (Page::Integrations, "Integrations"),
    (Page::About, "About"),
];

pub struct State {
    pub page: Page,
    /// Hotkey being re-bound: the next ignored keypress lands here.
    pub capturing: Option<ShortcutAction>,
    /// Last capture that hit an existing binding: (attempted action,
    /// display name of the holder). Cleared on the next interaction.
    pub conflict: Option<(ShortcutAction, &'static str)>,
    /// Font family is typed, not picked — buffered until Enter so a
    /// half-typed name doesn't restyle every terminal per keystroke.
    pub font_family_draft: String,
    /// paplay feedback for the preview buttons (GTK shows a toast).
    pub sound_error: Option<String>,
    /// Which setup row's config was last copied (shows a ✓).
    pub copied: Option<&'static str>,
    /// 0 = CLI tools, 1 = IDEs — the expander stand-in.
    pub setup_open: Option<u8>,
}

impl State {
    pub fn new(s: &AppSettings) -> Self {
        Self {
            page: Page::Appearance,
            capturing: None,
            conflict: None,
            font_family_draft: s.appearance.font_family.clone(),
            sound_error: None,
            copied: None,
            setup_open: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Close,
    Page(Page),
    // Appearance
    ColorScheme(&'static str),
    AccentApp(&'static str),
    AccentLocal(&'static str),
    AccentRemote(&'static str),
    TermTheme(&'static str),
    FontFamilyDraft(String),
    FontFamilyApply,
    FontSize(u32),
    FontWeight(u32),
    BoldWeight(u32),
    LineHeight(f64),
    LetterSpacing(f64),
    Scrollback(u32),
    // Sidebar
    SingleExpand(bool),
    AutoHide(bool),
    KeybindHints(bool),
    RecentFirst(bool),
    // Notifications
    NotifyCrash(bool),
    NotifyRestart(bool),
    NotifyFileWatch(bool),
    NotifyFinish(bool),
    NotifyAgentIdle(bool),
    NotifySilenceFallback(bool),
    IdleThreshold(u32),
    SuppressFocused(bool),
    SoundEnabled(bool),
    Sound(&'static str),
    /// (agent 0=claude 1=codex 2=gemini, picked label incl. "(Use default)")
    AgentSound(u8, &'static str),
    TestNotification,
    /// None = the global sound; Some(agent) previews that agent's pick.
    PreviewSound(Option<u8>),
    // Hotkeys
    Capture(ShortcutAction),
    ResetKeys,
    // Tools
    Composer(bool),
    RemoteMic(bool),
    Editor(&'static str),
    ReuseEditor(bool),
    TerminalApp(&'static str),
    // Integrations
    McpEnabled(bool),
    ToggleSetup(u8),
    CopySetup(&'static str, &'static str),
    // About
    OpenSource,
}

const USE_DEFAULT: &str = "(Use default)";
const SCHEMES: &[&str] = &["System", "Dark", "Light"];

pub fn view<'a>(state: &'a State, s: &'a AppSettings) -> Element<'a, Msg> {
    let content = match state.page {
        Page::Appearance => page_appearance(state, s),
        Page::Sidebar => page_sidebar(s),
        Page::Notifications => page_notifications(state, s),
        Page::Hotkeys => page_hotkeys(state, s),
        Page::Tools => page_tools(s),
        Page::Integrations => page_integrations(state, s),
        Page::About => page_about(),
    };

    let mut rail = column![].spacing(2).width(130);
    for (page, label) in PAGES {
        rail = rail.push(
            button(text(*label).size(12.5))
                .width(Length::Fill)
                .padding([6, 12])
                .style(theme::process_row(
                    theme::accent_for(false),
                    *page == state.page,
                ))
                .on_press(Msg::Page(*page)),
        );
    }

    let header = row![
        text("Settings").size(16).font(bold()),
        iced::widget::space::horizontal(),
        button(text("\u{00d7} Close").size(12))
            .padding([5, 12])
            .style(theme::pill_button(theme::accent_for(false)))
            .on_press(Msg::Close),
    ]
    .align_y(iced::Alignment::Center);

    container(
        column![
            header,
            row![
                rail,
                scrollable(container(content.width(560)).padding([0, 18]))
                    .height(Length::Fill)
                    .direction(scrollable::Direction::Vertical(
                        scrollable::Scrollbar::new().width(4).scroller_width(4),
                    ))
                    .style(theme::overlay_scrollbar)
                    .width(Length::Fill),
            ]
            .spacing(10)
            .height(Length::Fill),
        ]
        .spacing(16),
    )
    .padding(20)
    .style(theme::pane)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

// ── Pages ───────────────────────────────────────────────────────────────

fn page_appearance<'a>(state: &'a State, s: &'a AppSettings) -> iced::widget::Column<'a, Msg> {
    let a = &s.appearance;
    let scheme = match a.theme.as_str() {
        "system" => "System",
        "light" => "Light",
        _ => "Dark",
    };
    let accent_label = |name: &str, fallback: &str| palette::accent_by_name(name, fallback).label;

    column![
        group(
            "Theme",
            vec![
                pick_row(
                    "Color Scheme",
                    "Application theme \u{2014} applies to the GTK shell (this one is dark)",
                    SCHEMES.to_vec(),
                    scheme,
                    Msg::ColorScheme,
                ),
                pick_row(
                    "Accent Color",
                    "Accent for buttons and highlights",
                    palette::accent_choices(),
                    accent_label(&a.accent_color, palette::FALLBACK_LOCAL),
                    Msg::AccentApp,
                ),
                pick_row(
                    "Local Project Accent",
                    "Sidebar color for projects on this machine",
                    palette::accent_choices(),
                    accent_label(&a.local_accent_color, palette::FALLBACK_LOCAL),
                    Msg::AccentLocal,
                ),
                pick_row(
                    "Remote Project Accent",
                    "Sidebar color for projects opened over SSH",
                    palette::accent_choices(),
                    accent_label(&a.remote_accent_color, palette::FALLBACK_REMOTE),
                    Msg::AccentRemote,
                ),
            ],
        ),
        group(
            "Terminal",
            vec![
                pick_row(
                    "Terminal Theme",
                    "Color scheme for terminal output",
                    palette::theme_choices(),
                    palette::terminal_theme(&a.terminal_theme).label,
                    Msg::TermTheme,
                ),
                row_base(
                    "Font Family",
                    "Monospace family name \u{2014} Enter applies to every terminal",
                    text_input("Monospace", &state.font_family_draft)
                        .on_input(Msg::FontFamilyDraft)
                        .on_submit(Msg::FontFamilyApply)
                        .style(theme::input(theme::accent_for(false)))
                        .padding([5, 10])
                        .size(12.5)
                        .width(190)
                        .into(),
                ),
                spin_u32("Font Size", "", a.font_size, 1, 6, 32, Msg::FontSize),
                spin_u32(
                    "Font Weight",
                    "Regular text weight",
                    a.font_weight,
                    100,
                    100,
                    900,
                    Msg::FontWeight,
                ),
                spin_u32(
                    "Bold Font Weight",
                    "GTK shell \u{2014} bold here is the toolkit's",
                    a.bold_font_weight,
                    100,
                    100,
                    900,
                    Msg::BoldWeight,
                ),
                spin_f64(
                    "Line Height",
                    "",
                    a.line_height,
                    0.1,
                    0.8,
                    2.0,
                    Msg::LineHeight,
                ),
                spin_f64(
                    "Letter Spacing",
                    "GTK shell \u{2014} not yet plumbed here",
                    a.letter_spacing,
                    0.5,
                    -2.0,
                    10.0,
                    Msg::LetterSpacing,
                ),
                spin_u32(
                    "Scrollback Lines",
                    "Applies to terminals started from now on",
                    a.scrollback_lines,
                    1000,
                    100,
                    100000,
                    Msg::Scrollback,
                ),
            ],
        ),
    ]
    .spacing(20)
}

fn page_sidebar<'a>(s: &'a AppSettings) -> iced::widget::Column<'a, Msg> {
    let sb = &s.sidebar;
    column![group(
        "Display",
        vec![
            switch_row(
                "Single Project Expand",
                "Only one project can be expanded at a time",
                sb.single_project_expand,
                Msg::SingleExpand,
            ),
            switch_row(
                "Auto-Hide Sidebar",
                "GTK shell \u{2014} this one has no sidebar hiding yet",
                sb.auto_hide_sidebar,
                Msg::AutoHide,
            ),
            switch_row(
                "Show Keybind Hints",
                // The only place the chord is spelled out now that the caps
                // carry a bare digit — and the only hint that holding Ctrl
                // shows them at all.
                "Hold Ctrl to number the first nine running processes in the sidebar",
                sb.show_keybind_hints,
                Msg::KeybindHints,
            ),
            switch_row(
                "Recently Used First",
                "Keep recently started projects at the top of the sidebar",
                sb.recent_first,
                Msg::RecentFirst,
            ),
        ],
    )]
    .spacing(20)
}

fn page_notifications<'a>(state: &'a State, s: &'a AppSettings) -> iced::widget::Column<'a, Msg> {
    let n = &s.notifications;

    let sound_labels: Vec<&'static str> = BUNDLED_SOUNDS.iter().map(|b| b.label).collect();
    let label_for = |id: &str| {
        BUNDLED_SOUNDS
            .iter()
            .find(|b| b.id == id)
            .map(|b| b.label)
            .unwrap_or(sound_labels[0])
    };
    let mut per_agent = vec![USE_DEFAULT];
    per_agent.extend_from_slice(&sound_labels);
    let agent_label = |pick: &Option<String>| match pick {
        Some(id) => label_for(id),
        None => USE_DEFAULT,
    };

    let mut sound_rows = vec![
        switch_row(
            "Play Sound",
            "Requires paplay (pulseaudio-utils)",
            n.sound_enabled,
            Msg::SoundEnabled,
        ),
        preview_row(
            "Notification Sound",
            "",
            sound_labels.clone(),
            label_for(&n.sound_name),
            Msg::Sound,
            Msg::PreviewSound(None),
        ),
        preview_row(
            "Claude Sound",
            "",
            per_agent.clone(),
            agent_label(&n.claude_sound_name),
            |l| Msg::AgentSound(0, l),
            Msg::PreviewSound(Some(0)),
        ),
        preview_row(
            "Codex Sound",
            "",
            per_agent.clone(),
            agent_label(&n.codex_sound_name),
            |l| Msg::AgentSound(1, l),
            Msg::PreviewSound(Some(1)),
        ),
        preview_row(
            "Gemini Sound",
            "",
            per_agent,
            agent_label(&n.gemini_sound_name),
            |l| Msg::AgentSound(2, l),
            Msg::PreviewSound(Some(2)),
        ),
        label_row(
            "OpenCode",
            "Uses its own desktop notifications \u{2014} TuxFlow stays silent for it.",
        ),
    ];
    if let Some(err) = &state.sound_error {
        sound_rows.push(
            container(text(err.as_str()).size(11).color(CRASHED))
                .padding([6, 14])
                .into(),
        );
    }

    column![
        group(
            "Desktop Notifications",
            vec![
                row_base(
                    "Send Test",
                    "Fire a sample notification right now",
                    small_button("Send Test", Msg::TestNotification),
                ),
                switch_row(
                    "Process Crash",
                    "Notify when a process crashes",
                    n.on_crash,
                    Msg::NotifyCrash,
                ),
                switch_row(
                    "Auto-Restart",
                    "Notify when a process is auto-restarted",
                    n.on_auto_restart,
                    Msg::NotifyRestart,
                ),
                switch_row(
                    "File Watch Restart",
                    "GTK shell \u{2014} the file watcher isn't ported yet",
                    n.on_file_watch_restart,
                    Msg::NotifyFileWatch,
                ),
                switch_row(
                    "Process Finished",
                    "Notify when a process exits on its own",
                    n.on_process_finish,
                    Msg::NotifyFinish,
                ),
                switch_row(
                    "Agent Idle",
                    "Notify when an AI agent rings its terminal bell. Claude Code: set its notification channel to Terminal Bell (\"auto\" is silent here); it rings ~1 min after a turn ends.",
                    n.on_agent_idle,
                    Msg::NotifyAgentIdle,
                ),
                switch_row(
                    "Silence-based Fallback",
                    "Also notify after N seconds of no agent output. May false-positive on long tool calls.",
                    n.on_agent_idle_silence_fallback,
                    Msg::NotifySilenceFallback,
                ),
                spin_u32(
                    "Idle Silence Threshold",
                    "Seconds of no output before firing the idle notification",
                    n.agent_idle_silence_seconds,
                    5,
                    5,
                    120,
                    Msg::IdleThreshold,
                ),
                switch_row(
                    "Suppress When Focused",
                    "Skip notifications for the terminal you're currently viewing",
                    n.suppress_when_focused,
                    Msg::SuppressFocused,
                ),
            ],
        ),
        group("Sound", sound_rows),
    ]
    .spacing(20)
}

fn page_hotkeys<'a>(state: &'a State, s: &'a AppSettings) -> iced::widget::Column<'a, Msg> {
    let mut col = column![].spacing(20);
    for category in ["General", "Navigation", "Terminal"] {
        let mut rows = Vec::new();
        for (action, display, cat) in action_metadata() {
            if cat != category {
                continue;
            }
            let label: String = if state.capturing == Some(action) {
                String::from("Press a key combo\u{2026}")
            } else if let Some((conflicted, holder)) = state.conflict
                && conflicted == action
            {
                format!("Used by {holder}")
            } else {
                s.keybindings.get(action).to_string()
            };
            rows.push(row_base(
                display,
                "",
                small_button_owned(label, Msg::Capture(action)),
            ));
        }
        col = col.push(group(category, rows));
    }

    let mut fixed_rows: Vec<Element<'_, Msg>> = vec![];
    for (name, shortcut) in [
        ("Switch to Process 1-9", "Ctrl+1-9"),
        ("Switch to Project 1-9", "Alt+1-9"),
        ("Focus Terminal", "Ctrl+Return"),
        ("Close Palette", "Escape"),
        ("Search Next", "Enter"),
        ("Search Previous", "Shift+Enter"),
        ("Close Search", "Escape"),
    ] {
        fixed_rows.push(row_base(
            name,
            "",
            text(shortcut).size(12).color(TEXT_SECONDARY).into(),
        ));
    }

    col = col
        .push(
            container(
                button(text("Reset All to Defaults").size(12))
                    .padding([6, 16])
                    .style(theme::pill_intent(theme::accent_for(false), CRASHED))
                    .on_press(Msg::ResetKeys),
            )
            .center_x(Length::Fill),
        )
        .push(group("Fixed Shortcuts \u{2014} not changeable", fixed_rows));
    col
}

fn page_tools<'a>(s: &'a AppSettings) -> iced::widget::Column<'a, Msg> {
    let t = &s.tools;
    let choice_label = |choices: &'static [(&'static str, &'static str)], cmd: &str| {
        choices
            .iter()
            .find(|(c, _)| *c == cmd)
            .map(|(_, l)| *l)
            .unwrap_or(choices[0].1)
    };
    column![
        group(
            "Agents",
            vec![
                switch_row(
                    "Message Composer",
                    "Compose messages locally under agent terminals and send in one go \u{2014} avoids per-keystroke lag on remote projects",
                    t.agent_composer,
                    Msg::Composer,
                ),
                switch_row(
                    "Remote Microphone",
                    "Let agents on remote hosts record voice input through this machine's microphone \u{2014} while a remote project is open, the host can listen",
                    t.remote_microphone,
                    Msg::RemoteMic,
                ),
            ],
        ),
        group(
            "Default Applications",
            vec![
                pick_row(
                    "Default Editor",
                    "Used when opening projects. Can be overridden per-project.",
                    EDITOR_CHOICES.iter().map(|(_, l)| *l).collect(),
                    choice_label(EDITOR_CHOICES, &t.default_editor),
                    Msg::Editor,
                ),
                switch_row(
                    "Reuse Editor Window",
                    "Open projects in the current editor window instead of a new one",
                    t.reuse_editor_window,
                    Msg::ReuseEditor,
                ),
                pick_row(
                    "Default Terminal",
                    "Used when opening projects from the sidebar.",
                    TERMINAL_CHOICES.iter().map(|(_, l)| *l).collect(),
                    choice_label(TERMINAL_CHOICES, &t.default_terminal),
                    Msg::TerminalApp,
                ),
            ],
        ),
    ]
    .spacing(20)
}

fn page_integrations<'a>(state: &'a State, s: &'a AppSettings) -> iced::widget::Column<'a, Msg> {
    let mut rows = vec![switch_row(
        "Enable MCP Server",
        "GTK shell \u{2014} this shell's MCP server is still pending. Expose process info via Unix socket.",
        s.integrations.mcp_enabled,
        Msg::McpEnabled,
    )];
    for (name, desc) in setup::EXPOSED_TOOLS {
        rows.push(label_row(name, desc));
    }

    let mut col = column![group("MCP Server", rows)].spacing(20);

    for (idx, (title, entries)) in [
        ("Setup: CLI tools", setup::CLI_SETUP),
        ("Setup: IDEs and apps", setup::IDE_SETUP),
    ]
    .into_iter()
    .enumerate()
    {
        let open = state.setup_open == Some(idx as u8);
        let mut rows: Vec<Element<'_, Msg>> = vec![row_base(
            title,
            "",
            small_button(
                if open { "Hide" } else { "Show" },
                Msg::ToggleSetup(idx as u8),
            ),
        )];
        if open {
            for (tool, location, config) in entries {
                let copied = state.copied == Some(*tool);
                rows.push(row_base(
                    tool,
                    location,
                    small_button(
                        if copied { "\u{2713} Copied" } else { "Copy" },
                        Msg::CopySetup(tool, config),
                    ),
                ));
            }
        }
        col = col.push(group("", rows));
    }
    col
}

fn page_about<'a>() -> iced::widget::Column<'a, Msg> {
    column![group(
        "TuxFlow \u{2014} a Linux-native dev environment manager",
        vec![
            label_row("Version", env!("CARGO_PKG_VERSION")),
            label_row("License", "MIT"),
            row_base(
                "Source Code",
                "github.com/markovic-nikola/tuxflow",
                small_button("Open", Msg::OpenSource),
            ),
        ],
    )]
    .spacing(20)
}

// ── Row builders ────────────────────────────────────────────────────────

fn pick_row<'a>(
    title: &'a str,
    subtitle: &'a str,
    options: Vec<&'static str>,
    selected: &'static str,
    f: fn(&'static str) -> Msg,
) -> Element<'a, Msg> {
    row_base(
        title,
        subtitle,
        pick_list(options, Some(selected), f)
            .text_size(12)
            .padding([4, 10])
            .into(),
    )
}

/// A pick_row with a play button beside it (sound previews).
fn preview_row<'a>(
    title: &'a str,
    subtitle: &'a str,
    options: Vec<&'static str>,
    selected: &'static str,
    f: impl Fn(&'static str) -> Msg + 'a,
    preview: Msg,
) -> Element<'a, Msg> {
    row_base(
        title,
        subtitle,
        row![
            pick_list(options, Some(selected), f)
                .text_size(12)
                .padding([4, 10]),
            small_button("\u{25b6}", preview),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into(),
    )
}

fn spin_u32<'a>(
    title: &'a str,
    subtitle: &'a str,
    value: u32,
    step: u32,
    min: u32,
    max: u32,
    f: fn(u32) -> Msg,
) -> Element<'a, Msg> {
    spin(
        title,
        subtitle,
        value.to_string(),
        (value > min).then(|| f(value.saturating_sub(step).max(min))),
        (value < max).then(|| f((value + step).min(max))),
    )
}

fn spin_f64<'a>(
    title: &'a str,
    subtitle: &'a str,
    value: f64,
    step: f64,
    min: f64,
    max: f64,
    f: fn(f64) -> Msg,
) -> Element<'a, Msg> {
    // One decimal keeps 0.1 steps exact enough and matches GTK's digits(1).
    let round = |v: f64| (v * 10.0).round() / 10.0;
    spin(
        title,
        subtitle,
        format!("{value:.1}"),
        (value > min).then(|| f(round((value - step).max(min)))),
        (value < max).then(|| f(round((value + step).min(max)))),
    )
}

fn spin<'a>(
    title: &'a str,
    subtitle: &'a str,
    display: String,
    dec: Option<Msg>,
    inc: Option<Msg>,
) -> Element<'a, Msg> {
    let side = |label: &'static str, msg: Option<Msg>| {
        let mut b = button(text(label).size(12))
            .padding([2, 9])
            .style(theme::pill_button(theme::accent_for(false)));
        if let Some(m) = msg {
            b = b.on_press(m);
        }
        b
    };
    row_base(
        title,
        subtitle,
        row![
            side("\u{2212}", dec),
            container(text(display).size(12.5)).center_x(52),
            side("+", inc),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center)
        .into(),
    )
}

fn small_button<'a>(label: &'static str, msg: Msg) -> Element<'a, Msg> {
    small_button_owned(label.to_string(), msg)
}

fn small_button_owned<'a>(label: String, msg: Msg) -> Element<'a, Msg> {
    button(text(label).size(11.5))
        .padding([4, 12])
        .style(theme::pill_button(theme::accent_for(false)))
        .on_press(msg)
        .into()
}
