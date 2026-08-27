//! Recognizing the built-in AI agents from a process command, and their
//! CLI affordances. Extracted from the GTK app's notifications module so
//! both shells share one agent table (context menus, notification rules).

/// Identifies a built-in AI agent so notifications can apply per-agent
/// preferences (e.g. a different sound for Claude vs. Codex) or suppression
/// rules (OpenCode emits its own desktop notifications, so TuxFlow stays
/// silent for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Unknown,
}

impl AgentKind {
    /// Derived from the first whitespace-separated token of `ProcessConfig.command`.
    /// Matches the agent labels set by `new_agent:<kind>` in the command
    /// palette (see the GTK app's `src/ui/window.rs`).
    pub fn from_command(command: &str) -> Self {
        // Take the executable name (basename of first whitespace-separated token,
        // lowercased) so paths and shell aliases both resolve. Common aliases
        // like `cc` for Claude Code are recognized.
        let token = command.split_whitespace().next().unwrap_or("");
        let exe = token.rsplit('/').next().unwrap_or(token).to_lowercase();
        match exe.as_str() {
            "claude" | "claude-code" | "cc" => Self::Claude,
            "codex" => Self::Codex,
            "gemini" => Self::Gemini,
            "opencode" => Self::OpenCode,
            _ => Self::Unknown,
        }
    }
}

/// Build a "resume previous session" command for the given agent invocation.
/// Returns `None` when the agent kind has no CLI affordance for resuming
/// (Gemini uses an in-app `/restore` slash command) or the command is unknown.
///
/// Preserves the user's executable path/alias but drops any extra argv tokens.
/// For Codex this is required because `resume` is a subcommand with its own
/// option set; for Claude/OpenCode top-level flags would still apply, but we
/// drop them for consistency and predictability.
pub fn resume_command_for(command: &str) -> Option<String> {
    let token = command.split_whitespace().next().unwrap_or("");
    if token.is_empty() {
        return None;
    }
    match AgentKind::from_command(command) {
        AgentKind::Claude | AgentKind::OpenCode => Some(format!("{token} --continue")),
        AgentKind::Codex => Some(format!("{token} resume --last")),
        AgentKind::Gemini | AgentKind::Unknown => None,
    }
}
