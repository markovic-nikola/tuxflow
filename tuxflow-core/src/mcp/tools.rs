use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::wrapper::{Json, Parameters},
    model::{
        ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams, RawResource,
        RawResourceTemplate, ReadResourceRequestParams, ReadResourceResult, Resource,
        ResourceContents, ResourceTemplate, ResourcesCapability, ServerInfo,
    },
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::mcp::bridge::{CommandResult, McpBridge, McpCommand, ProcessSnapshot};

// --- Output types ---

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProcessInfo {
    pub name: String,
    /// Running, Stopped, Crashed, Restarting or Reconnecting.
    pub status: String,
    pub command: String,
    /// Command, Agent, Terminal or SSH.
    pub category: String,
    /// The URL this process serves, once its output announced one. If a
    /// dev server is Running with a url, use that url instead of starting
    /// another server.
    pub url: Option<String>,
    pub working_dir: Option<String>,
}

impl From<&ProcessSnapshot> for ProcessInfo {
    fn from(p: &ProcessSnapshot) -> Self {
        ProcessInfo {
            name: p.name.clone(),
            status: p.status.clone(),
            command: p.command.clone(),
            category: p.category.clone(),
            url: p.url.clone(),
            working_dir: p.working_dir.clone(),
        }
    }
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
    pub url: Option<String>,
    pub working_dir: Option<String>,
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

// `#[tool_router]` generates the `Self::tool_router()` constructor and
// `#[tool_handler]` routes through it directly, so the server holds no
// router of its own.
#[derive(Clone)]
pub struct TuxFlowMcpServer {
    bridge: McpBridge,
}

#[tool_router]
impl TuxFlowMcpServer {
    pub fn new(bridge: McpBridge) -> Self {
        Self { bridge }
    }

    #[tool(
        description = "List every process TuxFlow manages for this project, with status, command \
                       and the URL it serves. Call this BEFORE starting any dev server, watcher, \
                       test runner or other long-running command: if the user already runs it \
                       here, use that one (its url and get_process_logs) instead of starting \
                       your own, and do not stop or restart it unless asked to."
    )]
    fn list_processes(&self) -> Json<ProcessListOutput> {
        let state = self.bridge.process_state.lock().unwrap();
        let processes = state.values().map(ProcessInfo::from).collect();
        Json(ProcessListOutput { processes })
    }

    #[tool(description = "Get project information including all configured processes")]
    fn get_project_info(&self) -> Json<ProjectInfoOutput> {
        let state = self.bridge.process_state.lock().unwrap();
        let total = state.len();
        let running = state.values().filter(|p| p.status == "Running").count();
        let processes = state.values().map(ProcessInfo::from).collect();
        Json(ProjectInfoOutput {
            total,
            running,
            processes,
        })
    }

    #[tool(
        description = "Get detailed status of a specific process including PID, uptime, and restart count"
    )]
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
                    url: s.url.clone(),
                    working_dir: s.working_dir.clone(),
                    pid: s.pid,
                    restart_count: s.restart_count,
                    uptime_secs: s.uptime_secs(),
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

    #[tool(
        description = "Restart a managed process (e.g. after changing its config). Only for a \
                       process the user asked you to restart, or that you started yourself."
    )]
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

    #[tool(
        description = "Stop a running process. Do not stop a process the user is running \
                       unless they asked you to."
    )]
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

    #[tool(
        description = "Start a stopped process by name — the way to bring up a dev server this \
                       project already defines, rather than running your own copy."
    )]
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
            "TuxFlow manages this project's long-running processes (dev servers, watchers, \
             test runners) in its own terminals, and the user is watching them. Before you \
             start a dev server or any other long-running command, call list_processes: if it \
             is already Running here, use it — its `url` is where it serves and \
             get_process_logs shows its output — and do not start a second copy, and do not \
             stop or restart it unless the user asked. If a defined process is Stopped and you \
             need it, start_process brings it up under the user's eyes rather than in your own \
             shell. Resources: tuxflow://processes, tuxflow://logs/{name}, tuxflow://config."
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
                raw: RawResource::new(format!("tuxflow://logs/{name}"), format!("logs/{name}"))
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
            let processes: Vec<ProcessInfo> = state.values().map(ProcessInfo::from).collect();
            let json = serde_json::to_string_pretty(&processes).unwrap_or_default();
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(json, uri.clone()).with_mime_type("application/json"),
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
                        "url": p.url,
                        "working_dir": p.working_dir,
                        "pid": p.pid,
                        "restart_count": p.restart_count,
                        "uptime_secs": p.uptime_secs(),
                    })
                })
                .collect();
            let json = serde_json::to_string_pretty(&config).unwrap_or_default();
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(json, uri.clone()).with_mime_type("application/json"),
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
                            ResourceContents::text(text, uri.clone()).with_mime_type("text/plain"),
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
                        ResourceContents::text(text, uri.clone()).with_mime_type("text/plain"),
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
