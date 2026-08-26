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

const MCP_CONFIG: &str = r#"{
  "mcpServers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}"#;

/// CLI-tool setup rows, as (tool, where it goes, config to copy).
pub const CLI_SETUP: &[(&str, &str, &str)] = &[
    (
        "Claude Code",
        ".mcp.json or ~/.claude/settings.json",
        r#"Per-project: add to .mcp.json
Global: add to ~/.claude/settings.json

{
  "mcpServers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}

Auto-detects which project you're in.
If tuxflow-mcp is not in PATH, use the full path."#,
    ),
    (
        "Codex",
        "CLI flag or ~/.codex/config.toml",
        r#"codex --mcp-config '{"tuxflow":{"command":"tuxflow-mcp"}}'
Or add to ~/.codex/config.toml under [mcp]"#,
    ),
    ("OpenCode", ".opencode/mcp.json", MCP_CONFIG),
    (
        "Gemini CLI",
        "CLI flag or ~/.gemini/settings.json",
        r#"gemini --mcp '{"tuxflow":{"command":"tuxflow-mcp"}}'
Or add to ~/.gemini/settings.json under mcpServers"#,
    ),
    ("Amp", ".amp/mcp.json", MCP_CONFIG),
    (
        "Aider",
        ".aider.conf.yml",
        r#"Add to .aider.conf.yml:
mcp-servers:
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
