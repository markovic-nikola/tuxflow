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
