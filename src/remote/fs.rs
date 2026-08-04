use std::path::PathBuf;
use std::process::Command;

use super::{sh_quote, ssh_mux_options};

/// Read cap for remote files — marker files (package.json, tuxflow.toml, …)
/// are small; this guards against `cat`-ing something huge over the wire.
const REMOTE_READ_CAP: &str = "262144";

/// Minimal project-filesystem abstraction: exactly what config loading and
/// stack detection need. Paths are relative to the project root.
pub trait ProjectFs: Send + Sync {
    fn read_to_string(&self, rel: &str) -> std::io::Result<String>;
    fn exists(&self, rel: &str) -> bool;
    /// Batched existence check — one round trip over ssh for detection's marker files.
    fn exists_many(&self, rels: &[&str]) -> Vec<bool> {
        rels.iter().map(|r| self.exists(r)).collect()
    }
}

/// Check a directory exists on `host`, distinguishing "no such dir"
/// (Ok(false)) from connection/auth failure (Err). ssh exits 255 on its own
/// errors; the remote `test` exits 0/1. BatchMode so this never hangs on a
/// password prompt — callers surface Err as "authenticate first".
pub fn remote_dir_exists(host: &str, dir: &str) -> Result<bool, String> {
    let out = Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg("--")
        .arg(format!("test -d {}", sh_quote(dir)))
        .output()
        .map_err(|e| format!("Failed to run ssh: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "Could not connect to {host}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// List remote directories whose absolute path starts with `prefix` (which
/// may end mid-name). One ssh round trip; returns absolute paths with a
/// trailing '/'. Used by the add-remote-project path autocompletion — call
/// from a worker thread only.
pub fn list_remote_dirs(host: &str, prefix: &str) -> Vec<String> {
    // Quote the typed prefix but leave the glob star outside the quotes so
    // the remote shell expands it. -d: don't descend, -p: mark dirs with '/'.
    let cmd = format!("ls -1dp -- {}* 2>/dev/null | head -n 10", sh_quote(prefix));
    let out = Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg("--")
        .arg(cmd)
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| l.ends_with('/') && l.starts_with('/'))
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

pub struct LocalFs {
    root: PathBuf,
}

impl LocalFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl ProjectFs for LocalFs {
    fn read_to_string(&self, rel: &str) -> std::io::Result<String> {
        std::fs::read_to_string(self.root.join(rel))
    }

    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }
}

/// Project filesystem on an ssh host, one exec per operation over the
/// shared ControlMaster connection. BatchMode: probes must never hang on
/// an interactive auth prompt — the add-project flow authenticates first.
pub struct SshFs {
    host: String,
    root: String,
}

impl SshFs {
    pub fn new(host: impl Into<String>, root: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            root: root.into(),
        }
    }

    fn ssh_exec(&self, remote_cmd: &str) -> std::io::Result<std::process::Output> {
        Command::new("ssh")
            .args(ssh_mux_options())
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
            .arg(&self.host)
            .arg("--")
            .arg(remote_cmd)
            .output()
    }

    fn abs(&self, rel: &str) -> String {
        format!("{}/{}", self.root.trim_end_matches('/'), rel)
    }
}

impl ProjectFs for SshFs {
    fn read_to_string(&self, rel: &str) -> std::io::Result<String> {
        let cmd = format!(
            "head -c {} -- {}",
            REMOTE_READ_CAP,
            sh_quote(&self.abs(rel))
        );
        let out = self.ssh_exec(&cmd)?;
        if !out.status.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "ssh read {}:{} failed: {}",
                    self.host,
                    rel,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        String::from_utf8(out.stdout)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    fn exists(&self, rel: &str) -> bool {
        let cmd = format!("test -e {}", sh_quote(&self.abs(rel)));
        self.ssh_exec(&cmd)
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn exists_many(&self, rels: &[&str]) -> Vec<bool> {
        if rels.is_empty() {
            return Vec::new();
        }
        let quoted: Vec<String> = rels.iter().map(|r| sh_quote(&self.abs(r))).collect();
        let cmd = format!(
            "for f in {}; do test -e \"$f\" && printf '%s\\n' \"$f\"; done; true",
            quoted.join(" ")
        );
        let Ok(out) = self.ssh_exec(&cmd) else {
            return vec![false; rels.len()];
        };
        let found: std::collections::HashSet<&str> = std::str::from_utf8(&out.stdout)
            .unwrap_or("")
            .lines()
            .collect();
        rels.iter()
            .map(|r| {
                let abs = self.abs(r);
                found.contains(abs.as_str())
            })
            .collect()
    }
}
