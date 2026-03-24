# TuxFlow

Linux-only desktop app for managing dev environments — processes, AI coding agents, and terminals from one window. Inspired by [SoloTerm](https://soloterm.com/) but native GTK4 for Linux.

## Stack

- **Language:** Rust
- **GUI:** GTK4 + libadwaita (gtk4-rs, vte4-rs)
- **Terminal:** VTE4 (vte-2.91-gtk4)
- **Async:** tokio
- **Config:** TOML (serde + toml crate)
- **MCP:** rmcp (Rust MCP SDK, Unix socket transport)
- **File watching:** notify v7

## Build & Run

```bash
# System deps (Ubuntu 24.04)
sudo apt install libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev build-essential

# Build & run
cargo build
cargo run
cargo run -- /path/to/project
```

## Project Structure

```
src/
  main.rs                  # Entry point
  app.rs                   # GtkApplication subclass
  workspace.rs             # Multi-project workspace management
  bin/tuxflow-mcp.rs       # Standalone MCP server binary
  config/
    schema.rs              # Serde structs for tuxflow.toml (ProcessConfig, ProcessCategory, etc.)
    loader.rs              # TOML loading, validation, defaults
    keybindings.rs         # Keyboard shortcut config & persistence
    projects.rs            # Project config management (~/.config/tuxflow/projects.toml)
    settings.rs            # Global settings (~/.config/tuxflow/settings.toml)
    ssh.rs                 # SSH config parser (~/.ssh/config host extraction)
  process/
    manager.rs             # ProcessManager: spawn/kill/restart via VTE
    auto_restart.rs        # Crash detection + exponential backoff
    pid_file.rs            # PID file tracking
  detect/
    detector.rs            # Tech stack auto-detection (package.json, Cargo.toml, etc.)
  watcher/
    file_watcher.rs        # notify + glob matching, triggers process restart
  mcp/
    server.rs              # MCP server on Unix socket (/tmp/tuxflow-<project>.sock)
    tools.rs               # MCP tools: list_processes, get_process_logs, start/stop/restart
    bridge.rs              # GTK<->MCP thread bridge, LogBuffer (VecDeque ring buffer)
  ui/
    window.rs              # Main AdwApplicationWindow — central wiring hub
    accent.rs              # Accent color theming
    terminal_theme.rs      # Terminal color schemes
    terminal_view.rs       # VTE terminal wrapper
    terminal_search.rs     # Ctrl+F search overlay
    command_palette.rs     # Ctrl+K command palette
    add_command_dialog.rs  # Add command/agent dialog
    add_ssh_dialog.rs      # Add SSH connection dialog
    edit_project_dialog.rs # Edit project dialog
    project_detail.rs      # Project overview panel
    status_bar.rs          # Bottom bar: actions + CPU/MEM
    sidebar/
      project_list.rs      # Sidebar with expandable project sections
      project_row.rs       # Project row (icon, name, controls)
      process_row.rs       # Process row (status dot, name, port, resources)
      section_header.rs    # Section header (AGENTS 5/5, COMMANDS 2/5, SSH 1/3)
      dnd.rs               # Drag-and-drop reordering
    settings/
      settings_window.rs   # AdwPreferencesWindow with all settings tabs
  util/
    port_detector.rs       # Regex scan terminal output for ports/URLs
    resource_monitor.rs    # CPU/memory via /proc/<pid>/stat
    icon_detector.rs       # Project icon auto-detection
    notifications.rs       # Desktop notifications via libnotify
data/
  style.css                # Application stylesheet
  icons/                   # SVG icons + hicolor hierarchy
  com.tuxflow.TuxFlow.desktop
  com.tuxflow.TuxFlow.metainfo.xml
packaging/
  flatpak/com.tuxflow.TuxFlow.yml
  appimage/AppImageBuilder.yml
  aur/PKGBUILD
```

## Architecture Notes

- **Process categories:** `ProcessCategory::Command`, `Agent`, `Terminal`, `SSH` — each gets a dedicated sidebar section. SSH connections are VTE terminals running `ssh` commands, reusing the full process management infrastructure (auto-restart handles reconnection)
- **MCP bridge:** `mcp/bridge.rs` uses `LazyLock<Arc<Mutex<>>>` globals for cross-thread state. `LogBuffer` is a 1000-line VecDeque ring buffer fed by VTE `contents-changed` signal
- **Settings persistence:** All settings save immediately to `~/.config/tuxflow/settings.toml` (24 save points in settings_window.rs)
- **URL handling:** VTE regex matching + Ctrl+click opens via `xdg-open`. Sidebar/status bar browser buttons use `gtk4::UriLauncher`
- **Resource monitoring:** Polls `/proc/<pid>/stat` and `/proc/<pid>/statm` every 2s, updates sidebar process rows
- **`TUXFLOW_CHILD=1`** env var is set in window.rs and inherited by child processes (used to prevent recursive spawning)

## Config Files

- **Project config:** `tuxflow.toml` in project root (version-controlled)
- **Global settings:** `~/.config/tuxflow/settings.toml`
- **Custom commands:** `~/.config/tuxflow/projects.toml`

## TODO

- **Split terminal view** — Currently `gtk4::Stack` (one at a time). Would need `gtk4::Paned` for side-by-side
- **Tests** — Core modules covered (config, detector, port detector, log buffer). UI and process management untested (require GTK runtime)
- **CI** — No GitHub Actions or verified packaging builds yet
