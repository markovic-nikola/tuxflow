use std::collections::HashMap;
use std::process::{Child, Command, Stdio};

/// One `ssh -N -L` forward: remote port → `local_port` on this machine.
/// `local_port` usually equals the remote port, but is remapped to a free
/// ephemeral port when the preferred one is already taken locally.
struct Tunnel {
    local_port: u16,
    child: Child,
}

/// Local port forwards (`ssh -N -L`) for one remote project, keyed by the
/// *remote* port. Each tunnel is a dedicated ssh connection, deliberately
/// NOT multiplexed over the shared ControlMaster: forwards requested through
/// a mux client live in the master and survive the client being killed, so
/// close()/app-exit would leak open ports. A dedicated process owns its
/// forward — kill the process, the port closes. PDEATHSIG makes the kernel
/// deliver that kill if TuxFlow itself dies.
pub struct TunnelManager {
    host: String,
    tunnels: HashMap<u16, Tunnel>,
}

fn local_port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Ask the OS for a free ephemeral port. Racy by nature (freed before ssh
/// re-binds it), but collisions in that window are vanishingly rare.
fn free_ephemeral_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .ok()
}

impl TunnelManager {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            tunnels: HashMap::new(),
        }
    }

    /// Ensure a `localhost:<local> -> remote:<port>` forward exists, preferring
    /// `local == port` and remapping to a free ephemeral port when it's taken.
    /// Returns the local port to use, or None if the tunnel could not spawn.
    pub fn ensure(&mut self, port: u16) -> Option<u16> {
        self.ensure_with(port, true)
    }

    /// Like [`Self::ensure`], but never remaps: the forward either listens on
    /// `port` itself or does not come up. For a port the *remote* side has
    /// already baked into a URL it serves — Vite writes its dev-server address
    /// into `public/hot`, and the page tells this machine's browser to fetch
    /// assets from exactly that port — a remapped forward would be listening
    /// where nothing ever knocks.
    pub fn ensure_exact(&mut self, port: u16) -> Option<u16> {
        self.ensure_with(port, false)
    }

    fn ensure_with(&mut self, port: u16, remap: bool) -> Option<u16> {
        // Reap a dead tunnel (e.g. the local port got taken mid-run), and
        // replace a remapped one when an exact forward is required: output
        // scanning may already have opened this same remote port off on some
        // ephemeral local port, which an exact caller cannot use.
        let stale = match self.tunnels.get_mut(&port) {
            Some(tunnel) => match tunnel.child.try_wait() {
                Ok(None) if remap || tunnel.local_port == port => {
                    return Some(tunnel.local_port); // still running, usable
                }
                Ok(None) => true,
                _ => true,
            },
            None => false,
        };
        if stale {
            self.close(port);
        }
        let local_port = if local_port_available(port) {
            port
        } else if remap {
            let Some(free) = free_ephemeral_port() else {
                log::error!("No free local port to forward remote port {port}");
                return None;
            };
            log::info!("Local port {port} is taken — remapping to {free}");
            free
        } else {
            log::warn!("Local port {port} is taken — cannot forward remote port {port} 1:1");
            return None;
        };
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
            "-L",
            &format!("{local_port}:localhost:{port}"),
        ])
        .arg(&self.host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        // Die with TuxFlow: without this, an app crash (or any exit path
        // that skips Drop) leaves the forward listening forever.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGTERM, 0, 0, 0);
                Ok(())
            });
        }
        let spawned = cmd.spawn();
        match spawned {
            Ok(child) => {
                log::info!("Tunnel localhost:{local_port} -> {}:{port}", self.host);
                self.tunnels.insert(port, Tunnel { local_port, child });
                Some(local_port)
            }
            Err(e) => {
                log::error!("Failed to spawn tunnel for port {port}: {e}");
                None
            }
        }
    }

    pub fn close(&mut self, port: u16) {
        if let Some(mut tunnel) = self.tunnels.remove(&port) {
            let _ = tunnel.child.kill();
            let _ = tunnel.child.wait();
        }
    }

    pub fn close_all(&mut self) {
        let ports: Vec<u16> = self.tunnels.keys().copied().collect();
        for port in ports {
            self.close(port);
        }
    }
}

impl Drop for TunnelManager {
    fn drop(&mut self) {
        self.close_all();
    }
}
