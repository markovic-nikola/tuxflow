use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// Tracks PIDs of running child processes on disk so orphans can be
/// detected after a crash.
pub struct PidFile {
    path: PathBuf,
    pids: HashSet<i32>,
}

impl PidFile {
    pub fn new() -> Self {
        let path = Self::file_path();
        let pids = Self::read_from_disk(&path);
        Self { path, pids }
    }

    pub fn add(&mut self, pid: i32) {
        self.pids.insert(pid);
        self.save();
    }

    pub fn remove(&mut self, pid: i32) {
        self.pids.remove(&pid);
        self.save();
    }

    pub fn clear(&mut self) {
        self.pids.clear();
        let _ = fs::remove_file(&self.path);
    }

    /// Returns PIDs from the previous run that are still alive.
    pub fn orphaned_pids() -> Vec<i32> {
        let path = Self::file_path();
        let pids = Self::read_from_disk(&path);
        pids.into_iter()
            .filter(|&pid| is_process_alive(pid))
            .collect()
    }

    /// Kill a list of orphaned PIDs (SIGTERM then SIGKILL).
    pub fn kill_orphans(pids: &[i32]) {
        for &pid in pids {
            let neg = nix::unistd::Pid::from_raw(-pid);
            let pos = nix::unistd::Pid::from_raw(pid);
            // Try process group first, fall back to single process
            let _ = nix::sys::signal::kill(neg, nix::sys::signal::Signal::SIGTERM);
            let _ = nix::sys::signal::kill(pos, nix::sys::signal::Signal::SIGTERM);
        }
        // Force kill after a short delay
        let pids_owned: Vec<i32> = pids.to_vec();
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            for pid in &pids_owned {
                let neg = nix::unistd::Pid::from_raw(-pid);
                let pos = nix::unistd::Pid::from_raw(*pid);
                let _ = nix::sys::signal::kill(neg, nix::sys::signal::Signal::SIGKILL);
                let _ = nix::sys::signal::kill(pos, nix::sys::signal::Signal::SIGKILL);
            }
        });
        // Clean up the stale pid file
        let _ = fs::remove_file(Self::file_path());
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let content: String = self
            .pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = fs::write(&self.path, content);
    }

    fn file_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("tuxflow")
            .join("running.pid")
    }

    fn read_from_disk(path: &PathBuf) -> HashSet<i32> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse::<i32>().ok())
            .collect()
    }
}

fn is_process_alive(pid: i32) -> bool {
    // signal 0 checks if process exists without sending a real signal
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}
