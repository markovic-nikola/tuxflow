use std::fs;

use tempfile::TempDir;

use tuxflow::detect::detector::detect_stacks;

#[test]
fn detect_nodejs_npm() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"dev":"vite","build":"vite build","test":"vitest"}}"#,
    )
    .unwrap();

    let stacks = detect_stacks(dir.path());
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].name, "Node.js");

    let names: Vec<&str> = stacks[0]
        .suggested_processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"dev"));
    assert!(names.contains(&"build"));
    assert!(names.contains(&"test"));

    // Should use npm by default
    let dev = stacks[0]
        .suggested_processes
        .iter()
        .find(|p| p.name == "dev")
        .unwrap();
    assert_eq!(dev.command, "npm run dev");
}

#[test]
fn detect_nodejs_yarn() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"dev":"vite"}}"#,
    )
    .unwrap();
    fs::write(dir.path().join("yarn.lock"), "").unwrap();

    let stacks = detect_stacks(dir.path());
    let dev = stacks[0]
        .suggested_processes
        .iter()
        .find(|p| p.name == "dev")
        .unwrap();
    assert_eq!(dev.command, "yarn dev");
}

#[test]
fn detect_nodejs_bun_modern_lockfile() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"dev":"vite","dev:server":"bun server.js","db:reset":"rm db"}}"#,
    )
    .unwrap();
    fs::write(dir.path().join("bun.lock"), "").unwrap();

    let stacks = detect_stacks(dir.path());
    let by_name = |name: &str| -> String {
        stacks[0]
            .suggested_processes
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("missing script {name}"))
            .command
            .clone()
    };

    assert_eq!(by_name("dev"), "bun run dev");
    assert_eq!(by_name("dev:server"), "bun run dev:server");
    assert_eq!(by_name("db:reset"), "bun run db:reset");
}

#[test]
fn detect_nodejs_includes_custom_scripts() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"dev":"vite","lint":"eslint .","db:reset":"rm db"}}"#,
    )
    .unwrap();

    let stacks = detect_stacks(dir.path());
    let names: Vec<&str> = stacks[0]
        .suggested_processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"lint"));
    assert!(names.contains(&"db:reset"));
}

#[test]
fn detect_nodejs_pnpm() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"dev":"vite"}}"#,
    )
    .unwrap();
    fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();

    let stacks = detect_stacks(dir.path());
    let dev = stacks[0]
        .suggested_processes
        .iter()
        .find(|p| p.name == "dev")
        .unwrap();
    assert_eq!(dev.command, "pnpm dev");
}

#[test]
fn detect_nodejs_start_only_when_no_dev() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"start":"node index.js"}}"#,
    )
    .unwrap();

    let stacks = detect_stacks(dir.path());
    let names: Vec<&str> = stacks[0]
        .suggested_processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"start"));
    assert!(!names.contains(&"dev"));
}

#[test]
fn detect_rust_non_tuxflow() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"
[package]
name = "my-project"
version = "0.1.0"
"#,
    )
    .unwrap();

    // Make sure TUXFLOW_CHILD is not set for this test
    unsafe {
        std::env::remove_var("TUXFLOW_CHILD");
    }

    let stacks = detect_stacks(dir.path());
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].name, "Rust");

    let names: Vec<&str> = stacks[0]
        .suggested_processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"cargo run"));
    assert!(names.contains(&"cargo test"));
}

#[test]
fn detect_rust_skips_cargo_run_for_tuxflow() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"
[package]
name = "tuxflow"
version = "0.1.0"
"#,
    )
    .unwrap();

    let stacks = detect_stacks(dir.path());
    let names: Vec<&str> = stacks[0]
        .suggested_processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(
        !names.contains(&"cargo run"),
        "should skip cargo run for tuxflow itself"
    );
    assert!(names.contains(&"cargo test"));
}

#[test]
fn detect_go() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("go.mod"), "module example.com/app").unwrap();

    let stacks = detect_stacks(dir.path());
    assert_eq!(stacks[0].name, "Go");

    let names: Vec<&str> = stacks[0]
        .suggested_processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"go run"));
    assert!(names.contains(&"go test"));
}

#[test]
fn detect_django() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("manage.py"), "").unwrap();

    let stacks = detect_stacks(dir.path());
    assert_eq!(stacks[0].name, "Django");
    assert!(
        stacks[0]
            .suggested_processes
            .iter()
            .any(|p| p.command.contains("runserver"))
    );
}

#[test]
fn detect_laravel() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("composer.json"),
        r#"{"require":{"laravel/framework":"^11.0"}}"#,
    )
    .unwrap();

    let stacks = detect_stacks(dir.path());
    assert_eq!(stacks[0].name, "PHP");
    assert!(
        stacks[0]
            .suggested_processes
            .iter()
            .any(|p| p.command.contains("artisan serve"))
    );
}

#[test]
fn detect_docker_compose() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("docker-compose.yml"), "version: '3'").unwrap();

    let stacks = detect_stacks(dir.path());
    assert_eq!(stacks[0].name, "Docker Compose");
    assert!(
        stacks[0]
            .suggested_processes
            .iter()
            .any(|p| p.command == "docker compose up")
    );
}

#[test]
fn detect_multiple_stacks() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"dev":"vite"}}"#,
    )
    .unwrap();
    fs::write(dir.path().join("docker-compose.yml"), "version: '3'").unwrap();

    let stacks = detect_stacks(dir.path());
    let stack_names: Vec<&str> = stacks.iter().map(|s| s.name.as_str()).collect();
    assert!(stack_names.contains(&"Node.js"));
    assert!(stack_names.contains(&"Docker Compose"));
}

#[test]
fn detect_makefile_targets() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("Makefile"),
        "build:\n\tcargo build\n\ntest:\n\tcargo test\n\nclean:\n\trm -rf target\n",
    )
    .unwrap();

    let stacks = detect_stacks(dir.path());
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].name, "Make");

    let names: Vec<&str> = stacks[0]
        .suggested_processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"make build"));
    assert!(names.contains(&"make test"));
    assert!(names.contains(&"make clean"));
}

#[test]
fn detect_makefile_skips_variables() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("Makefile"),
        "\
APP_NAME := my-app\n\
SSH_HOST = example.com\n\
VERSION ?= 1.0\n\
CFLAGS += -Wall\n\
RELEASE ::= $(shell date)\n\
\n\
deploy:\n\
\t@echo deploying\n\
\n\
build:\n\
\tcargo build\n",
    )
    .unwrap();

    let stacks = detect_stacks(dir.path());
    let names: Vec<&str> = stacks[0]
        .suggested_processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();

    // Should detect targets
    assert!(names.contains(&"make deploy"));
    assert!(names.contains(&"make build"));

    // Should NOT detect variables
    assert!(
        !names.contains(&"make APP_NAME"),
        "should skip := assignments"
    );
    assert!(
        !names.contains(&"make SSH_HOST"),
        "should skip = assignments"
    );
    assert!(
        !names.contains(&"make VERSION"),
        "should skip ?= assignments"
    );
    assert!(
        !names.contains(&"make CFLAGS"),
        "should skip += assignments"
    );
    assert!(
        !names.contains(&"make RELEASE"),
        "should skip ::= assignments"
    );
}

#[test]
fn detect_makefile_skips_special_targets() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("Makefile"),
        "\
.PHONY: all clean\n\
.DEFAULT_GOAL := all\n\
\n\
all:\n\
\t@echo all\n\
\n\
# This is a comment\n\
\trecipe-line-not-a-target:\n",
    )
    .unwrap();

    let stacks = detect_stacks(dir.path());
    let names: Vec<&str> = stacks[0]
        .suggested_processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();

    assert!(names.contains(&"make all"));
    assert!(!names.iter().any(|n| n.contains("PHONY")));
    assert!(!names.iter().any(|n| n.contains("DEFAULT_GOAL")));
}

#[test]
fn detect_empty_directory() {
    let dir = TempDir::new().unwrap();
    let stacks = detect_stacks(dir.path());
    assert!(stacks.is_empty());
}
