//! The MCP server over its real socket: a client speaking newline-delimited
//! JSON-RPC (what `tuxflow-mcp` relays) sees the snapshot table, and a tool
//! call that needs the UI comes out of the bridge's command channel and
//! its reply goes back to the caller. The same traffic is then driven
//! through the Python shim the remote half installs on hosts, both by the
//! env var and by cwd discovery.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tuxflow_core::mcp::bridge::{CommandResult, McpBridge, McpCommand, ProcessSnapshot};
use tuxflow_core::mcp::server::{SOCKET_ENV, start_mcp_server};
use tuxflow_core::mcp::{remote, server};

fn snapshot(name: &str, status: &str, url: Option<&str>) -> ProcessSnapshot {
    ProcessSnapshot {
        name: name.into(),
        status: status.into(),
        command: format!("npm run {name}"),
        category: "Command".into(),
        working_dir: Some("/srv/app".into()),
        url: url.map(String::from),
        pid: None,
        restart_count: 0,
        started_unix: None,
    }
}

/// A server on a unique socket, its snapshot table filled, plus the
/// receiving end of its command channel.
fn server(
    tag: &str,
) -> (
    server::McpServerHandle,
    tokio::sync::mpsc::UnboundedReceiver<McpCommand>,
) {
    let (bridge, rx) = McpBridge::isolated();
    bridge.replace_snapshots(vec![
        snapshot("dev", "Running", Some("http://localhost:5173")),
        snapshot("test", "Stopped", None),
    ]);
    let name = format!("mcp-test-{tag}-{}", std::process::id());
    let handle = start_mcp_server(&name, "/srv/app", bridge);
    // The listener binds on its own thread.
    for _ in 0..100 {
        if UnixStream::connect(handle.socket_path()).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    (handle, rx)
}

/// One JSON-RPC exchange over a line-oriented stream.
fn call<W: Write, R: BufRead>(w: &mut W, r: &mut R, id: u64, method: &str, params: Value) -> Value {
    let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    writeln!(w, "{req}").unwrap();
    w.flush().unwrap();
    let mut line = String::new();
    loop {
        line.clear();
        assert!(
            r.read_line(&mut line).unwrap() > 0,
            "server closed on {method}"
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        if v.get("id") == Some(&json!(id)) {
            return v;
        }
    }
}

fn handshake<W: Write, R: BufRead>(w: &mut W, r: &mut R) {
    let init = call(
        w,
        r,
        1,
        "initialize",
        json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    );
    let instructions = init["result"]["instructions"].as_str().unwrap_or_default();
    assert!(
        instructions.contains("Before you start a dev server"),
        "instructions steer the agent: {instructions}"
    );
    writeln!(
        w,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    w.flush().unwrap();
}

fn tool_result(v: &Value) -> Value {
    // rmcp puts the typed output under structuredContent and a JSON text
    // copy under content; either is fine for an agent, the typed one is
    // what we assert on.
    v["result"]["structuredContent"].clone()
}

#[test]
fn socket_client_sees_snapshots_and_tool_calls_reach_the_bridge() {
    let (handle, mut rx) = server("direct");
    let stream = UnixStream::connect(handle.socket_path()).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    handshake(&mut writer, &mut reader);

    let tools = call(&mut writer, &mut reader, 2, "tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for expected in [
        "list_processes",
        "get_process_logs",
        "start_process",
        "stop_process",
    ] {
        assert!(names.contains(&expected), "{names:?}");
    }

    let list = call(
        &mut writer,
        &mut reader,
        3,
        "tools/call",
        json!({"name": "list_processes", "arguments": {}}),
    );
    let procs = tool_result(&list)["processes"].clone();
    assert_eq!(procs[0]["name"], "dev");
    assert_eq!(procs[0]["status"], "Running");
    assert_eq!(procs[0]["url"], "http://localhost:5173");
    assert_eq!(procs[0]["working_dir"], "/srv/app");
    assert_eq!(procs[1]["name"], "test");
    assert_eq!(procs[1]["url"], Value::Null);

    // A lifecycle call parks on the bridge until the UI answers it.
    let ui = std::thread::spawn(move || match rx.blocking_recv() {
        Some(McpCommand::StartProcess { name, reply }) => {
            let _ = reply.send(CommandResult::Ok(format!("Process '{name}' started")));
            name
        }
        other => panic!("unexpected command: {}", other.is_some()),
    });
    let start = call(
        &mut writer,
        &mut reader,
        4,
        "tools/call",
        json!({"name": "start_process", "arguments": {"process_name": "test"}}),
    );
    assert_eq!(ui.join().unwrap(), "test");
    let result = tool_result(&start);
    assert_eq!(result["success"], true);
    assert_eq!(result["message"], "Process 'test' started");

    handle.stop();
    assert!(
        !std::path::Path::new(handle.socket_path()).exists(),
        "stop removes the socket"
    );
}

fn python3() -> Option<&'static str> {
    Command::new("python3")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| "python3")
}

fn run_shim(
    shim: &std::path::Path,
    env: &[(&str, &str)],
    cwd: &std::path::Path,
    arg: Option<&str>,
) -> Value {
    let mut cmd = Command::new(python3().unwrap());
    cmd.arg(shim)
        .args(arg)
        .current_dir(cwd)
        .env_remove(SOCKET_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let mut writer = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    handshake(&mut writer, &mut reader);
    let list = call(
        &mut writer,
        &mut reader,
        2,
        "tools/call",
        json!({"name": "list_processes", "arguments": {}}),
    );
    drop(writer);
    let status = child.wait().unwrap();
    assert!(status.success(), "shim exit: {status}");
    tool_result(&list)
}

#[test]
fn python_shim_relays_by_env_var_and_by_cwd_discovery() {
    if python3().is_none() {
        eprintln!("python3 not available, skipping shim test");
        return;
    }
    let (handle, _rx) = server("shim");
    let tmp = tempfile::tempdir().unwrap();
    let shim = tmp.path().join("tuxflow-mcp");
    std::fs::write(&shim, remote::shim_script()).unwrap();

    // 1. The env var — what a TuxFlow-spawned process carries.
    let result = run_shim(
        &shim,
        &[(SOCKET_ENV, handle.socket_path())],
        tmp.path(),
        None,
    );
    assert_eq!(result["processes"][0]["url"], "http://localhost:5173");

    // 2. cwd discovery through ~/.cache/tuxflow/mcp: a live socket whose
    //    .dir sidecar contains the cwd wins over a dead one listed first.
    let home = tmp.path().join("home");
    let sock_dir = home.join(".cache/tuxflow/mcp");
    std::fs::create_dir_all(&sock_dir).unwrap();
    let project_dir = tmp.path().join("proj");
    std::fs::create_dir_all(project_dir.join("src")).unwrap();
    // A stale socket from a project whose forward is gone.
    let dead = sock_dir.join("aaa-dead.sock");
    drop(std::os::unix::net::UnixListener::bind(&dead).unwrap());
    std::fs::write(
        sock_dir.join("aaa-dead.sock.dir"),
        project_dir.to_str().unwrap(),
    )
    .unwrap();
    std::os::unix::fs::symlink(handle.socket_path(), sock_dir.join("proj.sock")).unwrap();
    std::fs::write(
        sock_dir.join("proj.sock.dir"),
        project_dir.to_str().unwrap(),
    )
    .unwrap();
    let result = run_shim(
        &shim,
        &[("HOME", home.to_str().unwrap())],
        &project_dir.join("src"),
        None,
    );
    assert_eq!(result["processes"][0]["name"], "dev");

    // 3. A project name argument.
    let result = run_shim(
        &shim,
        &[("HOME", home.to_str().unwrap())],
        tmp.path(),
        Some("proj"),
    );
    assert_eq!(result["processes"][1]["name"], "test");

    handle.stop();
}
