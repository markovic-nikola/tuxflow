//! Hold-to-talk relay for remote agents.
//!
//! Claude Code recognises a held Space from keyboard auto-repeat alone —
//! there is no key-release reporting over a plain terminal: five repeats
//! within 120 ms of each other start a recording, and while recording
//! every repeat re-arms a 200 ms release timer. Forwarding the repeats
//! byte-by-byte over ssh and tmux breaks that contract. Measured against a
//! VPS 50 ms away, a steady 30 ms stream arrived with gaps of up to ~290 ms
//! once tmux was in the path (link jitter plus the tmux server's own
//! scheduling); every such gap ended the recording mid-hold and the
//! repeats still arriving started a second one — "double recordings", and
//! no "listening…" while holding. Keys generated ON the host, inside tmux,
//! arrived cleanly in the same measurements (max ~110 ms), so the hold is
//! relayed as an intent rather than as bytes: a small Python relay
//! attached to the agent's tmux session as a control-mode client (tmux
//! ignores such clients for window sizing) sends a space every
//! [`PERIOD_MS`] until told to stop, or until the app's keep-alives fall
//! silent for [`STALE_MS`] — a link that dies mid-hold must never leave a
//! runaway stream behind.
//!
//! The terminal side is fork patch 22: with hold reporting on, a bare
//! Space auto-repeat is not written to the PTY but surfaces as an action,
//! and so does the release. The first press still types its space.

use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use super::{TMUX_SOCKET, sh_quote, ssh_mux_options};

/// Interval between relayed spaces: GNOME's default repeat interval, the
/// cadence Claude Code sees from a local keyboard.
pub const PERIOD_MS: u32 = 30;
/// Keep-alive silence after which the relay stops on its own. Well above
/// the link's normal jitter, well below a hold anyone would tolerate a
/// runaway stream for.
pub const STALE_MS: u32 = 1200;
/// How often the app refreshes the relay's keep-alive while the key is
/// held (each repeat event is an opportunity; most are skipped).
pub const KEEPALIVE_MS: u64 = 100;

/// The host-side relay. argv: tmux session, period ms, stale ms, tmux
/// socket name. stdin carries keep-alives (any bytes) and `stop`; EOF ends
/// it too. Attaches in control mode so the agent's window keeps its size,
/// and asks for no pane output so the attach costs the server nothing.
const RELAY: &str = r#"
import os, select, shlex, subprocess, sys, time
sess, period, stale, sock = sys.argv[1], int(sys.argv[2]) / 1000.0, int(sys.argv[3]) / 1000.0, sys.argv[4]
ctl = subprocess.Popen(["tmux", "-L", sock, "-C", "attach-session", "-t", sess],
                       stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
key = ("send-keys -t %s -l ' '\n" % shlex.quote(sess)).encode()
fd = sys.stdin.fileno()
last = time.monotonic()
nxt = last
sent = 0
try:
    ctl.stdin.write(b"refresh-client -f no-output\n")
    ctl.stdin.flush()
    while ctl.poll() is None:
        now = time.monotonic()
        if now - last > stale:
            break
        r, _, _ = select.select([fd], [], [], max(0.0, nxt - now))
        if r:
            data = os.read(fd, 4096)
            if not data or b"stop" in data:
                break
            last = time.monotonic()
        now = time.monotonic()
        if now >= nxt:
            ctl.stdin.write(key)
            ctl.stdin.flush()
            sent += 1
            nxt = max(nxt + period, now)
finally:
    try:
        ctl.stdin.write(b"detach-client\n")
        ctl.stdin.flush()
        ctl.wait(timeout=1)
    except Exception:
        ctl.kill()
    sys.stderr.write("tuxflow hold relay: %d spaces\n" % sent)
"#;

/// The ssh command line that runs the relay for `session` on the host.
fn relay_command(session: &str) -> String {
    format!(
        "python3 -c {} {} {} {} {}",
        sh_quote(RELAY),
        sh_quote(session),
        PERIOD_MS,
        STALE_MS,
        sh_quote(TMUX_SOCKET)
    )
}

/// One held key, relayed on one host.
pub struct HoldRelay {
    child: Child,
    stdin: Option<ChildStdin>,
    last_keepalive: Instant,
}

impl HoldRelay {
    /// Start relaying into `session` on `host`. **Non-blocking** — the ssh
    /// exec is spawned, never awaited; the first relayed space lands one
    /// round trip plus a Python start later (~150 ms). Spawn from a thread
    /// that outlives the hold: `PR_SET_PDEATHSIG` is thread-scoped (see
    /// `mic.rs`), and the UI thread is the one that never exits early.
    pub fn spawn(host: &str, session: &str) -> std::io::Result<Self> {
        let mut cmd = Command::new("ssh");
        cmd.args(ssh_mux_options())
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
            .arg(host)
            .arg("--")
            .arg(relay_command(session))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // Die with TuxFlow: a relay must never outlive the app that is
        // keeping it alive.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGTERM, 0, 0, 0);
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        if let Some(stderr) = child.stderr.take() {
            let host = host.to_string();
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stderr)
                    .lines()
                    .map_while(Result::ok)
                {
                    log::info!("hold relay {host}: {line}");
                }
            });
        }
        Ok(Self {
            child,
            stdin,
            last_keepalive: Instant::now(),
        })
    }

    /// The key is still held. Rate-limited to [`KEEPALIVE_MS`]; `false`
    /// means the relay is gone (the link stalled past [`STALE_MS`], the
    /// session ended, or the host has no tmux) and the caller should fall
    /// back to forwarding the repeat itself.
    pub fn keepalive(&mut self) -> bool {
        if !matches!(self.child.try_wait(), Ok(None)) {
            return false;
        }
        if self.last_keepalive.elapsed() < Duration::from_millis(KEEPALIVE_MS) {
            return true;
        }
        self.last_keepalive = Instant::now();
        match self.stdin.as_mut() {
            Some(stdin) => stdin.write_all(b"k").and_then(|()| stdin.flush()).is_ok(),
            None => false,
        }
    }

    /// The key was released: tell the relay and let it wind down off the
    /// UI thread — its exit is a round trip away.
    pub fn stop(mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"stop");
            let _ = stdin.flush();
        }
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if !matches!(self.child.try_wait(), Ok(None)) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            // Drop kills what didn't leave.
        });
    }
}

impl Drop for HoldRelay {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_command_targets_the_session_on_the_tuxflow_server() {
        let cmd = relay_command("tf-claude-1a2b3c4d");
        assert!(cmd.starts_with("python3 -c "));
        assert!(cmd.ends_with(&format!(
            " 'tf-claude-1a2b3c4d' {PERIOD_MS} {STALE_MS} 'tuxflow'"
        )));
    }

    #[test]
    fn relay_stops_on_stop_eof_and_silence() {
        assert!(RELAY.contains("b\"stop\" in data"));
        assert!(RELAY.contains("if not data"));
        assert!(RELAY.contains("now - last > stale"));
        assert!(RELAY.contains("-C\", \"attach-session\""));
    }

    /// The embedded script must at least parse — a typo here would only
    /// surface as a silently dead relay on the host.
    #[test]
    fn relay_script_is_valid_python() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let Ok(mut py) = Command::new("python3")
            .args(["-c", "import ast, sys; ast.parse(sys.stdin.read())"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        else {
            eprintln!("python3 not on PATH; skipping");
            return;
        };
        py.stdin
            .take()
            .unwrap()
            .write_all(RELAY.as_bytes())
            .unwrap();
        let out = py.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "relay script does not parse: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
