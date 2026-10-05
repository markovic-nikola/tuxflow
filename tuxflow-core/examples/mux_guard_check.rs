//! Run one `mux_guard` pass against a host and report what it did: the
//! shared connection's path before and after, and whether it was replaced.
//!
//! ```sh
//! cargo run --example mux_guard_check -- my-server
//! ```
//!
//! Acts on the ControlMaster under `$XDG_RUNTIME_DIR/tuxflow` — the running
//! app's, unless `XDG_RUNTIME_DIR` points somewhere else (keep it SHORT:
//! socket paths are capped at 108 bytes).

use std::process::Command;

fn master(host: &str) -> Option<(u32, f64)> {
    let dir = format!(
        "{}/tuxflow",
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())
    );
    let out = Command::new("ssh")
        .args([
            "-o",
            &format!("ControlPath={dir}/ssh-%C"),
            "-O",
            "check",
            host,
        ])
        .output()
        .ok()?;
    let pid =
        tuxflow_core::remote::mux_guard::parse_master_pid(&String::from_utf8_lossy(&out.stderr))?;
    let ss = Command::new("ss")
        .args(["-tnpiH", "state", "established"])
        .output()
        .ok()?;
    let rtt = tuxflow_core::remote::mux_guard::parse_ss(&String::from_utf8_lossy(&ss.stdout))
        .into_iter()
        .find(|f| f.pid == Some(pid))?
        .min_rtt_ms;
    Some((pid, rtt))
}

fn main() {
    let host = std::env::args()
        .nth(1)
        .expect("usage: mux_guard_check <host>");
    println!("before: {:?}", master(&host));
    tuxflow_core::remote::mux_guard::check_now(&host);
    println!("after:  {:?}", master(&host));
    println!(
        "rerouted: {}",
        tuxflow_core::remote::mux_guard::rerouted_recently(&host)
    );
}
