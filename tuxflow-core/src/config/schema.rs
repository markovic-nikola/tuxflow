use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuxFlowConfig {
    pub project: ProjectConfig,
    #[serde(default)]
    pub process: Vec<ProcessConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default, alias = "auto_start")]
    pub start_with_project: bool,
    #[serde(default)]
    pub auto_restart: bool,
    /// Open the process's detected URL in the local browser once it appears —
    /// only after user-initiated starts (never auto-restarts or reattaches).
    /// For remote projects the tunnelled (possibly remapped) URL is opened.
    #[serde(default)]
    pub open_in_browser: bool,
    #[serde(default)]
    pub restart_when_changed: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_category")]
    pub category: ProcessCategory,
    #[serde(default)]
    pub auto_named: bool,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ProcessCategory {
    #[default]
    Command,
    Agent,
    Terminal,
    #[serde(alias = "ssh_connection")]
    SSH,
}

fn default_category() -> ProcessCategory {
    ProcessCategory::Command
}
