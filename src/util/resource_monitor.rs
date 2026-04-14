use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::rc::Rc;
use std::time::Duration;

use gtk4::glib;

#[derive(Clone, Debug, Default)]
pub struct ProcessResources {
    pub cpu_percent: f64,
    pub memory_mb: f64,
}

pub struct ResourceMonitor {
    prev_ticks: HashMap<i32, (u64, u64)>, // pid -> (process_ticks, total_ticks)
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceMonitor {
    pub fn new() -> Self {
        Self {
            prev_ticks: HashMap::new(),
        }
    }

    /// Sample resources for the entire process tree rooted at `pid`.
    pub fn sample(&mut self, pid: i32) -> Option<ProcessResources> {
        let tree_pids = collect_process_tree(pid);

        // Aggregate memory across all processes in the tree
        let memory_mb: f64 = tree_pids.iter().filter_map(|&p| self.read_memory(p)).sum();

        // Aggregate CPU across all processes in the tree
        let total_ticks = self.read_total_ticks()?;
        let cpu_percent: f64 = tree_pids
            .iter()
            .filter_map(|&p| self.read_cpu_for_pid(p, total_ticks))
            .sum();

        Some(ProcessResources {
            cpu_percent,
            memory_mb,
        })
    }

    fn read_memory(&self, pid: i32) -> Option<f64> {
        let statm = fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        let page_size = 4096u64; // Most Linux systems
        Some((resident_pages * page_size) as f64 / (1024.0 * 1024.0))
    }

    fn read_total_ticks(&self) -> Option<u64> {
        let proc_stat = fs::read_to_string("/proc/stat").ok()?;
        let cpu_line = proc_stat.lines().next()?;
        Some(
            cpu_line
                .split_whitespace()
                .skip(1)
                .filter_map(|s| s.parse::<u64>().ok())
                .sum(),
        )
    }

    fn read_cpu_for_pid(&mut self, pid: i32, total_ticks: u64) -> Option<f64> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // comm can contain spaces/parens, so find the last ')' first
        let after_comm = stat.rfind(')')?;
        let fields: Vec<&str> = stat[after_comm + 2..].split_whitespace().collect();
        // fields: [0]=state [1]=ppid ... [11]=utime [12]=stime
        if fields.len() < 13 {
            return None;
        }
        let utime: u64 = fields[11].parse().ok()?;
        let stime: u64 = fields[12].parse().ok()?;
        let process_ticks = utime + stime;

        let cpu_percent = if let Some((prev_proc, prev_total)) = self.prev_ticks.get(&pid) {
            let proc_delta = process_ticks.saturating_sub(*prev_proc) as f64;
            let total_delta = total_ticks.saturating_sub(*prev_total) as f64;
            if total_delta > 0.0 {
                (proc_delta / total_delta) * 100.0 * num_cpus() as f64
            } else {
                0.0
            }
        } else {
            0.0
        };

        self.prev_ticks.insert(pid, (process_ticks, total_ticks));
        Some(cpu_percent)
    }

    /// Clean up stale PID entries for PIDs that no longer exist
    pub fn remove_stale(&mut self, _active_pids: &[i32]) {
        self.prev_ticks
            .retain(|pid, _| std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
}

/// Collect all PIDs in the process tree rooted at `root_pid` (BFS).
fn collect_process_tree(root_pid: i32) -> Vec<i32> {
    let mut children_map: HashMap<i32, Vec<i32>> = HashMap::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
                continue;
            };
            let stat_path = entry.path().join("stat");
            if let Ok(stat) = fs::read_to_string(&stat_path) {
                if let Some(after_comm) = stat.rfind(')') {
                    let fields: Vec<&str> = stat[after_comm + 2..].split_whitespace().collect();
                    if let Some(ppid) = fields.get(1).and_then(|s| s.parse::<i32>().ok()) {
                        children_map.entry(ppid).or_default().push(pid);
                    }
                }
            }
        }
    }

    let mut result = vec![root_pid];
    let mut queue = VecDeque::new();
    queue.push_back(root_pid);
    while let Some(pid) = queue.pop_front() {
        if let Some(children) = children_map.get(&pid) {
            for &child in children {
                result.push(child);
                queue.push_back(child);
            }
        }
    }
    result
}

fn num_cpus() -> usize {
    fs::read_to_string("/proc/cpuinfo")
        .map(|s| s.matches("processor").count())
        .unwrap_or(1)
}

/// Start periodic resource monitoring.
/// `get_pids` returns current (process_name, pid) pairs.
/// `on_update` receives resource data per process.
pub fn start_monitoring(
    get_pids: impl Fn() -> Vec<(String, i32)> + 'static,
    on_update: impl Fn(&str, &ProcessResources) + 'static,
) {
    let monitor = Rc::new(RefCell::new(ResourceMonitor::new()));

    glib::timeout_add_local(Duration::from_secs(2), move || {
        let pids = get_pids();
        let active_pids: Vec<i32> = pids.iter().map(|(_, pid)| *pid).collect();
        let mut mon = monitor.borrow_mut();
        mon.remove_stale(&active_pids);

        for (name, pid) in &pids {
            if let Some(resources) = mon.sample(*pid) {
                on_update(name, &resources);
            }
        }
        glib::ControlFlow::Continue
    });
}
