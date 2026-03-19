use std::path::{Path, PathBuf};
use thiserror::Error;

use super::schema::TuxFlowConfig;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    NotFound(PathBuf),
    #[error("Failed to read config: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse TOML: {0}")]
    Parse(#[from] toml::de::Error),
}

pub fn find_config(project_dir: &Path) -> Option<PathBuf> {
    let config_path = project_dir.join("tuxflow.toml");
    if config_path.exists() {
        Some(config_path)
    } else {
        None
    }
}

pub fn load_config(path: &Path) -> Result<TuxFlowConfig, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::NotFound(path.to_path_buf()));
    }
    let content = std::fs::read_to_string(path)?;
    let config: TuxFlowConfig = toml::from_str(&content)?;
    Ok(config)
}
