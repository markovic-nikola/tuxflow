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

use std::collections::{HashMap, HashSet};
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

/// Asks each of a host's sockets the one question that matters — does a
/// TuxFlow ANSWER on it — and prints the names that fail. A connect is not
/// enough: sshd accepts on a forward whose far end went to sleep and then
/// says nothing, so the probe sends an MCP `initialize` and waits for the
/// first byte of a reply. Names arrive as arguments, one thread each (a
/// dead one costs its whole timeout).
const PROBE: &str = r#"import os, socket, sys, threading
D = os.path.expanduser("~/.cache/tuxflow/mcp")
INIT = b'{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"tuxflow-probe","version":"0"}}}\n'
dead = []


def probe(name):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(5)
    try:
        s.connect(os.path.join(D, name + ".sock"))
        s.sendall(INIT)
        if s.recv(1):
            return
    except OSError:
        pass
    finally:
        s.close()
    dead.append(name)


threads = [threading.Thread(target=probe, args=(n,)) for n in sys.argv[1:]]
for t in threads:
    t.start()
for t in threads:
    t.join()
print("\n".join(dead))
"#;

/// The probe's text, for tests that run it against a local server.
pub fn probe_script() -> &'static str {
    PROBE
}

/// Which of `names` (sanitized) have no TuxFlow answering on `host`. `Err`
/// means the host could not be asked — NOT that everything is dead; a link
/// that is down has nothing to be re-raised over.
/// **Blocking (ssh) — worker thread only.**
fn dead_sockets(host: &str, names: &[String]) -> Result<Vec<String>, String> {
    let args: Vec<String> = names.iter().map(|n| sh_quote(n)).collect();
    let script = format!("python3 - {}", args.join(" "));
    let out = ssh_stream_stdin(host, &script, PROBE.as_bytes())?;
    Ok(out.lines().map(str::to_string).collect())
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
    /// Keys whose project still wants a forward. `close` clears it, and the
    /// worker reads it on both sides of a bring-up: that takes seconds, and
    /// a project closed meanwhile must not be handed a forward nobody will
    /// ever tear down.
    wanted: HashSet<String>,
    /// Hosts with a health check queued or running — the check is
    /// periodic, and a slow host must not grow a queue of them.
    checking: HashSet<String>,
}

/// Provision, spawn and WAIT until the forward is bound
/// ([`crate::remote::bring_up_reverse_forward`]). The wait is what paces a
/// workspace load: every forward is a dedicated connection, a connection
/// counts against sshd's `MaxStartups` (10 unauthenticated) until it has
/// authenticated, and three dozen projects on one host spawned back to back
/// had most of them reset during key exchange. One handshake in flight at a
/// time, and a refused one is tried again.
/// **Blocking — the worker thread only**, and never under the `FORWARDS`
/// lock (`close` takes it from the UI thread).
fn bring_up(spec: &ForwardSpec) -> Result<(Child, String), String> {
    crate::remote::bring_up_reverse_forward(
        &spec.host,
        || provision(&spec.host, &spec.project_name, &spec.remote_dir),
        std::path::Path::new(&spec.local_socket),
        "MCP forward",
    )
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
        self.wanted.remove(key);
        self.kill(key);
    }

    /// End `key`'s ssh, leaving whether the project wants one alone.
    fn kill(&mut self, key: &str) {
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

/// Served by one long-lived thread — the forward's PDEATHSIG is bound to
/// the thread that spawned it (see `mic.rs`), and provisioning runs ssh,
/// which must not block the UI.
enum Request {
    Ensure(String, ForwardSpec),
    Check(String, Vec<(String, ForwardSpec)>),
}

fn forwards() -> std::sync::MutexGuard<'static, Forwards> {
    FORWARDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Bring `key`'s forward up unless the project went away, before or during.
fn raise(key: String, spec: &ForwardSpec) {
    if !forwards().wanted.contains(&key) {
        return;
    }
    match bring_up(spec) {
        Ok((mut child, remote_socket)) => {
            let mut forwards = forwards();
            if forwards.wanted.contains(&key) {
                log::info!(
                    "MCP forward up: {} -> {remote_socket} ({})",
                    spec.host,
                    spec.local_socket
                );
                forwards.live.insert(key, child);
            } else {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Err(e) => log::error!("MCP forward for {} unavailable: {e}", spec.host),
    }
}

/// Re-raise the forwards of `specs` whose host socket nobody answers on.
/// A running ssh proves nothing here: the socket path is per PROJECT, so a
/// second machine opening the same project takes the path over (its
/// provisioning unlinks ours) and leaves it dead when it sleeps or quits,
/// all while our own ssh stays healthy. Hence the question goes to the
/// socket, and a live answer — whoever gives it — is left alone, so two
/// machines do not take the path from each other in turns.
fn check(host: &str, specs: Vec<(String, ForwardSpec)>) {
    let names: Vec<String> = specs
        .iter()
        .map(|(_, spec)| sanitize_name(&spec.project_name))
        .collect();
    let dead = match dead_sockets(host, &names) {
        Ok(dead) => dead,
        Err(e) => return log::debug!("MCP health check on {host} skipped: {e}"),
    };
    for ((key, spec), name) in specs.into_iter().zip(names) {
        if dead.contains(&name) {
            log::warn!("MCP socket for {name} on {host} is dead; re-raising");
            forwards().kill(&key);
            raise(key, &spec);
        }
    }
}

static WORKER: std::sync::LazyLock<std::sync::mpsc::Sender<Request>> =
    std::sync::LazyLock::new(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Request>();
        std::thread::spawn(move || {
            for request in rx {
                match request {
                    Request::Ensure(key, spec) => {
                        if !forwards().is_up(&key) {
                            raise(key, &spec);
                        }
                    }
                    Request::Check(host, specs) => {
                        check(&host, specs);
                        forwards().checking.remove(&host);
                    }
                }
            }
        });
        tx
    });

/// Bring the forward for `key` up (or back up — a dead ssh is replaced).
/// Non-blocking: queued to the worker.
pub fn ensure(key: &str, spec: ForwardSpec) {
    forwards().wanted.insert(key.to_string());
    let _ = WORKER.send(Request::Ensure(key.to_string(), spec));
}

/// Verify `host`'s forwards end to end and re-raise the dead ones — what
/// brings MCP back after an outage without the app being restarted (the
/// agents outlive the link in tmux, so nothing else would). One ssh round
/// trip per call; a call made while the host's last one is still pending
/// is dropped. Non-blocking: queued to the worker.
pub fn check_host(host: &str, specs: Vec<(String, ForwardSpec)>) {
    if specs.is_empty() || !forwards().checking.insert(host.to_string()) {
        return;
    }
    let _ = WORKER.send(Request::Check(host.to_string(), specs));
}

/// Tear `key`'s forward down and remove its socket files on the host,
/// fire-and-forget. Killing runs here (safe from any thread — only
/// spawning is thread-sensitive); the host-side rm is its own short ssh
/// on a throwaway thread, so a closing project never waits on the link.
pub fn close(key: &str, host: &str, project_name: &str) {
    forwards().close(key);
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
    forwards().close_all();
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
