use rmcp::ServiceExt;
use tokio::net::UnixListener;

use crate::mcp::bridge::{self, McpBridge};
use crate::mcp::tools::TuxFlowMcpServer;

pub fn socket_path(project_name: &str) -> String {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/tuxflow-{}.sock", base, sanitize_name(project_name))
}

pub fn start_mcp_server(project_name: &str, project_dir: &str, bridge: McpBridge) {
    let socket_path = socket_path(project_name);

    // Remove existing socket
    let _ = std::fs::remove_file(&socket_path);

    // Write sidecar file with project directory for auto-discovery
    let dir_file = format!("{}.dir", socket_path);
    let _ = std::fs::write(&dir_file, project_dir);

    let path = socket_path.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
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
                // Check if MCP is still enabled
                if !bridge::is_mcp_enabled() {
                    log::info!("MCP server disabled, removing socket {path}");
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_file(format!("{}.dir", path));
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

                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let server = TuxFlowMcpServer::new(bridge.clone());
                        tokio::spawn(async move {
                            match server.serve(stream).await {
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
}

pub fn stop_mcp_server(project_name: &str) {
    let path = socket_path(project_name);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}.dir", path));
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
