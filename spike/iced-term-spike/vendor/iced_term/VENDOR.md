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
   - The focused match lands in `RenderableContent.search_match`
     (absolute grid coordinates, like the cells); the view highlights it
     with the selection's fg/bg swap. `Action::SearchResult(bool)` tells
     the embedder whether anything was found.
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
