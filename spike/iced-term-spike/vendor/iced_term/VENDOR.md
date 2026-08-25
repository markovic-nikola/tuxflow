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
