# TuxFlow — Implementation Plan

## Context

TuxFlow is a new open-source (MIT/Apache) Linux-only desktop application inspired by [SoloTerm](https://soloterm.com/). SoloTerm is a proprietary macOS-first app (built with Tauri) that serves as a lightweight agentic development environment — managing dev processes, AI coding agents, and terminals from a single interface. TuxFlow aims to bring this concept to Linux with a native GTK4 app, a distinct design identity, and TOML-based configuration instead of YAML.

**What TuxFlow does:** Lets developers manage their entire dev environment from one window — start/stop/restart dev servers, run multiple AI coding agents (Claude Code, Codex, Gemini CLI, etc.) side by side, monitor process status and resource usage, auto-restart crashed processes, watch files for changes, and expose an MCP server so AI agents can observe and control neighboring processes.

**Stack:** Rust + GTK4 + VTE4 (libadwaita for modern GNOME-style widgets)

---

## Tech Stack Details

| Component | Choice | Rationale |
|-----------|--------|-----------|
| Language | **Rust** | Memory-safe, excellent GTK4 bindings (gtk4-rs), proven for terminal apps (Alacritty, WezTerm, Zellij) |
| GUI toolkit | **GTK4 + libadwaita** | Native on every Linux distro/DE. libadwaita provides modern adaptive widgets, dark/light themes, and follows GNOME HIG while working on KDE/XFCE/etc. |
| Terminal widget | **VTE4** (vte4-rs) | Battle-tested terminal emulator widget used by GNOME Terminal, Tilix, Ptyxis. Full ANSI, 256-color, true color, GPU-accelerated rendering. |
| Async runtime | **tokio** | Process lifecycle management, file watching, MCP server |
| Config format | **TOML** | Human-readable, supports comments, native Rust support via `toml` + `serde` |
| MCP SDK | **rmcp** | Official Rust MCP SDK (v0.16+) |
| File watching | **notify** (v7) | Cross-platform fs events, inotify on Linux |
| Packaging | AppImage + Flatpak + .deb + .rpm | Maximum distro coverage |

### System Dependencies (runtime)
- `gtk4` (>= 4.12)
- `libadwaita-1` (>= 1.4)
- `vte-2.91-gtk4`

These are available in package repos of Ubuntu 24.04+, Fedora 39+, Arch, openSUSE Tumbleweed, etc.

---

## Project Structure

```
/home/nikola/Projects/tuxflow/
├── Cargo.toml                      # Workspace manifest
├── LICENSE-MIT
├── LICENSE-APACHE
├── tuxflow.example.toml            # Example config for users
├── data/
│   ├── com.tuxflow.TuxFlow.desktop       # .desktop file
│   ├── com.tuxflow.TuxFlow.metainfo.xml  # AppStream metadata
│   ├── icons/                            # App icons (SVG + PNGs)
│   └── resources.gresource.xml           # GTK resource bundle
├── src/
│   ├── main.rs                     # Entry point, GTK Application setup
│   ├── app.rs                      # TuxFlowApp — GtkApplication subclass
│   ├── config/
│   │   ├── mod.rs
│   │   ├── schema.rs               # Serde structs for tuxflow.toml
│   │   └── loader.rs               # TOML loading, validation, defaults, hot-reload
│   ├── process/
│   │   ├── mod.rs
│   │   ├── manager.rs              # ProcessManager: spawn/kill/restart/status
│   │   ├── pty.rs                  # PTY lifecycle via VTE, bridging to ProcessManager
│   │   └── auto_restart.rs         # Crash detection + exponential backoff restart
│   ├── detect/
│   │   ├── mod.rs
│   │   └── detector.rs             # Tech stack auto-detection (package.json, Cargo.toml, etc.)
│   ├── watcher/
│   │   ├── mod.rs
│   │   └── file_watcher.rs         # notify crate integration, glob matching, debounce
│   ├── mcp/
│   │   ├── mod.rs
│   │   ├── server.rs               # MCP server (rmcp, Unix socket transport)
│   │   └── tools.rs                # MCP tool definitions (list_processes, read_logs, etc.)
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── window.rs               # Main AdwApplicationWindow
│   │   ├── sidebar/
│   │   │   ├── mod.rs
│   │   │   ├── project_list.rs     # Sidebar project list with expandable sections
│   │   │   ├── project_row.rs      # Single project row (icon, name, memory, controls)
│   │   │   ├── process_row.rs      # Process/agent/terminal row (dot, name, summary, port)
│   │   │   └── section_header.rs   # Section header (AGENTS 5/5, COMMANDS 2/5, etc.)
│   │   ├── terminal_view.rs        # VTE terminal wrapper widget
│   │   ├── project_detail.rs       # Project overview/settings panel (right side)
│   │   ├── command_palette.rs      # Ctrl+K command palette overlay
│   │   ├── add_command_dialog.rs   # "Add command" modal dialog
│   │   ├── settings/
│   │   │   ├── mod.rs
│   │   │   ├── settings_window.rs  # Settings panel with tabs
│   │   │   ├── appearance.rs       # Theme, font, font weight, terminal preview
│   │   │   ├── notifications.rs    # System notifications, bell sound
│   │   │   ├── sidebar_settings.rs # Filter, sections, headers, footer toggles
│   │   │   ├── hotkeys.rs          # Keyboard shortcut configuration
│   │   │   ├── agents.rs           # Agent tools config, auto-summarization
│   │   │   ├── tools.rs            # Default editor, default terminal
│   │   │   └── integrations.rs     # MCP server, HTTP API toggles
│   │   └── status_bar.rs           # Bottom bar: actions + CPU/MEM/process/status
│   └── util/
│       ├── mod.rs
│       ├── port_detector.rs        # Scan process output for port numbers/URLs
│       └── resource_monitor.rs     # CPU/memory per-process via /proc
├── packaging/
│   ├── flatpak/
│   │   └── com.tuxflow.TuxFlow.yml
│   ├── appimage/
│   │   └── AppImageBuilder.yml
│   └── aur/
│       └── PKGBUILD
└── tests/
    ├── config_test.rs
    ├── process_test.rs
    └── detector_test.rs
```

---

## Rust Crate Dependencies

```toml
[dependencies]
gtk4 = "0.11"
libadwaita = "0.8"
vte4 = "0.9"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
tokio = { version = "1", features = ["full"] }
notify = "7"
notify-debouncer-mini = "0.5"
rmcp = "0.16"
log = "0.4"
env_logger = "0.11"
thiserror = "2"
uuid = { version = "1", features = ["v4"] }
glob = "0.3"
dirs = "6"
nix = { version = "0.29", features = ["signal", "process"] }
```

---

## UI Layout (Distinct from SoloTerm)

SoloTerm uses a light theme with a macOS-native feel and a simple left sidebar. TuxFlow will differentiate with:

1. **Dark-first design** using libadwaita's dark preference with a custom color palette (deep blues/purples, Tokyo Night-inspired)
2. **Native GTK4/Adwaita widgets** — follows Linux desktop conventions (headerbar, GtkListBox, AdwPreferencesWindow)
3. **Flap-based sidebar** — AdwFlap for responsive collapsible sidebar instead of fixed width
4. **Integrated headerbar** — title, project name, running status, and action buttons in the headerbar (no separate top bar)

```
┌──────────────────────────────────────────────────────────┐
│  [☰]  TuxFlow — my-project    ● 2/5 Running    [⚙] [+] │  ← HeaderBar
├────────────┬─────────────────────────────────────────────┤
│ PROJECTS   │                                             │
│            │  Terminal output for selected               │
│ ▼ my-proj  │  process/agent/terminal                     │
│   ⚙ AGENTS │                                             │
│   ● Claude │  $ npm run dev                              │
│   ● Codex  │  VITE v7.3.1 ready in 2286ms               │
│            │                                             │
│   □ TERMS  │  → Local: http://localhost:5174/            │
│   ○ Term 1 │  → Network: use --host to expose            │
│            │                                             │
│   ◈ CMDS   │                                             │
│   ● npm:dev│  LARAVEL v12.52.0 plugin v2.1.0             │
│   ● Logs   │                                             │
│   ○ Pint   │                                             │
│   ○ Queue  │                                             │
│            │                                             │
│ ▸ other-pr │                                             │
│            │                                             │
├────────────┴─────────────────────────────────────────────┤
│ ◉ Focus  ⏸ Pause  ⌧ Clear  ■ Stop  ↻ Restart    │ CPU…│  ← StatusBar
└──────────────────────────────────────────────────────────┘
```

### Key UI Components

**Sidebar (`sidebar/`):**
- Project list using `GtkListBox` with expandable rows
- Three collapsible sections per project: AGENTS, TERMINALS, COMMANDS (with running/total counts)
- Each process row: colored status dot (green/gray/red/yellow), name, optional auto-summary text, optional port number, optional memory usage
- Right-click context menus on process rows (Stop, Restart, Clear output, etc.)
- Right-click context menus on section headers (Add agent ▸, Add terminal, Add command)
- Drag-and-drop reordering within sections
- Project-level controls on hover: play (start auto-start), restart all, stop all

**Terminal View (`terminal_view.rs`):**
- VTE4 widget wrapped in a custom GTK widget
- Full ANSI color, true color, mouse events, ligatures
- Resize handling (VTE handles this natively)
- Search within terminal output (Ctrl+F)
- Port/URL detection in output — clickable links

**Command Palette (`command_palette.rs`):**
- Centered overlay triggered by Ctrl+K
- Text input with fuzzy filtering
- Categorized results: PROJECT (new terminal, new agent, add process), NAVIGATION (switch to process), ACTIONS (start all, stop all)
- Keyboard navigation (arrows + enter)

**Project Detail View (`project_detail.rs`):**
- Shown when clicking project name in header
- Overview: directory path, config file status (valid/invalid), running/total commands
- Settings: auto-start toggle, editor override, icon customization, notifications
- Commands list: each command shows name, actual command string, status (Running/Stopped/Exited), tags (AUTO, TOML), uptime
- Add command button

**Add Command Dialog (`add_command_dialog.rs`):**
- AdwDialog modal
- Fields: name, command, working directory (with Browse), auto-start toggle, auto-restart toggle, file watching globs
- Save location: "Save to tuxflow.toml" vs "Store locally only"

**Settings Window (`settings/settings_window.rs`):**
- AdwPreferencesWindow with tabs/pages:
  - **Appearance**: theme (light/dark/system), interface font scale, terminal font family, font weight, bold weight, terminal font scale, line height, letter spacing, terminal preview
  - **Notifications**: system notifications toggle, test notification, bell sound selection
  - **Sidebar**: filter input toggle, hide empty sections, project headers (CPU/memory thresholds), process headers (CPU/memory thresholds), show settings footer
  - **Hotkeys**: searchable list of all keyboard shortcuts, organized by category (General, Navigation, Projects, Processes, Terminal), click to edit, reset to defaults
  - **Agents**: agent tool list with enable/disable toggles, add/edit/remove agents, auto-summarization config (summarizer tool, model, command preview, test button)
  - **Tools**: default editor, default terminal
  - **Integrations**: MCP server toggle + port + exposed tools list, HTTP API toggle + port + endpoints list

**Status Bar (`status_bar.rs`):**
- Left side: action buttons (Focus, Pause, Clear, Stop, Restart) — context-sensitive to selected process
- Right side: CPU %, MEM, process name, running status dot

---

## Config Format: `tuxflow.toml`

```toml
[project]
name = "my-app"
icon = "public/favicon.svg"

[[process]]
name = "npm:dev"
command = "npm run dev"
auto_start = true
auto_restart = false
restart_when_changed = []
env = {}

[[process]]
name = "Logs"
command = "tail -f storage/logs/laravel.log"
auto_start = true

[[process]]
name = "Queue"
command = "php artisan queue:work"
working_dir = "."
auto_start = false
auto_restart = true
restart_when_changed = ["app/Jobs/**/*.php"]
```

**Local-only config** stored at `~/.config/tuxflow/projects/<project-hash>/local.toml` for processes the user doesn't want in version control.

**Global settings** stored at `~/.config/tuxflow/settings.toml` for appearance, hotkeys, agent tools, integrations.

---

## Core Feature Implementation

### 1. Process Manager (`process/manager.rs`)

Central orchestrator holding `HashMap<String, ManagedProcess>` in `Arc<Mutex<>>`.

```
ManagedProcess {
    id: Uuid,
    name: String,
    config: ProcessConfig,
    status: ProcessStatus (Running | Stopped | Crashed | Restarting),
    vte_terminal: vte4::Terminal,
    pid: Option<i32>,
    restart_count: u32,
    started_at: Option<Instant>,
    output_buffer: VecDeque<String>,  // Ring buffer for MCP log access
}
```

Key operations:
- `spawn(config)` — Create VTE terminal, spawn command via `vte_terminal.spawn_async()`
- `kill(id)` — Send SIGTERM, wait 5s, SIGKILL if needed (via `nix` crate)
- `restart(id)` — Kill then spawn with configurable delay
- `write(id, data)` — Feed input to VTE terminal
- `list()` — All processes with status summary

### 2. Auto-Restart (`process/auto_restart.rs`)

- Monitor VTE `child-exited` signal
- If `auto_restart` is true and exit code != 0, restart with exponential backoff
- Max restart attempts configurable (default 5)
- Emit notification on crash

### 3. Stack Detector (`detect/detector.rs`)

Scan project directory for known files:

| File | Stack | Suggested Command |
|------|-------|-------------------|
| `package.json` with `scripts.dev` | Node.js | `npm run dev` |
| `Cargo.toml` | Rust | `cargo run` |
| `manage.py` | Django | `python manage.py runserver` |
| `go.mod` | Go | `go run .` |
| `composer.json` | PHP/Laravel | `php artisan serve` |
| `docker-compose.yml` | Docker | `docker compose up` |

Returns suggestions for the user to confirm before writing to `tuxflow.toml`.

### 4. File Watcher (`watcher/file_watcher.rs`)

- Uses `notify` crate v7 with debouncing
- For each process with `restart_when_changed` globs:
  - Watch parent directories
  - On change event, match against globs, filter ignores
  - Trigger process restart via ProcessManager
  - Emit status bar notification

### 5. Port/URL Detection (`util/port_detector.rs`)

- Regex scan VTE terminal output for patterns like `localhost:PORT`, `http://...`, `0.0.0.0:PORT`
- Store detected ports/URLs per process
- Show port number in sidebar next to process name (like SoloTerm shows "5174")
- Make URLs clickable (open in default browser via `xdg-open`)

### 6. Resource Monitor (`util/resource_monitor.rs`)

- Read `/proc/<pid>/stat` and `/proc/<pid>/statm` for CPU% and RSS memory
- Poll on interval (every 2s)
- Show per-process and per-project aggregate in sidebar
- Configurable thresholds for when to display (e.g., only show CPU when > 30%)

### 7. MCP Server (`mcp/server.rs`)

Using `rmcp` crate, expose via Unix domain socket at `/tmp/tuxflow-<project>.sock`:

**Tools:**
| Tool | Description | Parameters |
|------|-------------|------------|
| `list_processes` | List all processes with status | — |
| `get_process_logs` | Recent output from a process | `name`, `lines` |
| `restart_process` | Restart a process | `name` |
| `stop_process` | Stop a process | `name` |
| `start_process` | Start a stopped process | `name` |
| `get_process_status` | Status of a specific process | `name` |
| `get_file_changes` | Recent file change events | `since_seconds` |
| `get_project_info` | Detected stacks and config | — |

**Resources:**
| URI | Description |
|-----|-------------|
| `tuxflow://processes` | JSON list of all processes |
| `tuxflow://logs/{name}` | Log output for a process |
| `tuxflow://config` | Current project config |

### 8. Auto-Summarization

- Configurable summarizer agent (default: Claude with haiku model)
- When a terminal/agent becomes idle, pipe last N lines of output to the summarizer
- Display one-line summary below the process name in the sidebar
- Uses the agent CLI tool with a prompt flag — runs as a background process

### 9. Keyboard Shortcuts

Mapped to Linux conventions (Ctrl instead of Cmd):

**General:**
| Key | Action |
|-----|--------|
| `Ctrl+K` | Command palette |
| `Ctrl+P` | Quick actions |
| `Ctrl+E` | Quick jump |
| `Ctrl+T` | New agent/terminal |
| `Ctrl+,` | Settings |
| `Ctrl+F` | Terminal search |

**Navigation:**
| Key | Action |
|-----|--------|
| `Ctrl+←` | Focus sidebar |
| `Ctrl+↵` | Focus terminal |
| `Ctrl+[` | Go back |
| `Ctrl+]` | Go forward |
| `Ctrl+1..9` | Switch to process N |
| `Alt+1..9` | Switch to project N |

**Projects (sidebar focused):**
| Key | Action |
|-----|--------|
| `S` | Start auto-start |
| `A` | Start all |
| `P` | Stop all |
| `R` | Restart running |

**Processes (sidebar focused):**
| Key | Action |
|-----|--------|
| `C` | Clear output |
| `S` | Start/stop |
| `P` | Pause/resume follow |
| `R` | Restart |

**Terminal:**
| Key | Action |
|-----|--------|
| `Ctrl+↑` | Previous process |
| `Ctrl+↓` | Next process |
| `Ctrl+=` | Increase font size |
| `Ctrl+-` | Decrease font size |

All shortcuts configurable via Settings > Hotkeys and stored in `settings.toml`.

---

## Implementation Phases

### Phase 1: Foundation — Window with a working terminal
1. Initialize Rust project with Cargo workspace
2. Set up GTK4 + libadwaita application (`TuxFlowApp`, `TuxFlowWindow`)
3. Create basic window with AdwFlap (sidebar + content)
4. Embed a VTE4 terminal in the content area, spawn a shell
5. Verify terminal works: input, output, colors, resize

**Result:** A GTK4 window with a functional VTE terminal.

### Phase 2: Process management + sidebar
1. Implement `config/schema.rs` and `config/loader.rs` for `tuxflow.toml`
2. Implement `process/manager.rs` — spawn/kill/restart via VTE
3. Implement sidebar: project list, expandable sections, process rows with status dots
4. Wire up: load config → spawn processes → show in sidebar → click to switch terminal
5. Implement `process/auto_restart.rs`
6. Implement status bar with action buttons

**Result:** Multi-process management from tuxflow.toml, sidebar navigation.

### Phase 3: Stack detection + file watching
1. Implement `detect/detector.rs` with file-pattern scanning
2. Implement `watcher/file_watcher.rs` with notify + glob matching
3. Add "auto-detect" flow on first run (no tuxflow.toml found)
4. Add file change notifications in UI

**Result:** Auto-detection and auto-restart on code changes.

### Phase 4: Command palette + keyboard UX
1. Implement `command_palette.rs` — overlay, fuzzy search, categories
2. Implement keyboard shortcut registry with configurable bindings
3. Implement right-click context menus on sidebar items
4. Implement Add Command dialog
5. Implement project detail view

**Result:** Full keyboard-driven workflow.

### Phase 5: Settings + theming
1. Implement AdwPreferencesWindow with all tabs
2. Implement theme system (dark/light/system via libadwaita)
3. Implement terminal appearance settings (font, weight, size, line height, spacing)
4. Implement terminal preview in settings
5. Port/URL detection + clickable links
6. Resource monitoring (CPU/memory from /proc)
7. Desktop notifications via libnotify

**Result:** Polished, configurable application.

### Phase 6: MCP server + agent integration
1. Implement MCP server with rmcp on Unix socket
2. Implement all MCP tools and resources
3. Implement agent management UI (add/configure agent tools)
4. Implement auto-summarization feature
5. Test with Claude Code connecting to TuxFlow's MCP server

**Result:** AI agents can observe and control TuxFlow processes.

### Phase 7: Packaging + distribution
1. Configure AppImage build
2. Create Flatpak manifest
3. Create .deb and .rpm packaging configs
4. Set up GitHub Actions CI/CD
5. Test on Ubuntu, Fedora, Arch
6. Write README

**Result:** Installable on all major Linux distros.

---

## Verification

### Build & Run
```bash
# Install system deps (Ubuntu 24.04)
sudo apt install libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev build-essential

# Build
cargo build

# Run
cargo run

# Run with a project dir
cargo run -- /path/to/project
```

### Test
```bash
cargo test
```

### Verify terminal works
- Launch app, verify shell spawns in VTE
- Type commands, verify output renders correctly (colors, cursor, scroll)
- Resize window, verify terminal reflows

### Verify process management
- Create `tuxflow.toml` with 2-3 processes
- Launch app, verify auto-start processes begin
- Click sidebar items, verify terminal switches
- Stop/restart processes via sidebar and command palette
- Kill a process externally, verify auto-restart triggers

### Verify MCP
- Configure Claude Code to connect to TuxFlow's Unix socket
- Ask Claude to "list processes" and "read logs from npm:dev"
- Verify Claude receives correct data

### Verify packaging
- Build AppImage, test on clean Ubuntu VM
- Build Flatpak, test install via flatpak
- Build .deb, test install on Debian/Ubuntu
