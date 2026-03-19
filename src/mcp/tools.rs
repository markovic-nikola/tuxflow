use std::sync::{Arc, Mutex};

use rmcp::{
    ServerHandler,
    handler::server::{tool::ToolRouter, wrapper::Json},
    model::ServerInfo,
    schemars, tool, tool_router,
};
use serde::{Deserialize, Serialize};

// Thread-safe snapshot of process state for the MCP server
#[derive(Clone, Debug, Serialize)]
pub struct ProcessSnapshot {
    pub name: String,
    pub status: String,
    pub command: String,
    pub category: String,
}

pub type ProcessStateRef = Arc<Mutex<Vec<ProcessSnapshot>>>;

#[derive(Clone)]
pub struct TuxFlowMcpServer {
    tool_router: ToolRouter<Self>,
    process_state: ProcessStateRef,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProcessListOutput {
    pub processes: Vec<ProcessInfo>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProcessInfo {
    pub name: String,
    pub status: String,
    pub command: String,
    pub category: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProjectInfoOutput {
    pub total: usize,
    pub running: usize,
    pub processes: Vec<ProcessInfo>,
}

#[tool_router]
impl TuxFlowMcpServer {
    pub fn new(process_state: ProcessStateRef) -> Self {
        Self {
            tool_router: Self::tool_router(),
            process_state,
        }
    }

    #[tool(description = "List all managed processes with their current status")]
    fn list_processes(&self) -> Json<ProcessListOutput> {
        let state = self.process_state.lock().unwrap();
        let processes = state
            .iter()
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
        let state = self.process_state.lock().unwrap();
        let total = state.len();
        let running = state.iter().filter(|p| p.status == "Running").count();
        let processes = state
            .iter()
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
}

impl ServerHandler for TuxFlowMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }
}
