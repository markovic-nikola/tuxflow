use std::cell::RefCell;
use std::collections::HashMap;
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

impl ResourceMonitor {
    pub fn new() -> Self {
        Self {
            prev_ticks: HashMap::new(),
        }
    }

    pub fn sample(&mut self, pid: i32) -> Option<ProcessResources> {
        let memory_mb = self.read_memory(pid)?;
        let cpu_percent = self.read_cpu(pid)?;

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

    fn read_cpu(&mut self, pid: i32) -> Option<f64> {
        // Read process CPU ticks
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let fields: Vec<&str> = stat.split_whitespace().collect();
        if fields.len() < 17 {
            return None;
        }
        let utime: u64 = fields[13].parse().ok()?;
        let stime: u64 = fields[14].parse().ok()?;
        let process_ticks = utime + stime;

        // Read total system CPU ticks
        let proc_stat = fs::read_to_string("/proc/stat").ok()?;
        let cpu_line = proc_stat.lines().next()?;
        let total_ticks: u64 = cpu_line
            .split_whitespace()
            .skip(1)
            .filter_map(|s| s.parse::<u64>().ok())
            .sum();

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

    /// Clean up stale PID entries that are no longer tracked
    pub fn remove_stale(&mut self, active_pids: &[i32]) {
        self.prev_ticks.retain(|pid, _| active_pids.contains(pid));
    }
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
