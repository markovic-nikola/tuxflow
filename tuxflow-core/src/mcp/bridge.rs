use std::collections::{BTreeMap, HashMap, VecDeque};
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
    LazyLock::new(|| Arc::new(Mutex::new(BTreeMap::new())));

pub static MCP_LOG_BUFFERS: LazyLock<SharedLogBuffers> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

// --- Process state (GTK → MCP) ---

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub name: String,
    pub status: String,
    pub command: String,
    pub category: String,
    /// Where the process runs, as the agent would `cd` to it — the host's
    /// path for a remote project (the agent is on the host too).
    pub working_dir: Option<String>,
    /// The URL the process serves, when its output announced one. This is
    /// the field the whole feature exists for: an agent that can read "dev
    /// server, Running, http://localhost:5173" has no reason to start its
    /// own. Given as the HOST's port for remote projects, not the tunnel's
    /// local remap — the agent asking lives on the host.
    pub url: Option<String>,
    pub pid: Option<i32>,
    pub restart_count: u32,
    /// Unix time of the current run's start. Stored as a timestamp rather
    /// than an uptime so a snapshot written once stays truthful for as
    /// long as the run lasts — the shells rewrite snapshots on change,
    /// not on a clock.
    pub started_unix: Option<u64>,
}

impl ProcessSnapshot {
    pub fn uptime_secs(&self) -> Option<u64> {
        let started = self.started_unix?;
        Some(now_unix().saturating_sub(started))
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Keyed by process name. A `BTreeMap` so `list_processes` answers in a
/// stable order — a `HashMap` handed the agent a differently shuffled list
/// on every call.
pub type SharedProcessState = Arc<Mutex<BTreeMap<String, ProcessSnapshot>>>;

// --- Log buffer (GTK → MCP) ---

pub struct LogBuffer {
    lines: VecDeque<String>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new()
    }
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

/// A bridge over the process-wide maps — the GTK app's shape, where every
/// project's socket answers from ONE table keyed by process name.
pub fn create_mcp_bridge() -> (McpBridge, mpsc::UnboundedReceiver<McpCommand>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let bridge = McpBridge {
        process_state: MCP_PROCESS_STATE.clone(),
        log_buffers: MCP_LOG_BUFFERS.clone(),
        command_tx: tx,
    };
    (bridge, rx)
}

impl McpBridge {
    /// A bridge with maps of its own: one per project, so an agent
    /// connected to project A's socket sees A's processes and nothing
    /// else — two projects with a `dev` each would otherwise share (and
    /// overwrite) one row in the global table.
    pub fn isolated() -> (McpBridge, mpsc::UnboundedReceiver<McpCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let bridge = McpBridge {
            process_state: Arc::new(Mutex::new(BTreeMap::new())),
            log_buffers: Arc::new(Mutex::new(HashMap::new())),
            command_tx: tx,
        };
        (bridge, rx)
    }

    /// Replace the snapshot table wholesale. `Vec` rather than a map so the
    /// caller's entry order survives into `list_processes`.
    pub fn replace_snapshots(&self, snapshots: Vec<ProcessSnapshot>) {
        if let Ok(mut state) = self.process_state.lock() {
            state.clear();
            for s in snapshots {
                state.insert(s.name.clone(), s);
            }
        }
    }
}
