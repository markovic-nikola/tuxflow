# TuxFlow

A Linux-native dev environment manager. Manage dev servers, AI coding agents, and terminals from a single window.

Built with Rust, GTK4, and libadwaita for a native Linux desktop experience.

## Features

- **Process management** — Start, stop, restart dev servers and background tasks from one place
- **AI agent support** — Run Claude Code, Codex, Gemini CLI, and other AI agents side-by-side
- **Embedded terminals** — Full VTE4 terminals with ANSI color, true color, and mouse support
- **Auto-restart** — Crashed processes restart automatically with exponential backoff
- **File watching** — Restart processes when source files change (glob patterns)
- **Stack detection** — Auto-detects your tech stack (Node.js, Rust, Go, Python, PHP, Docker) and suggests commands
- **Port/URL detection** — Detects ports and URLs in terminal output, Ctrl+click to open in browser
- **Resource monitoring** — Per-process CPU and memory usage via `/proc`
- **MCP server** — Exposes tools over Unix socket so AI agents can observe and control processes
- **Command palette** — Ctrl+K fuzzy search for quick actions
- **Keyboard-driven** — Fully configurable keyboard shortcuts
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
git clone https://github.com/nicholasgasior/tuxflow.git
cd tuxflow
cargo build --release

# Run
./target/release/tuxflow
```

## Quick Start

1. Navigate to your project directory
2. Create a `tuxflow.toml`:

```toml
[project]
name = "my-app"

[[process]]
name = "npm:dev"
command = "npm run dev"
auto_start = true

[[process]]
name = "Logs"
command = "tail -f storage/logs/laravel.log"
auto_start = true

[[process]]
name = "Queue"
command = "php artisan queue:work"
auto_start = false
auto_restart = true
restart_when_changed = ["app/Jobs/**/*.php"]
```

3. Run TuxFlow:

```bash
tuxflow              # auto-detects tuxflow.toml in current dir
tuxflow /path/to/project  # or specify project path
```

If no `tuxflow.toml` exists, TuxFlow auto-detects your tech stack and suggests commands.

## Configuration

### Project config (`tuxflow.toml`)

```toml
[project]
name = "my-app"
# icon = "public/favicon.svg"

[[process]]
name = "npm:dev"
command = "npm run dev"
auto_start = true
auto_restart = false
# category = "command"  # command (default), agent, terminal
# working_dir = "."
# restart_when_changed = ["src/**/*.ts"]
# env = { NODE_ENV = "development" }
```

### Global settings

Settings are stored at `~/.config/tuxflow/settings.toml` and managed through the Settings window (Ctrl+,).

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+K` | Command palette |
| `Ctrl+T` | New agent/terminal |
| `Ctrl+,` | Settings |
| `Ctrl+F` | Terminal search |
| `Ctrl+1..9` | Switch to process N |
| `Alt+1..9` | Switch to project N |

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
