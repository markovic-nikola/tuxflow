use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gtk4::glib;
use gtk4::prelude::*;
use vte4::prelude::*;

use crate::config::schema::{ProcessCategory, ProcessConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    Stopped,
    Running,
    Crashed,
    Restarting,
}

pub struct ManagedProcess {
    pub id: String,
    pub config: ProcessConfig,
    pub terminal: vte4::Terminal,
    pub status: ProcessStatus,
    pub pid: Option<i32>,
    pub restart_count: u32,
    pub started_at: Option<Instant>,
}

impl ManagedProcess {
    fn new(config: ProcessConfig) -> Self {
        let terminal = vte4::Terminal::new();
        terminal.set_scroll_on_output(true);
        terminal.set_scroll_on_keystroke(true);
        terminal.set_scrollback_lines(10000);
        terminal.set_vexpand(true);
        terminal.set_hexpand(true);

        let font_desc = gtk4::pango::FontDescription::from_string("Monospace 12");
        terminal.set_font(Some(&font_desc));

        let id = config.name.clone();

        Self {
            id,
            config,
            terminal,
            status: ProcessStatus::Stopped,
            pid: None,
            restart_count: 0,
            started_at: None,
        }
    }
}

pub type ProcessManagerRef = Rc<RefCell<ProcessManager>>;

pub struct ProcessManager {
    processes: HashMap<String, ManagedProcess>,
    order: Vec<String>,
    on_status_change: Option<Box<dyn Fn(&str, ProcessStatus)>>,
}

impl ProcessManager {
    pub fn new() -> ProcessManagerRef {
        Rc::new(RefCell::new(Self {
            processes: HashMap::new(),
            order: Vec::new(),
            on_status_change: None,
        }))
    }

    pub fn set_on_status_change(&mut self, cb: impl Fn(&str, ProcessStatus) + 'static) {
        self.on_status_change = Some(Box::new(cb));
    }

    pub fn add_process(&mut self, config: ProcessConfig) {
        let name = config.name.clone();
        let proc = ManagedProcess::new(config);
        self.order.push(name.clone());
        self.processes.insert(name, proc);
    }

    pub fn spawn(&mut self, name: &str) {
        let Some(proc) = self.processes.get_mut(name) else {
            log::warn!("Process not found: {name}");
            return;
        };

        if proc.status == ProcessStatus::Running {
            log::info!("Process {name} already running");
            return;
        }

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let command = &proc.config.command;

        // Build argv: shell -c "command"
        let argv = [shell.as_str(), "-c", command.as_str()];

        // Build envv from config
        let env_strings: Vec<String> = proc
            .config
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let env_refs: Vec<&str> = env_strings.iter().map(|s| s.as_str()).collect();

        let working_dir = proc.config.working_dir.clone();

        let name_clone = name.to_string();

        proc.terminal.spawn_async(
            vte4::PtyFlags::DEFAULT,
            working_dir.as_deref(),
            &argv,
            &env_refs,
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            gtk4::gio::Cancellable::NONE,
            move |result| match result {
                Ok(pid) => {
                    log::info!("Spawned process {name_clone} with PID {pid:?}");
                }
                Err(e) => {
                    log::error!("Failed to spawn process {name_clone}: {e}");
                }
            },
        );

        proc.status = ProcessStatus::Running;
        proc.started_at = Some(Instant::now());
        proc.restart_count = 0;

        let name_owned = name.to_string();
        if let Some(ref cb) = self.on_status_change {
            cb(&name_owned, ProcessStatus::Running);
        }
    }

    pub fn kill(&mut self, name: &str) {
        let Some(proc) = self.processes.get_mut(name) else {
            return;
        };

        if proc.status != ProcessStatus::Running {
            return;
        }

        // Reset the terminal (kills the child process)
        proc.terminal.reset(true, true);

        proc.status = ProcessStatus::Stopped;
        proc.pid = None;
        proc.started_at = None;

        if let Some(ref cb) = self.on_status_change {
            cb(name, ProcessStatus::Stopped);
        }

        log::info!("Killed process {name}");
    }

    pub fn restart(&mut self, name: &str) {
        self.kill(name);
        self.spawn(name);
    }

    pub fn spawn_auto_start(&mut self) {
        let auto_start_names: Vec<String> = self
            .processes
            .values()
            .filter(|p| p.config.auto_start)
            .map(|p| p.config.name.clone())
            .collect();

        for name in auto_start_names {
            self.spawn(&name);
        }
    }

    pub fn get_process(&self, name: &str) -> Option<&ManagedProcess> {
        self.processes.get(name)
    }

    pub fn get_process_mut(&mut self, name: &str) -> Option<&mut ManagedProcess> {
        self.processes.get_mut(name)
    }

    pub fn process_names(&self) -> &[String] {
        &self.order
    }

    pub fn processes_by_category(&self, category: ProcessCategory) -> Vec<&ManagedProcess> {
        self.order
            .iter()
            .filter_map(|name| self.processes.get(name))
            .filter(|p| p.config.category == category)
            .collect()
    }

    pub fn running_count(&self) -> usize {
        self.processes
            .values()
            .filter(|p| p.status == ProcessStatus::Running)
            .count()
    }

    pub fn total_count(&self) -> usize {
        self.processes.len()
    }

    pub fn stop_all(&mut self) {
        let names: Vec<String> = self
            .processes
            .values()
            .filter(|p| p.status == ProcessStatus::Running)
            .map(|p| p.config.name.clone())
            .collect();
        for name in names {
            self.kill(&name);
        }
    }

    pub fn restart_all(&mut self) {
        let names: Vec<String> = self
            .processes
            .values()
            .filter(|p| p.status == ProcessStatus::Running)
            .map(|p| p.config.name.clone())
            .collect();
        for name in names {
            self.restart(&name);
        }
    }
}
