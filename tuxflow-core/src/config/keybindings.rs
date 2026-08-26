use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutAction {
    Copy,
    Paste,
    TerminalSearch,
    CommandPalette,
    AddNew,
    FilterProcesses,
    Settings,
    FocusSidebar,
    FocusTerminal,
    PrevProcess,
    NextProcess,
    FontIncrease,
    FontDecrease,
    QuickJump,
    NewTerminal,
    CloseProcess,
    PrevProject,
    NextProject,
    ClearOutput,
    ToggleProcess,
    RestartProcess,
    ToggleSidebar,
}

/// Returns (action, display_name, category) for all configurable actions.
pub fn action_metadata() -> Vec<(ShortcutAction, &'static str, &'static str)> {
    vec![
        (ShortcutAction::CommandPalette, "Command Palette", "General"),
        (ShortcutAction::AddNew, "Add Project or Process", "General"),
        (ShortcutAction::Settings, "Settings", "General"),
        (
            ShortcutAction::FilterProcesses,
            "Filter Processes",
            "General",
        ),
        (ShortcutAction::TerminalSearch, "Terminal Search", "General"),
        (ShortcutAction::Copy, "Copy", "General"),
        (ShortcutAction::Paste, "Paste", "General"),
        (ShortcutAction::FocusSidebar, "Focus Sidebar", "General"),
        (ShortcutAction::FocusTerminal, "Focus Terminal", "General"),
        (ShortcutAction::NewTerminal, "New Terminal", "General"),
        (
            ShortcutAction::CloseProcess,
            "Close Agent/Terminal",
            "General",
        ),
        (ShortcutAction::QuickJump, "Quick Jump", "Navigation"),
        (
            ShortcutAction::PrevProcess,
            "Previous Process",
            "Navigation",
        ),
        (ShortcutAction::NextProcess, "Next Process", "Navigation"),
        (
            ShortcutAction::PrevProject,
            "Previous Project",
            "Navigation",
        ),
        (ShortcutAction::NextProject, "Next Project", "Navigation"),
        (ShortcutAction::ClearOutput, "Clear Output", "General"),
        (
            ShortcutAction::ToggleProcess,
            "Start/Stop Process",
            "General",
        ),
        (ShortcutAction::RestartProcess, "Restart Process", "General"),
        (ShortcutAction::ToggleSidebar, "Toggle Sidebar", "General"),
        (
            ShortcutAction::FontIncrease,
            "Increase Font Size",
            "Terminal",
        ),
        (
            ShortcutAction::FontDecrease,
            "Decrease Font Size",
            "Terminal",
        ),
    ]
}
// ---------------------------------------------------------------------------
// KeybindingsSettings — serde-compatible, persisted to TOML
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsSettings {
    pub copy: String,
    pub paste: String,
    pub terminal_search: String,
    pub command_palette: String,
    pub add_new: String,
    pub filter_processes: String,
    pub settings: String,
    pub focus_sidebar: String,
    pub focus_terminal: String,
    pub prev_process: String,
    pub next_process: String,
    pub font_increase: String,
    pub font_decrease: String,
    pub quick_jump: String,
    pub new_terminal: String,
    pub close_process: String,
    pub prev_project: String,
    pub next_project: String,
    pub clear_output: String,
    pub toggle_process: String,
    pub restart_process: String,
    pub toggle_sidebar: String,
}

impl Default for KeybindingsSettings {
    fn default() -> Self {
        Self {
            copy: "Ctrl+Shift+C".into(),
            paste: "Ctrl+Shift+V".into(),
            terminal_search: "Ctrl+Shift+F".into(),
            command_palette: "Ctrl+Shift+P".into(),
            add_new: "Ctrl+N".into(),
            filter_processes: "Ctrl+F".into(),
            settings: "Ctrl+,".into(),
            focus_sidebar: "Ctrl+Left".into(),
            focus_terminal: "Ctrl+Right".into(),
            prev_process: "Ctrl+Up".into(),
            next_process: "Ctrl+Down".into(),
            font_increase: "Ctrl+=".into(),
            font_decrease: "Ctrl+-".into(),
            quick_jump: "Ctrl+G".into(),
            new_terminal: "Ctrl+T".into(),
            close_process: "Ctrl+Shift+W".into(),
            prev_project: "Ctrl+Shift+Up".into(),
            next_project: "Ctrl+Shift+Down".into(),
            clear_output: "Ctrl+Alt+C".into(),
            toggle_process: "Ctrl+Alt+S".into(),
            restart_process: "Ctrl+Alt+R".into(),
            toggle_sidebar: "Ctrl+\\".into(),
        }
    }
}

impl KeybindingsSettings {
    pub fn get(&self, action: ShortcutAction) -> &str {
        match action {
            ShortcutAction::Copy => &self.copy,
            ShortcutAction::Paste => &self.paste,
            ShortcutAction::TerminalSearch => &self.terminal_search,
            ShortcutAction::CommandPalette => &self.command_palette,
            ShortcutAction::AddNew => &self.add_new,
            ShortcutAction::FilterProcesses => &self.filter_processes,
            ShortcutAction::Settings => &self.settings,
            ShortcutAction::FocusSidebar => &self.focus_sidebar,
            ShortcutAction::FocusTerminal => &self.focus_terminal,
            ShortcutAction::PrevProcess => &self.prev_process,
            ShortcutAction::NextProcess => &self.next_process,
            ShortcutAction::FontIncrease => &self.font_increase,
            ShortcutAction::FontDecrease => &self.font_decrease,
            ShortcutAction::QuickJump => &self.quick_jump,
            ShortcutAction::NewTerminal => &self.new_terminal,
            ShortcutAction::CloseProcess => &self.close_process,
            ShortcutAction::PrevProject => &self.prev_project,
            ShortcutAction::NextProject => &self.next_project,
            ShortcutAction::ClearOutput => &self.clear_output,
            ShortcutAction::ToggleProcess => &self.toggle_process,
            ShortcutAction::RestartProcess => &self.restart_process,
            ShortcutAction::ToggleSidebar => &self.toggle_sidebar,
        }
    }

    pub fn set(&mut self, action: ShortcutAction, value: String) {
        match action {
            ShortcutAction::Copy => self.copy = value,
            ShortcutAction::Paste => self.paste = value,
            ShortcutAction::TerminalSearch => self.terminal_search = value,
            ShortcutAction::CommandPalette => self.command_palette = value,
            ShortcutAction::AddNew => self.add_new = value,
            ShortcutAction::FilterProcesses => self.filter_processes = value,
            ShortcutAction::Settings => self.settings = value,
            ShortcutAction::FocusSidebar => self.focus_sidebar = value,
            ShortcutAction::FocusTerminal => self.focus_terminal = value,
            ShortcutAction::PrevProcess => self.prev_process = value,
            ShortcutAction::NextProcess => self.next_process = value,
            ShortcutAction::FontIncrease => self.font_increase = value,
            ShortcutAction::FontDecrease => self.font_decrease = value,
            ShortcutAction::QuickJump => self.quick_jump = value,
            ShortcutAction::NewTerminal => self.new_terminal = value,
            ShortcutAction::CloseProcess => self.close_process = value,
            ShortcutAction::PrevProject => self.prev_project = value,
            ShortcutAction::NextProject => self.next_project = value,
            ShortcutAction::ClearOutput => self.clear_output = value,
            ShortcutAction::ToggleProcess => self.toggle_process = value,
            ShortcutAction::RestartProcess => self.restart_process = value,
            ShortcutAction::ToggleSidebar => self.toggle_sidebar = value,
        }
    }
}
