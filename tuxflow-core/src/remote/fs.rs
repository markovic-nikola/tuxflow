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
    /// Like `exists_many`, but only counts non-empty regular files. Icon
    /// detection needs this — Laravel ships a 0-byte favicon.ico
    /// placeholder that would otherwise win the candidate scan and render
    /// nothing.
    fn exists_many_nonempty(&self, rels: &[&str]) -> Vec<bool> {
        self.exists_many(rels)
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

/// Raw `ls -1dp` over ssh for `prefix*`: absolute paths, dirs marked with a
/// trailing '/'. `remote_filter` is a grep -iE pattern applied on the host
/// BEFORE the result cap — filtering after `head` would let unwanted
/// entries eat the whole budget (a project root full of .md/.json files
/// would starve out the directories). Call from a worker thread only.
fn list_remote_raw(
    host: &str,
    prefix: &str,
    remote_filter: Option<&str>,
    limit: u32,
) -> Vec<String> {
    // Quote the typed prefix but leave the glob star outside the quotes so
    // the remote shell expands it. -d: don't descend, -p: mark dirs with '/'.
    let filter = remote_filter
        .map(|pat| format!(" | grep -iE {}", sh_quote(pat)))
        .unwrap_or_default();
    let cmd = format!(
        "ls -1dp -- {}* 2>/dev/null{filter} | head -n {limit}",
        sh_quote(prefix)
    );
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
            .filter(|l| l.starts_with('/'))
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// List remote directories whose absolute path starts with `prefix` (which
/// may end mid-name). Used by the add-remote-project path autocompletion.
pub fn list_remote_dirs(host: &str, prefix: &str) -> Vec<String> {
    list_remote_raw(host, prefix, Some("/$"), DIR_SUGGESTION_LIMIT)
}

/// How many completions a path field offers. Shared by both halves so a
/// local project's dropdown is never a different length than a remote one's.
const DIR_SUGGESTION_LIMIT: u32 = 10;

/// Local twin of [`list_remote_dirs`], with a deliberately identical
/// contract: absolute paths, a trailing `/` on every entry, prefix matched
/// mid-name, sorted, capped.
///
/// This mirrors `ls -1dp -- <prefix>*` rather than doing anything cleverer,
/// so one completion widget can drive both halves of the add-project dialog
/// without caring which side of an ssh connection the path lives on. That
/// includes glob's dotfile rule — a bare `*` does not match `.config`, so
/// hidden directories surface only once the user types the leading dot.
pub fn list_local_dirs(prefix: &str) -> Vec<String> {
    // Split at the last '/': everything through it is the directory to read,
    // the tail is the partial name being completed.
    let split = match prefix.rfind('/') {
        Some(i) => i + 1,
        None => return Vec::new(), // only absolute paths complete
    };
    let (parent, partial) = prefix.split_at(split);

    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(partial) {
                return false;
            }
            if partial.is_empty() && name.starts_with('.') {
                return false;
            }
            // `ls -p` marks by the resolved type, so a symlink to a
            // directory is a directory here too.
            e.path().is_dir()
        })
        .map(|e| format!("{}{}/", parent, e.file_name().to_string_lossy()))
        .collect();
    out.sort();
    out.truncate(DIR_SUGGESTION_LIMIT as usize);
    out
}

/// Directory completions for either half of the add-project dialog:
/// `host` present means the path lives on the other end of an ssh
/// connection. Blocking either way — worker threads only.
pub fn list_dirs(host: Option<&str>, prefix: &str) -> Vec<String> {
    match host {
        Some(host) => list_remote_dirs(host, prefix),
        None => list_local_dirs(prefix),
    }
}

/// Directories (for descending) plus image files — the completion set for
/// the remote icon picker. The cap is generous because that picker is a
/// scrolling dialog rather than a dropdown: a cap tuned to what fits on
/// screen would hide entries in any directory bigger than the viewport.
pub fn list_remote_icon_paths(host: &str, prefix: &str) -> Vec<String> {
    list_remote_raw(host, prefix, Some(r"(/|\.(svg|png|webp|ico|jpe?g))$"), 100)
}

/// Fetch a remote file's bytes (capped at `max_bytes`) over the shared
/// connection. None on any failure or an empty file. Blocking — worker
/// threads only. Used to pull small assets (project icons) to a local cache.
pub fn fetch_remote_file(host: &str, abs_path: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let out = Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg("--")
        .arg(format!("head -c {} -- {}", max_bytes, sh_quote(abs_path)))
        .output()
        .ok()?;
    if out.status.success() && !out.stdout.is_empty() {
        Some(out.stdout)
    } else {
        None
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

    fn exists_many_nonempty(&self, rels: &[&str]) -> Vec<bool> {
        rels.iter()
            .map(|r| {
                std::fs::metadata(self.root.join(r))
                    .map(|m| m.is_file() && m.len() > 0)
                    .unwrap_or(false)
            })
            .collect()
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
        self.exists_batch(rels, "-e")
    }

    fn exists_many_nonempty(&self, rels: &[&str]) -> Vec<bool> {
        self.exists_batch(rels, "-s")
    }
}

impl SshFs {
    /// One round trip: `test <flag>` each path, report which passed.
    fn exists_batch(&self, rels: &[&str], test_flag: &str) -> Vec<bool> {
        if rels.is_empty() {
            return Vec::new();
        }
        let quoted: Vec<String> = rels.iter().map(|r| sh_quote(&self.abs(r))).collect();
        let cmd = format!(
            "for f in {}; do test {test_flag} \"$f\" && printf '%s\\n' \"$f\"; done; true",
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
