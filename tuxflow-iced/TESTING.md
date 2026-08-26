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
| Ctrl+Up / Ctrl+Down | previous / next process |
| Ctrl+Shift+Up / Ctrl+Shift+Down | previous / next project |
| Ctrl+T | new terminal in the active project |
| Ctrl+Shift+W | close the selected terminal / stop the process |
| Ctrl+= / Ctrl+- | font size (applies to every terminal) |
| Alt+Shift+Up / Alt+Shift+Down | move the selected process up / down (persists order) |
| Ctrl+Shift+C / Ctrl+Shift+V | copy / paste (remote: tmux buffer bridge / image upload) |

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

## Not there yet (deliberate, GTK still covers these)

- Settings window — edit `settings.toml` by hand or via the GTK app;
  the iced shell re-reads it at launch.
- Drag-and-drop reorder (use Alt+Shift+Up/Down).
- Local-only trio: file watcher restarts, MCP socket, update chip.
- Voice bridge for remote agents.

## Reporting

Anything that feels off — a paste that didn't land, a badge on the wrong
port, a stutter — note the project, the process, and what the terminal
showed. `RUST_LOG=info ./target/release/tuxflow-iced 2>tuxflow-iced.log`
captures the app's own account.
