# TuxFlow

A Linux-native dev environment manager. Manage dev servers, AI coding agents, and terminals from a single window.

Built with Rust, GTK4, and libadwaita for a native Linux desktop experience.

## Features

- **Process management** — Start, stop, restart dev servers and background tasks from one place
- **AI agent support** — Run Claude Code, Codex, Gemini CLI, and other AI agents side-by-side
- **Multi-project workspace** — Open multiple projects in one window with expandable sidebar sections
- **SSH connections** — Connect to remote hosts from `~/.ssh/config`, managed like any other process
- **Embedded terminals** — Full VTE4 terminals with ANSI color, true color, and mouse support
- **Auto-restart** — Crashed processes restart automatically with exponential backoff
- **File watching** — Restart processes when source files change (glob patterns)
- **Stack detection** — Auto-detects your tech stack (Node.js, Rust, Go, Python, PHP, Docker) and suggests commands
- **Port/URL detection** — Detects ports and URLs in terminal output, Ctrl+click to open in browser
- **Resource monitoring** — Per-process CPU and memory usage via `/proc`
- **MCP server** — Exposes tools over Unix socket so AI agents can observe and control processes
- **Command palette** — Ctrl+K fuzzy search for quick actions
- **Terminal search** — Ctrl+F to search terminal output
- **Drag-and-drop** — Reorder processes in the sidebar
- **Keyboard-driven** — Fully configurable keyboard shortcuts
- **Desktop notifications** — Get notified when processes crash or restart
- **TOML config** — Simple, human-readable project configuration
- **Theming** — Dark/light/system theme with accent colors and terminal color schemes

## Requirements

- Linux (Ubuntu 24.04+, Fedora 39+, Arch, openSUSE Tumbleweed)
- GTK4 (>= 4.12)
- libadwaita (>= 1.4)
- VTE4 (vte-2.91-gtk4)

## Installation

### Build from source

```bash
# Install system dependencies (Ubuntu/Debian)
sudo apt install libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev build-essential

# Install system dependencies (Fedora)
sudo dnf install gtk4-devel libadwaita-devel vte291-gtk4-devel gcc

# Install system dependencies (Arch)
sudo pacman -S gtk4 libadwaita vte4

# Clone and build
git clone https://github.com/markovic-nikola/tuxflow.git
cd tuxflow
cargo build --release

# Run
./target/release/tuxflow
```

## Quick Start

```bash
tuxflow                   # opens in current directory
tuxflow /path/to/project  # or specify a project path
```

TuxFlow auto-detects your tech stack and suggests commands. Add processes, agents, and terminals through the GUI with `Ctrl+P`.

## Configuration

Processes added through the GUI are saved to `~/.config/tuxflow/projects.toml`. Global settings are managed through the Settings window (`Ctrl+,`) and stored at `~/.config/tuxflow/settings.toml`.

Optionally, you can create a `tuxflow.toml` in your project root for version-controlled config:

```toml
[project]
name = "my-app"

[[process]]
name = "dev"
command = "npm run dev"
start_with_project = true
auto_restart = true
```

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+P` | Add project or process |
| `Ctrl+T` | New terminal |
| `Ctrl+,` | Settings |
| `Ctrl+F` | Filter processes |
| `Ctrl+Shift+F` | Terminal search |
| `Ctrl+Up/Down` | Previous/Next process |
| `Ctrl+Shift+Up/Down` | Previous/Next project |
| `Ctrl+O` | Quick jump |
| `Ctrl+W` | Close agent/terminal |
| `Ctrl+Left/Right` | Focus sidebar/terminal |
| `Ctrl+\` | Toggle sidebar |
| `Ctrl+Alt+S` | Start/Stop process |
| `Ctrl+Alt+R` | Restart process |
| `Ctrl+Alt+C` | Clear output |
| `Ctrl+=/-` | Increase/Decrease font size |
| `Ctrl+Shift+C/V` | Copy/Paste |

All shortcuts are configurable via Settings > Hotkeys.

## MCP Server

TuxFlow exposes an MCP server over a Unix socket at `/tmp/tuxflow-<project>.sock`. AI agents can use it to:

- List and monitor running processes
- Read terminal output / logs
- Start, stop, and restart processes

To connect from Claude Code, add to your MCP config:

```json
{
  "mcpServers": {
    "tuxflow": {
      "command": "tuxflow-mcp",
      "args": ["/path/to/project"]
    }
  }
}
```

## License

[MIT](LICENSE-MIT)
