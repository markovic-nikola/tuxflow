use std::io::Write;
use tempfile::NamedTempFile;

use tuxflow::config::loader::{load_config, ConfigError};
use tuxflow::config::schema::{ProcessCategory, TuxFlowConfig};

#[test]
fn parse_minimal_config() {
    let toml = r#"
[project]
name = "my-app"
"#;
    let config: TuxFlowConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.project.name, "my-app");
    assert!(config.project.icon.is_none());
    assert!(config.process.is_empty());
}

#[test]
fn parse_full_config() {
    let toml = r#"
[project]
name = "my-app"
icon = "logo.svg"

[[process]]
name = "dev"
command = "npm run dev"
auto_start = true
auto_restart = true
restart_when_changed = ["src/**/*.ts"]

[process.env]
NODE_ENV = "development"

[[process]]
name = "queue"
command = "php artisan queue:work"
category = "command"
"#;
    let config: TuxFlowConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.project.name, "my-app");
    assert_eq!(config.project.icon.as_deref(), Some("logo.svg"));
    assert_eq!(config.process.len(), 2);

    let dev = &config.process[0];
    assert_eq!(dev.name, "dev");
    assert_eq!(dev.command, "npm run dev");
    assert!(dev.start_with_project);
    assert!(dev.auto_restart);
    assert_eq!(dev.restart_when_changed, vec!["src/**/*.ts"]);
    assert_eq!(dev.env.get("NODE_ENV").unwrap(), "development");
    assert_eq!(dev.category, ProcessCategory::Command);

    let queue = &config.process[1];
    assert_eq!(queue.name, "queue");
    assert!(!queue.start_with_project);
    assert!(!queue.auto_restart);
}

#[test]
fn parse_agent_and_terminal_categories() {
    let toml = r#"
[project]
name = "test"

[[process]]
name = "claude"
command = "claude"
category = "agent"

[[process]]
name = "shell"
command = "bash"
category = "terminal"
"#;
    let config: TuxFlowConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.process[0].category, ProcessCategory::Agent);
    assert_eq!(config.process[1].category, ProcessCategory::Terminal);
}

#[test]
fn default_category_is_command() {
    let toml = r#"
[project]
name = "test"

[[process]]
name = "dev"
command = "npm run dev"
"#;
    let config: TuxFlowConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.process[0].category, ProcessCategory::Command);
}

#[test]
fn load_config_from_file() {
    let mut tmp = NamedTempFile::new().unwrap();
    write!(
        tmp,
        r#"
[project]
name = "file-test"

[[process]]
name = "run"
command = "cargo run"
"#
    )
    .unwrap();

    let config = load_config(tmp.path()).unwrap();
    assert_eq!(config.project.name, "file-test");
    assert_eq!(config.process.len(), 1);
}

#[test]
fn load_config_not_found() {
    let result = load_config(std::path::Path::new("/tmp/nonexistent-tuxflow.toml"));
    assert!(matches!(result, Err(ConfigError::NotFound(_))));
}

#[test]
fn load_config_invalid_toml() {
    let mut tmp = NamedTempFile::new().unwrap();
    write!(tmp, "this is not valid toml {{{{").unwrap();

    let result = load_config(tmp.path());
    assert!(matches!(result, Err(ConfigError::Parse(_))));
}

#[test]
fn auto_start_alias_works() {
    let toml = r#"
[project]
name = "test"

[[process]]
name = "dev"
command = "npm run dev"
auto_start = true
"#;
    let config: TuxFlowConfig = toml::from_str(toml).unwrap();
    assert!(config.process[0].start_with_project);
}
