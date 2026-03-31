use std::path::Path;

use crate::config::schema::{ProcessCategory, ProcessConfig};

#[derive(Clone)]
pub struct DetectedStack {
    pub name: String,
    pub suggested_processes: Vec<ProcessConfig>,
}

struct StackRule {
    marker_file: &'static str,
    name: &'static str,
    detect: fn(&Path, &str) -> Vec<ProcessConfig>,
}

const RULES: &[StackRule] = &[
    StackRule {
        marker_file: "package.json",
        name: "Node.js",
        detect: detect_nodejs,
    },
    StackRule {
        marker_file: "Cargo.toml",
        name: "Rust",
        detect: detect_rust,
    },
    StackRule {
        marker_file: "manage.py",
        name: "Django",
        detect: detect_django,
    },
    StackRule {
        marker_file: "go.mod",
        name: "Go",
        detect: detect_go,
    },
    StackRule {
        marker_file: "composer.json",
        name: "PHP",
        detect: detect_php,
    },
    StackRule {
        marker_file: "Gemfile",
        name: "Ruby",
        detect: detect_ruby,
    },
    StackRule {
        marker_file: "docker-compose.yml",
        name: "Docker Compose",
        detect: detect_docker,
    },
    StackRule {
        marker_file: "docker-compose.yaml",
        name: "Docker Compose",
        detect: detect_docker,
    },
    StackRule {
        marker_file: "Makefile",
        name: "Make",
        detect: detect_makefile,
    },
];

pub fn detect_stacks(project_dir: &Path) -> Vec<DetectedStack> {
    let mut stacks = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for rule in RULES {
        if project_dir.join(rule.marker_file).exists() && seen.insert(rule.name) {
            let content =
                std::fs::read_to_string(project_dir.join(rule.marker_file)).unwrap_or_default();
            let processes = (rule.detect)(project_dir, &content);
            if !processes.is_empty() {
                stacks.push(DetectedStack {
                    name: rule.name.to_string(),
                    suggested_processes: processes,
                });
            }
        }
    }

    stacks
}

fn make_process(name: &str, command: &str, _auto_start: bool) -> ProcessConfig {
    ProcessConfig {
        name: name.to_string(),
        command: command.to_string(),
        working_dir: None,
        start_with_project: false,
        auto_restart: false,
        restart_when_changed: Vec::new(),
        env: std::collections::HashMap::new(),
        category: ProcessCategory::Command,
        auto_named: false,
        display_name: None,
    }
}

fn detect_nodejs(dir: &Path, content: &str) -> Vec<ProcessConfig> {
    let mut procs = Vec::new();

    // Parse scripts from package.json
    if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(content)
        && let Some(scripts) = pkg.get("scripts").and_then(|s| s.as_object())
    {
        if scripts.contains_key("dev") {
            let cmd = if dir.join("yarn.lock").exists() {
                "yarn dev"
            } else if dir.join("pnpm-lock.yaml").exists() {
                "pnpm dev"
            } else if dir.join("bun.lockb").exists() {
                "bun dev"
            } else {
                "npm run dev"
            };
            procs.push(make_process("dev", cmd, true));
        }
        if scripts.contains_key("start") && !scripts.contains_key("dev") {
            procs.push(make_process("start", "npm start", true));
        }
        if scripts.contains_key("build") {
            procs.push(make_process("build", "npm run build", false));
        }
        if scripts.contains_key("test") {
            procs.push(make_process("test", "npm test", false));
        }
    }

    procs
}

fn detect_rust(_dir: &Path, content: &str) -> Vec<ProcessConfig> {
    let mut procs = Vec::new();

    // Skip "cargo run" if this project would re-launch TuxFlow itself
    let is_self = std::env::var("TUXFLOW_CHILD").is_ok() || content.contains("name = \"tuxflow\"");
    if !is_self {
        procs.push(make_process("cargo run", "cargo run", true));
    }
    procs.push(make_process("cargo test", "cargo test", false));

    procs
}

fn detect_django(_dir: &Path, _content: &str) -> Vec<ProcessConfig> {
    vec![
        make_process("Django server", "python manage.py runserver", true),
        make_process("Django migrate", "python manage.py migrate", false),
    ]
}

fn detect_go(_dir: &Path, _content: &str) -> Vec<ProcessConfig> {
    vec![
        make_process("go run", "go run .", true),
        make_process("go test", "go test ./...", false),
    ]
}

fn is_composer_lifecycle_hook(name: &str) -> bool {
    name.starts_with("pre-")
        || name.starts_with("post-")
        || name.starts_with("pre_")
        || name.starts_with("post_")
}

fn detect_php(dir: &Path, content: &str) -> Vec<ProcessConfig> {
    let mut procs = Vec::new();

    if let Ok(composer) = serde_json::from_str::<serde_json::Value>(content) {
        let is_laravel = composer
            .get("require")
            .and_then(|r| r.as_object())
            .is_some_and(|r| r.contains_key("laravel/framework"));

        if is_laravel {
            procs.push(make_process("artisan serve", "php artisan serve", true));
            if dir.join("vite.config.js").exists() || dir.join("vite.config.ts").exists() {
                procs.push(make_process("npm:dev", "npm run dev", true));
            }
            procs.push(make_process("queue", "php artisan queue:work", false));
        } else {
            procs.push(make_process("PHP server", "php -S localhost:8000", true));
        }

        // Detect composer scripts (skip lifecycle hooks)
        if let Some(scripts) = composer.get("scripts").and_then(|s| s.as_object()) {
            for key in scripts.keys() {
                if is_composer_lifecycle_hook(key) {
                    continue;
                }
                let cmd = format!("composer {key}");
                procs.push(make_process(&cmd, &cmd, false));
            }
        }
    }

    procs
}

fn detect_ruby(dir: &Path, _content: &str) -> Vec<ProcessConfig> {
    if dir.join("bin/rails").exists() {
        vec![
            make_process("Rails server", "bin/rails server", true),
            make_process("Rails console", "bin/rails console", false),
        ]
    } else {
        vec![make_process("bundle exec", "bundle exec ruby app.rb", true)]
    }
}

fn detect_makefile(_dir: &Path, content: &str) -> Vec<ProcessConfig> {
    let mut procs = Vec::new();

    for line in content.lines() {
        // Skip recipe lines, comments, and lines starting with whitespace or dots
        if line.starts_with('\t')
            || line.starts_with('#')
            || line.starts_with('.')
            || line.starts_with(' ')
            || line.is_empty()
        {
            continue;
        }
        // Skip variable assignments: lines containing = without a preceding :
        // (covers VAR = val, VAR := val, VAR ::= val, VAR ?= val, VAR += val)
        if let Some(eq_pos) = line.find('=') {
            let before_eq = &line[..eq_pos];
            if !before_eq.contains(':') || before_eq.ends_with(':') || before_eq.ends_with(':') {
                continue;
            }
        }
        // Must have a colon to be a target rule
        if let Some(colon_pos) = line.find(':') {
            let target = line[..colon_pos].trim();
            if target.is_empty()
                || target.contains('%')
                || target.contains('$')
                || target.contains('/')
            {
                continue;
            }
            // Only alphanumeric, hyphens, underscores in target names
            if target
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                let cmd = format!("make {target}");
                procs.push(make_process(&cmd, &cmd, false));
            }
        }
    }

    procs
}

fn detect_docker(_dir: &Path, _content: &str) -> Vec<ProcessConfig> {
    vec![
        make_process("docker compose up", "docker compose up", true),
        make_process("docker compose logs", "docker compose logs -f", false),
    ]
}
