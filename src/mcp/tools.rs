use rmcp::{
    ServerHandler,
    ErrorData as McpError,
    handler::server::{tool::ToolRouter, wrapper::{Json, Parameters}},
    model::{
        ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
        RawResource, RawResourceTemplate, ReadResourceRequestParams, ReadResourceResult,
        Resource, ResourceContents, ResourceTemplate, ResourcesCapability,
        ServerInfo,
    },
    service::RequestContext,
    RoleServer,
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::mcp::bridge::{CommandResult, McpBridge, McpCommand};

// --- Output types ---

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProcessInfo {
    pub name: String,
    pub status: String,
    pub command: String,
    pub category: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProcessListOutput {
    pub processes: Vec<ProcessInfo>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProjectInfoOutput {
    pub total: usize,
    pub running: usize,
    pub processes: Vec<ProcessInfo>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProcessStatusOutput {
    pub name: String,
    pub status: String,
    pub command: String,
    pub category: String,
    pub pid: Option<i32>,
    pub restart_count: u32,
    pub uptime_secs: Option<u64>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct LogsOutput {
    pub process_name: String,
    pub lines: Vec<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ActionResult {
    pub success: bool,
    pub message: String,
}

// --- Input types ---

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ProcessNameParam {
    /// Name of the process
    pub process_name: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct GetLogsParams {
    /// Name of the process
    pub process_name: String,
    /// Number of recent lines to return (default: 100)
    pub lines: Option<usize>,
}

// --- Server ---

#[derive(Clone)]
pub struct TuxFlowMcpServer {
    tool_router: ToolRouter<Self>,
    bridge: McpBridge,
}

#[tool_router]
impl TuxFlowMcpServer {
    pub fn new(bridge: McpBridge) -> Self {
        Self {
            tool_router: Self::tool_router(),
            bridge,
        }
    }

    #[tool(description = "List all managed processes with their current status")]
    fn list_processes(&self) -> Json<ProcessListOutput> {
        let state = self.bridge.process_state.lock().unwrap();
        let processes = state
            .values()
            .map(|p| ProcessInfo {
                name: p.name.clone(),
                status: p.status.clone(),
                command: p.command.clone(),
                category: p.category.clone(),
            })
            .collect();
        Json(ProcessListOutput { processes })
    }

    #[tool(description = "Get project information including all configured processes")]
    fn get_project_info(&self) -> Json<ProjectInfoOutput> {
        let state = self.bridge.process_state.lock().unwrap();
        let total = state.len();
        let running = state.values().filter(|p| p.status == "Running").count();
        let processes = state
            .values()
            .map(|p| ProcessInfo {
                name: p.name.clone(),
                status: p.status.clone(),
                command: p.command.clone(),
                category: p.category.clone(),
            })
            .collect();
        Json(ProjectInfoOutput {
            total,
            running,
            processes,
        })
    }

    #[tool(description = "Get detailed status of a specific process including PID, uptime, and restart count")]
    fn get_process_status(
        &self,
        Parameters(params): Parameters<ProcessNameParam>,
    ) -> Result<Json<ProcessStatusOutput>, String> {
        let state = self.bridge.process_state.lock().unwrap();
        state
            .get(&params.process_name)
            .map(|s| {
                Json(ProcessStatusOutput {
                    name: s.name.clone(),
                    status: s.status.clone(),
                    command: s.command.clone(),
                    category: s.category.clone(),
                    pid: s.pid,
                    restart_count: s.restart_count,
                    uptime_secs: s.uptime_secs,
                })
            })
            .ok_or_else(|| format!("Process '{}' not found", params.process_name))
    }

    #[tool(description = "Get recent terminal output from a process")]
    async fn get_process_logs(
        &self,
        Parameters(params): Parameters<GetLogsParams>,
    ) -> Result<Json<LogsOutput>, String> {
        let n = params.lines.unwrap_or(100);

        // Try the ring buffer first
        {
            let buffers = self.bridge.log_buffers.lock().unwrap();
            let log_lines = buffers
                .get(&params.process_name)
                .map(|b| b.recent(n))
                .unwrap_or_default();

            if !log_lines.is_empty() {
                return Ok(Json(LogsOutput {
                    process_name: params.process_name,
                    lines: log_lines,
                }));
            }
        }

        // Fallback: read directly from VTE terminal on the GTK thread
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.bridge
            .command_tx
            .send(McpCommand::ReadLogs {
                name: params.process_name.clone(),
                lines: n,
                reply: tx,
            })
            .map_err(|_| "TuxFlow is not running".to_string())?;

        match rx.await {
            Ok(CommandResult::Ok(text)) => Ok(Json(LogsOutput {
                process_name: params.process_name,
                lines: text.lines().map(String::from).collect(),
            })),
            Ok(CommandResult::Error(e)) => Err(e),
            Err(_) => Err("Command channel closed".to_string()),
        }
    }

    #[tool(description = "Restart a managed process")]
    async fn restart_process(
        &self,
        Parameters(params): Parameters<ProcessNameParam>,
    ) -> Result<Json<ActionResult>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.bridge
            .command_tx
            .send(McpCommand::RestartProcess {
                name: params.process_name,
                reply: tx,
            })
            .map_err(|_| "TuxFlow is not running".to_string())?;

        match rx.await {
            Ok(CommandResult::Ok(msg)) => Ok(Json(ActionResult {
                success: true,
                message: msg,
            })),
            Ok(CommandResult::Error(e)) => Ok(Json(ActionResult {
                success: false,
                message: e,
            })),
            Err(_) => Err("Command channel closed".to_string()),
        }
    }

    #[tool(description = "Stop a running process")]
    async fn stop_process(
        &self,
        Parameters(params): Parameters<ProcessNameParam>,
    ) -> Result<Json<ActionResult>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.bridge
            .command_tx
            .send(McpCommand::StopProcess {
                name: params.process_name,
                reply: tx,
            })
            .map_err(|_| "TuxFlow is not running".to_string())?;

        match rx.await {
            Ok(CommandResult::Ok(msg)) => Ok(Json(ActionResult {
                success: true,
                message: msg,
            })),
            Ok(CommandResult::Error(e)) => Ok(Json(ActionResult {
                success: false,
                message: e,
            })),
            Err(_) => Err("Command channel closed".to_string()),
        }
    }

    #[tool(description = "Start a stopped process")]
    async fn start_process(
        &self,
        Parameters(params): Parameters<ProcessNameParam>,
    ) -> Result<Json<ActionResult>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.bridge
            .command_tx
            .send(McpCommand::StartProcess {
                name: params.process_name,
                reply: tx,
            })
            .map_err(|_| "TuxFlow is not running".to_string())?;

        match rx.await {
            Ok(CommandResult::Ok(msg)) => Ok(Json(ActionResult {
                success: true,
                message: msg,
            })),
            Ok(CommandResult::Error(e)) => Ok(Json(ActionResult {
                success: false,
                message: e,
            })),
            Err(_) => Err("Command channel closed".to_string()),
        }
    }
}

#[tool_handler]
impl ServerHandler for TuxFlowMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities.resources = Some(ResourcesCapability::default());
        info.instructions = Some(
            "TuxFlow dev environment manager. Use list_processes to see all processes, \
             get_process_logs to read terminal output, and restart/stop/start to control processes. \
             Resources: tuxflow://processes, tuxflow://logs/{name}, tuxflow://config."
                .into(),
        );
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut resources = vec![
            Resource {
                raw: RawResource::new("tuxflow://processes", "processes")
                    .with_description("JSON list of all managed processes with status")
                    .with_mime_type("application/json"),
                annotations: None,
            },
            Resource {
                raw: RawResource::new("tuxflow://config", "config")
                    .with_description("Current project configuration")
                    .with_mime_type("application/json"),
                annotations: None,
            },
        ];

        // Add a resource for each known process's logs
        let state = self.bridge.process_state.lock().unwrap();
        for name in state.keys() {
            resources.push(Resource {
                raw: RawResource::new(
                    format!("tuxflow://logs/{name}"),
                    format!("logs/{name}"),
                )
                .with_description(format!("Recent terminal output from '{name}'"))
                .with_mime_type("text/plain"),
                annotations: None,
            });
        }

        Ok(ListResourcesResult {
            resources,
            meta: None,
            next_cursor: None,
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![ResourceTemplate {
                raw: RawResourceTemplate::new("tuxflow://logs/{name}", "process_logs")
                    .with_description("Recent terminal output from a named process")
                    .with_mime_type("text/plain"),
                annotations: None,
            }],
            meta: None,
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = &request.uri;

        if uri == "tuxflow://processes" {
            let state = self.bridge.process_state.lock().unwrap();
            let processes: Vec<ProcessInfo> = state
                .values()
                .map(|p| ProcessInfo {
                    name: p.name.clone(),
                    status: p.status.clone(),
                    command: p.command.clone(),
                    category: p.category.clone(),
                })
                .collect();
            let json = serde_json::to_string_pretty(&processes).unwrap_or_default();
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(json, uri.clone())
                    .with_mime_type("application/json"),
            ]));
        }

        if uri == "tuxflow://config" {
            let state = self.bridge.process_state.lock().unwrap();
            let config: Vec<serde_json::Value> = state
                .values()
                .map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "command": p.command,
                        "category": p.category,
                        "status": p.status,
                        "pid": p.pid,
                        "restart_count": p.restart_count,
                        "uptime_secs": p.uptime_secs,
                    })
                })
                .collect();
            let json = serde_json::to_string_pretty(&config).unwrap_or_default();
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(json, uri.clone())
                    .with_mime_type("application/json"),
            ]));
        }

        if let Some(name) = uri.strip_prefix("tuxflow://logs/") {
            // Try ring buffer first
            {
                let buffers = self.bridge.log_buffers.lock().unwrap();
                if let Some(buffer) = buffers.get(name) {
                    let lines = buffer.recent(200);
                    if !lines.is_empty() {
                        let text = lines.join("\n");
                        return Ok(ReadResourceResult::new(vec![
                            ResourceContents::text(text, uri.clone())
                                .with_mime_type("text/plain"),
                        ]));
                    }
                }
            }

            // Fallback: read from VTE via bridge
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.bridge
                .command_tx
                .send(McpCommand::ReadLogs {
                    name: name.to_string(),
                    lines: 200,
                    reply: tx,
                })
                .map_err(|_| McpError::internal_error("TuxFlow is not running", None))?;

            match rx.await {
                Ok(CommandResult::Ok(text)) => {
                    return Ok(ReadResourceResult::new(vec![
                        ResourceContents::text(text, uri.clone())
                            .with_mime_type("text/plain"),
                    ]));
                }
                Ok(CommandResult::Error(e)) => {
                    return Err(McpError::internal_error(e, None));
                }
                Err(_) => {
                    return Err(McpError::internal_error("Command channel closed", None));
                }
            }
        }

        Err(McpError::resource_not_found(
            format!("Unknown resource URI: {uri}"),
            None,
        ))
    }
}
