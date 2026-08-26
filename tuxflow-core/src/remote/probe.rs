//! Remote project probing — read tuxflow.toml or run stack detection over
//! ssh, and list which tmux sessions from previous runs are still alive.
//! Shared by both shells (GTK adds icon fetching on top). Blocking — call
//! from a worker thread, never a UI thread.

use crate::config::loader;
use crate::config::schema::TuxFlowConfig;
use crate::detect::detector::{self, DetectedStack};
use crate::remote::fs::{ProjectFs, SshFs, remote_dir_exists};

/// Unreachable is retryable (host down, network); Invalid is not (bad dir,
/// broken config) — the distinction drives background-retry policy.
#[derive(Debug, Clone)]
pub enum ProbeError {
    Unreachable(String),
    Invalid(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(msg) | Self::Invalid(msg) => f.write_str(msg),
        }
    }
}

pub struct RemoteProbe {
    pub config: Option<TuxFlowConfig>,
    pub stacks: Vec<DetectedStack>,
    /// tmux sessions of this TuxFlow's naming scheme still running on the
    /// host — processes to reattach instead of showing "stopped".
    pub live_sessions: Vec<String>,
}

pub fn probe_remote(host: &str, dir: &str, conservative: bool) -> Result<RemoteProbe, ProbeError> {
    // Held for the whole probe (existence check, session list, config
    // read), so a workspace load can't open more ssh channels than the
    // host's MaxSessions allows.
    let _permit = crate::remote::ssh_permit();

    match remote_dir_exists(host, dir) {
        Ok(true) => {}
        Ok(false) => {
            return Err(ProbeError::Invalid(format!(
                "No such directory on {host}: {dir}"
            )));
        }
        Err(e) => return Err(ProbeError::Unreachable(e)),
    }
    let live_sessions = crate::remote::list_live_sessions(host);
    let fs = SshFs::new(host, dir);
    if let Ok(content) = fs.read_to_string("tuxflow.toml") {
        return match loader::load_config_str(&content) {
            Ok(config) => Ok(RemoteProbe {
                config: Some(config),
                stacks: Vec::new(),
                live_sessions,
            }),
            Err(e) => Err(ProbeError::Invalid(format!(
                "Failed to parse tuxflow.toml on {host}: {e}"
            ))),
        };
    }
    let stacks = if conservative {
        detector::detect_stacks_conservative_fs(&fs)
    } else {
        detector::detect_stacks_fs(&fs)
    };
    Ok(RemoteProbe {
        config: None,
        stacks,
        live_sessions,
    })
}
