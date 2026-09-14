//! Restart-on-file-change for a local project. The glob rules and the
//! debounced directory watch live in core (`util/watch.rs`, shared with
//! the iced shell); this is the GTK plumbing — the watcher thread's
//! reports cross to the GLib main loop over a channel drained every 250 ms,
//! and the restart goes through the ProcessManager like every other.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use gtk4::glib;
use tuxflow_core::util::watch::{self, WatchSet};

use crate::process::manager::ProcessManagerRef;

pub struct FileWatcher {
    _watcher: watch::Watcher,
}

impl FileWatcher {
    pub fn new(project_dir: &Path, manager: &ProcessManagerRef) -> Option<Self> {
        let set = {
            let mgr = manager.borrow();
            WatchSet::from_configs(
                mgr.process_names()
                    .into_iter()
                    .filter_map(|name| mgr.get_process(name).map(|p| &p.config)),
            )
        };
        if set.is_empty() {
            log::info!("No file watch patterns configured");
            return None;
        }

        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let watcher = watch::start(project_dir, move |paths| {
            let _ = tx.send(paths);
        })?;

        // Process events on the GLib main loop
        let manager_ref = manager.clone();
        let project_dir = project_dir.to_path_buf();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            while let Ok(paths) = rx.try_recv() {
                for name in set.matches_any(&project_dir, &paths) {
                    log::info!("File change matched for process '{name}', restarting");
                    let cb = manager_ref.borrow().file_watch_restart_callback();
                    manager_ref.borrow_mut().restart(name);
                    if let Some(cb) = cb {
                        cb(name);
                    }
                }
            }
            glib::ControlFlow::Continue
        });

        Some(Self { _watcher: watcher })
    }
}
