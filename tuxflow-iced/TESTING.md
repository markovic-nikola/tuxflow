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
  glyph on the row and chip in the status bar, one-shot auto-open if
  enabled.
- **Clipboard**: drag-select publishes to PRIMARY, middle-click pastes,
  Ctrl+Shift+C fetches the newest tmux buffer (incl. agent OSC 52),
  image paste uploads and types the remote path.
- **Composer** under agent terminals: type locally, Enter/send delivers.
- **Lifecycle**: crash → auto-restart backoff (1s→32s, gives up at 5,
  60 s of stability resets), notifications per your settings flags.
- **Output survives the run** (GTK's one-VTE-per-process): a command that
  finishes, crashes or is stopped leaves everything it printed on screen —
  the status is the sidebar dot and the exit banner. A bad exit gets the same
  `[tuxflow] …` line GTK feeds (exit 127 names the host and the command),
  and starting it again appends under a dim rule rather than on a blank
  pane, so a crash loop reads as a stack of runs. Worth pushing on: a TUI
  that dies without cleaning up (the respawn leaves the alt screen for
  you), and scrollback/search/copy on a process that is no longer running.
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
- **Bare terminal pane**: no strip above the terminal. The window title
  bar names what you are looking at — `TuxFlow - {project}: {process}`,
  where the process half is its OSC title while a program sets one (an
  agent's current task) and its configured name otherwise.
- **Right-click menus** (the GTK popovers): a project header offers
  Start / Stop / Restart All, New Terminal (the card's old "+ terminal"
  pill lives here now), New Command / New Agent (the pane toolbar's old
  pills), Open in Editor, Copy Path, and Remove
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
- **The bottom bar** (ported to GTK's layout): on the left, the remote
  glyph for ssh projects (hover for `host:dir`), this project's
  `running/total`, and `Total r/t` across every open project — hover that
  one to see which processes are alive where. On the right, two git chips
  and the action buttons.
  - The **sync chip** shows `⎇ branch` with `↓N` amber (to pull) and
    `↑N` green (to push). One click fetches, fast-forward pulls, then
    pushes; the counters hide while it runs. A diverged history can't
    fast-forward, so it fails on purpose — you should get a card with
    git's own explanation, not a silent no-op.
  - The **changes chip** shows `+N −M` against HEAD (hover for the exact
    numbers and the untracked count, which line counts can't include).
    Click it for the **Git Changes** view.
  - **Focus** hides the sidebar (same as Ctrl+\). **Clear** empties the
    selected terminal — scrollback included — WITHOUT touching the
    process, so try it on something mid-output and confirm the program
    keeps printing. **Stop** only appears while the selection is running;
    **Restart** is always there.
- **Git Changes** (the changes chip): file list with `M`/`A`/`D`/`R`/`U`
  badges, syntax-highlighted diff per file, a commit box (Commit stages
  everything, like GTK), and Push / Pull. It refreshes itself every 2 s
  and re-fetches every ~30 s, so edits in your editor show up without a
  click. Esc closes it — twice if the commit box has focus, since the
  first Esc unfocuses the field. Worth poking at: an untracked file
  (diffed against /dev/null, so it reads as all additions), a big diff
  (capped at 5000 lines / 256 KB), and switching project while it's open
  (it should close rather than show the wrong repo).

## Not there yet (deliberate, GTK still covers these)

- Drag-and-drop reorder (use Alt+Shift+Up/Down).
- Local-only trio: file watcher restarts, MCP socket, update chip — the
  update chip is why the bottom bar still has no "Update available".
- Voice bridge for remote agents.
- Menu stragglers: Edit Project (dialog not ported), Redraw Terminal (a
  VTE workaround with nothing to port).

## Reporting

Anything that feels off — a paste that didn't land, a badge on the wrong
port, a stutter — note the project, the process, and what the terminal
showed. `RUST_LOG=info ./target/release/tuxflow-iced 2>tuxflow-iced.log`
captures the app's own account.
