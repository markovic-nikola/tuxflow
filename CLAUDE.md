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
    mic.rs                 # MicBridgeManager: ssh -R socket + fake arecord, so remote agents get voice input
    ports.rs               # What a remote run actually listens on (tmux pane → process tree → ss)
    tunnel.rs              # TunnelManager: ssh -L port forwards for detected ports on remote projects
    vite.rs                # Reads laravel-vite-plugin's public/hot — fallback for hosts without tmux
  watcher/
    file_watcher.rs        # notify + glob matching, triggers process restart
  mcp/
    server.rs              # MCP server on Unix socket (/tmp/tuxflow-<project>.sock)
    tools.rs               # MCP tools: list_processes, get_process_logs, start/stop/restart
    bridge.rs              # GTK<->MCP thread bridge, LogBuffer (VecDeque ring buffer)
  ui/
    window.rs              # Main AdwApplicationWindow — central wiring hub
    accent.rs              # Accent theming: app accent + local/remote sidebar accents
    terminal_theme.rs      # Terminal color schemes
    terminal_view.rs       # VTE terminal wrapper
    terminal_search.rs     # Ctrl+F search overlay
    command_palette.rs     # Command palette (Ctrl+Shift+P)
    composer_bar.rs        # Local message composer under agent terminals (beats ssh typing lag)
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
- **Accents (ui/accent.rs):** one palette feeds three pickers — the app accent (`accent_bg_color`/`accent_fg_color`/`accent_color`) plus `local_accent` and `remote_accent`, the sidebar hues that say where a project lives (green / logo gold by default). The sidebar takes an entry's *text-weight* `accent`, never its button `bg`, and swaps to `accent_light` on light surfaces, since these are read as text and thin borders — that scheme-dependence is the whole reason the colors are generated in Rust at USER priority instead of living in style.css. style.css keeps only the dark values as a pre-`apply` fallback, and they must stay at the top of the file: GTK resolves `@color` references at parse time, so a use above the `@define-color` silently drops the declaration. Local rules are the base ones and `.project-remote` overrides follow them — remote wins over "running" on purpose. Unknown names (hand-edited settings) fall back to the shipped defaults rather than leaving the color undefined, which would drop every rule using it. The same generator carries `status_working`/`status_restarting`: fixed meanings, but both ambers measure ~2.2:1 on the light sidebar (below the 3:1 a dot owes) so they need a dark twin. Contrast is unit-tested against both schemes — 4:1 for the accents (read as text), 3:1 for the dots. The constants there are the *stricter* #ebebeb/#303030, not the #fafafa/#222226 the app actually paints, so the assertions keep a margin
- **URL handling:** VTE regex matching + Ctrl+click opens via `xdg-open`. Sidebar/status bar browser buttons use `gtk4::UriLauncher`
- **Port/URL detection (util/port_detector.rs):** badge priority is labeled "Preview URL:" (preferred, locks even though remote — `shopify app dev`'s admin URL beats its localhost GraphiQL) > local > provisional remote (OAuth links, upgradeable). Infra/error lines never badge ("proxy server started", "econnrefused", vite tool lines) but their ports still harvest into `seen_local` for tunnelling. Tool lines do have a *fallback* tier below `local`: a dev server's own address line (`➜  Local:  http://localhost:5173/`, matched by shape in `is_arrowed_label` — vite picks `➜` or `→` by font, so literal phrases made the badge depend on a glyph) badges when nothing else claims one, marked `tool: true` so `badge_final` stays false and the app's port still takes over when it prints. Without it a project whose API dies (bun on EADDRINUSE beside vite) shows no port, no URL and never auto-opens, since every consumer in window.rs hangs off the badge. Only *address* lines get the fallback, never the rest of the tool class — auto-opening `shopify app dev`'s proxy port is the bug that tier must not reintroduce. A stricter class (`FOREIGN_PORT_PHRASES`) is skipped *entirely*, harvesting included, because the port named belongs to someone else: vite's "Port 5173 is in use, trying another one" and — since `php artisan serve` walks `--tries` ports and echoes PHP's bind failure verbatim — "Failed to listen on 127.0.0.1:8000 (reason: Address already in use)". Both announce the retry and the port they retried onto, so dropping the line loses nothing. Match the bare phrase (`in use`), not each server's sentence around it — bun's "error: Failed to start server. Is port 3000 in use?" puts the port *inside* what used to be matched as "is in use", and bun then exits rather than retrying, so no later line corrects a badge that latched the neighbour's port. Getting this wrong is not a cosmetic off-by-one on a remote project: 8000 there is a *neighbouring project's* server, so the badge locks to it, tunnels to it (remapped, since local 8000 is already forwarded elsewhere) and the browser button silently opens the wrong app under this project's name. Match on the phrase, not the reason — the multiplex TUI clips the line to pane width, so "Address already…" is elided while "Failed to listen" survives on the same row as the port. Auto-open (one-shot per user start) fires when the badge is *final*, or after a 5 s grace for provisional-only badges — never on first-URL-seen. Ink-based CLIs (shopify) hard-wrap output at width−1 with a real newline, which even `tmux capture-pane -J` can't rejoin — remote scans go through `scan_output_wrapped`, which re-joins rows of width/width−1 before parsing (local VTE joins its own soft wraps natively). A row ending on a box-drawing glyph is exempt: full-screen TUIs (`php artisan dev` → `@laravel/multiplex`) border the pane, so *every* row measures full width and joining them fuses the whole screen into one logical line — where one "Failed to listen" swallows the port announced rows below and the badge comes back empty. Width alone cannot tell a frame edge from a wrap; Ink's wraps break mid-token and never land on a border. `examples/port_scan_check.rs` replays a `tmux capture-pane` dump through this path when a badge misbehaves. Custom commands in projects.toml override same-named detected processes at load (a user's edit of a detected process persists there)
- **`TUXFLOW_CHILD=1`** env var is set in window.rs and inherited by child processes (used to prevent recursive spawning)
- **Updates:** the GitHub check runs once per launch (15-min cache in `~/.cache/tuxflow/update-check.json`, storing the latest version unfiltered so the same cache re-evaluates correctly against a changed running version). The status-bar chip has two states behind a single click handler (`UpdateBadge` in status_bar.rs — re-connecting `clicked` per state would stack handlers and open two dialogs): "Update available" opens release notes + `pkexec apt-get install` of the .deb; "Restart to finish updating" goes straight to the restart prompt. The second state comes from a 30 s poll of `/proc/self/exe` — dpkg renames the new binary over ours, so the link gains a `" (deleted)"` suffix, and that is the *only* signal that the system's software manager upgraded us underneath a running window (the process keeps running the old code quite happily). Release builds only: `cargo run` replaces its own binary on every rebuild and would light the chip permanently. Restart hands off to a detached `sh` that waits for our PID to exit, since the app is single-instance and a second launch just activates the old window
- **Remote projects:** a project's `ProjectLocation` is `Local(PathBuf)` or `Ssh { host, dir }`, persisted in projects.toml as an opaque `ssh://host/path` key (no schema migration). Remote processes spawn as `exec ssh -t <mux> host '<wrap>'` in the normal VTE pipeline; all ssh invocations share a ControlMaster connection (`$XDG_RUNTIME_DIR/tuxflow/ssh-%C`). The wrap runs the command inside a tmux session on a dedicated server (`tmux -L tuxflow`, status off, mouse on, deterministic `tf-<proc>-<fnv32>` session names from project key + process name), so connection loss and app quit only *detach* — reconnect and app relaunch reattach via `new-session -A`; the command's exit code round-trips through `/tmp/.<session>-<uid>.exit` (uid-namespaced — deterministic session names would otherwise collide across users on a shared host) so crash detection still works. Hosts without tmux fall back inline to direct exec (no persistence; `pkill -s` on the pidfile PID covers the kill). Stop interrupts the tmux session (C-c) and waits up to ~2 s for it to end before killing it, then sweeps the pidfile login session, and flags the next spawn to kill-before-create (`remote_fresh_next` — avoids racing the fire-and-forget remote kill). The grace exists so a program gets to run its own cleanup: Vite removes the `public/hot` file naming its dev server, and an orphaned one leaves the app serving asset URLs pointing at a dead port long after the run that wrote it. Everything after the initial lookup targets tmux's unique *session id*, never the name — names are deterministic and reused, so a restart respawns the same name inside the grace window and a name-targeted wait would end by killing the session it just started. Agents treat C-c as "interrupt" rather than "quit" and are expected to fall through to the kill; app quit calls `detach_all()` instead, so remote processes deliberately outlive TuxFlow. Exit 255 on a remote process is surfaced as "connection lost — reconnecting", not a crash; reconnects retry forever (backoff capped at 32 s, one notification per outage) since the session is still alive on the host. Project load probes `tmux list-sessions` and auto-reattaches processes whose sessions are still alive, so the UI never shows "stopped" for a running detached process. Mouse selections inside tmux reach the local clipboard via TuxFlow's own bridge (selection gesture on a remote terminal → newest tmux paste buffer over ssh → GTK CLIPBOARD *and* PRIMARY, change-detected by hash) — NO released VTE implements OSC 52 (verified through 0.84), so the standard escape-sequence route is a dead end. The hard problem is not fetching the buffer but knowing whether it is *ours*: `show-buffer` always answers, and answers with the newest buffer on that server, which may be the drag that just finished or an OSC 52 an agent sent half an hour ago that nothing has displaced since. Publishing the second as if it were the first is what silently reverted the user's Ctrl+C to scrollback nobody had selected — it reads as "Ctrl+C sometimes doesn't work". Three gates decide, and dropping any one brings the bug back in a different disguise: the gesture must be able to *make* a selection (`SelectionGesture` in window.rs — a drag, or a double/triple click for tmux's word/line copy; firing on every button release meant a plain click to focus a pane republished a buffer from minutes earlier), the buffer must be younger than 5 s (`fetch_tmux_buffer` returns `#{buffer_created}` next to the host's own `date`, both read on the host so a skewed local clock can't fake freshness — the window is loose on purpose, since a false negative silently drops the user's copy while the race it guards needs a gesture that selected nothing within 5 s of a program's copy), and the hash must differ from the last publish. GTK4 dropped the multi-press event types and GtkGestureClick is unusable here (VTE claims mouse sequences; a claimed sequence cancels other gestures, so `released` never fires — hence `EventControllerLegacy`), so click sequences are recognised by GTK's own time/distance rule. Ctrl+Shift+C is the explicit route and skips the age gate: on a remote pane VTE has no selection of its own (tmux owns the mouse, except under Shift), so it takes the newest buffer whatever its age — which is also the only way a copy-mode `y` (no mouse gesture to hang off) or an **agent's** OSC 52 copy is collected. `set-clipboard` stays `on` for that last reason — agents copy by emitting OSC 52 and reporting success, and with `off` tmux discards it (verified against 3.3a), so the agent's "copied!" is a lie. The cost is tmux re-emitting OSC 52 outward to every attached client, which VTE ignores but a kitty/foot attached to the same (deterministic) session name would honour; that is the tolerable half, staleness is not. The session-exit replay of `capture-pane -e` strips OSC 52 in its awk pass — replayed bytes are parsed exactly like live output, so a clipboard-set sequence in the scrollback would be re-executed on exit. The reverse direction too: pasting (Ctrl+Shift+V) while the clipboard holds an image uploads the PNG to the host (`remote::upload_image` → `/tmp/.tuxflow-img-*.png`) and types the path into the terminal — agents treat image paths as attachments. Voice input for remote agents works the same way as the clipboard shim (remote/mic.rs, opt-in via Settings → Tools → Agents): Claude Code records by spawning `arecord -f S16_LE -r 16000 -c 1 -t raw -q -` and reading raw PCM off stdout, never checking that the binary is real, so a fake `arecord` in `~/.local/bin` reads the same stream off `~/.cache/tuxflow/mic.sock`, which `ssh -R` forwards to a local `UnixListener` that answers each connection with a real recorder. Hold-to-talk needs no key-release reporting (VTE has none — kitty-protocol support is still unmerged): it counts 5 space *repeats* arriving <120 ms apart, and auto-repeat is just bytes, so it survives VTE + ssh + tmux untouched. `~` is not expanded by sshd in an `-R` listen path, so provisioning resolves `$HOME` in the same round trip that installs the shim; it also `rm`s a stale socket first, because the host's `StreamLocalBindUnlink` default leaves one behind and sshd then refuses to bind ("remote port forwarding failed"). Bridges are keyed by host (projects share one) and torn down on quit — remote processes deliberately outlive TuxFlow, but a live microphone must not. Detected ports auto-tunnel via `ssh -N -L` (remote/tunnel.rs) — every local port seen in output, remapping to a free local port on collision (badge/URL show the local port); tunnels use dedicated connections (NOT the mux — a forward requested via a mux client survives in the master after the client dies) with PDEATHSIG so they can't outlive the app. Output scanning only sees what a process *prints*, which TUI runners break outright: Laravel 13's `php artisan dev` draws `@laravel/multiplex`, a tabbed TUI rendering only the selected tab — a run parked on `vite` never shows its server URL, one parked on `server` never shows Vite's. The missing port is not wrapped or truncated, it is absent, so no amount of parsing recovers it. `remote/ports.rs` asks the host instead: tmux session → `list-panes -F '#{pane_pid}'` → descendants → `ss -ltnpH`, one round trip for all of a project's sessions, joined in Rust (testable) rather than in shell. Walk by **ppid, not session id** — multiplex starts each child under its own `setsid`, so `pgrep -s <pane_pid>` returns multiplex and nothing beneath it. The reply labels each session by *index*, never name: a name must be shell-quoted to reach tmux safely and those quotes are literal inside the echo, so a name-labelled reply comes back wearing them and matches nothing. Everything found forwards **1:1** (`ensure_exact`, not `ensure`) — a remote dev server hands the browser its own address (Vite bakes its port into `public/hot`), so a remapped forward listens where nothing knocks; a taken local port is a hard failure, not a remap. The poll resets to 2 s whenever it opens a new forward and backs off to 30 s once a run has settled; forwards drop when a project's last process stops, so the next run rediscovers rather than inheriting stale ports. `remote/vite.rs` (laravel-vite-plugin's `public/hot`) remains the fallback for hosts without tmux, where the inline exec leaves no pane to walk down from. Scanning still picks the badge URL — this only decides what gets tunnelled. Note `public/hot` is removed by the plugin only on *graceful* shutdown, which is why stop interrupts before killing. Git UI works remotely (`git_changes_dialog::git_command` routes through ssh; `.git` probe runs on a worker thread). Editor button uses `code --remote ssh-remote+host` for code-family editors (util/editor.rs). Add Remote dialog autocompletes remote paths (debounced `ls -1dp` over ssh). Still gated off for remote: file watcher, icon detection, MCP, live re-detection in Edit Project. Remote probing (config/detection/git) runs on worker threads via `util::worker::run` — never block the GTK main thread with ssh, and never poll a channel from `idle_add_local` (it busy-spins the main loop). Startup loads of unreachable remote projects notify once and retry in the background with capped backoff (`ProbeError::Unreachable` vs `Invalid` decides retryability)

## Config Files

- **Project config:** `tuxflow.toml` in project root (optional, version-controlled)
- **Global settings:** `~/.config/tuxflow/settings.toml` (appearance, keybindings, notifications)
- **Project state:** `~/.config/tuxflow/projects.toml` (open directories, custom commands, process order, UI state)

Every map serialized into these files is a `BTreeMap`, never a `HashMap` — `HashMap` iteration order is randomised per process, so each save rewrote the whole file in a fresh order and a one-key change surfaced as a whole-file diff (these files get tracked in dotfiles repos). `Vec` fields like `directories` and `process` keep user-defined order. `tests/projects_roundtrip_test.rs` pins the invariant.

## Formatting

Always run `cargo fmt --all` after changing Rust code, before committing.

## TODO

- **File upload to remote terminals** — Extend the image-paste bridge to arbitrary files: Ctrl+Shift+V with a file on the clipboard (`text/uri-list`) uploads over the existing ssh connection and types the remote path, like images do today (`remote::upload_image`). Plus a `GtkDropTarget` on the terminal view so drag-and-drop works too — local projects get path-paste-on-drop natively from VTE; remote should match
- **Split terminal view** — Currently `gtk4::Stack` (one at a time). Would need `gtk4::Paned` for side-by-side
- **Composer inline images** — Composer delivers attachments first, then text. Nicer: `insert_paintable` thumbnails at the cursor, send walks the buffer and interleaves text chunks with Ctrl+V per image so `[Image #N]` lands where it was placed. Replaces the chip row; needs thumbnail scaling, upload-in-flight gating on send, identity-based (not order) texture→path matching
- **Tests** — Core modules covered (config, detector, port detector, log buffer). UI and process management untested (require GTK runtime)
- **CI** — `.github/workflows/ci.yml` (fmt/clippy/test on push + PR) and `release.yml` (on `v*` tags: builds the .deb + tarball, publishes the GitHub Release, force-pushes the signed apt repo to gh-pages). `scripts/release.sh` only bumps, tags and pushes — the artifacts come from the tag, so they land a few minutes later; a release checked too early looks half-published when it is just mid-run
