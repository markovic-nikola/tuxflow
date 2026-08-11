//! Ask a host what a remote run is listening on, bypassing terminal output.
//!
//! ```sh
//! cargo run --example remote_ports_check -- my-server tf-dev-9ee1daac tf-dev-90db9ad2
//! ```

fn main() {
    let mut args = std::env::args().skip(1);
    let host = args
        .next()
        .expect("usage: remote_ports_check <host> <session>...");
    let sessions: Vec<String> = args.collect();
    assert!(!sessions.is_empty(), "pass at least one tmux session");

    let found = tuxflow::remote::ports::session_ports(&host, &sessions);
    for session in &sessions {
        match found.get(session) {
            Some(ports) => println!("{session:<24} {ports:?}"),
            None => println!("{session:<24} (no live pane)"),
        }
    }
}
