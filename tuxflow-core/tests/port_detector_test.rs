use tuxflow_core::util::port_detector::PortDetector;

#[test]
fn detect_http_url() {
    let mut pd = PortDetector::new();
    pd.scan_output("dev", "Server running at http://localhost:3000/");
    assert_eq!(pd.get_port("dev"), Some(3000));
    assert_eq!(pd.get_url("dev"), Some("http://localhost:3000/"));
}

#[test]
fn detect_https_url() {
    let mut pd = PortDetector::new();
    pd.scan_output("dev", "Listening on https://localhost:8443");
    assert_eq!(pd.get_port("dev"), Some(8443));
}

#[test]
fn detect_localhost_without_scheme() {
    let mut pd = PortDetector::new();
    pd.scan_output("dev", "Listening on localhost:5174");
    assert_eq!(pd.get_port("dev"), Some(5174));
    assert!(pd.get_url("dev").unwrap().contains("5174"));
}

#[test]
fn detect_zero_address() {
    let mut pd = PortDetector::new();
    pd.scan_output("server", "Listening on 0.0.0.0:8080");
    assert_eq!(pd.get_port("server"), Some(8080));
}

#[test]
fn detect_loopback_address() {
    let mut pd = PortDetector::new();
    pd.scan_output("api", "Bound to 127.0.0.1:9090");
    assert_eq!(pd.get_port("api"), Some(9090));
}

#[test]
fn detect_port_keyword() {
    let mut pd = PortDetector::new();
    pd.scan_output("app", "Application started on port 4000");
    assert_eq!(pd.get_port("app"), Some(4000));
}

#[test]
fn no_port_in_output() {
    let mut pd = PortDetector::new();
    pd.scan_output("build", "Build completed successfully in 3.2s");
    assert_eq!(pd.get_port("build"), None);
    assert_eq!(pd.get_url("build"), None);
}

#[test]
fn multiple_processes_tracked() {
    let mut pd = PortDetector::new();
    pd.scan_output("frontend", "http://localhost:3000");
    pd.scan_output("backend", "http://localhost:8000");
    assert_eq!(pd.get_port("frontend"), Some(3000));
    assert_eq!(pd.get_port("backend"), Some(8000));
}

#[test]
fn vite_port_badges_but_stays_upgradeable() {
    // Vite's banner ("VITE v…") and the "→ Local:" line are tool-line markers,
    // so they never *win* the badge over a real app port. But when nothing else
    // claims one they're the only address the user can open, so they badge as a
    // fallback — and stay non-final, so the app port still takes over later.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "vite",
        "  VITE v7.3.1  ready in 2286ms\n\n  → Local:   http://localhost:5174/\n  → Network: use --host to expose",
    );
    assert_eq!(pd.get_port("vite"), Some(5174));
    assert!(
        !pd.badge_final("vite"),
        "a tool port must not lock the badge"
    );

    pd.scan_output("vite", "Server running on http://localhost:8000\n");
    assert_eq!(pd.get_port("vite"), Some(8000));
    assert!(pd.badge_final("vite"));
}

#[test]
fn vite_local_line_badges_whichever_arrow_it_prints() {
    // vite picks `➜` or `→` by font capability, and pads to taste. Matching
    // literal phrases made the badge depend on that glyph — same project, same
    // port, badge or no badge.
    for arrow in ["➜  Local:", "→  Local:", "→ Local:", "->  Local:"] {
        let mut pd = PortDetector::new();
        pd.scan_output("vite", &format!("  {arrow}   http://localhost:5173/\n"));
        assert_eq!(pd.get_port("vite"), Some(5173), "arrow: {arrow:?}");
    }
}

#[test]
fn url_with_path() {
    let mut pd = PortDetector::new();
    pd.scan_output("app", "Running at http://localhost:3000/api/v1");
    assert_eq!(pd.get_port("app"), Some(3000));
}

#[test]
fn url_in_brackets() {
    let mut pd = PortDetector::new();
    pd.scan_output("app", "Server ready [http://localhost:4200]");
    assert_eq!(pd.get_port("app"), Some(4200));
}

#[test]
fn concurrently_vite_plus_php_picks_app_port() {
    let mut pd = PortDetector::new();
    let output = "\
[php] [Thu Apr 30 18:56:19 2026] PHP 8.4.1 Development Server (http://0.0.0.0:8000) started
[vite]
[vite]   VITE v5.4.21  ready in 149 ms
[vite]
[vite]   ➜  Local:   http://localhost:5173/
";
    pd.scan_output("serve", output);
    assert_eq!(pd.get_port("serve"), Some(8000));
}

#[test]
fn concurrently_vite_plus_shopify_picks_local_app_port() {
    let mut pd = PortDetector::new();
    let output = "\
[vite] Port 5173 is in use, trying another one...
[vite] Port 5174 is in use, trying another one...
[vite]   VITE v6.4.1  ready in 489 ms
[vite]   ➜  Local:   http://localhost:5175/
[shopify] ╭─ success ──────────────────────────────────────╮
[shopify] │  Preview your theme (t)                         │
[shopify] │    • http://127.0.0.1:9292                      │
[shopify] │  Next steps                                     │
[shopify] │    • Share your theme preview (p) [1] https://3d-printing-canada.myshopify.com/?preview_theme_id=148443988037
[shopify] ╰────────────────────────────────────────────────╯
";
    pd.scan_output("serve", output);
    assert_eq!(pd.get_port("serve"), Some(9292));
    assert!(pd.get_url("serve").unwrap().contains("127.0.0.1:9292"));
    // The vite asset-server port doesn't win the badge, but a remote project
    // must tunnel it too — the theme proxy loads CSS/JS from it.
    let all = pd.all_local_ports("serve");
    assert!(all.contains(&5175) && all.contains(&9292), "{all:?}");
}

#[test]
fn port_is_sticky_after_buffer_scrolls() {
    let mut pd = PortDetector::new();
    let initial = "\
[php] PHP Development Server (http://0.0.0.0:8000) started
[vite]   VITE v5  ready
[vite]   ➜  Local:   http://localhost:5173/
";
    pd.scan_output("serve", initial);
    assert_eq!(pd.get_port("serve"), Some(8000));

    // Later: PHP startup line has scrolled out, only Vite output remains.
    let later = "\
[vite] [HMR] update applied
[vite]   ➜  Local:   http://localhost:5173/
";
    pd.scan_output("serve", later);
    assert_eq!(pd.get_port("serve"), Some(8000));
}

#[test]
fn plain_vite_output_badges_as_fallback() {
    // Vite alone: no other port will ever arrive, so its own has to badge —
    // otherwise the row shows no port, the status bar no URL, and
    // open-in-browser has nothing to open. (This is the "relax `is_tool_line`
    // if it proves too aggressive" case; a bun API dying on EADDRINUSE next to
    // vite left the project with no badge at all.)
    let mut pd = PortDetector::new();
    let output = "\
  VITE v7.3.1  ready in 2286ms

  → Local:   http://localhost:5174/
  → Network: use --host to expose
";
    pd.scan_output("vite", output);
    assert_eq!(pd.get_port("vite"), Some(5174));
    assert_eq!(pd.get_url("vite"), Some("http://localhost:5174/"));
}

#[test]
fn remote_url_used_only_when_no_local() {
    let mut pd = PortDetector::new();
    pd.scan_output("preview", "Preview at https://app.example.com/path");
    assert_eq!(pd.get_port("preview"), Some(443));
}

#[test]
fn local_beats_remote_in_same_buffer() {
    let mut pd = PortDetector::new();
    let output = "\
Local:  http://127.0.0.1:9292
Remote: https://app.example.com/preview
";
    pd.scan_output("serve", output);
    assert_eq!(pd.get_port("serve"), Some(9292));
}

#[test]
fn bracket_prefix_not_tool_keeps_line() {
    let mut pd = PortDetector::new();
    // "[shopify]" is not a tool prefix, so this line is kept even though
    // the word "Vite" appears in the content.
    pd.scan_output(
        "serve",
        "[shopify] Vite proxy ready at http://127.0.0.1:9292",
    );
    assert_eq!(pd.get_port("serve"), Some(9292));
}

#[test]
fn foreign_in_use_port_not_harvested() {
    // "Port 5173 is in use, trying another one..." names a port owned by
    // some other process (e.g. the neighbouring Laravel project's vite).
    // Harvesting it would open a redundant cross-project tunnel.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "dev",
        "\
[vite] Port 5173 is in use, trying another one...
[vite]   ➜  Local:   http://localhost:5174/
[server]    INFO  Server running on [http://localhost:8001].
",
    );
    let all = pd.all_local_ports("dev");
    assert!(!all.contains(&5173), "{all:?}");
    assert!(all.contains(&5174) && all.contains(&8001), "{all:?}");
}

#[test]
fn laravel_serve_retry_badges_the_port_it_landed_on() {
    // `php artisan serve` walks --tries ports when the default is taken, and
    // echoes PHP's bind failure verbatim before announcing the one it got.
    // Latching the failed port is not a cosmetic slip on a remote project:
    // 8000 there belongs to a *different* project's server, so the badge
    // tunnels to it and opens someone else's app under this project's name.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "dev",
        "\
   Failed to listen on 127.0.0.1:8000 (reason: Address already in use)

   INFO  Server running on [http://127.0.0.1:8001].
",
    );
    assert_eq!(pd.get_port("dev"), Some(8001));
    let all = pd.all_local_ports("dev");
    assert!(
        !all.contains(&8000),
        "must not tunnel a foreign port: {all:?}"
    );
    assert!(all.contains(&8001), "{all:?}");
}

#[test]
fn laravel_multiplex_tui_serve_retry() {
    // The same failure as rendered by `php artisan dev`'s TUI
    // (@laravel/multiplex), captured verbatim from a real run: box-drawing
    // chrome around it, and the text clipped to the pane width — so the
    // reason is elided to "Address already…" and only "Failed to listen"
    // survives to mark the line. It has to, because the port it names is
    // still right there on the same row.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "dev",
        "\
│ 3logs       ││   Failed to listen on 127.0.0.1:8000 (reason: Address already…│
│             ││    INFO  Server running on [http://127.0.0.1:8001].           │
",
    );
    assert_eq!(pd.get_port("dev"), Some(8001));
    let all = pd.all_local_ports("dev");
    assert!(
        !all.contains(&8000),
        "must not tunnel a foreign port: {all:?}"
    );
}

#[test]
fn laravel_serve_failure_line_alone_badges_nothing() {
    // The failure can arrive in its own scan, before the retry has landed —
    // it must not badge in the gap, or the lock makes it permanent.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "dev",
        "   Failed to listen on localhost:8000 (reason: Address already in use)\n",
    );
    assert_eq!(pd.get_port("dev"), None);
    assert!(pd.all_local_ports("dev").is_empty());

    pd.scan_output(
        "dev",
        "   INFO  Server running on [http://localhost:8001].\n",
    );
    assert_eq!(pd.get_port("dev"), Some(8001));
}

#[test]
fn bun_eaddrinuse_port_is_foreign() {
    // `bun server/index.js & cd client && bun run dev`, verbatim from a real
    // remote run: the API failed to bind because 3000 belongs to a *different*
    // project on the same VPS, and bun — unlike vite — doesn't retry, so
    // nothing later corrects a badge that latched 3000. It would tunnel to,
    // and open, the neighbour's app. Only vite's 5173 is really ours here.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "dev",
        "\
  VITE v8.0.8  ready in 4219 ms

  ➜  Local:   http://localhost:5173/
  ➜  Network: use --host to expose

error: Failed to start server. Is port 3000 in use?
 syscall: \"listen\",
   errno: 0,
    code: \"EADDRINUSE\"

      at /home/deployer/Projects/pdf_invoice/server/index.js:125:1
",
    );
    let all = pd.all_local_ports("dev");
    assert!(
        !all.contains(&3000),
        "must not tunnel a foreign port: {all:?}"
    );
    assert!(all.contains(&5173), "{all:?}");
    // …and with the API dead, vite's port is the only thing left to badge,
    // tunnel and open. Dropping 3000 must not leave the project blank.
    assert_eq!(pd.get_port("dev"), Some(5173));
    assert_eq!(pd.get_url("dev"), Some("http://localhost:5173/"));
}

#[test]
fn multiplex_tui_frame_rows_are_not_wrap_fragments() {
    // `php artisan dev`'s TUI borders the pane, so every row measures exactly
    // the terminal width — indistinguishable from an Ink hard wrap by width
    // alone. Joining them fuses the entire screen into one logical line, and
    // there the "Failed to listen" marker swallows the port announced rows
    // below it: badge empty, nothing tunnelled, no auto-open. Shape and
    // widths taken from a real `php artisan dev` run at 84 columns.
    const COLS: usize = 84;
    let row = |content: &str| {
        let inner = COLS - 2;
        format!("│{content:<inner$}│")
    };
    let screen = [
        row(" 3 logs      ││   Failed to listen on 127.0.0.1:8000 (reason: Address already in"),
        row("             ││ use)"),
        row("             ││"),
        row("             ││    INFO  Server running on [http://127.0.0.1:8001]."),
    ]
    .join("\n")
        + "\n";
    for line in screen.lines() {
        assert_eq!(line.chars().count(), COLS, "row not full width: {line}");
    }

    let mut pd = PortDetector::new();
    pd.scan_output_wrapped("dev", &screen, COLS);
    assert_eq!(pd.get_port("dev"), Some(8001));
    let all = pd.all_local_ports("dev");
    assert!(!all.contains(&8000), "{all:?}");
    assert!(all.contains(&8001), "{all:?}");
}

#[test]
fn clear_resets_for_new_run() {
    let mut pd = PortDetector::new();
    pd.scan_output("dev", "Server at http://localhost:3000");
    assert_eq!(pd.get_port("dev"), Some(3000));
    pd.clear("dev");
    assert_eq!(pd.get_port("dev"), None);
    pd.scan_output("dev", "Server at http://localhost:4000");
    assert_eq!(pd.get_port("dev"), Some(4000));
}

#[test]
fn sticky_does_not_overwrite() {
    let mut pd = PortDetector::new();
    pd.scan_output("dev", "Server at http://localhost:3000");
    assert_eq!(pd.get_port("dev"), Some(3000));
    // Even if a later scan finds a different port, the locked one wins.
    pd.scan_output("dev", "Server at http://localhost:9999");
    assert_eq!(pd.get_port("dev"), Some(3000));
}

#[test]
fn remap_url_port_replaces_exact_port_only() {
    use tuxflow_core::util::port_detector::remap_url_port;
    assert_eq!(
        remap_url_port("http://localhost:3000/", 3000, 4123),
        "http://localhost:4123/"
    );
    // Port 80 must not touch the ":8080" substring
    assert_eq!(
        remap_url_port("http://localhost:8080", 80, 9999),
        "http://localhost:8080"
    );
    assert_eq!(
        remap_url_port("http://localhost:80", 80, 9999),
        "http://localhost:9999"
    );
    // Port at end of URL with a path
    assert_eq!(
        remap_url_port("http://127.0.0.1:5173/app", 5173, 5174),
        "http://127.0.0.1:5174/app"
    );
    // No occurrence — unchanged
    assert_eq!(
        remap_url_port("http://localhost:3000", 4000, 5000),
        "http://localhost:3000"
    );
}

#[test]
fn clicked_url_rewrites_through_tunnel_map() {
    use tuxflow_core::util::port_detector::rewrite_clicked_url;
    // Remapped forward: the printed port must become the local end.
    assert_eq!(
        rewrite_clicked_url("http://localhost:8000/login", |p| {
            assert_eq!(p, 8000);
            Some(42345)
        }),
        "http://localhost:42345/login"
    );
    // 1:1 forward: nothing to rewrite.
    assert_eq!(
        rewrite_clicked_url("http://localhost:5173/", |_| Some(5173)),
        "http://localhost:5173/"
    );
    // Tunnel failed to come up: open verbatim rather than not at all.
    assert_eq!(
        rewrite_clicked_url("http://localhost:5173/", |_| None),
        "http://localhost:5173/"
    );
    // All three local spellings route through the map.
    assert_eq!(
        rewrite_clicked_url("http://127.0.0.1:9292", |_| Some(9293)),
        "http://127.0.0.1:9293"
    );
    assert_eq!(
        rewrite_clicked_url("http://0.0.0.0:4000/x", |_| Some(4001)),
        "http://0.0.0.0:4001/x"
    );
    // VTE's bare `localhost:\d+` match form (no scheme).
    assert_eq!(
        rewrite_clicked_url("localhost:3000", |_| Some(3001)),
        "localhost:3001"
    );
}

#[test]
fn clicked_url_leaves_foreign_and_portless_urls_alone() {
    use tuxflow_core::util::port_detector::rewrite_clicked_url;
    // Public URLs mean what they say — the lookup must not even run,
    // or the click would spawn a tunnel to a port nothing local needs.
    assert_eq!(
        rewrite_clicked_url("https://x.trycloudflare.com/admin", |_| panic!(
            "lookup ran for a public URL"
        )),
        "https://x.trycloudflare.com/admin"
    );
    // Implicit :80 has no ":80" text to rewrite — pass through untouched.
    assert_eq!(
        rewrite_clicked_url("http://localhost/status", |_| panic!(
            "lookup ran for a portless URL"
        )),
        "http://localhost/status"
    );
}

#[test]
fn oauth_link_does_not_lock_out_later_local_url() {
    // Regression: `shopify theme dev` prints its login URL before the dev
    // server URL. The public auth link must stay provisional so the real
    // local port can still lock in when it appears seconds later.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "shopify",
        "[shopify] Opened link to start the auth process: \
         https://accounts.shopify.com/activate-with-code?device_code%5Buser_code%5D=CMQH-TSJS",
    );
    // Provisional remote fallback (port 443) — better than nothing…
    assert_eq!(pd.get_port("shopify"), Some(443));
    // …but a later local URL replaces it and locks.
    pd.scan_output("shopify", "[shopify] [1] http://127.0.0.1:9292");
    assert_eq!(pd.get_port("shopify"), Some(9292));
    // Locked: further remote URLs can no longer change it.
    pd.scan_output("shopify", "See https://something.example.com/");
    assert_eq!(pd.get_port("shopify"), Some(9292));
}

#[test]
fn labeled_preview_url_beats_local_graphiql() {
    // `shopify app dev` prints a public admin "Preview URL:" alongside a
    // localhost GraphiQL URL. The labeled preview must win the badge (it's
    // the page the user actually wants opened, and it works in a local
    // browser as-is), while the GraphiQL port is still harvested so a
    // remote project tunnels it.
    let mut pd = PortDetector::new();
    let output = "\
Preview URL: https://admin.shopify.com/store/nik-chris/apps/4a74c956?dev-console=show
GraphiQL URL: http://localhost:3457/graphiql?key=85b3d5bf
";
    pd.scan_output("app-dev", output);
    assert_eq!(
        pd.get_url("app-dev"),
        Some("https://admin.shopify.com/store/nik-chris/apps/4a74c956?dev-console=show")
    );
    assert!(pd.badge_final("app-dev"));
    // Not a local badge — nothing to tunnel or show a port number for.
    assert!(!pd.has_local_port("app-dev"));
    // GraphiQL port still collected for tunnelling.
    assert!(pd.all_local_ports("app-dev").contains(&3457));
    // Preferred badge is locked — later local URLs don't replace it.
    pd.scan_output("app-dev", "Server at http://localhost:9999");
    assert!(pd.get_url("app-dev").unwrap().contains("admin.shopify.com"));
}

#[test]
fn shopify_app_dev_infra_lines_do_not_lock_badge() {
    // Regression: `shopify app dev` prints its proxy/graphiql server ports
    // and a proxy error seconds before the "Preview URL:" panel. Those
    // infra lines must not lock the badge — the run that exposed this had
    // TuxFlow auto-open http://localhost:38157/ (the proxy) instead of the
    // admin preview page.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "dev",
        "\
15:40:44 | proxy | Proxy server started on port 38157
15:40:44 | graphiql | GraphiQL server started on port 3457
15:40:45 | proxy | Error forwarding web request: connect ECONNREFUSED 127.0.0.1:41615
15:40:45 | proxy | Unreachable target \"http://localhost:41615\" for path: \"/\"
",
    );
    assert!(!pd.badge_final("dev"));
    assert_eq!(pd.get_port("dev"), None);
    // …but the infra ports are still harvested for tunnelling.
    let all = pd.all_local_ports("dev");
    assert!(all.contains(&38157) && all.contains(&3457), "{all:?}");

    // The cloudflare tunnel URL arrives next — provisional only.
    pd.scan_output(
        "dev",
        "15:40:47 | app_home | Using URL: https://champagne-developmental-nato-prev.trycloudflare.com",
    );
    assert_eq!(pd.get_port("dev"), Some(443));
    assert!(!pd.badge_final("dev"));

    // The preview panel finally locks the badge on the admin URL.
    pd.scan_output(
        "dev",
        "\
Preview URL: https://admin.shopify.com/store/nik-chris/apps/4a74c956?dev-console=show
GraphiQL URL: http://localhost:3457/graphiql?key=85b3d5bf
",
    );
    assert!(pd.badge_final("dev"));
    assert!(pd.get_url("dev").unwrap().contains("admin.shopify.com"));
}

#[test]
fn wrapped_preview_url_rejoined_at_terminal_width() {
    // Regression: through tmux, a long URL is redrawn as two hard rows, so
    // the live-screen scan saw only the first fragment and TuxFlow opened a
    // truncated admin URL. Rows filled to the exact terminal width are
    // re-joined before scanning.
    let mut pd = PortDetector::new();
    let line1 =
        "Preview URL: https://admin.shopify.com/store/nik-chris/apps/4a74c956c6cc86a667104e";
    let line2 = "5f76c8afda?dev-console=show";
    let cols = line1.chars().count();
    let text = format!("{line1}\n{line2}\nGraphiQL URL: http://localhost:3457/graphiql");
    pd.scan_output_wrapped("dev", &text, cols);
    assert_eq!(
        pd.get_url("dev"),
        Some(
            "https://admin.shopify.com/store/nik-chris/apps/\
             4a74c956c6cc86a667104e5f76c8afda?dev-console=show"
        )
    );
}

#[test]
fn ink_wrap_at_width_minus_one_rejoined() {
    // Regression: shopify's Ink renderer wraps one column short of the
    // terminal width and writes a *hard* newline — even tmux capture -J
    // leaves it split (measured live: pane_width 84, URL row 83 chars).
    // Width-1 rows must count as wrap fragments too.
    let mut pd = PortDetector::new();
    let line1 =
        " Preview URL: https://admin.shopify.com/store/nik-chris/apps/4a74c956c6cc86a667104e";
    let line2 = "5f76c8afda?dev-console=show";
    let cols = line1.chars().count() + 1;
    let text = format!("{line1}\n{line2}\n");
    pd.scan_output_wrapped("dev", &text, cols);
    assert_eq!(
        pd.get_url("dev"),
        Some(
            "https://admin.shopify.com/store/nik-chris/apps/\
             4a74c956c6cc86a667104e5f76c8afda?dev-console=show"
        )
    );
}

#[test]
fn trailing_wrap_fragment_deferred_until_complete() {
    // A scan can land mid-frame: the first row of a wrapped URL is in the
    // scanned range but its continuation isn't (yet). The fragment parses
    // as a valid URL on its own, so it must be dropped from the scan
    // entirely — otherwise it locks the badge truncated and auto-open
    // fires on a broken admin URL. The next scan sees the whole line.
    let mut pd = PortDetector::new();
    let line1 =
        " Preview URL: https://admin.shopify.com/store/nik-chris/apps/4a74c956c6cc86a667104e";
    let cols = line1.chars().count() + 1;
    pd.scan_output_wrapped("dev", line1, cols);
    assert_eq!(pd.get_url("dev"), None);
    assert!(!pd.badge_final("dev"));

    // Same when blank rows (the unwritten screen below the frame) follow
    // the fragment — blankness is not a continuation.
    pd.scan_output_wrapped("dev", &format!("{line1}\n\n\n"), cols);
    assert_eq!(pd.get_url("dev"), None);

    let text = format!("{line1}\n5f76c8afda?dev-console=show\n");
    pd.scan_output_wrapped("dev", &text, cols);
    assert!(pd.get_url("dev").unwrap().ends_with("?dev-console=show"));
    assert!(pd.badge_final("dev"));
}

#[test]
fn ink_padded_box_continuation_strips_indent() {
    // The real captured shape from `shopify app dev` in tmux (pane 84):
    // Ink draws its panel with a 1-space left padding, so the wrap
    // continuation row ALSO starts with a space. Joined verbatim, that
    // space lands inside the URL and truncates it at exactly the wrap
    // point — the parent line's indent must be stripped from the
    // continuation.
    let mut pd = PortDetector::new();
    let line1 =
        " Preview URL: https://admin.shopify.com/store/nik-chris/apps/4a74c956c6cc86a667104e";
    let line2 = " 5f76c8afda?dev-console=show";
    let cols = line1.chars().count() + 1; // pane is one wider (Ink wraps at width-1)
    let text =
        format!("{line1}\n{line2}\n GraphiQL URL: http://localhost:3457/graphiql?key=85b3d5bf\n");
    pd.scan_output_wrapped("dev", &text, cols);
    assert_eq!(
        pd.get_url("dev"),
        Some(
            "https://admin.shopify.com/store/nik-chris/apps/\
             4a74c956c6cc86a667104e5f76c8afda?dev-console=show"
        )
    );
}

#[test]
fn overlong_joined_line_does_not_continue() {
    // History text already joined by capture -J can contain lines longer
    // than the terminal width — those are complete and must not swallow
    // the following line.
    let mut pd = PortDetector::new();
    let long_line = format!("prefix {} http://localhost:4000", "x".repeat(50));
    let text = format!("{long_line}\nUnrelated: https://evil.example.com/x");
    pd.scan_output_wrapped("dev", &text, 40);
    assert_eq!(pd.get_port("dev"), Some(4000));
    assert_eq!(pd.get_url("dev"), Some("http://localhost:4000"));
}

#[test]
fn full_width_false_join_keeps_other_detections() {
    // A genuinely full row followed by an unrelated line merges only the
    // words at the junction — the URL later on the second line still scans.
    let mut pd = PortDetector::new();
    let line1 = "some log line that happens to fill width";
    let cols = line1.chars().count();
    let text = format!("{line1}\nServer at http://localhost:3000");
    pd.scan_output_wrapped("dev", &text, cols);
    assert_eq!(pd.get_port("dev"), Some(3000));
}

#[test]
fn labeled_preview_url_with_local_host_stays_local() {
    // A preview label on a localhost URL keeps normal local semantics
    // (port badge + tunnel) while still locking the badge.
    let mut pd = PortDetector::new();
    pd.scan_output("dev", "Preview URL: http://localhost:3000/");
    assert_eq!(pd.get_port("dev"), Some(3000));
    assert!(pd.has_local_port("dev"));
    assert!(pd.badge_final("dev"));
}

#[test]
fn plain_preview_word_stays_provisional() {
    // Only the explicit "preview url" label promotes; a mere mention of
    // "preview" (theme-share links, OAuth pages) stays a provisional
    // remote fallback that a later local URL replaces.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "theme",
        "Share your theme preview: https://x.myshopify.com/?preview_theme_id=1",
    );
    assert!(!pd.badge_final("theme"));
    pd.scan_output("theme", "[shopify] [1] http://127.0.0.1:9292");
    assert_eq!(pd.get_port("theme"), Some(9292));
}

#[test]
fn late_scan_still_collects_secondary_ports_after_badge_lock() {
    // Regression: at tmux reattach, a partial-redraw screen scan can lock
    // the badge before the pane-history seed arrives. The seed must still
    // harvest secondary ports (vite asset server) or their tunnels never
    // come up — styling breaks with hardcoded :5173 asset URLs.
    let mut pd = PortDetector::new();
    pd.scan_output("dev", "[shopify] [1] http://127.0.0.1:9292");
    assert_eq!(pd.get_port("dev"), Some(9292));
    pd.scan_output("dev", "[vite]  \u{279c}  Local:   https://localhost:5173/");
    assert!(pd.all_local_ports("dev").contains(&5173));
    // Badge stays locked
    assert_eq!(pd.get_port("dev"), Some(9292));
}
