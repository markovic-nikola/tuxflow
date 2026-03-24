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
    pd.scan_output("dev", "→ Local: localhost:5174");
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
fn vite_output() {
    let mut pd = PortDetector::new();
    pd.scan_output(
        "vite",
        "  VITE v7.3.1  ready in 2286ms\n\n  → Local:   http://localhost:5174/\n  → Network: use --host to expose",
    );
    assert_eq!(pd.get_port("vite"), Some(5174));
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
