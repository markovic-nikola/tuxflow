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
    detector.rs            # Tech stack auto-detection (package.json, Cargo.toml, etc.) over ProjectFs
  remote/
    mod.rs                 # ProjectLocation (Local/Ssh), sh_quote, ssh ControlMaster options, remote command wrapping
    fs.rs                  # ProjectFs trait: LocalFs (std::fs) / SshFs (ssh exec over shared connection)
    tunnel.rs              # TunnelManager: ssh -L port forwards for detected ports on remote projects
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
    add_remote_project_dialog.rs # Add remote (SSH) project dialog with BatchMode verification
    add_ssh_dialog.rs      # Add SSH connection dialog
    edit_project_dialog.rs # Edit project dialog
    project_detail.rs      # Project overview panel
    status_bar.rs          # Bottom bar: actions + git state + remote indicator
    sidebar/
      project_list.rs      # Sidebar with expandable project sections
      project_row.rs       # Project row (icon, name, controls)
      process_row.rs       # Process row (status dot, name, port)
      section_header.rs    # Section header (AGENTS 5/5, COMMANDS 2/5, SSH 1/3)
      dnd.rs               # Drag-and-drop reordering
    settings/
      settings_window.rs   # AdwPreferencesWindow with all settings tabs
  util/
    port_detector.rs       # Regex scan terminal output for ports/URLs
    worker.rs              # run(): blocking work on a thread, result on the GTK main loop
    icon_detector.rs       # Project icon auto-detection
    notifications.rs       # Desktop notifications via libnotify
data/
  style.css                # Application stylesheet
  icons/                   # SVG icons + hicolor hierarchy
  com.tuxflow.TuxFlow.desktop
  com.tuxflow.TuxFlow.metainfo.xml
```

## Architecture Notes

- **Process categories:** `ProcessCategory::Command`, `Agent`, `Terminal`, `SSH` — each gets a dedicated sidebar section. SSH connections are VTE terminals running `ssh` commands, reusing the full process management infrastructure (auto-restart handles reconnection)
- **MCP bridge:** `mcp/bridge.rs` uses `LazyLock<Arc<Mutex<>>>` globals for cross-thread state. `LogBuffer` is a 1000-line VecDeque ring buffer fed by VTE `contents-changed` signal
- **Settings persistence:** All settings save immediately to `~/.config/tuxflow/settings.toml` (24 save points in settings_window.rs)
- **URL handling:** VTE regex matching + Ctrl+click opens via `xdg-open`. Sidebar/status bar browser buttons use `gtk4::UriLauncher`
- **`TUXFLOW_CHILD=1`** env var is set in window.rs and inherited by child processes (used to prevent recursive spawning)
- **Remote projects:** a project's `ProjectLocation` is `Local(PathBuf)` or `Ssh { host, dir }`, persisted in projects.toml as an opaque `ssh://host/path` key (no schema migration). Remote processes spawn as `exec ssh -t <mux> host '<wrap>'` in the normal VTE pipeline; all ssh invocations share a ControlMaster connection (`$XDG_RUNTIME_DIR/tuxflow/ssh-%C`). The wrap runs the command inside a tmux session on a dedicated server (`tmux -L tuxflow`, status off, mouse on, deterministic `tf-<proc>-<fnv32>` session names from project key + process name), so connection loss and app quit only *detach* — reconnect and app relaunch reattach via `new-session -A`; the command's exit code round-trips through `/tmp/.<session>-<uid>.exit` (uid-namespaced — deterministic session names would otherwise collide across users on a shared host) so crash detection still works. Hosts without tmux fall back inline to direct exec (no persistence; `pkill -s` on the pidfile PID covers the kill). Stop kills the tmux session + pidfile login session and flags the next spawn to kill-before-create (`remote_fresh_next` — avoids racing the fire-and-forget remote kill); app quit calls `detach_all()` instead, so remote processes deliberately outlive TuxFlow. Exit 255 on a remote process is surfaced as "connection lost — reconnecting", not a crash; reconnects retry forever (backoff capped at 32 s, one notification per outage) since the session is still alive on the host. Project load probes `tmux list-sessions` and auto-reattaches processes whose sessions are still alive, so the UI never shows "stopped" for a running detached process. Mouse selections inside tmux reach the local clipboard via TuxFlow's own bridge (mouse-up on a remote terminal → `tmux show-buffer` over ssh → GTK clipboard, change-detected by hash) — NO released VTE implements OSC 52 (verified through 0.84), so the standard escape-sequence route is a dead end. The reverse direction too: pasting (Ctrl+Shift+V) while the clipboard holds an image uploads the PNG to the host (`remote::upload_image` → `/tmp/.tuxflow-img-*.png`) and types the path into the terminal — agents treat image paths as attachments. Detected ports auto-tunnel via `ssh -N -L` (remote/tunnel.rs) — every local port seen in output, remapping to a free local port on collision (badge/URL show the local port); tunnels use dedicated connections (NOT the mux — a forward requested via a mux client survives in the master after the client dies) with PDEATHSIG so they can't outlive the app. Git UI works remotely (`git_changes_dialog::git_command` routes through ssh; `.git` probe runs on a worker thread). Editor button uses `code --remote ssh-remote+host` for code-family editors (util/editor.rs). Add Remote dialog autocompletes remote paths (debounced `ls -1dp` over ssh). Still gated off for remote: file watcher, icon detection, MCP, live re-detection in Edit Project. Remote probing (config/detection/git) runs on worker threads via `util::worker::run` — never block the GTK main thread with ssh, and never poll a channel from `idle_add_local` (it busy-spins the main loop). Startup loads of unreachable remote projects notify once and retry in the background with capped backoff (`ProbeError::Unreachable` vs `Invalid` decides retryability)

## Config Files

- **Project config:** `tuxflow.toml` in project root (optional, version-controlled)
- **Global settings:** `~/.config/tuxflow/settings.toml` (appearance, keybindings, notifications)
- **Project state:** `~/.config/tuxflow/projects.toml` (open directories, custom commands, process order, UI state)

## Formatting

Always run `cargo fmt --all` after changing Rust code, before committing.

## TODO

- **Split terminal view** — Currently `gtk4::Stack` (one at a time). Would need `gtk4::Paned` for side-by-side
- **Tests** — Core modules covered (config, detector, port detector, log buffer). UI and process management untested (require GTK runtime)
- **CI** — No GitHub Actions or verified packaging builds yet
