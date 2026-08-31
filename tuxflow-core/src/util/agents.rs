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

/// One entry in the "which agent?" list both shells offer when adding an
/// agent process.
///
/// Deliberately NOT keyed on [`AgentKind`]. That enum earns its variants by
/// having behaviour attached — a per-agent notification sound, a resume
/// affordance, OpenCode's own-notifications suppression — and a settings
/// field behind each. This table is only "what do people run", so it can
/// carry an agent the app has no special handling for; that one gets
/// `AgentKind::Unknown`'s defaults, which is honest rather than a
/// half-wired variant.
pub struct AgentPreset {
    /// Menu label.
    pub label: &'static str,
    /// Stem for the generated process name, and the `new_agent:<slug>`
    /// palette action id in the GTK app.
    pub slug: &'static str,
    /// What actually gets run. Editable after picking — a preset is a
    /// starting point, not a constraint.
    pub command: &'static str,
}

pub const AGENT_PRESETS: &[AgentPreset] = &[
    AgentPreset {
        label: "Claude Code",
        slug: "claude",
        command: "claude",
    },
    AgentPreset {
        label: "Codex",
        slug: "codex",
        command: "codex",
    },
    AgentPreset {
        label: "Gemini CLI",
        slug: "gemini",
        command: "gemini",
    },
    AgentPreset {
        label: "OpenCode",
        slug: "opencode",
        command: "opencode",
    },
    AgentPreset {
        label: "Aider",
        slug: "aider",
        command: "aider",
    },
];

/// A process name that doesn't collide with `taken`: the slug itself, then
/// `slug-2`, `slug-3`… Agents get added several at a time to one project
/// (two Claudes on different parts of a task is the normal case), and GTK's
/// `claude-a1b2c3d4` uuid suffix is unreadable in a sidebar row.
pub fn unique_agent_name(taken: &[String], slug: &str) -> String {
    if !taken.iter().any(|n| n == slug) {
        return slug.to_string();
    }
    (2..)
        .map(|n| format!("{slug}-{n}"))
        .find(|candidate| !taken.iter().any(|n| n == candidate))
        .unwrap_or_else(|| slug.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preset's command must resolve back through `from_command`, or a
    /// picked agent would lose its resume item and notification sound the
    /// moment it was created. Presets with no integrated kind are allowed —
    /// they just have to be a deliberate choice, which this pins.
    #[test]
    fn presets_map_to_the_kind_they_claim() {
        for preset in AGENT_PRESETS {
            let kind = AgentKind::from_command(preset.command);
            let expected = match preset.slug {
                "claude" => AgentKind::Claude,
                "codex" => AgentKind::Codex,
                "gemini" => AgentKind::Gemini,
                "opencode" => AgentKind::OpenCode,
                // Aider has no per-agent sound or resume flag wired up.
                "aider" => AgentKind::Unknown,
                other => panic!("preset {other} has no expected kind"),
            };
            assert_eq!(kind, expected, "preset {}", preset.label);
        }
    }

    /// Adding a second Claude to a project must not collide with the first —
    /// the iced form refuses a duplicate name outright, so a colliding
    /// suggestion would read as a dead "add" button.
    #[test]
    fn names_step_around_what_is_taken() {
        assert_eq!(unique_agent_name(&[], "claude"), "claude");
        let taken = vec!["claude".to_string()];
        assert_eq!(unique_agent_name(&taken, "claude"), "claude-2");
        let taken = vec!["claude".to_string(), "claude-2".to_string()];
        assert_eq!(unique_agent_name(&taken, "claude"), "claude-3");
        // An unrelated name never pushes the stem along.
        let taken = vec!["web".to_string(), "codex".to_string()];
        assert_eq!(unique_agent_name(&taken, "claude"), "claude");
    }
}
