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

/// The hooks: `Stop` (turn finished — not fired on an Esc interrupt; its
/// last message decides finished vs [`closing_ask`], its background tasks
/// finished vs [`awaits_background`]), `StopFailure` (the
/// turn ended on an API error INSTEAD of a `Stop` — without it such an
/// agent sat with no marker at all, its bell already ignored as hooked),
/// `PermissionRequest` (the permission dialog is about to show), the
/// `Notification`s that mean "waiting on you", and `UserPromptSubmit`
/// (the user answered, wherever from).
const HOOK_EVENTS: &[&str] = &[
    "Stop",
    "StopFailure",
    "PermissionRequest",
    "Notification",
    "UserPromptSubmit",
];

/// Forwards the hook's input, cut to the fields TuxFlow reads.
const SCRIPT: &str = r#"#!/usr/bin/env python3
# TuxFlow agent hook (auto-installed; safe to delete)
import json, os, socket, sys

TAIL = 2000  # enough for the closing paragraphs; the line stays small

path = os.environ.get("TUXFLOW_MCP_SOCKET")
process = os.environ.get("TUXFLOW_PROCESS")
if path and process:
    try:
        hook = json.load(sys.stdin)
    except ValueError:
        sys.exit(0)
    keep = {k: hook[k] for k in ("hook_event_name", "notification_type", "message", "tool_name", "error") if k in hook}
    last = hook.get("last_assistant_message")
    if isinstance(last, str):
        keep["last_assistant_message"] = last[-TAIL:]
    tasks = hook.get("background_tasks")
    if isinstance(tasks, list):
        keep["background_tasks"] = [{k: t[k] for k in ("type", "status") if k in t} for t in tasks if isinstance(t, dict)]
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
    /// The turn ended on an API error ("Connection lost mid-response"):
    /// the agent stopped short and waits for a prompt to go on. `reason`
    /// is [`failure_reason`]'s cut of Claude's error line; `retryable`
    /// says whether prompting it again could get anywhere
    /// ([`failure_retryable`]).
    Failed { reason: String, retryable: bool },
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
        "Stop" => match field("last_assistant_message").and_then(closing_ask) {
            Some(ask) => Some(AgentEvent::NeedsInput { reason: Some(ask) }),
            None if awaits_background(hook) => None,
            None => Some(AgentEvent::Finished),
        },
        "StopFailure" => Some(AgentEvent::Failed {
            reason: failure_reason(field("last_assistant_message"), field("error")),
            retryable: failure_retryable(field("error")),
        }),
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

/// Background task types that END on their own and wake the agent when
/// they do. A `shell` is left out on purpose: a dev server or a `tail -f`
/// started in the background never ends, and counting it would hold back
/// "finished" for the rest of the session.
const WAKING_TASKS: &[&str] = &["subagent", "workflow", "teammate"];

/// Statuses of a task that will not wake anyone any more, should Claude
/// ever list one — an unknown status counts as still running.
const ENDED_STATUSES: &[&str] = &[
    "completed",
    "failed",
    "killed",
    "cancelled",
    "canceled",
    "stopped",
];

/// A `Stop` that is a PAUSE, not the end: the agent sent subagents to the
/// background and its turn "ended" only until one reports back (Claude
/// Code's `background_tasks`, one entry per in-flight task). Each report
/// starts a short turn with a `Stop` of its own, so N subagents made N+1
/// "finished" banners, all but the last one false. An ask still gets
/// through — [`parse_event_line`] checks for it first.
fn awaits_background(hook: &serde_json::Value) -> bool {
    let Some(tasks) = hook.get("background_tasks").and_then(|t| t.as_array()) else {
        return false;
    };
    let field = |task: &serde_json::Value, k: &str| {
        task.get(k).and_then(|v| v.as_str()).map(str::to_lowercase)
    };
    tasks.iter().any(|task| {
        field(task, "type").is_some_and(|t| WAKING_TASKS.contains(&t.as_str()))
            && field(task, "status").is_none_or(|s| !ENDED_STATUSES.contains(&s.as_str()))
    })
}

/// The longest ask (or failure) quoted in a notification.
const ASK_MAX_CHARS: usize = 120;

/// What stopped a failed turn, from the error message Claude printed in
/// the transcript (`API Error: Connection lost mid-response. The response
/// above may be incomplete.`, verified against 2.1.281): its first
/// sentence, prefix and full stop dropped — "Connection lost
/// mid-response". The advice after it ("may be incomplete", "Try again")
/// is the same for every failure and says nothing about which one this
/// was. With no message, the hook's error code ("server_error" → "server
/// error").
pub fn failure_reason(message: Option<&str>, error: Option<&str>) -> String {
    let line = message
        .and_then(|m| m.lines().map(str::trim).find(|l| !l.is_empty()))
        .map(|l| l.strip_prefix("API Error:").unwrap_or(l).trim());
    let first = line
        .and_then(|l| sentences(l).first().copied())
        .map(|s| s.trim_end_matches('.'))
        .filter(|s| !s.is_empty());
    match first {
        Some(sentence) => clip(sentence),
        None => error.map_or_else(|| String::from("API error"), |e| e.replace('_', " ")),
    }
}

/// `StopFailure` error codes (Claude Code 2.1.322) that only the user can
/// clear — a login, a plan, a request the API refuses as it stands.
/// Sending the same conversation again fails the same way.
const NEEDS_THE_USER: &[&str] = &[
    "authentication_failed",
    "oauth_org_not_allowed",
    "account_on_hold",
    "verification_required",
    "billing_error",
    "invalid_request",
    "model_not_found",
    "cloud_credential_error",
];

/// Could a "continue" get a failed turn going again? Yes for whatever the
/// network or the API's load did (a dropped stream, a server error, a rate
/// limit) and for a code this list has never seen; no for the ones in
/// [`NEEDS_THE_USER`].
pub fn failure_retryable(error: Option<&str>) -> bool {
    error.is_none_or(|code| !NEEDS_THE_USER.contains(&code))
}

/// Openings of a closing paragraph that hand the next step to the user.
/// Kept to requests that wait on a reply or an action — a parting "let me
/// know if…" or "try it now" is a finished turn, not a question.
const ASK_OPENINGS: &[&str] = &[
    "paste ",
    "send me",
    "share ",
    "tell me",
    "reply ",
    "confirm ",
    "choose ",
    "pick ",
    "please ",
    "run this",
    "run these",
    "run the ",
    "can you ",
    "could you ",
    "would you ",
    "should i ",
    "do you want",
    "want me to",
    "which ",
];

/// A turn that ENDED by asking the user something: to Claude that is a
/// plain `Stop` — only a blocking dialog fires `PermissionRequest` — but
/// to the user it is the same "needs you" as a permission prompt. Read off
/// the last message's closing paragraph (code blocks dropped, so "Run this
/// on the server:" above a fenced command still counts; a closing list
/// reads with the line that introduced it), and `Some(the ask)` when it is
/// a question, a request from [`ASK_OPENINGS`], or the message carries a
/// `! command` line — Claude's form for a command the user must run in
/// the prompt. A guess from wording, deliberately narrow: a missed ask
/// still gets the finished dot, a false one would cry wolf on every turn.
pub fn closing_ask(message: &str) -> Option<String> {
    let mut paragraphs: Vec<Vec<&str>> = vec![Vec::new()];
    let mut in_fence = false;
    let mut bang_command = false;
    for line in message.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            paragraphs.push(Vec::new());
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.starts_with("! ") {
            bang_command = true;
        }
        match trimmed {
            "" => paragraphs.push(Vec::new()),
            _ => paragraphs.last_mut().expect("never empty").push(trimmed),
        }
    }
    paragraphs.retain(|p| !p.is_empty());
    let is_list = |p: &[&str]| p.iter().all(|l| is_list_item(l));
    let last = paragraphs.pop()?;
    let mut closing = Vec::new();
    if is_list(&last) {
        closing.extend(paragraphs.pop().filter(|p| !is_list(p)));
    }
    closing.push(last);
    let asks = closing.iter().any(|p| {
        let first = p[0].trim_start_matches(['*', '_', '>', ' ']);
        sentences(&plain(p)).iter().any(|s| s.ends_with('?')) || opens_ask(first)
    }) || bang_command;
    asks.then(|| ask_excerpt(&closing))
}

fn is_list_item(line: &str) -> bool {
    line.starts_with("- ")
        || line.starts_with("* ")
        || line
            .split_once(". ")
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

fn opens_ask(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    ASK_OPENINGS.iter().any(|o| lower.starts_with(o))
}

/// A paragraph's lines as one line of text, emphasis and code marks gone.
fn plain(paragraph: &[&str]) -> String {
    paragraph.join(" ").replace(['*', '_', '`'], "")
}

/// `text` cut after each `.`, `?` or `!` that ends a sentence.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        if matches!(c, '.' | '?' | '!') && (end == text.len() || text[end..].starts_with(' ')) {
            out.push(text[start..end].trim());
            start = end;
        }
    }
    out.push(text[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

/// The sentence that asks — the closing paragraph's last question, else
/// its sentence that opens with a request, else its last sentence (the
/// introducing line stands for a closing list of options) — cut to
/// [`ASK_MAX_CHARS`].
fn ask_excerpt(closing: &[Vec<&str>]) -> String {
    let text = plain(&closing[0]);
    let sentences = sentences(&text);
    let sentence = sentences
        .iter()
        .rev()
        .find(|s| s.ends_with('?'))
        .or_else(|| sentences.iter().find(|s| opens_ask(s)))
        .or(sentences.last())
        .copied()
        .unwrap_or_default();
    clip(sentence)
}

/// `sentence` cut to [`ASK_MAX_CHARS`].
fn clip(sentence: &str) -> String {
    match sentence.char_indices().nth(ASK_MAX_CHARS) {
        Some((cut, _)) => format!("{}…", sentence[..cut].trim_end()),
        None => sentence.to_string(),
    }
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

    /// The turn that ends on an API error: `StopFailure`, never `Stop`,
    /// with the transcript's error line as its last message.
    #[test]
    fn a_turn_ending_on_an_api_error_failed() {
        let fail = |hook| match parse_event_line(&line(hook)) {
            Some((_, Some(AgentEvent::Failed { reason, .. }))) => reason,
            other => panic!("not a failure: {other:?}"),
        };
        assert_eq!(
            fail(serde_json::json!({
                "hook_event_name": "StopFailure",
                "error": "server_error",
                "last_assistant_message": "API Error: Connection lost mid-response. The response above may be incomplete.",
            })),
            "Connection lost mid-response"
        );
        assert_eq!(
            fail(serde_json::json!({
                "hook_event_name": "StopFailure",
                "last_assistant_message": "API Error: Your computer went to sleep before a response was produced. Try again.",
            })),
            "Your computer went to sleep before a response was produced"
        );
        // A status and the server's body: one long "sentence", clipped.
        let body = format!(
            "API Error: 500 {{\"type\":\"error\",\"message\":\"{}\"}}",
            "x".repeat(200)
        );
        let reason = fail(
            serde_json::json!({"hook_event_name": "StopFailure", "last_assistant_message": body}),
        );
        assert!(
            reason.starts_with("500 {") && reason.ends_with('…'),
            "{reason}"
        );
        // No message: the hook's own code.
        assert_eq!(
            fail(serde_json::json!({"hook_event_name": "StopFailure", "error": "rate_limit"})),
            "rate limit"
        );
        assert_eq!(
            fail(serde_json::json!({"hook_event_name": "StopFailure"})),
            "API error"
        );
    }

    /// A dropped stream is worth another prompt; a login or a plan is not.
    #[test]
    fn only_what_a_retry_can_fix_is_retryable() {
        assert!(failure_retryable(Some("server_error")));
        assert!(failure_retryable(Some("rate_limit")));
        assert!(failure_retryable(Some("a_code_from_the_future")));
        assert!(failure_retryable(None));
        assert!(!failure_retryable(Some("authentication_failed")));
        assert!(!failure_retryable(Some("billing_error")));
        assert!(!failure_retryable(Some("invalid_request")));
    }

    /// A turn that ends by asking is "needs you", quoting the ask; a plain
    /// summary stays "finished".
    #[test]
    fn a_turn_ending_in_an_ask_needs_the_user() {
        let stop = |msg: &str| {
            parse_event_line(&line(serde_json::json!({
                "hook_event_name": "Stop",
                "last_assistant_message": msg,
            })))
            .and_then(|(_, e)| e)
        };
        let needs = |reason: &str| {
            Some(AgentEvent::NeedsInput {
                reason: Some(reason.into()),
            })
        };
        // The report that started this: a `!` command, then a request.
        assert_eq!(
            stop(
                "That command was my mistake. Here's the real one:\n\n! ssh host 'sqlite3 -readonly db'\nselect 1;\n\nThe first result answers it.\n\nPaste the output here and I'll read it."
            ),
            needs("Paste the output here and I'll read it.")
        );
        assert_eq!(
            stop("Both are ready. Want me to commit this? It builds clean."),
            needs("Want me to commit this?")
        );
        // A question introducing a closing list of options.
        assert_eq!(
            stop(
                "Two ways to go.\n\nWhich do you prefer?\n- **Exact.** Halo only for blocks.\n- **Guess.** Read the wording."
            ),
            needs("Which do you prefer?")
        );
        // A request above a fenced command.
        assert_eq!(
            stop("Run this on the server:\n\n```bash\nsystemctl status caddy\n```"),
            needs("Run this on the server:")
        );
        assert_eq!(
            stop("Fixed the badge.\n\n**Could you check it on the VPS?**"),
            needs("Could you check it on the VPS?")
        );
        // Summaries — including a question earlier on, a parting offer and
        // a code block with a `?` inside — are finished turns.
        assert_eq!(
            stop("Why did it fail? The socket was stale.\n\nRemoved it; the forward is back up."),
            Some(AgentEvent::Finished)
        );
        assert_eq!(
            stop("Done — tests pass. Let me know if you want the light theme too."),
            Some(AgentEvent::Finished)
        );
        assert_eq!(
            stop("Here's the change:\n\n```rust\nlet x = y?;\n```"),
            Some(AgentEvent::Finished)
        );
        assert_eq!(stop(""), Some(AgentEvent::Finished));
        // An older plugin's line, without the message.
        assert_eq!(
            parse_event_line(&line(serde_json::json!({"hook_event_name": "Stop"})))
                .and_then(|(_, e)| e),
            Some(AgentEvent::Finished)
        );
    }

    /// A turn that "ends" with subagents still out is a pause: no event
    /// until the last one is back. A background shell never holds it.
    #[test]
    fn a_stop_with_subagents_still_running_is_not_finished() {
        let stop = |msg: &str, tasks: serde_json::Value| {
            parse_event_line(&line(serde_json::json!({
                "hook_event_name": "Stop",
                "last_assistant_message": msg,
                "background_tasks": tasks,
            })))
            .map(|(_, e)| e)
        };
        let task = |ty: &str, status: &str| serde_json::json!({"type": ty, "status": status});
        let done = "Launched three reviewers; I'll report when they are back.";
        // Hooks are live (`Some`), nothing to say (`None`).
        for ty in WAKING_TASKS {
            assert_eq!(
                stop(done, serde_json::json!([task(ty, "running")])),
                Some(None),
                "{ty}"
            );
        }
        assert_eq!(
            stop(done, serde_json::json!([{"type": "subagent"}])),
            Some(None)
        );
        // The last report's turn: nothing in flight, or only what never ends.
        let finished = Some(Some(AgentEvent::Finished));
        assert_eq!(stop(done, serde_json::json!([])), finished);
        assert_eq!(
            stop(done, serde_json::json!([task("shell", "running")])),
            finished
        );
        assert_eq!(
            stop(done, serde_json::json!([task("subagent", "completed")])),
            finished
        );
        // A question is waiting on the user whatever else is running.
        assert_eq!(
            stop(
                "Two are still out. Want me to commit the first fix?",
                serde_json::json!([task("subagent", "running")])
            ),
            Some(Some(AgentEvent::NeedsInput {
                reason: Some("Want me to commit the first fix?".into())
            }))
        );
    }

    /// The script cuts each task to the two fields the rule reads.
    #[test]
    fn the_hook_script_forwards_background_tasks() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!("tuxflow-hook-fwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (script, sock) = (dir.join("hook.py"), dir.join("s"));
        std::fs::write(&script, SCRIPT).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let mut child = std::process::Command::new("python3")
            .arg(&script)
            .env("TUXFLOW_MCP_SOCKET", &sock)
            .env(PROCESS_ENV, "claude")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let input = serde_json::json!({
            "hook_event_name": "Stop",
            "last_assistant_message": "Launched.",
            "background_tasks": [
                {"id": "t1", "type": "subagent", "status": "running", "description": "review"}
            ],
        });
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            input.to_string().as_bytes(),
        )
        .unwrap();
        let (mut conn, _) = listener.accept().unwrap();
        let mut sent = String::new();
        std::io::Read::read_to_string(&mut conn, &mut sent).unwrap();
        child.wait().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!sent.contains("review"), "{sent}");
        assert_eq!(
            parse_event_line(sent.trim()),
            Some((String::from("claude"), None))
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
