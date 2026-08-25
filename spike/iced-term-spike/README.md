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
- **TERM is the embedder's job** — found live when `top` couldn't enter the alternate screen: nothing in the stack sets `TERM` for the PTY children (VTE does this for you). The demo app now calls `alacritty_terminal::tty::setup_env()` like Alacritty's `main()` does.

### Known gaps (= the real migration work list)

1. **Scrollback search UI/API** (VTE `search_set_regex`/`find_next`) — alacritty ships the regex engine (it powers the URL test above); the search overlay and iteration API must be built. Bounded, not researchy.
2. **IME (preedit only)** — zero `Ime` handling in the widget (grep-verified). Narrower than first scored: dead-key composition **works** (verified live — `ü` composes and renders; winit handles xkb compose itself). What's missing is preedit-style IME (CJK input methods). iced 0.14 itself has IME support (text_input uses it), so this is widget work, not framework work.
3. **Image clipboard** — iced's clipboard tasks are text-only (confirmed live: Ctrl+Shift+V with an image does nothing). TuxFlow's paste-PNG-to-remote path needs raw clipboard access (e.g. `window_clipboard`/arboard directly). GTK wins here today.
4. **Perf** — ~~the backend clones the entire `Grid` (incl. scrollback) on every sync~~ hit for real during manual testing, in three escalating rounds, **all fixed in the fork** (VENDOR.md patches 3, 5, 6): viewport-only snapshots + event-classified syncs (scroll freeze); unbounded event channel + coalescing (a genuine three-way deadlock — alacritty emits events while the PTY thread holds the terminal lock); run-merged text drawing + lock-free event handling + contention-skipping sync (typing latency under floods). Verified: 45 s of two endless `yes` panes on a software renderer — zero stalls, zero syncs >10 ms, zero draws >20 ms.
5. **Scrollback length config, cursor styles, bold-is-bright etc.** — alacritty `term::Config` is used at `Default`; wiring TuxFlow's terminal_theme settings through is mechanical.
6. **Accessibility** — iced has none yet (roadmap). GTK regression, not terminal-specific.

## Manual checklist (needs a real display — `cargo run`)

The window: pane-grid of terminals (│ ─ split, ✕ close), a composer input
(Enter sends to the focused terminal — feed_child parity), a Scrape button
(dumps the focused grid to stdout, shows the badge-candidate URL in the status
bar), and a status bar showing the focused terminal's `TermMode` plus the last
3 events (OSC 52 / exits / bell).

Walked in full on 2026-08-25 (real display, real VPS, real tmux). Every
failure found was diagnosed to root cause and fixed in the fork the same day
(VENDOR.md patches 3–6):

- [x] Rendering: colors, emoji, box-drawing, p10k prompt — pass. (Found and
      fixed: canvas painting outside widget bounds; clip-mask coordinate
      space.)
- [x] Scrollback, alt screen (`less`), Ctrl+C — pass. (Found and fixed:
      full-grid clone per event; `top` "not fullscreen" was a missing TERM —
      embedders must call `tty::setup_env()`; also note `top` never uses the
      alternate screen, so scrollback-during-top is correct in VTE too.)
- [x] Selection: drag/double/triple-click, Ctrl+Shift+C/V, bracketed paste —
      pass. Middle-click PRIMARY gap confirmed as scored.
- [x] Composer → focused terminal — pass.
- [x] **OSC 52, both routes** — pass: plain ssh and tmux-mediated
      (`set-clipboard on`) copies land in the local clipboard, decoded, with
      the status bar logging each. The entire tmux clipboard bridge (three
      gates, buffer-age probe, EventControllerLegacy) is obsolete on this
      stack. Bonus finding: multiplex/tmux emit **empty** OSC 52 clears —
      a production handler must ignore them (the demo now does).
- [x] Mouse → tmux: click, divider drag, wheel → copy-mode, drag-select
      (with OSC 52 sync back!), Shift+drag bypass — pass. (Found and fixed:
      upstream only reported motion for mode 1003, never encoded wheel
      reports, had no Shift bypass.)
- [x] `php artisan dev` multiplex TUI borders + tabs — pass. Scrape found
      the badge URL on the real workload.
- [x] Perf gauntlet (6+ panes, two endless `yes`, `top`, typing) — pass
      after the three-round perf saga (gaps list, item 4).
- [x] Window resize/scaling — pass.
- [x] Dead-key composition (`ü`) — **works** (winit xkb compose).
- [x] Image paste — fails as scored (gaps list, item 3).

## Verdict

**The iced path is viable.** All 9 headless probes pass; the full manual
walkthrough passes. Every VTE capability TuxFlow uses is either working or
bounded, known work: scrollback-search UI, preedit IME, image-clipboard
access, PRIMARY auto-publish, config plumbing — plus iced's missing
accessibility story, the one framework-level regression.

Two findings tip the balance beyond parity:

1. **OSC 52 works end-to-end** — agents' and tmux's copies land in the local
   clipboard as typed events. The most intricate subsystem in TuxFlow's
   remote architecture stops needing to exist.
2. **The stock widget was not production-grade, but its faults were all
   fixable in one day** — a rendering perf rework, a real deadlock, an input
   fidelity pass — because the architecture underneath (alacritty_terminal)
   is sound. The fork in `vendor/` is now substantially ahead of upstream
   iced_term for terminal-heavy use, with 9 probes + 25 unit tests + a
   stress harness (`TUXFLOW_SPIKE_STRESS=1`) pinning the behavior.
