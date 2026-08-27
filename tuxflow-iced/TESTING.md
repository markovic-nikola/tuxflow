# TuxFlow Preview — test drive guide

The iced shell, feature-complete for daily remote work. It shares
`~/.config/tuxflow/{projects,settings}.toml` with the GTK app — same
workspace, same custom commands, same keybindings; changes made in either
shell land in the other. Running both at once is fine (saves are atomic),
just don't edit the *same* process in both simultaneously.

## Run it

```bash
cd ~/Projects/tuxflow
git pull
make iced                                  # release build + run (debug misrepresents latency)
./target/release/tuxflow-iced ssh://host/dir   # add a project from the CLI
```

`make dev` still runs the GTK app; `make dev-iced` is the live-reload
equivalent for hacking on the iced shell.

From the next tagged release the .deb ships it as **TuxFlow Preview** in
the app grid (`/usr/bin/tuxflow-iced`).

## Shortcuts

Your settings.toml strings apply; defaults shown.

| Chord | Action |
|---|---|
| Ctrl+Shift+F | search scrollback (Enter next, Shift+Enter previous, Esc close) |
| Ctrl+Shift+P | command palette |
| Ctrl+F | filter the sidebar (projects & processes; Esc unfocuses, Esc again closes) |
| Ctrl+\ | hide / show the sidebar |
| Ctrl+Up / Ctrl+Down | previous / next process |
| Ctrl+Shift+Up / Ctrl+Shift+Down | previous / next project |
| Ctrl+T | new terminal in the active project |
| Ctrl+Shift+W | close the selected terminal / stop the process |
| Ctrl+= / Ctrl+- | font size (applies to every terminal) |
| Alt+Shift+Up / Alt+Shift+Down | move the selected process up / down (persists order) |
| Ctrl+Shift+C / Ctrl+Shift+V | copy / paste (remote: tmux buffer bridge / image upload) |
| Ctrl+V | remote agent terminals: paste text / bridge an image (GTK parity — a raw ^V would read the host's clipboard); elsewhere literal ^V |
| Alt+Enter | ESC+CR to the terminal — newline in Claude Code |

## What to exercise (the parity walkthrough)

Everything on this list is implemented and was verified headlessly against
an sshd sandbox — the point of your pass is *feel*: latency, focus, muscle
memory, rendering under your real workload.

- **Remote projects**: probe on open, tmux reattach of live sessions,
  start/stop/restart, detach on quit (sessions survive), pulled-cable
  reconnect ("connection lost", endless retry, no crash).
- **Ports**: badge on the sidebar row, auto-tunnel (remap on collision),
  Ctrl+click a printed URL opens through the rewritten forward, ↗ open
  chip in the toolbar and status bar, one-shot auto-open if enabled.
- **Clipboard**: drag-select publishes to PRIMARY, middle-click pastes,
  Ctrl+Shift+C fetches the newest tmux buffer (incl. agent OSC 52),
  image paste uploads and types the remote path.
- **Composer** under agent terminals: type locally, Enter/send delivers.
- **Lifecycle**: crash → auto-restart backoff (1s→32s, gives up at 5,
  60 s of stability resets), notifications per your settings flags.
- **Workspace**: recently-used project order, add/edit/delete process
  (working directory field included), add project, SSH section rows,
  git chip in the status bar (branch ↑ahead ↓behind ±changed, 20 s
  refresh, over ssh for remote projects).
- **Header cluster** at the top of the sidebar: sidebar toggle, sidebar
  filter, settings, add project — same Adwaita symbolic icons as GTK,
  tooltips with your chords, toggled buttons keep their wash. Hiding the
  sidebar collapses it to a slim icon rail (everything stays clickable);
  filtering from the rail reopens the sidebar.
- **Sidebar lifecycle controls** (GTK's hover buttons, design round F —
  bare glyphs that slide in): hover a process row for play
  (stopped/crashed), restart + stop (running), or cancel
  (restarting/reconnecting); hover a project header for start-all-marked
  / restart-all-running / stop-all — the ⌃N hint and the counter pill
  step aside while the glyphs are in. A row with a detected URL grows a
  standing ↗ that opens it through the tunnel map — any row, not just
  the selected one.
- **Right-click menus** (the GTK popovers): a project header offers
  Start / Stop / Restart All, New Terminal (the card's old "+ terminal"
  pill lives here now), Open in Editor, Copy Path, and Remove
  Project behind the confirmation card; a process row offers
  Start / Stop, Restart, Resume Session (agents), Open in Browser (when
  a URL is live), Edit / Copy Command, and Delete Command (also
  confirmed). The header's ✕ is gone — project removal lives here now,
  like GTK. Esc or a click elsewhere closes.
- **Settings** (gear in the header bar, or Ctrl+,): full GTK-parity
  port — all seven pages, every change saves to the shared settings.toml
  immediately. Live-applies here: terminal theme + font family / size /
  weight / line height (running terminals restyle in place), sidebar
  accents, keybindings (click a chip, press the combo — conflicts
  refused), notification flags + sounds, composer toggle, single-expand,
  recent-first, keybind hints (Ctrl+1-9 / Alt+1-9 switchers now work
  here too). Rows whose consumer only exists in the GTK shell say so in
  their subtitle. Window geometry (size/position/maximized) persists —
  debounced, so even a cargo-watch kill keeps the last position.

## Not there yet (deliberate, GTK still covers these)

- Drag-and-drop reorder (use Alt+Shift+Up/Down).
- Local-only trio: file watcher restarts, MCP socket, update chip.
- Voice bridge for remote agents.
- Menu stragglers: Edit Project (dialog not ported), Clear Output /
  Redraw Terminal (no backend command; redraw was a VTE workaround).

## Reporting

Anything that feels off — a paste that didn't land, a badge on the wrong
port, a stutter — note the project, the process, and what the terminal
showed. `RUST_LOG=info ./target/release/tuxflow-iced 2>tuxflow-iced.log`
captures the app's own account.
