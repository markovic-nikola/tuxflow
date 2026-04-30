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
