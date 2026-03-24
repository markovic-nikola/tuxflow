use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use tokio::sync::{mpsc, oneshot};

const MAX_LOG_LINES: usize = 1000;

// --- Global shared state (accessible from both GTK and MCP threads) ---

static MCP_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn is_mcp_enabled() -> bool {
    MCP_ENABLED.load(Ordering::Relaxed)
}

pub fn set_mcp_enabled(enabled: bool) {
    MCP_ENABLED.store(enabled, Ordering::Relaxed);
}

pub static MCP_PROCESS_STATE: LazyLock<SharedProcessState> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

pub static MCP_LOG_BUFFERS: LazyLock<SharedLogBuffers> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

// --- Process state (GTK → MCP) ---

#[derive(Clone, Debug)]
pub struct ProcessSnapshot {
    pub name: String,
    pub status: String,
    pub command: String,
    pub category: String,
    pub pid: Option<i32>,
    pub restart_count: u32,
    pub uptime_secs: Option<u64>,
}

pub type SharedProcessState = Arc<Mutex<HashMap<String, ProcessSnapshot>>>;

// --- Log buffer (GTK → MCP) ---

pub struct LogBuffer {
    lines: VecDeque<String>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            lines: VecDeque::with_capacity(MAX_LOG_LINES),
        }
    }

    pub fn push(&mut self, line: String) {
        if self.lines.len() >= MAX_LOG_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn recent(&self, n: usize) -> Vec<String> {
        self.lines.iter().rev().take(n).rev().cloned().collect()
    }
}

pub type SharedLogBuffers = Arc<Mutex<HashMap<String, LogBuffer>>>;

// --- Commands (MCP → GTK) ---

pub enum McpCommand {
    StartProcess {
        name: String,
        reply: oneshot::Sender<CommandResult>,
    },
    StopProcess {
        name: String,
        reply: oneshot::Sender<CommandResult>,
    },
    RestartProcess {
        name: String,
        reply: oneshot::Sender<CommandResult>,
    },
    ReadLogs {
        name: String,
        lines: usize,
        reply: oneshot::Sender<CommandResult>,
    },
}

pub enum CommandResult {
    Ok(String),
    Error(String),
}

// --- Bridge ---

#[derive(Clone)]
pub struct McpBridge {
    pub process_state: SharedProcessState,
    pub log_buffers: SharedLogBuffers,
    pub command_tx: mpsc::UnboundedSender<McpCommand>,
}

pub fn create_mcp_bridge() -> (McpBridge, mpsc::UnboundedReceiver<McpCommand>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let bridge = McpBridge {
        process_state: MCP_PROCESS_STATE.clone(),
        log_buffers: MCP_LOG_BUFFERS.clone(),
        command_tx: tx,
    };
    (bridge, rx)
}
