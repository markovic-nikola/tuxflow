use std::sync::{Arc, Mutex};

use rmcp::ServiceExt;
use tokio::net::UnixListener;

use crate::mcp::tools::{ProcessSnapshot, ProcessStateRef, TuxFlowMcpServer};
use crate::process::manager::ProcessManagerRef;

pub fn create_process_state(manager: &ProcessManagerRef) -> ProcessStateRef {
    let mgr = manager.borrow();
    let snapshots: Vec<ProcessSnapshot> = mgr
        .process_names()
        .iter()
        .filter_map(|name| {
            mgr.get_process(name).map(|proc| ProcessSnapshot {
                name: proc.config.name.clone(),
                status: format!("{:?}", proc.status),
                command: proc.config.command.clone(),
                category: format!("{:?}", proc.config.category),
            })
        })
        .collect();

    Arc::new(Mutex::new(snapshots))
}

pub fn start_mcp_server(project_name: &str, process_state: ProcessStateRef) {
    let socket_path = format!("/tmp/tuxflow-{}.sock", sanitize_name(project_name));

    // Remove existing socket
    let _ = std::fs::remove_file(&socket_path);

    let state = process_state.clone();
    let path = socket_path.clone();

    // Spawn the MCP server on a tokio runtime in a background thread
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
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let server = TuxFlowMcpServer::new(state.clone());
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

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect()
}
