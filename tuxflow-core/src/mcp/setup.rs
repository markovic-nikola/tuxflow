//! Static copy for the Settings → Integrations page, shared by both
//! shells: what the MCP server exposes and how to point each client at
//! it. Data only — each shell renders it with its own widgets.

/// The tools `tuxflow-mcp` exposes, as (name, description).
pub const EXPOSED_TOOLS: &[(&str, &str)] = &[
    (
        "list_processes",
        "List all managed processes with their current status",
    ),
    (
        "get_project_info",
        "Get project overview with running/total counts",
    ),
    (
        "get_process_status",
        "Get detailed status of a process (PID, uptime, restarts)",
    ),
    (
        "get_process_logs",
        "Get recent terminal output from a process",
    ),
    ("restart_process", "Restart a managed process"),
    ("stop_process", "Stop a running process"),
    ("start_process", "Start a stopped process"),
];

/// The command an agent runs to reach TuxFlow: the shipped binary locally,
/// the shim TuxFlow installs on a remote host (not on the pane's PATH, so
/// spelled out). Both honour `TUXFLOW_MCP_SOCKET`, so no project argument.
pub const LOCAL_COMMAND: &str = "tuxflow-mcp";
pub const REMOTE_COMMAND: &str = "~/.local/bin/tuxflow-mcp";

/// One line under the setup rows saying where the command lives.
pub const COMMAND_NOTE: &str = "The command is tuxflow-mcp on this machine and ~/.local/bin/tuxflow-mcp on a remote host \
     (TuxFlow installs it there). Agents started from a TuxFlow terminal find their project on \
     their own; restart an agent after adding the server.";

/// CLI-tool setup rows, as (tool, the one line shown under the name, text
/// to copy). For a tool with an `mcp add` subcommand the copied text IS
/// the shown line — a click on Copy must give exactly what the row says,
/// nothing to trim — and the remote-host variant is its own row rather
/// than a paragraph inside the copy. Tools without a subcommand show
/// where the config goes and copy the config.
pub const CLI_SETUP: &[(&str, &str, &str)] = &[
    (
        "Claude Code",
        "claude mcp add --scope user tuxflow -- tuxflow-mcp",
        "claude mcp add --scope user tuxflow -- tuxflow-mcp",
    ),
    (
        "Claude Code on a remote host",
        "claude mcp add --scope user tuxflow -- ~/.local/bin/tuxflow-mcp",
        "claude mcp add --scope user tuxflow -- ~/.local/bin/tuxflow-mcp",
    ),
    (
        "Codex",
        "codex mcp add tuxflow -- tuxflow-mcp",
        "codex mcp add tuxflow -- tuxflow-mcp",
    ),
    (
        "Codex on a remote host",
        "codex mcp add tuxflow -- ~/.local/bin/tuxflow-mcp",
        "codex mcp add tuxflow -- ~/.local/bin/tuxflow-mcp",
    ),
    (
        "Gemini CLI",
        "gemini mcp add tuxflow tuxflow-mcp",
        "gemini mcp add tuxflow tuxflow-mcp",
    ),
    (
        "Gemini CLI on a remote host",
        "gemini mcp add tuxflow ~/.local/bin/tuxflow-mcp",
        "gemini mcp add tuxflow ~/.local/bin/tuxflow-mcp",
    ),
    (
        "Amp",
        "amp mcp add tuxflow -- tuxflow-mcp",
        "amp mcp add tuxflow -- tuxflow-mcp",
    ),
    (
        "Amp on a remote host",
        "amp mcp add tuxflow -- ~/.local/bin/tuxflow-mcp",
        "amp mcp add tuxflow -- ~/.local/bin/tuxflow-mcp",
    ),
    (
        "OpenCode",
        "opencode.json (project or ~/.config/opencode/)",
        r#"{
  "mcp": {
    "tuxflow": {
      "type": "local",
      "command": ["tuxflow-mcp"]
    }
  }
}"#,
    ),
    (
        "Aider",
        ".aider.conf.yml",
        r#"mcp-servers:
  - command: tuxflow-mcp"#,
    ),
];

const CURSOR_CONFIG: &str = r#"Add to .cursor/mcp.json:
{
  "mcpServers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}"#;

/// IDE / app setup rows, as (tool, where it goes, config to copy).
pub const IDE_SETUP: &[(&str, &str, &str)] = &[
    (
        "VS Code",
        ".vscode/mcp.json",
        r#"Add to .vscode/mcp.json:
{
  "servers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}"#,
    ),
    ("Cursor", ".cursor/mcp.json", CURSOR_CONFIG),
    ("Windsurf", ".windsurf/mcp.json", CURSOR_CONFIG),
    (
        "Zed",
        "Zed settings.json",
        r#"Add to Zed settings.json:
{
  "context_servers": {
    "tuxflow": {
      "command": { "path": "tuxflow-mcp" }
    }
  }
}"#,
    ),
    (
        "Cline",
        "Cline settings panel",
        r#"Add via Cline settings:
  Command: tuxflow-mcp"#,
    ),
    (
        "Claude Desktop",
        "claude_desktop_config.json",
        r#"Add to claude_desktop_config.json:
{
  "mcpServers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}"#,
    ),
];
