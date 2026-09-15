//! Single instance — GTK's `HANDLES_OPEN` activation, done by hand.
//!
//! The GtkApplication made `tuxflow /path` from a second shell add the
//! project to the RUNNING window (and a bare `tuxflow` just raise it); a
//! plain iced binary opens a second window. So the first instance listens
//! on a Unix socket under `$XDG_RUNTIME_DIR` and every later launch
//! connects, hands over its (already normalized) project keys, waits for
//! the ack and exits — the caller's shell returns once the window has
//! acted, not before.
//!
//! A socket FILE left behind by a crashed instance answers connect with
//! ECONNREFUSED; that is the one case where the file is unlinked and the
//! address re-bound. Any other failure runs the app standalone: a missing
//! runtime dir must not stop the app from starting at all.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// The listener the primary instance keeps for its lifetime, parked here
/// between `claim` (before iced starts) and the stream that drains it
/// (started from `App::new`).
static LISTENER: Mutex<Option<UnixListener>> = Mutex::new(None);

pub enum Claim {
    /// This process is the instance; run the app.
    Primary,
    /// A running instance took the keys; exit.
    Forwarded,
}

/// `$XDG_RUNTIME_DIR/tuxflow-instance.sock` — per user by construction;
/// the /tmp fallback carries the uid for the same reason.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("tuxflow-instance.sock"),
        _ => PathBuf::from(format!("/tmp/tuxflow-instance-{}.sock", uid())),
    }
}

fn uid() -> u32 {
    // No libc dependency for one number: /proc says who we are.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|l| l.split_whitespace().next())
                .and_then(|u| u.parse().ok())
        })
        .unwrap_or(0)
}

/// Become the instance, or hand `keys` to the one already running.
pub fn claim(keys: &[String]) -> Claim {
    let path = socket_path();
    match UnixStream::connect(&path) {
        Ok(stream) => {
            if forward(stream, keys).is_ok() {
                return Claim::Forwarded;
            }
            // Connected but the exchange failed: treat it like a dead
            // socket rather than leave the user with no window.
            log::warn!("running instance did not answer; starting standalone");
            Claim::Primary
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::ConnectionRefused {
                let _ = std::fs::remove_file(&path);
            }
            match UnixListener::bind(&path) {
                Ok(listener) => {
                    *LISTENER.lock().unwrap() = Some(listener);
                }
                Err(e) => log::warn!(
                    "single-instance socket {} unavailable ({e}); later launches open their own window",
                    path.display()
                ),
            }
            Claim::Primary
        }
    }
}

/// The wire format: one key per line, then EOF; the instance answers with
/// one line once it has acted.
fn forward(mut stream: UnixStream, keys: &[String]) -> std::io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut payload = keys.join("\n");
    payload.push('\n');
    stream.write_all(payload.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut ack = String::new();
    stream.read_to_string(&mut ack)?;
    if ack.trim() == "ok" {
        Ok(())
    } else {
        Err(std::io::Error::other("no ack"))
    }
}

/// Take the listener the primary holds, once, for the stream that serves
/// it. `None` when this process forwarded or the bind failed.
pub fn take_listener() -> Option<UnixListener> {
    LISTENER.lock().unwrap().take()
}

/// Read one launch's keys off an accepted connection. Blank lines (a bare
/// `tuxflow`) yield an empty list, which still raises the window.
pub fn read_request(stream: &mut UnixStream) -> std::io::Result<Vec<String>> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    Ok(buf
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

pub fn ack(stream: &mut UnixStream) {
    let _ = stream.write_all(b"ok\n");
}

/// Drop the socket file with the instance, so the next launch binds
/// cleanly instead of going through the refused-connect path.
pub fn release() {
    if LISTENER.lock().unwrap().is_some() || socket_path().exists() {
        let _ = std::fs::remove_file(socket_path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_over_a_socketpair() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            a.write_all(b"/home/n/app\n\nssh://box/srv\n").unwrap();
            a.shutdown(std::net::Shutdown::Write).unwrap();
            let mut ack = String::new();
            a.read_to_string(&mut ack).unwrap();
            assert_eq!(ack, "ok\n");
        });
        let keys = read_request(&mut b).unwrap();
        assert_eq!(keys, vec!["/home/n/app", "ssh://box/srv"]);
        ack(&mut b);
    }

    #[test]
    fn bare_launch_is_an_empty_request() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_all(b"\n").unwrap();
        a.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(read_request(&mut b).unwrap().is_empty());
    }
}
