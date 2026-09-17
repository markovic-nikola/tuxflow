//! MCP for remote projects.
//!
//! The MCP server answers on a Unix socket on THIS machine, but for a
//! remote project the agent that should be asking lives on the host, inside
//! its tmux session — it can no more reach `$XDG_RUNTIME_DIR` here than
//! the mic can be plugged in there. Same answer as [`super::super::remote::mic`]:
//! a socket on the host, reverse-forwarded (`ssh -R`) to the local server,
//! and a shim in the host's `~/.local/bin` giving the agent the interface it
//! already knows — a `tuxflow-mcp` command speaking MCP over stdio, exactly
//! what the local binary is. Nothing on the host runs a server; every
//! request crosses the forward to the one that owns the terminals.
//!
//! One forward per remote project (not per host): each project has its own
//! socket and its own process table, so an agent in project A never sees
//! B's `dev` as its own. The forward is a dedicated ssh connection with
//! PDEATHSIG (see [`super::super::remote::spawn_reverse_forward`]), so it
//! cannot outlive the app that answers on it.

use std::collections::HashMap;
use std::process::Child;

use crate::remote::{sh_quote, ssh_stream_stdin};

use super::server::sanitize_name;

/// Where the host-side sockets live, relative to the host's `$HOME`.
/// `0700` (provisioning chmods it): anything on the host that can reach a
/// socket can start and stop this project's processes.
const REMOTE_SOCKET_DIR: &str = ".cache/tuxflow/mcp";

/// The host-side socket for `project_name`, spelled with `~` — this is the
/// value of [`super::server::SOCKET_ENV`] in a remote process's
/// environment, set BEFORE provisioning has resolved the host's `$HOME`
/// (the env travels inside the tmux wrap, quoted, so the remote shell
/// never expands it either). The shim expands `~` itself; the absolute
/// path the forward binds comes back from [`provision`].
pub fn remote_socket_path(project_name: &str) -> String {
    format!("~/{REMOTE_SOCKET_DIR}/{}.sock", sanitize_name(project_name))
}

/// `tuxflow-mcp` for the host: the local binary's contract in Python, since
/// no Rust binary of ours is installed there. Discovery order matches
/// `src/bin/tuxflow-mcp.rs`: `TUXFLOW_MCP_SOCKET`, then a project name
/// argument, then the socket directory — a `.dir` sidecar whose path
/// contains the cwd wins, else the only live socket, else the first live
/// one with a warning. "Live" is checked by connecting: a socket file
/// whose forward is gone (the app quit, the link died) is skipped rather
/// than handed to the agent as a dead server.
///
/// The relay is a plain byte pump both ways; MCP over stdio is
/// newline-delimited JSON and needs no framing of its own here.
const SHIM: &str = r#"#!/usr/bin/env python3
# TuxFlow MCP shim v1 (auto-installed; safe to delete)
# Speaks MCP over stdio to the TuxFlow instance managing this project,
# through a socket ssh -R forwards from the machine running TuxFlow.
import glob, os, select, socket, sys

SOCK_DIR = os.path.expanduser("~/.cache/tuxflow/mcp")


def connect(path):
    s = socket.socket(socket.AF_UNIX)
    try:
        s.connect(path)
        return s
    except OSError:
        s.close()
        return None


def find_socket():
    env = os.environ.get("TUXFLOW_MCP_SOCKET")
    if env:
        return connect(os.path.expanduser(env)), env
    if len(sys.argv) > 1:
        name = "".join(c if c.isalnum() or c in "-_" else "-" for c in sys.argv[1])
        path = os.path.join(SOCK_DIR, name + ".sock")
        return connect(path), path
    live = []
    for path in sorted(glob.glob(os.path.join(SOCK_DIR, "*.sock"))):
        s = connect(path)
        if s is None:
            continue
        live.append((path, s))
    if not live:
        return None, SOCK_DIR
    cwd = os.getcwd()
    for path, s in live:
        try:
            with open(path + ".dir") as f:
                project_dir = f.read().strip()
        except OSError:
            continue
        if project_dir and (cwd == project_dir or cwd.startswith(project_dir.rstrip("/") + "/")):
            for other, so in live:
                if other != path:
                    so.close()
            return s, path
    if len(live) > 1:
        names = [os.path.basename(p)[:-5] for p, _ in live]
        sys.stderr.write(
            "Multiple TuxFlow projects found: %s. Connecting to '%s'.\n"
            "Tip: run from within a project directory for auto-detection, "
            "or pass the project name: tuxflow-mcp <project-name>\n" % (", ".join(names), names[0])
        )
    for _, so in live[1:]:
        so.close()
    return live[0][1], live[0][0]


sock, where = find_socket()
if sock is None:
    sys.stderr.write("No TuxFlow MCP socket at %s. Make sure TuxFlow is running with this project open.\n" % where)
    sys.exit(1)

stdin = sys.stdin.buffer
stdout = sys.stdout.buffer
fds = [stdin.fileno(), sock.fileno()]
while fds:
    ready, _, _ = select.select(fds, [], [])
    for fd in ready:
        if fd == sock.fileno():
            data = sock.recv(65536)
            if not data:
                sys.exit(0)
            stdout.write(data)
            stdout.flush()
        else:
            data = os.read(fd, 65536)
            if not data:
                fds.remove(fd)
                try:
                    sock.shutdown(socket.SHUT_WR)
                except OSError:
                    pass
                continue
            sock.sendall(data)
"#;

/// The shim's text, for tests that run it against a local server.
pub fn shim_script() -> &'static str {
    SHIM
}

/// Install the shim, write the project's `.dir` sidecar, clear any stale
/// socket, and report the absolute socket path (`~` is not expanded by
/// sshd in an `-R` listen path, so the host's `$HOME` is resolved in the
/// same round trip). **Blocking (ssh) — worker thread only.**
///
/// The shim is rewritten every time rather than only when absent: a shim
/// from an older TuxFlow would otherwise persist forever on the host, and
/// the failure that causes is invisible from this side.
fn provision(host: &str, project_name: &str, remote_dir: &str) -> Result<String, String> {
    let name = sanitize_name(project_name);
    let script = format!(
        "mkdir -p ~/{dir} ~/.local/bin && chmod 700 ~/.cache/tuxflow ~/{dir} && \
         printf '%s' {shim} > ~/.local/bin/tuxflow-mcp && chmod +x ~/.local/bin/tuxflow-mcp && \
         rm -f ~/{dir}/{name}.sock && printf '%s' {pdir} > ~/{dir}/{name}.sock.dir && \
         echo \"$HOME/{dir}/{name}.sock\"",
        dir = REMOTE_SOCKET_DIR,
        shim = sh_quote(SHIM),
        pdir = sh_quote(remote_dir),
    );
    ssh_stream_stdin(host, &script, &[])
}

/// What one forward needs to exist.
#[derive(Clone, Debug)]
pub struct ForwardSpec {
    pub host: String,
    pub project_name: String,
    pub remote_dir: String,
    /// The local server's socket (from [`super::server::McpServerHandle`]).
    pub local_socket: String,
}

/// Reverse forwards for remote projects' MCP sockets, keyed by the
/// project key (`ssh://host/dir`).
#[derive(Default)]
struct Forwards {
    live: HashMap<String, Child>,
}

/// How often a forward that never came up is tried again, and the pause
/// before each retry.
const FORWARD_RETRY_PAUSES: [std::time::Duration; 2] = [
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(6),
];

/// Script that waits (≤5 s, host-side, one round trip over the shared
/// connection) for `remote_socket` to be bound, i.e. for the forward's own
/// connection to have authenticated. Provisioning removed the socket, so
/// its existence is the forward's doing.
fn wait_bound_script(remote_socket: &str) -> String {
    let sock = sh_quote(remote_socket);
    format!(
        "i=0; while [ $i -lt 25 ] && [ ! -S {sock} ]; do sleep 0.2; i=$((i+1)); done; [ -S {sock} ]"
    )
}

/// Provision, spawn and WAIT until the forward is bound. The wait is what
/// paces a workspace load: every forward is a dedicated connection, a
/// connection counts against sshd's `MaxStartups` (10 unauthenticated)
/// until it has authenticated, and three dozen projects on one host spawned
/// back to back had most of them reset during key exchange — silently
/// leaving those projects without MCP, since nothing retried. One handshake
/// in flight at a time, and a refused one is tried again.
/// **Blocking — the worker thread only**, and never under the `FORWARDS`
/// lock (`close` takes it from the UI thread).
fn bring_up(spec: &ForwardSpec) -> Result<(Child, String), String> {
    let mut pauses = FORWARD_RETRY_PAUSES.iter();
    loop {
        let attempt = (|| {
            let remote_socket = provision(&spec.host, &spec.project_name, &spec.remote_dir)?;
            let mut child = crate::remote::spawn_reverse_forward(
                &spec.host,
                &remote_socket,
                std::path::Path::new(&spec.local_socket),
                "MCP forward",
            )?;
            let bound =
                ssh_stream_stdin(&spec.host, &wait_bound_script(&remote_socket), &[]).is_ok();
            if bound && matches!(child.try_wait(), Ok(None)) {
                Ok((child, remote_socket))
            } else {
                let _ = child.kill();
                let _ = child.wait();
                Err("the forward's connection did not come up".to_string())
            }
        })();
        match (attempt, pauses.next()) {
            (Ok(up), _) => return Ok(up),
            (Err(e), None) => return Err(e),
            (Err(e), Some(pause)) => {
                log::warn!("MCP forward for {}: {e}; retrying", spec.project_name);
                std::thread::sleep(*pause);
            }
        }
    }
}

impl Forwards {
    /// Whether `key`'s forward is still running; a dead one is forgotten.
    fn is_up(&mut self, key: &str) -> bool {
        match self.live.get_mut(key).map(|child| child.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                self.live.remove(key);
                false
            }
            None => false,
        }
    }

    fn close(&mut self, key: &str) {
        if let Some(mut child) = self.live.remove(key) {
            let _ = child.kill();
            let _ = child.wait();
            log::info!("MCP forward down: {key}");
        }
    }

    fn close_all(&mut self) {
        let keys: Vec<String> = self.live.keys().cloned().collect();
        for key in keys {
            self.close(&key);
        }
    }
}

static FORWARDS: std::sync::LazyLock<std::sync::Mutex<Forwards>> =
    std::sync::LazyLock::new(Default::default);

/// Requests to bring a forward up, served by one long-lived thread — the
/// forward's PDEATHSIG is bound to the thread that spawned it (see
/// `mic.rs`), and provisioning runs ssh, which must not block the UI.
type Request = (String, ForwardSpec);

static WORKER: std::sync::LazyLock<std::sync::mpsc::Sender<Request>> =
    std::sync::LazyLock::new(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Request>();
        std::thread::spawn(move || {
            for (key, spec) in rx {
                let lock = || FORWARDS.lock().unwrap_or_else(|e| e.into_inner());
                if lock().is_up(&key) {
                    continue;
                }
                match bring_up(&spec) {
                    Ok((child, remote_socket)) => {
                        log::info!(
                            "MCP forward up: {} -> {remote_socket} ({})",
                            spec.host,
                            spec.local_socket
                        );
                        lock().live.insert(key, child);
                    }
                    Err(e) => log::error!("MCP forward for {} unavailable: {e}", spec.host),
                }
            }
        });
        tx
    });

/// Bring the forward for `key` up (or back up — a dead ssh is replaced).
/// Non-blocking: queued to the worker.
pub fn ensure(key: &str, spec: ForwardSpec) {
    let _ = WORKER.send((key.to_string(), spec));
}

/// Tear `key`'s forward down and remove its socket files on the host,
/// fire-and-forget. Killing runs here (safe from any thread — only
/// spawning is thread-sensitive); the host-side rm is its own short ssh
/// on a throwaway thread, so a closing project never waits on the link.
pub fn close(key: &str, host: &str, project_name: &str) {
    FORWARDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .close(key);
    let host = host.to_string();
    let name = sanitize_name(project_name);
    std::thread::spawn(move || {
        let script = format!(
            "rm -f ~/{REMOTE_SOCKET_DIR}/{name}.sock ~/{REMOTE_SOCKET_DIR}/{name}.sock.dir"
        );
        if let Err(e) = ssh_stream_stdin(&host, &script, &[]) {
            log::debug!("MCP socket cleanup on {host} failed: {e}");
        }
    });
}

/// Tear every forward down. Called on quit; `Drop` can't be relied on for
/// a `static`. PDEATHSIG covers the paths that skip this. The host-side
/// socket files are left behind on purpose — quitting must not wait on N
/// ssh round trips — and the shim skips a socket nothing answers on.
pub fn shutdown() {
    FORWARDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .close_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_socket_path_is_home_relative_and_sanitized() {
        assert_eq!(
            remote_socket_path("My App.v2"),
            "~/.cache/tuxflow/mcp/My-App-v2.sock"
        );
    }

    #[test]
    fn shim_is_a_python_script() {
        assert!(SHIM.starts_with("#!/usr/bin/env python3\n"));
        // The three discovery routes the local binary offers.
        assert!(SHIM.contains("TUXFLOW_MCP_SOCKET"));
        assert!(SHIM.contains("sys.argv[1]"));
        assert!(SHIM.contains("+ \".dir\""));
    }
}
