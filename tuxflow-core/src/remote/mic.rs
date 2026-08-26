//! Microphone bridge for remote projects.
//!
//! Claude Code records voice input by spawning `arecord -f S16_LE -r 16000
//! -c 1 -t raw -q -` and reading raw PCM off its stdout — it never checks
//! that the binary is the real ALSA one. Remote hosts are headless and have
//! no capture device, so voice dictation is dead there.
//!
//! This installs a fake `arecord` on the host that reads the same stream off
//! a Unix socket, which `ssh -R` forwards back to this machine, where a
//! listener answers each connection with a *real* recorder. Same shape as the
//! `xclip` shim in [`super::CLIPBOARD_SHIM`]: give the agent the interface it
//! already knows, and satisfy it locally.
//!
//! Opt-in (Settings → Tools → Agents). While a bridge is up, anything on the
//! host that can reach the socket can open this machine's microphone; the
//! `0700` parent directory limits that to the login user and root.

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Once;

use super::{control_dir, sh_quote, ssh_stream_stdin};

/// Wire format Claude Code expects: 16 kHz mono signed 16-bit little-endian.
/// Both ends must agree, so the shim (remote) and recorder (local) derive
/// their arguments from these.
const SAMPLE_RATE: u32 = 16_000;
const CHANNELS: u32 = 1;

/// Fake `arecord` installed into `~/.local/bin` on remote hosts. Shadows
/// nothing — hosts that have a real `arecord` have real audio and don't need
/// this. Exits non-zero when the bridge is down, so Claude Code reports "no
/// microphone" honestly instead of recording silence.
const ARECORD_SHIM: &str = r#"#!/bin/sh
# TuxFlow microphone shim v1 (auto-installed; safe to delete)
# Streams audio from the machine running TuxFlow over an ssh -R socket.
#
# Reading a Unix socket to stdout needs *some* helper, and no single one is
# present on every host, so try the four that between them cover almost all
# of them rather than hard-depending on any one.
SOCK="$HOME/.cache/tuxflow/mic.sock"

case " $* " in
    *" --version "*) echo "arecord: tuxflow microphone bridge"; exit 0 ;;
esac

if command -v socat >/dev/null 2>&1; then
    exec socat -u UNIX-CONNECT:"$SOCK" -
elif command -v python3 >/dev/null 2>&1; then
    exec python3 -u -c '
import socket, sys
try:
    s = socket.socket(socket.AF_UNIX)
    s.connect(sys.argv[1])
except OSError as e:
    sys.stderr.write("tuxflow mic bridge: %s (%s)\n" % (e.strerror, sys.argv[1]))
    sys.exit(1)
out = sys.stdout.buffer
while True:
    chunk = s.recv(4096)
    if not chunk:
        break
    out.write(chunk)
' "$SOCK"
elif command -v perl >/dev/null 2>&1; then
    exec perl -e '
use IO::Socket::UNIX;
my $s = IO::Socket::UNIX->new(Peer => $ARGV[0])
    or die "tuxflow mic bridge: $! ($ARGV[0])\n";
binmode(STDOUT);
while (sysread($s, my $buf, 4096)) { print $buf }
' "$SOCK"
elif nc -h 2>&1 | grep -q -- "-U"; then
    exec nc -U "$SOCK"
fi

echo "tuxflow mic bridge: need socat, python3, perl or a nc with -U" >&2
exit 1
"#;

/// Local socket the forward terminates on. One is enough for any number of
/// hosts — it serves audio, not per-host state.
fn local_socket_path() -> PathBuf {
    control_dir().join("mic.sock")
}

/// The local recorder, as (program, args). Prefers ALSA's `arecord`, falling
/// back to PipeWire's `pw-record` on systems that ship no alsa-utils.
fn recorder() -> Option<(&'static str, Vec<String>)> {
    let rate = SAMPLE_RATE.to_string();
    let channels = CHANNELS.to_string();
    if which("arecord") {
        return Some((
            "arecord",
            vec![
                "-f".into(),
                "S16_LE".into(),
                "-r".into(),
                rate,
                "-c".into(),
                channels,
                "-t".into(),
                "raw".into(),
                "-q".into(),
                "-".into(),
            ],
        ));
    }
    if which("pw-record") {
        return Some((
            "pw-record",
            vec![
                "--rate".into(),
                rate,
                "--channels".into(),
                channels,
                "--format".into(),
                "s16".into(),
                "-".into(),
            ],
        ));
    }
    None
}

fn which(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(prog).is_file()))
        .unwrap_or(false)
}

/// Answer one connection: spawn a recorder and pump it into the socket until
/// the far end hangs up (Claude Code SIGTERMs the shim when you release the
/// key, which closes the socket and lands here as a write error).
fn serve(mut stream: UnixStream) {
    let Some((prog, args)) = recorder() else {
        log::error!("Mic bridge: no local recorder (install alsa-utils or pipewire)");
        return;
    };
    let mut child = match Command::new(prog)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log::error!("Mic bridge: failed to spawn {prog}: {e}");
            return;
        }
    };
    if let Some(mut out) = child.stdout.take() {
        // Ends on EPIPE when the agent stops recording. Rust ignores SIGPIPE,
        // so this surfaces as an error rather than killing TuxFlow.
        let _ = io::copy(&mut out, &mut stream);
    }
    // Without this the recorder outlives the take and holds the capture device.
    let _ = child.kill();
    let _ = child.wait();
}

static LISTENER: Once = Once::new();

/// Bind the local socket and answer connections forever. Idempotent; the
/// listener outlives individual bridges because it is stateless, and is
/// independent of any host — `examples/mic_bridge_check.rs` drives it alone.
pub fn ensure_listener() -> Result<(), String> {
    let mut result = Ok(());
    LISTENER.call_once(|| {
        let path = local_socket_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // A leftover file from a crash would make bind() fail with EADDRINUSE.
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                result = Err(format!("failed to bind {}: {e}", path.display()));
                return;
            }
        };
        // Defence in depth: XDG_RUNTIME_DIR is already 0700.
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        std::thread::spawn(move || serve(s));
                    }
                    Err(e) => {
                        log::error!("Mic bridge listener stopped: {e}");
                        break;
                    }
                }
            }
        });
        log::info!("Mic bridge listening on {}", path.display());
    });
    result
}

/// Install the shim, clear any stale socket, and report the absolute remote
/// socket path. `~` is not expanded by sshd in an `-R` listen path, so the
/// host's `$HOME` has to be resolved here — folded into the same round trip.
fn provision(host: &str) -> Result<String, String> {
    // Rewritten every time rather than only when absent: a shim from an older
    // TuxFlow would otherwise persist forever on the host, and the failure
    // that causes is invisible from this side.
    let script = format!(
        "mkdir -p ~/.cache/tuxflow ~/.local/bin && chmod 700 ~/.cache/tuxflow && \
         printf '%s' {shim} > ~/.local/bin/arecord && chmod +x ~/.local/bin/arecord && \
         rm -f ~/.cache/tuxflow/mic.sock && \
         echo \"$HOME/.cache/tuxflow/mic.sock\"",
        shim = sh_quote(ARECORD_SHIM)
    );
    ssh_stream_stdin(host, &script, &[])
}

/// One `ssh -N -R` reverse forward: remote socket → this machine's listener.
struct Bridge {
    child: Child,
}

/// Microphone bridges for remote hosts, keyed by host.
///
/// Like [`super::tunnel::TunnelManager`], each forward is a dedicated ssh
/// connection rather than a mux client: a forward requested over the shared
/// ControlMaster lives in the *master* and survives the client being killed,
/// so it would keep the microphone reachable after the bridge was closed.
#[derive(Default)]
pub struct MicBridgeManager {
    bridges: HashMap<String, Bridge>,
}

impl MicBridgeManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bring up the bridge for `host` if it isn't already running. **Blocking
    /// (provisioning runs ssh) — call from a worker thread.**
    pub fn ensure(&mut self, host: &str) -> Result<(), String> {
        if let Some(bridge) = self.bridges.get_mut(host) {
            match bridge.child.try_wait() {
                Ok(None) => return Ok(()), // still up
                _ => {
                    self.bridges.remove(host);
                }
            }
        }
        ensure_listener()?;
        let remote_socket = provision(host)?;
        let local_socket = local_socket_path();
        let mut cmd = Command::new("ssh");
        cmd.args([
            "-N",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=15",
            // Belt and braces: the host's sshd default leaves a stale socket
            // behind, which is why provision() removes it first.
            "-o",
            "StreamLocalBindUnlink=yes",
            "-R",
            &format!("{remote_socket}:{}", local_socket.display()),
        ])
        .arg(host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // Captured, not discarded: `ExitOnForwardFailure` makes ssh exit
        // silently on a refused forward, and without this the bridge just
        // fails to exist with nothing to explain why.
        .stderr(Stdio::piped());
        // Die with TuxFlow: an exit path that skips Drop must not leave the
        // microphone reachable from the host.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGTERM, 0, 0, 0);
                Ok(())
            });
        }
        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(stderr) = child.stderr.take() {
                    let host = host.to_string();
                    std::thread::spawn(move || {
                        use std::io::BufRead;
                        for line in std::io::BufReader::new(stderr)
                            .lines()
                            .map_while(Result::ok)
                        {
                            log::error!("Mic bridge {host}: {line}");
                        }
                    });
                }
                log::info!("Mic bridge up: {host} -> {remote_socket}");
                self.bridges.insert(host.to_string(), Bridge { child });
                Ok(())
            }
            Err(e) => Err(format!("failed to spawn mic forward: {e}")),
        }
    }

    pub fn close(&mut self, host: &str) {
        if let Some(mut bridge) = self.bridges.remove(host) {
            let _ = bridge.child.kill();
            let _ = bridge.child.wait();
            log::info!("Mic bridge down: {host}");
        }
    }

    pub fn close_all(&mut self) {
        let hosts: Vec<String> = self.bridges.keys().cloned().collect();
        for host in hosts {
            self.close(&host);
        }
    }
}

impl Drop for MicBridgeManager {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// Bridges are keyed by host, not by project: several projects commonly live
/// on one host and must share a single forward, so ownership can't sit on
/// per-project state.
static MANAGER: std::sync::LazyLock<std::sync::Mutex<MicBridgeManager>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(MicBridgeManager::new()));

/// Hosts with a remote project open, whether or not bridging is on. Kept
/// separately from the live bridges so that flipping the setting can act on
/// projects that are *already* open — otherwise enabling it would appear to
/// do nothing until the next project load.
static HOSTS: std::sync::LazyLock<std::sync::Mutex<std::collections::BTreeSet<String>>> =
    std::sync::LazyLock::new(Default::default);

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

fn ensure_for(host: &str) -> Result<(), String> {
    let mut manager = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    manager.ensure(host).inspect_err(|e| {
        log::error!("Mic bridge for {host} unavailable: {e}");
    })
}

/// Whether bridging is switched on.
pub fn is_enabled() -> bool {
    enabled()
}

/// How long a caller will wait for a bridge before giving up and carrying on
/// without one. Provisioning is two ssh round trips; a host that hasn't
/// answered by now is not going to.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Bring the bridge for `host` up and wait for the result.
///
/// **Blocking — call from a worker thread, never the GTK main thread and
/// never from [`WORKER`] itself (it would deadlock waiting on its own reply).**
pub fn wait_ready(host: &str) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    if WORKER.send((host.to_string(), Some(tx))).is_err() {
        return Err("mic bridge worker is gone".into());
    }
    rx.recv_timeout(READY_TIMEOUT)
        .unwrap_or_else(|_| Err(format!("timed out bringing up the mic bridge for {host}")))
}

/// Same, for every host with a project open. Returns one entry per failure.
/// **Blocking — worker threads only.**
pub fn wait_ready_all() -> Vec<(String, String)> {
    let hosts: Vec<String> = HOSTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect();
    hosts
        .into_iter()
        .filter_map(|host| wait_ready(&host).err().map(|e| (host, e)))
        .collect()
}

/// Requests to bring a host's bridge up, served by one long-lived thread.
///
/// The thread must outlive the forwards it spawns: `PR_SET_PDEATHSIG` fires
/// when the parent *thread* exits, not the parent process, so spawning ssh
/// from a short-lived worker gets it SIGTERM'd the moment that worker
/// finishes. (`tunnel.rs` avoids this only by spawning from the GTK main
/// thread, which never exits.) Provisioning runs ssh and must not block the
/// main thread, so it gets a dedicated thread rather than a transient one.
type Request = (String, Option<std::sync::mpsc::Sender<Result<(), String>>>);

static WORKER: std::sync::LazyLock<std::sync::mpsc::Sender<Request>> =
    std::sync::LazyLock::new(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Request>();
        std::thread::spawn(move || {
            for (host, reply) in rx {
                let result = ensure_for(&host);
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                }
            }
        });
        tx
    });

/// Note that `host` has a project open, bridging it if the setting is on.
/// Non-blocking.
pub fn register_host(host: &str) {
    HOSTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(host.to_string());
    if enabled() {
        let _ = WORKER.send((host.to_string(), None));
    }
}

/// Apply the "Remote Microphone" setting. Turning it on bridges every host
/// with a project already open; turning it off closes all of them at once.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
    if !on {
        // Off its own thread: this is called from the GTK main thread, and
        // the lock it needs is held across ssh provisioning. Killing children
        // from another thread is safe — only *spawning* is thread-sensitive
        // (see WORKER). ENABLED is already false, so nothing new starts.
        std::thread::spawn(shutdown);
        return;
    }
    let hosts: Vec<String> = HOSTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect();
    for host in hosts {
        let _ = WORKER.send((host, None));
    }
}

/// Tear every bridge down. Called on quit; `Drop` can't be relied on for a
/// `static`. PDEATHSIG covers the paths that skip this (crash, SIGKILL).
pub fn shutdown() {
    MANAGER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .close_all();
}
