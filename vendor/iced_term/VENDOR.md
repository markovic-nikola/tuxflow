# Vendored iced_term

Upstream: https://github.com/Harzu/iced_term, tag `0.8.0`, commit `e0e90be2c160bd68f1bf544a51bf772d75b98ffc`, MIT (LICENSE preserved).

Copied: `src/` only. `Cargo.toml` rewritten to a standalone package (upstream uses
workspace inheritance); dependency versions identical to upstream.

## Fork diff against upstream

Kept deliberately minimal — every change here is a data point for the spike: it
marks something VTE gives TuxFlow that stock iced_term does not.

1. `src/lib.rs`: `mod backend;` → `pub mod backend;` — the backend (grid,
   renderable content, mouse-mode state) is otherwise completely inaccessible
   to the embedding app.
2. `src/terminal.rs`: added `pub fn backend(&self) -> &backend::Backend` —
   needed for TuxFlow's port/URL detection, which scrapes displayed terminal
   text (VTE equivalent: `text_range_format` + `contents-changed`), and for
   reading `TermMode` (is the app inside tmux/mouse mode?).
3. **Perf rework** (found during manual testing: scrolling a 5k-line history
   pegged a core and froze). Upstream cloned the ENTIRE `Grid` — scrollback
   included — on every event, re-measured the font each time, and cleared the
   canvas cache unconditionally. Changes:
   - `backend.rs`: `RenderableContent.grid: Grid<Cell>` replaced with
     `cells: Vec<Indexed<Cell>>` (visible viewport only, ~50 KB max) plus
     `display_offset`/`cursor_point`; the hovered URL's text is extracted at
     hover time (`hovered_url`) instead of re-walking the grid on open.
   - `terminal.rs`: `handle()` classifies commands — snapshot + cache-clear
     only for content-changing commands (Write/Scroll/Resize/Select/Wakeup/
     Exit); mouse reports do neither; link hover only redraws; font
     re-measure only on `ChangeFont`.
   - `view.rs`: drawing wrapped in `frame.with_clip(layout.bounds(), ..)`
     with widget-relative coordinates — a partial bottom row can no longer
     paint outside the terminal's bounds (visible artifact before).
   Remaining known perf ceiling, deliberately NOT addressed in the spike: the
   canvas rebuild shapes every visible glyph per frame (`Shaping::Advanced`
   per cell). The production pattern is per-line shaping with caches, as in
   cosmic-term/Zed.
4. **Mouse fidelity under tmux** (found live in step 6 of the manual
   checklist: divider drag, wheel, drag-select and Shift+drag all broken).
   `view.rs` input routing:
   - Drag motion now reports when the app requested mode 1002
     (`MOUSE_DRAG`, what tmux uses — motion-while-pressed), not only 1003
     (`MOUSE_MOTION`); upstream checked 1003 alone, so tmux got the press
     but never the drag, and the fallback issued `SelectUpdate` for a
     selection that was never started.
   - The wheel is encoded as wheel reports (buttons 64/65) while
     `MOUSE_MODE` is active; upstream always fell through to `Scroll`,
     which in the alternate screen becomes arrow keys — wheel-up at a
     shell prompt browsed history instead of scrolling tmux copy-mode.
   - Shift bypasses reporting throughout (press/drag/wheel) — the
     standard escape hatch for a widget-side selection under tmux.
   - A drag is routed for its whole lifetime the way it started
     (`drag_is_mouse_report`), so releases only report for report-drags.
5. **Event-flood coalescing** (found live in step 8: two `yes` panes froze
   the UI until the desktop's not-responding watchdog fired). The
   subscription stream forwarded every alacritty event as an app message —
   a flooding PTY generates Wakeups far faster than a sync+redraw cycle can
   drain them, wedging PTY reader → bounded channel → forwarding task → UI
   thread. `terminal.rs`: the stream now drains each burst, forwards
   non-Wakeup events in order and collapses all Wakeups into one trailing
   event, so content syncs at the app's pace (what Zed/Alacritty do by
   syncing per frame). Also fixed upstream's post-exit hot spin: after the
   channel closed, the stream looped on `recv() == None` forever, burning a
   core per exited terminal.

   Coalescing alone was not enough — the hang persisted, and gdb backtraces
   under a stress harness (`TUXFLOW_SPIKE_STRESS=1`) exposed the real root
   cause: **alacritty's `Term` calls `send_event` (MouseCursorDirty,
   PtyWrite, ClipboardStore, ...) while the PTY thread holds the terminal
   lock** (`event_loop.rs` locks, then `parser.advance` emits). With a
   bounded event channel this deadlocks under floods: PTY thread blocks
   sending with the lock held → UI thread blocks on that lock inside
   `sync()` → the forwarding task blocks on the full iced channel because
   the blocked UI thread isn't draining messages. Fix: the proxy channel is
   **unbounded** (`send` never blocks under the lock; the drain loop keeps
   it near-empty), and `MouseCursorDirty`/`CursorBlinkingChange` — emitted
   per scroll during floods — coalesce like Wakeup. Verified: 45 s of two
   endless `yes` panes, zero UI stalls, worst sync 13 ms.
6. **Typing latency under floods** (step 8 re-test: no more hangs, but
   input in idle panes lagged badly while two panes flooded). Three causes,
   three fixes:
   - `view.rs`: text drawing merged into **runs** — contiguous same-style
     ASCII cells become a single `fill_text` with cheap `Basic` shaping
     (thousands of per-cell calls with full Unicode shaping per frame
     become dozens); non-ASCII and the cursor cell keep the per-cell
     `Advanced` path. Prerequisite: `TerminalSize` cell metrics became
     `f32` — they are the font's true advance/line-height, and the old u16
     truncation would walk a merged run off the grid within a few cells.
   - `backend.rs`: `handle()` no longer takes the terminal lock for
     commands that never touch the grid (alacritty events, mouse reports) —
     under floods that lock is busy and every Wakeup message paid a wait.
   - `backend.rs`: `sync()` uses `try_lock_unfair` and returns `false` on
     contention instead of parking the UI thread; `terminal.rs` skips the
     cache clear for skipped syncs (the flood's next Wakeup retries; the
     final Wakeup of a burst always finds the lock free).
   Verified: 45 s of two endless floods on a software renderer — zero
   stalls, zero syncs >10 ms, zero draws >20 ms; screenshot check confirms
   merged runs land glyphs exactly on the cell grid (p10k prompt with mixed
   icons/colors renders pixel-identically).
7. **X11 selection conventions** (the PRIMARY gap scored in the manual
   checklist, step 3). What VTE does internally must surface through the
   widget:
   - New `Command::SelectRelease`, pushed by `view.rs` when a selection
     gesture ends (left release after a drag or double/triple click; a
     release outside the widget bounds now reaches the handler too, so an
     off-widget release neither sticks the drag state nor loses the text).
     The backend extracts the selection and returns
     `Action::PublishSelection(text)` — the embedder routes it to
     `iced::clipboard::write_primary`. Riding the ordered command queue
     means every SelectStart/Update has been applied first; an empty
     (plain-click) selection publishes nothing.
   - Middle-click pastes PRIMARY (or reports middle press/release to the
     app when it owns the mouse, Shift bypassing as everywhere).
   - `selectable_content()` now uses alacritty's `selection_to_string`
     instead of walking viewport cells — the walk glued multi-line copies
     into one line (no newlines), broke on wide-char spacers, and missed
     scrolled-out selection parts.
   - Both paste paths (Ctrl+Shift+V, middle-click) go through
     `paste_content`: bracketed-paste wrapping when the app opted in (with
     the end-marker stripped from the payload — paste injection guard),
     newline→CR normalization otherwise. Upstream wrote clipboard bytes
     raw, so pasting into an app that requested bracketed paste typed the
     content as keystrokes.
8. **Scrollback search** (gap 1 of the README's migration work list — VTE
   `search_set_regex`/`find_next` parity). alacritty ships the engine
   (`RegexSearch`, `Term::search_next` with built-in wrap-around,
   `scroll_to_point`); this patch is the missing plumbing:
   - `Command::SearchNext(pattern, direction)` / `Command::SearchClear`.
     A changed pattern recompiles and restarts from the visible edge
     facing the search direction (so the nearest match wins); a repeated
     one advances past the focused match, `Boundary::None` wrapping at the
     grid edges — the same idiom `search_next` uses internally, so a lone
     match keeps being found. An uncompilable regex (the user mid-typing
     `(`) reports "no match" instead of erroring.
   - Visible matches land in `RenderableContent.search_matches`,
     recomputed from the live grid at every sync (the same
     `visible_regex_match_iter` the URL hover uses; alacritty recomputes
     per frame for the same reason); the view highlights them with the
     selection's fg/bg swap. They are deliberately NOT stored from search
     time: grid lines rotate when new output scrolls in, so a
     coordinate-stored match highlights the line BELOW the text it
     matched — found live as "typed 555, highlighted 556" (a p10k prompt
     redraw after the search was enough). The stored match survives only
     as the next/previous stepping anchor, where a stale origin is
     harmless. `Action::SearchResult(bool)` tells the embedder whether
     anything was found.
   - Search commands classify as content-changing (they scroll and move
     the highlight), so the existing sync/redraw pipeline covers them.
9. **term::Config plumbing** (gap 5 of the migration work list). Upstream
   hardcoded `term::Config::default()`; `BackendSettings` now carries
   `scrolling_history`, `semantic_escape_chars`, `kitty_keyboard` and
   `osc52`, defaulting to alacritty's own `Config::default()` values (one
   source of truth, no copied constants). `Osc52` is re-exported —
   `OnlyCopy` stays the default, which is exactly the agent-workflow
   policy: programs may set the clipboard, never read it.
10. **`BindingAction::Passthrough`** (found live: the demo's Ctrl+Shift+F
   search hotkey never fired — the default bindings map the whole
   Ctrl+Shift alphabet to control characters, so the widget typed ^F into
   the shell and captured the event before `iced::keyboard::listen`, which
   only sees ignored events, could deliver it). Passthrough is a binding
   that consumes nothing: no PTY write, no capture — the chord falls
   through to the application. An embedder reserves its shortcuts by
   overriding the default binding via the existing `AddBindings`
   replacement mechanism (same target+modifiers+modes → replaced in
   place). Any app shortcut on a Ctrl+Shift letter needs this.
11. **Graceful stream teardown** (found designing the process manager's
   stop button, which drops a RUNNING terminal). The subscription stream
   panicked when the event channel closed without an Exit event — but
   that is exactly what dropping a live terminal does: Backend drop sends
   the PTY loop `Msg::Shutdown`, alacritty's `Pty::drop` SIGHUPs and
   reaps the child on the PTY thread, and no Exit is ever emitted. Same
   for the mid-burst `output.send` unwrap when the subscription itself is
   dropped first. Both now end the stream quietly: a closed channel is
   teardown, not a bug — stopping a process must not be able to take
   down the app.
12. **Embedder-controlled link opening** (needed by the migration's M3).
   `LinkAction::Open` returned nothing and the backend launched the URL
   itself via `open::that` (panicking on failure, no less). On a remote
   project that opens the WRONG THING: the printed URL names the host's
   port, which locally is dead or another project's forward — TuxFlow
   must rewrite it through its tunnel map (creating the forward on
   demand) before any browser sees it. `ProcessLink(Open)` now returns
   `Action::OpenUrl(url)` and the embedder decides; the `open` crate
   dependency moved out of the widget accordingly.
13. **Cell-metrics initialization + widget-state carryover** (found live on
   the first real multi-project session: a reattached remote session
   rendered as normal-size glyphs mashed into a tiny corner — the whole
   grid drawn at 1×1-pixel cell positions).
   - `TerminalSize` defaults to 1×1 px cells, and a reattached tmux
     session replays its screen INSTANTLY — synced and drawn before the
     widget's first Resize ever lands. `Terminal::new` now seeds the
     backend with an 80×24 grid at the font's true advance/line-height,
     so nothing can ever render on the 1×1 grid.
   - iced reuses widget state by TYPE at a tree position: showing a
     DIFFERENT terminal in the same slot (a process switcher — exactly
     TuxFlow's main pane) inherited the previous terminal's recorded
     size, and "size unchanged" skipped the new terminal's resize
     forever. `TerminalViewState.sized_for` tracks which terminal the
     size was sent to, not just the size.
   Also logs each terminal's measured cell metrics at INFO — if a
   platform's font resolution ever produces degenerate metrics, the log
   says so immediately.
14. **Keyboard fidelity: Alt+Enter, Alt as ESC-prefix, Ctrl+U** (found
   test-driving Claude Code in the iced shell: Alt+Enter would not insert
   a newline, and readline's Alt+B/F did nothing). Upstream has no
   Alt+Enter binding and the named-key path has no text fallback, so the
   chord wrote NOTHING to the PTY; Alt+<letter> fell through to the bare
   character, losing the ESC prefix every other terminal sends (xterm
   metaSendsEscape, the VTE and alacritty defaults). Alt+Enter now sends
   `\x1b\r`, and the unmatched-text fallback inserts `\x1b` when Alt is
   held — single-byte input only, so AltGr-composed characters pass
   unmangled. Also fixed Ctrl+U (both the CTRL and SHIFT+CTRL tables)
   sending 0x51 ('Q') instead of 0x15 (NAK, vt100) — an upstream
   transposition that broke shell kill-line.

   A later sweep of the same table (2026-09-01) caught three more upstream
   misencodings of the same family: Ctrl+' sent 0x1c — the FS byte that
   belongs to **Ctrl+\\**, so pressing quote-with-Ctrl typed the SIGQUIT
   character and killed the foreground job (the binding moved onto `\`
   where it belongs); Alt+Insert sent `\x1b[3;2~`, which is Shift+Delete
   (Insert is `2~`, Alt is modifier 3 → `\x1b[2;3~`); and Ctrl+F1..F4 sent
   the malformed `\x1bO;5P`… instead of xterm's `\x1b[1;5P`… — a form no
   terminal app parses.
15. **Empty clipboard read must not capture the paste chord** (found live:
   Ctrl+V pasted text but did nothing with an image on the clipboard).
   iced's X11 clipboard backend (clipboard_x11) maps the owner's
   conversion refusal — SelectionNotify with property=None, which is
   exactly what an image-only clipboard owner answers to a UTF8_STRING
   request — to `Ok("")`, not an error (verified empirically under Xvfb
   with an image-only owner). The Paste arm therefore saw `Some("")`,
   wrote an empty bracketed paste and CAPTURED the event, so it never
   fell through to the embedder — whose image-paste bridge hangs off
   precisely that fall-through (same mechanism as `Passthrough`). Paste
   (and the middle-click PRIMARY paste) now treat an empty read as
   "nothing to paste": no write, no capture.
16. **A terminal outlives its child: `shutdown` / `respawn` / `feed`** (found
   live: a command that finished left the pane reading "not running" —
   everything it had printed was gone). Upstream ties the grid's lifetime to
   the PTY session: the ONLY way to kill a child is to drop the whole
   `Terminal`, which takes the scrollback with it, and the only way to run
   something again is to build a new one, starting blank. TuxFlow's GTK shell
   keeps one VTE widget per process for its whole life, so a process's
   terminal is its log ACROSS runs; these three make that possible here.
   - `Backend::shutdown` sends the PTY loop `Msg::Shutdown` — the same
     teardown `Drop` performs (loop exits → drops the PTY → SIGHUP + reap),
     minus dropping the grid. It emits no `Exit` event, so a stop is not
     mistaken for a crash.
   - `Backend::respawn` opens a new PTY on the SAME `Term` and the same event
     proxy, so the existing subscription and widget id keep working — no
     per-run identity churn in the embedder. Order inside is load-bearing:
     tidy (`RESET_BETWEEN_RUNS`), then the caller's banner, then the spawn. A
     banner fed before the tidy would be discarded with the alt screen; one
     fed after the spawn would race the new child through a second parser.
   - `RESET_BETWEEN_RUNS` undoes what a run left the emulator wearing (alt
     screen, mouse reporting, bracketed paste, scroll region, hidden cursor,
     SGR) WITHOUT RIS, which would clear the history the respawn exists to
     keep. DECSTBM homes the cursor, hence the DECSC/DECRC bracket around it.
   - `Backend::feed` advances a parser of the backend's own over the grid, for
     bytes that never came from a child (TuxFlow's exit banners). Safe only
     between runs — two parsers over one grid can interleave mid-sequence.
   - **`drain_on_exit` flipped to `true`** (`DRAIN_ON_EXIT`), which is
     alacritty's `hold`. With it off, the loop breaks out of the child-exit
     arm WITHOUT reading what is still buffered, so a command short enough to
     print and exit within one poll — `echo`, a failing build — lost its
     entire output. Invisible upstream (the grid died with the child anyway)
     and the reason the first respawn test failed: the run before it had
     printed nothing to keep.
   Covered by live-PTY tests in `backend.rs`: a second run keeps the first
   one's output, a run that died inside the alt screen comes back to the
   primary one, and `shutdown` kills the child while the grid survives.

17. **`clear()`: empty the grid without touching the child** (needed by the
    migration's status-bar port — GTK's Clear button is `VteTerminal::reset`).
    - `Backend::clear` / `Terminal::clear` wipe viewport, scrollback and
      cursor position under the `FairMutex` the PTY loop already takes. Patch
      16's `feed` could not serve here: it is documented as safe only BETWEEN
      runs, and Clear's whole point is being pressed at a running process —
      two parsers over one grid interleave mid-sequence.
    - Order is load-bearing the other way round from `respawn`'s. On the
      primary screen alacritty implements `ClearMode::All` as "scroll the
      viewport up into the history" (the xterm behaviour that makes `clear`
      preserve scrollback), so `ClearMode::Saved` has to come AFTER it. Doing
      the history first leaves the screen you just cleared sitting in it.
    - `clear_viewport` does not move the cursor, so `goto(0, 0)` follows, and
      the display is scrolled back to the bottom: someone who hits Clear while
      scrolled up is asking for the empty screen, not their old reading spot.

18. **Run-generation stamps on the event stream** (the attribution hole
    patch 16 opened: a terminal now spans runs, but its event queue doesn't
    know that). A child that dies just as the user clicks restart parks its
    `ChildExit`/`Exit` in the unbounded channel; the embedder's stop+respawn
    runs first, and the stale pair then arrives against the NEW run — which
    flips a healthy process to Crashed, feeds a crash banner into its
    RUNNING grid (the two-parsers hazard `feed` documents), and under
    auto-restart later kills and respawns it for a crash that never
    happened.
    - The channel carries `(run, Event)`; `Command::ProcessAlacrittyEvent`
      exposes the stamp; `Backend::run_generation()` is the current run,
      bumped by `respawn` just before the new PTY opens.
    - The stamp is read at SEND time from an atomic shared by every proxy —
      the loops' AND `Term`'s own, because `Term::exit()` emits the `Exit`
      half through Term's proxy, which lives across runs (a per-instance
      stamp would freeze at its birth value). Queued events therefore wear
      the run that produced them, whatever is current when they're finally
      processed; the embedder drops mismatches.
    - Remaining window, accepted: an old-run send racing the bump itself
      (microseconds, needs the child to die inside the respawn call) —
      against the unbounded queue latency it replaces.
    - The subscription's coalesced repaint events (Wakeup &c.) take the
      newest drained stamp: repaints are idempotent and the one that runs
      paints the current grid regardless.
    Pinned by a live-PTY test (`exit_events_carry_their_runs_generation`).

19. **`TerminalView::unfocus()`** — the modal-grab primitive. Stack layers
    above the base never capture keyboard events, so an embedder raising a
    menu/dialog over a FOCUSED terminal leaks every keystroke into the PTY
    (Esc meant for the menu reaches an agent as "interrupt"). The runtime
    exposes `focusable::focus` as a Task but not `unfocus`; `focus(target)`'s
    own contract is "focus the match, unfocus everything else", so focusing
    an `Id::unique()` that exists nowhere is unfocus-all through the public
    API.

20. **Named keys get the no-binding text fallback** (found live: spaces
    swallowed while typing prose to an agent). Binding lookup is an EXACT
    modifier match and Space is a NAMED key with one bare row — so a space
    struck while Shift was still held (a capital, then the spacebar; fast
    typists release Shift late) matched nothing, and the named-key arm,
    unlike the Character arm, wrote nothing at all. Shell sessions hid it
    for months because commands are lowercase; agent chat is prose. Both
    arms now share `text_fallback` (the key's own text, ESC-prefixed
    single-byte under Alt — the same metaSendsEscape rule patch 14 gave
    characters), so any modified-but-unbound key types what it produced,
    while keys with no text (F-keys, arrows) still write nothing. Bindings
    keep priority, and Ctrl+Space / Ctrl+Shift+Space gained their xterm
    NUL rows so the fallback can't type a plain space where a control code
    belongs.

21. **`Action::ReportedSelectionGesture` + the empty-Copy guard** — the
    copy-on-select hooks. When the pane's application owns the mouse
    (tmux), every press/drag/release leaves as reports and the widget
    selects nothing — but the APP may have just selected plenty, out of
    the widget's sight. The release handler now classifies the finished
    report gesture the way the embedder's GTK twin does: a drag that
    crossed a cell boundary (reports are per cell — less movement cannot
    have begun a selection over there) or a double/triple click (tmux's
    word/line copies) pushes `Command::ReportedSelectionGesture`, which
    bounces off the backend as the matching Action; a click that stayed
    in its cell stays silent, because "publish on any release" is how a
    stale tmux buffer overwrites a clipboard nobody selected into. Click
    kinds ride a separate `last_report_click` so report clicks can't
    chain a widget double-click across a pane switch. The action carries
    no text on purpose — only the embedder can reach where the app keeps
    its selections (the newest tmux paste buffer, over ssh, behind age
    and hash gates). Companion fix: `BindingAction::Copy` no longer
    writes an EMPTY `selectable_content()` to the clipboard — on a
    remote pane the selection the user is looking at is tmux's, so the
    unconditional write clobbered the clipboard with "" on every
    Ctrl+Shift+C there (patch 15's empty-paste lesson, in the other
    direction). Copy still never captures, so the chord falls through
    and the embedder routes it to the tmux buffer when the widget had
    nothing.
