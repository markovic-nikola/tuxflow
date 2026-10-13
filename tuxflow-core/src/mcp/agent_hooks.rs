//! Claude Code's own account of an agent's turn — finished, or waiting on
//! a permission — instead of the terminal bell, which says neither which
//! nor why (and rings only when the user has set Claude's notification
//! channel to `terminal_bell`).
//!
//! Nothing for the user to set up: TuxFlow ships a Claude Code PLUGIN that
//! carries the hooks and points its agents at it through the environment
//! (`CLAUDE_CODE_PLUGIN_DIRS`, Claude Code 2.1.280+). No settings file is
//! edited, no command line rewritten, and a Claude started anywhere else
//! never loads it. Older Claude versions ignore the variable and keep the
//! bell. The plugin lives at [`PLUGIN_DIR`] under `$HOME` on this machine
//! and on every host (written by the MCP provisioning there).
//!
//! A hook runs [`SCRIPT`], which sends ONE line — a JSON-RPC notification,
//! [`EVENT_METHOD`] — down the project's MCP socket (`TUXFLOW_MCP_SOCKET`,
//! already in every agent's environment, forwarded to the host for remote
//! projects) and hangs up. The server recognises it before the MCP
//! handshake ([`parse_event_line`]), so it needs no socket of its own.
//! `TUXFLOW_PROCESS` names the agent, since one project socket serves all
//! of its processes.

use std::path::PathBuf;

use crate::remote::sh_quote;

/// The agent's process name, in its environment: which row a hook speaks for.
pub const PROCESS_ENV: &str = "TUXFLOW_PROCESS";
/// Claude Code's list of extra plugin directories (`:`-separated).
pub const PLUGIN_DIRS_ENV: &str = "CLAUDE_CODE_PLUGIN_DIRS";
/// The plugin's directory, relative to `$HOME`.
pub const PLUGIN_DIR: &str = ".cache/tuxflow/claude-plugin";
/// The one-line notification a hook sends.
pub const EVENT_METHOD: &str = "tuxflow/agent_event";

const PLUGIN_JSON: &str = r#"{"name":"tuxflow","version":"1.0.0","description":"Tells TuxFlow when this agent finishes its turn or needs you. Loaded only by agents TuxFlow starts."}"#;

/// What every hook runs. Silent and always successful: a host without
/// python3, a socket that is gone, a script that fails — Claude shows a
/// "hook error" line in the transcript for any of those, and the marker is
/// not worth that.
const HOOK_COMMAND: &str = "command -v python3 >/dev/null 2>&1 && python3 \"${CLAUDE_PLUGIN_ROOT}/hooks/tuxflow-event.py\" >/dev/null 2>&1; exit 0";

/// The hooks: `Stop` (turn finished — not fired on an Esc interrupt),
/// `PermissionRequest` (the permission dialog is about to show), the
/// `Notification`s that mean "waiting on you", and `UserPromptSubmit`
/// (the user answered, wherever from).
const HOOK_EVENTS: &[&str] = &[
    "Stop",
    "PermissionRequest",
    "Notification",
    "UserPromptSubmit",
];

/// Forwards the hook's input, cut to the fields TuxFlow reads.
const SCRIPT: &str = r#"#!/usr/bin/env python3
# TuxFlow agent hook (auto-installed; safe to delete)
import json, os, socket, sys

path = os.environ.get("TUXFLOW_MCP_SOCKET")
process = os.environ.get("TUXFLOW_PROCESS")
if path and process:
    try:
        hook = json.load(sys.stdin)
    except ValueError:
        sys.exit(0)
    keep = {k: hook[k] for k in ("hook_event_name", "notification_type", "message", "tool_name") if k in hook}
    line = json.dumps({"jsonrpc": "2.0", "method": "tuxflow/agent_event", "params": {"process": process, "hook": keep}})
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(2)
    try:
        s.connect(os.path.expanduser(path))
        s.sendall((line + "\n").encode())
    except OSError:
        pass
    finally:
        s.close()
"#;

fn hooks_json() -> String {
    let handler = serde_json::json!([{
        "hooks": [{"type": "command", "command": HOOK_COMMAND, "timeout": 5}]
    }]);
    let hooks: serde_json::Map<String, serde_json::Value> = HOOK_EVENTS
        .iter()
        .map(|e| (e.to_string(), handler.clone()))
        .collect();
    serde_json::json!({ "hooks": hooks }).to_string()
}

/// The plugin's files, relative to its directory.
fn files() -> [(&'static str, String); 3] {
    [
        (".claude-plugin/plugin.json", PLUGIN_JSON.to_string()),
        ("hooks/hooks.json", hooks_json()),
        ("hooks/tuxflow-event.py", SCRIPT.to_string()),
    ]
}

/// Write the plugin on this machine and return its directory — rewritten
/// on every launch, so a plugin left by an older TuxFlow never lingers.
/// `None` when it can't be written; agents then go without it.
pub fn install_local() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(PLUGIN_DIR);
    for (rel, content) in files() {
        let path = dir.join(rel);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&path, content));
        if let Err(e) = written {
            log::warn!("agent hooks: can't write {}: {e}", path.display());
            return None;
        }
    }
    Some(dir)
}

/// The same files as a shell command for a host, run alongside the MCP
/// shim's install (and, like it, rewritten every time).
pub fn remote_install_script() -> String {
    let mut parts = vec![format!(
        "mkdir -p ~/{PLUGIN_DIR}/.claude-plugin ~/{PLUGIN_DIR}/hooks"
    )];
    for (rel, content) in files() {
        parts.push(format!(
            "printf '%s' {} > ~/{PLUGIN_DIR}/{rel}",
            sh_quote(&content)
        ));
    }
    parts.join(" && ")
}

/// What a hook says about its agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// The turn ended.
    Finished,
    /// A permission dialog or a question is waiting; `reason` is Claude's
    /// own wording when it gave one.
    NeedsInput { reason: Option<String> },
    /// The user answered — from this terminal or any other.
    Resumed,
}

/// A hook's line: `Some((process, event))` when it is one, with `None` for
/// an event that changes nothing (an `idle_prompt` after the `Stop` that
/// already marked the turn) — it still tells the app this agent's hooks
/// are live. `None` for every other line, which goes on to MCP.
pub fn parse_event_line(line: &str) -> Option<(String, Option<AgentEvent>)> {
    let msg: serde_json::Value = serde_json::from_str(line).ok()?;
    if msg.get("method")?.as_str()? != EVENT_METHOD {
        return None;
    }
    let params = msg.get("params")?;
    let process = params.get("process")?.as_str()?.to_string();
    let hook = params.get("hook")?;
    let field = |k: &str| hook.get(k).and_then(|v| v.as_str());
    let event = match field("hook_event_name")? {
        "Stop" => Some(AgentEvent::Finished),
        "PermissionRequest" => Some(AgentEvent::NeedsInput {
            reason: field("tool_name").map(|t| format!("wants to use {t}")),
        }),
        "Notification" => match field("notification_type") {
            Some("permission_prompt" | "elicitation_dialog" | "elicitation_url_dialog") => {
                Some(AgentEvent::NeedsInput {
                    reason: field("message").map(String::from),
                })
            }
            _ => None,
        },
        "UserPromptSubmit" => Some(AgentEvent::Resumed),
        _ => None,
    };
    Some((process, event))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(hook: serde_json::Value) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": EVENT_METHOD,
            "params": {"process": "claude", "hook": hook},
        })
        .to_string()
    }

    #[test]
    fn hook_lines_map_to_agent_events() {
        let parse = |hook| parse_event_line(&line(hook)).map(|(_, e)| e);
        assert_eq!(
            parse(serde_json::json!({"hook_event_name": "Stop"})),
            Some(Some(AgentEvent::Finished))
        );
        assert_eq!(
            parse(serde_json::json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash"})),
            Some(Some(AgentEvent::NeedsInput {
                reason: Some("wants to use Bash".into())
            }))
        );
        assert_eq!(
            parse(serde_json::json!({
                "hook_event_name": "Notification",
                "notification_type": "permission_prompt",
                "message": "Claude needs your permission to use Bash",
            })),
            Some(Some(AgentEvent::NeedsInput {
                reason: Some("Claude needs your permission to use Bash".into())
            }))
        );
        // The idle reminder comes a minute after the Stop that already
        // marked the turn: a live hook, but no news.
        assert_eq!(
            parse(
                serde_json::json!({"hook_event_name": "Notification", "notification_type": "idle_prompt"})
            ),
            Some(None)
        );
        assert_eq!(
            parse(serde_json::json!({"hook_event_name": "UserPromptSubmit"})),
            Some(Some(AgentEvent::Resumed))
        );
    }

    /// Everything else on the socket is an MCP client's and must reach rmcp.
    #[test]
    fn mcp_traffic_is_not_a_hook_line() {
        let init = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#;
        assert_eq!(parse_event_line(init), None);
        assert_eq!(parse_event_line("not json"), None);
    }

    /// The hooks file is generated, so pin its shape: every event, each
    /// running the one silent command.
    #[test]
    fn hooks_file_registers_every_event() {
        let v: serde_json::Value = serde_json::from_str(&hooks_json()).unwrap();
        for event in HOOK_EVENTS {
            let cmd = &v["hooks"][event][0]["hooks"][0]["command"];
            assert_eq!(cmd.as_str(), Some(HOOK_COMMAND), "{event}");
        }
    }

    /// The host's copy comes from a shell command full of quotes, `$` and
    /// newlines: run it for real and compare the files byte for byte.
    #[test]
    fn remote_install_writes_the_same_files() {
        let home = std::env::temp_dir().join(format!("tuxflow-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(remote_install_script())
            .env("HOME", &home)
            .status()
            .unwrap();
        assert!(status.success());
        for (rel, content) in files() {
            let written = std::fs::read_to_string(home.join(PLUGIN_DIR).join(rel)).unwrap();
            assert_eq!(written, content, "{rel}");
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
