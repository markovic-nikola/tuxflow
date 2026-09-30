use std::sync::Arc;

use rmcp::ServiceExt;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

use crate::mcp::bridge::{self, McpBridge};
use crate::mcp::tools::TuxFlowMcpServer;

/// Environment variable naming the socket an agent should talk to. Set on
/// every process TuxFlow spawns while the server is on, so `tuxflow-mcp`
/// inside an agent's terminal resolves ITS project without guessing from
/// the working directory (two open projects with nested paths, or a
/// process whose `working_dir` is outside the project root, both defeat
/// the cwd match). On a remote host the value is the host-side socket the
/// forward binds — see [`super::remote`].
pub const SOCKET_ENV: &str = "TUXFLOW_MCP_SOCKET";

pub fn socket_path(project_name: &str) -> String {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/tuxflow-{}.sock", base, sanitize_name(project_name))
}

/// The sidecar next to a socket naming the project directory it serves —
/// what `tuxflow-mcp`'s cwd-based auto-discovery reads.
pub fn sidecar_path(socket_path: &str) -> String {
    format!("{socket_path}.dir")
}

/// A running server. `stop()` closes the listener and removes the socket;
/// dropping the handle does nothing (the GTK app never stops its servers
/// and holds no handle).
#[derive(Clone)]
pub struct McpServerHandle {
    socket_path: String,
    stop: Arc<Notify>,
}

impl McpServerHandle {
    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    pub fn stop(&self) {
        self.stop.notify_one();
        remove_socket_files(&self.socket_path);
        log::info!("MCP server stopped, removed {}", self.socket_path);
    }
}

fn remove_socket_files(socket_path: &str) {
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_file(sidecar_path(socket_path));
}

pub fn start_mcp_server(
    project_name: &str,
    project_dir: &str,
    bridge: McpBridge,
) -> McpServerHandle {
    let socket_path = socket_path(project_name);

    // Remove existing socket
    remove_socket_files(&socket_path);

    // Write sidecar file with project directory for auto-discovery
    let _ = std::fs::write(sidecar_path(&socket_path), project_dir);

    let stop = Arc::new(Notify::new());
    let handle = McpServerHandle {
        socket_path: socket_path.clone(),
        stop: stop.clone(),
    };
    let path = socket_path;

    std::thread::spawn(move || {
        // Single-threaded runtime — the MCP server is one Unix-socket
        // accept loop with rare clients. The default multi-thread runtime
        // creates ~num_cpus worker threads + a 512-slot blocking pool, which
        // multiplied across all loaded projects accounted for ~300 idle
        // threads and most of tuxflow's resident memory.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");
        rt.block_on(async move {
            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    log::error!("Failed to bind MCP socket at {path}: {e}");
                    return;
                }
            };

            log::info!("MCP server listening on {path}");

            loop {
                // Check if MCP is still enabled (the GTK app's global switch;
                // the iced shell stops servers through the handle instead).
                if !bridge::is_mcp_enabled() {
                    log::info!("MCP server disabled, removing socket {path}");
                    remove_socket_files(&path);
                    // Wait until re-enabled
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        if bridge::is_mcp_enabled() {
                            break;
                        }
                    }
                    // Re-bind the socket
                    log::info!("MCP server re-enabled, but requires app restart to rebind socket");
                    return;
                }

                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    _ = stop.notified() => return,
                };
                match accepted {
                    Ok((stream, _addr)) => {
                        let server = TuxFlowMcpServer::new(bridge.clone());
                        tokio::spawn(async move {
                            let Some(transport) = answer_preinit_probes(stream).await else {
                                return;
                            };
                            match server.serve(transport).await {
                                Ok(service) => {
                                    log::info!("MCP client connected");
                                    let _ = service.waiting().await;
                                    log::info!("MCP client disconnected");
                                }
                                Err(e) => {
                                    log::error!("MCP serve error: {e}");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        log::error!("MCP accept error: {e}");
                    }
                }
            }
        });
    });
    handle
}

/// Reads up to the client's `initialize` (or a ping, which rmcp answers
/// itself) and hands back a transport that replays that line. rmcp treats
/// any other request ahead of `initialize` as fatal and closes WITHOUT a
/// reply, while newer clients open with a capability probe
/// (`server/discover`) and wait for "method not found" before falling back
/// to the classic handshake — so every such request is answered here and
/// the connection kept. `None` when the client left first.
async fn answer_preinit_probes(
    stream: UnixStream,
) -> Option<(
    impl AsyncRead + Send + Unpin + 'static,
    tokio::net::unix::OwnedWriteHalf,
)> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let Some(reply) = preinit_refusal(&line) else {
            break;
        };
        writer.write_all(reply.as_bytes()).await.ok()?;
        writer.flush().await.ok()?;
    }
    let replay = std::io::Cursor::new(std::mem::take(&mut line).into_bytes());
    Some((replay.chain(reader), writer))
}

/// The "method not found" line for a request that may not precede
/// `initialize`; `None` for everything rmcp should see (initialize, ping,
/// notifications, anything unparseable — its own error is the better one).
fn preinit_refusal(line: &str) -> Option<String> {
    let msg: serde_json::Value = serde_json::from_str(line).ok()?;
    let method = msg.get("method")?.as_str()?;
    let id = msg.get("id")?;
    if matches!(method, "initialize" | "ping") {
        return None;
    }
    log::debug!("MCP client probed {method} before initialize; answered method-not-found");
    let reply = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": -32601, "message": format!("Method not found: {method}")},
    });
    Some(format!("{reply}\n"))
}

pub fn stop_mcp_server(project_name: &str) {
    let path = socket_path(project_name);
    remove_socket_files(&path);
    log::info!("Removed MCP socket {path}");
}

pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}
