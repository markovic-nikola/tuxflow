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

- **Copy/paste bindings** — Ctrl+Shift+C/V default on Linux, same as TuxFlow's VTE setup. Paste respects bracketed-paste mode (fork patch 7; upstream wrote raw bytes).
- **Selection** — drag/word/line selection implemented in the widget; copy extracts via alacritty's `selection_to_string` (multi-line copies keep their newlines — fork patch 7).
- **PRIMARY selection** — ~~needs a small fork addition~~ done (fork patch 7): a finished selection gesture surfaces as `Action::PublishSelection` → `clipboard::write_primary`, middle-click pastes PRIMARY, and OSC 52 `Selection`-type copies route there too. Pinned by the `selection_release_publishes_text` probe.
- **Exit caveat** — a signal-killed child yields `Exit` without a code (VTE reports a waitpid status). Auto-restart logic would treat "no code" as abnormal exit; workable.
- **TERM is the embedder's job** — found live when `top` couldn't enter the alternate screen: nothing in the stack sets `TERM` for the PTY children (VTE does this for you). The demo app now calls `alacritty_terminal::tty::setup_env()` like Alacritty's `main()` does.

### Known gaps (= the real migration work list)

1. ~~**Scrollback search UI/API**~~ — done (fork patch 8): `SearchNext`/`SearchClear` commands over alacritty's `RegexSearch`, wrap-around stepping, scroll-to-match, focused-match highlight in the view. The demo binds Ctrl+Shift+F to a search bar (incremental as you type, Enter/▲ = older, ▼ = newer, Esc closes). Pinned by the `scrollback_search_scrolls_to_match_and_wraps` probe.
2. **IME (preedit only)** — zero `Ime` handling in the widget (grep-verified). Narrower than first scored: dead-key composition **works** (verified live — `ü` composes and renders; winit handles xkb compose itself). What's missing is preedit-style IME (CJK input methods). iced 0.14 itself has IME support (text_input uses it), so this is widget work, not framework work.
3. **Image clipboard** — iced's clipboard tasks are text-only (confirmed live: Ctrl+Shift+V with an image does nothing). ~~GTK wins here today~~ — demo-proven app-level work, not a framework wall: the widget leaves Ctrl+Shift+V *unconsumed* when there's no text, so the demo's `keyboard::listen` hook catches it and goes to arboard for the raw image → PNG in temp → path typed into the terminal (TuxFlow's paste-PNG story, local half). `png_encode_roundtrip` unit test + `examples/clipboard_image_check.rs` (arboard set→get byte-identical under Xvfb) pin the pieces; the hotkey fallthrough with an image-only clipboard still wants a check on a real session.
4. **Perf** — ~~the backend clones the entire `Grid` (incl. scrollback) on every sync~~ hit for real during manual testing, in three escalating rounds, **all fixed in the fork** (VENDOR.md patches 3, 5, 6): viewport-only snapshots + event-classified syncs (scroll freeze); unbounded event channel + coalescing (a genuine three-way deadlock — alacritty emits events while the PTY thread holds the terminal lock); run-merged text drawing + lock-free event handling + contention-skipping sync (typing latency under floods). Verified: 45 s of two endless `yes` panes on a software renderer — zero stalls, zero syncs >10 ms, zero draws >20 ms.
5. ~~**Scrollback length config etc.**~~ — mostly done (fork patch 9): `BackendSettings` plumbs `scrolling_history`, `semantic_escape_chars`, `kitty_keyboard` and OSC 52 policy into `term::Config` (probes pin the first two end-to-end). Still view-side, not config: cursor *styles* (beam/underline drawing) and bold-is-bright color mapping.
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
      pass. Middle-click PRIMARY gap confirmed as scored — then closed after
      the walkthrough (fork patch 7): selections auto-publish to PRIMARY,
      middle-click pastes it.
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
- [x] Image paste — failed as scored during the walkthrough; the arboard
      fallback added after (gaps list, item 3) **passed the real-session
      re-check** (2026-08-26): PNG path typed into the terminal.

Post-walkthrough re-check (2026-08-26) of the newly closed gaps:

- [x] PRIMARY: drag-select → middle-click paste in another pane — pass.
- [x] Image paste — pass (above).
- [ ] Ctrl+Shift+F search — **failed**, root-caused and fixed (fork patch
      10): the default bindings map the whole Ctrl+Shift alphabet to
      control characters, so the widget typed ^F and captured the event
      before the hotkey listener saw it. The demo now overrides the chord
      with `BindingAction::Passthrough`. Needs one more interactive try.

## Verdict

**The iced path is viable.** All 13 headless probes pass; the full manual
walkthrough passes. Of the original gap list, scrollback search, PRIMARY
auto-publish, config plumbing and image-clipboard access have since been
closed in the fork/demo (patches 7–9 + the arboard fallback). What remains:
preedit IME (widget work; iced itself has it), cursor-style/bold-is-bright
rendering, and iced's missing accessibility story — the one framework-level
regression.

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
