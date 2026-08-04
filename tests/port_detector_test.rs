use tuxflow::util::port_detector::PortDetector;

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
fn vite_branded_output_skipped_as_tool() {
    // Vite's banner ("VITE v…") and the "→ Local:" line are tool-line markers,
    // so port detection deliberately ignores them. This avoids picking Vite's
    // port over the real app port in concurrently-style setups. Plain Vite
    // projects rely on VTE's built-in URL hyperlinking instead.
    let mut pd = PortDetector::new();
    pd.scan_output(
        "vite",
        "  VITE v7.3.1  ready in 2286ms\n\n  → Local:   http://localhost:5174/\n  → Network: use --host to expose",
    );
    assert_eq!(pd.get_port("vite"), None);
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
fn plain_vite_output_skipped() {
    // Tool-line skipping is intentionally aggressive: even when Vite is the
    // only thing running, its banner and "→ Local:" line are filtered out, so
    // no port is detected. Plain-Vite users rely on VTE's built-in URL
    // hyperlinking. Relax `is_tool_line` if this proves too aggressive.
    let mut pd = PortDetector::new();
    let output = "\
  VITE v7.3.1  ready in 2286ms

  → Local:   http://localhost:5174/
  → Network: use --host to expose
";
    pd.scan_output("vite", output);
    assert_eq!(pd.get_port("vite"), None);
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
    use tuxflow::util::port_detector::remap_url_port;
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
