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
