# iced_term spike — can iced replace GTK4 + VTE?

De-risking spike for the GNOME-stack exit explored on 2026-08-25. The framework
question is downstream of one hard question: **can we get a VTE-quality
terminal outside GTK?** This crate answers it empirically for the leading
candidate stack: iced 0.14 + iced_term 0.8 (alacritty_terminal 0.25 underneath —
the same engine Zed and cosmic-term ship on).

```bash
cargo test --test probe   # headless VTE-parity probes (no display needed)
cargo run                 # multi-terminal demo window (manual checklist below)
```

`vendor/iced_term/` is a vendored fork of iced_term 0.8.0 (MIT); the diff
against upstream is 6 lines — see `vendor/iced_term/VENDOR.md`.

## Scorecard vs the VTE API surface TuxFlow uses

Every VTE call site in TuxFlow was inventoried first; each row below is one of
those needs.

### Proven by headless tests (tests/probe.rs — 8/8 passing)

| TuxFlow need (VTE API today) | Status | Evidence |
|---|---|---|
| PTY spawn (`spawn_async`) | ✅ | `pty_echo_and_grid_scrape` |
| Scrape displayed text for port/URL detection (`contents-changed` + `text_range_format`) | ✅ needs 6-line fork | same test + `Scrape` button |
| **OSC 52 copy from agents/tmux** (VTE: **impossible**, 8-years-open upstream refusal) | ✅ **decoded event, clipboard vs PRIMARY target included** | `osc52_clipboard_store_event` |
| Child exit code for crash detection (`child-exited`) | ✅ `ChildExit(i32)` | `child_exit_code_events` |
| Window title (`window-title-changed`) | ✅ | `title_change_event` |
| Type into terminal — composer bar, remote bridges (`feed_child`) | ✅ `Write` | `write_to_pty_feed_child` |
| Mouse reporting to tmux (VTE internal) | ✅ mode tracked, SGR encoding round-trips through the PTY | `mouse_mode_tracking_and_sgr_report` |
| Ctrl+click URLs (`match_add_regex`) | ✅ regex hit-test at grid point; Ctrl+click→open is the default binding | `url_regex_hover_match` |
| Resize/reflow (`column_count` wrap logic) | ✅ | `resize_and_wrap` |

### Confirmed by code inspection

- **Copy/paste bindings** — Ctrl+Shift+C/V default on Linux, same as TuxFlow's VTE setup.
- **Selection** — drag/word/line selection implemented in the widget; copy writes the Standard clipboard.
- **PRIMARY selection** — iced 0.14 has `clipboard::write_primary`/`read_primary` tasks; the demo app routes OSC 52 `Selection`-type copies there. Auto-publishing *drag selections* to PRIMARY (VTE does this) needs a small fork addition — the plumbing exists.
- **Exit caveat** — a signal-killed child yields `Exit` without a code (VTE reports a waitpid status). Auto-restart logic would treat "no code" as abnormal exit; workable.

### Known gaps (= the real migration work list)

1. **Scrollback search UI/API** (VTE `search_set_regex`/`find_next`) — alacritty ships the regex engine (it powers the URL test above); the search overlay and iteration API must be built. Bounded, not researchy.
2. **IME** — zero `Ime` handling in the widget (grep-verified). Dead keys/CJK/compose input in terminals is broken until built. iced 0.14 itself has IME support (text_input uses it), so this is widget work, not framework work.
3. **Image clipboard** — iced's clipboard tasks are text-only. TuxFlow's paste-PNG-to-remote path needs raw clipboard access (e.g. `window_clipboard`/arboard directly). GTK wins here today.
4. **Perf smell** — the backend clones the entire `Grid` (incl. scrollback) on every sync. Fine for a spike, wants fixing for 10+ live terminals (Zed/cosmic-term render without cloning).
5. **Scrollback length config, cursor styles, bold-is-bright etc.** — alacritty `term::Config` is used at `Default`; wiring TuxFlow's terminal_theme settings through is mechanical.
6. **Accessibility** — iced has none yet (roadmap). GTK regression, not terminal-specific.

## Manual checklist (needs a real display — `cargo run`)

The window: pane-grid of terminals (│ ─ split, ✕ close), a composer input
(Enter sends to the focused terminal — feed_child parity), a Scrape button
(dumps the focused grid to stdout, shows the badge-candidate URL in the status
bar), and a status bar showing the focused terminal's `TermMode` plus the last
3 events (OSC 52 / exits / bell).

- [ ] Rendering: fonts, emoji, box-drawing glyphs (run `php artisan dev` — the
      `@laravel/multiplex` borders were the wrap-rejoin pain point), themes.
- [ ] `ssh <vps>` + `tmux`: mouse click/scroll reaches tmux (status bar should
      show `MOUSE_REPORT_CLICK | SGR_MOUSE` once tmux has `mouse on`), pane
      resize by drag, alt-screen apps (htop).
- [ ] **The headline**: inside remote tmux (`set-clipboard on`), have an agent
      copy something (or `printf '\e]52;c;%s\a' "$(printf %s hi | base64)"`).
      Status bar should log `OSC 52 copy → clipboard`, and Ctrl+V elsewhere
      should paste it. This is the entire tmux clipboard bridge (three gates,
      buffer-age probe, EventControllerLegacy) made unnecessary.
- [ ] Selection: drag/double/triple-click select, Ctrl+Shift+C, paste in
      another app. Shift+drag inside tmux for widget-side selection.
- [ ] Composer: type + Enter → lands in the focused terminal.
- [ ] Scrape while a dev server runs → status bar shows the URL the port
      badge would latch.
- [ ] Perf: split to 6–8 panes, `yes` / `cat` a big file in several at once;
      compare feel against VTE.
- [ ] Wayland details: fractional scaling crispness, window resize behavior.
- [ ] Expect broken: dead keys/IME in the terminal, image paste.

## Early verdict

Everything TuxFlow *programmatically* needs from VTE either works (8/8 probes)
or is bounded widget work (search UI, IME, PRIMARY-on-selection, image
clipboard). Nothing looks researchy. OSC 52 alone deletes the gnarliest
workaround in the codebase. The open questions are experiential — rendering
quality, input feel under tmux, perf with many panes — which is exactly what
the manual checklist covers.
