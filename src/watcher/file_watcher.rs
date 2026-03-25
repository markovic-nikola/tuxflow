use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use gtk4::glib;
use notify::RecursiveMode;
use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};

use crate::process::manager::ProcessManagerRef;

struct WatchEntry {
    process_name: String,
    patterns: Vec<glob::Pattern>,
}

pub struct FileWatcher {
    _watcher: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
}

impl FileWatcher {
    pub fn new(project_dir: &Path, manager: &ProcessManagerRef) -> Option<Self> {
        let entries = Self::collect_watch_entries(manager);
        if entries.is_empty() {
            log::info!("No file watch patterns configured");
            return None;
        }

        let (tx, rx) = mpsc::channel();

        let mut debouncer = new_debouncer(Duration::from_millis(500), tx).ok()?;

        // Watch the project directory recursively
        debouncer
            .watcher()
            .watch(project_dir, RecursiveMode::Recursive)
            .ok()?;

        log::info!(
            "File watcher started for {} entries in {}",
            entries.len(),
            project_dir.display()
        );

        // Process events on the GLib main loop
        let manager_ref = manager.clone();
        let project_dir = project_dir.to_path_buf();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            while let Ok(Ok(events)) = rx.try_recv() {
                for event in &events {
                    if event.kind == DebouncedEventKind::Any {
                        Self::handle_change(&event.path, &project_dir, &entries, &manager_ref);
                    }
                }
            }
            glib::ControlFlow::Continue
        });

        Some(Self {
            _watcher: debouncer,
        })
    }

    fn collect_watch_entries(manager: &ProcessManagerRef) -> Vec<WatchEntry> {
        let mgr = manager.borrow();
        let mut entries = Vec::new();

        for name in mgr.process_names() {
            if let Some(proc) = mgr.get_process(name) {
                if proc.config.restart_when_changed.is_empty() {
                    continue;
                }

                let patterns: Vec<glob::Pattern> = proc
                    .config
                    .restart_when_changed
                    .iter()
                    .filter_map(|g| glob::Pattern::new(g).ok())
                    .collect();

                if !patterns.is_empty() {
                    entries.push(WatchEntry {
                        process_name: proc.config.name.clone(),
                        patterns,
                    });
                }
            }
        }

        entries
    }

    fn handle_change(
        path: &Path,
        project_dir: &Path,
        entries: &[WatchEntry],
        manager: &ProcessManagerRef,
    ) {
        let relative = path.strip_prefix(project_dir).unwrap_or(path);

        let rel_str = relative.to_string_lossy();

        for entry in entries {
            for pattern in &entry.patterns {
                if pattern.matches(&rel_str) {
                    log::info!(
                        "File change matched '{}' for process '{}', restarting",
                        rel_str,
                        entry.process_name
                    );
                    manager.borrow_mut().restart(&entry.process_name);
                    break;
                }
            }
        }
    }
}
