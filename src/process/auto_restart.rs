use gtk4::glib;
use vte4::prelude::*;

use super::manager::{ProcessManagerRef, ProcessStatus};
use crate::util::notifications;

const MAX_RESTART_ATTEMPTS: u32 = 5;
const BASE_DELAY_MS: u32 = 1000;

pub fn setup_auto_restart(manager: &ProcessManagerRef, process_name: &str) {
    let mgr = manager.borrow();
    let Some(proc) = mgr.get_process(process_name) else {
        return;
    };

    let auto_restart = proc.config.auto_restart;
    let manager_ref = manager.clone();
    let name = process_name.to_string();

    proc.terminal.connect_child_exited(move |_terminal, status| {
        // If already marked as Stopped (user-initiated kill), don't treat as crash
        {
            let mgr = manager_ref.borrow();
            if let Some(proc) = mgr.get_process(&name)
                && proc.status == ProcessStatus::Stopped {
                    log::info!("Process {name} exited after user stop (status {status})");
                    return;
                }
        }

        if status == 0 {
            log::info!("Process {name} exited cleanly (status 0)");
            let mut mgr = manager_ref.borrow_mut();
            if let Some(proc) = mgr.get_process_mut(&name) {
                proc.status = ProcessStatus::Stopped;
                proc.pid = None;
                proc.started_at = None;
            }
            mgr.notify_status_change(&name, ProcessStatus::Stopped);
            return;
        }

        log::warn!("Process {name} crashed (exit status {status})");

        if !auto_restart {
            let mut mgr = manager_ref.borrow_mut();
            if let Some(proc) = mgr.get_process_mut(&name) {
                proc.status = ProcessStatus::Crashed;
                proc.pid = None;
                proc.started_at = None;
            }
            mgr.notify_status_change(&name, ProcessStatus::Crashed);
            notifications::notify_crash(&name);
            return;
        }

        let restart_count;
        {
            let mut mgr = manager_ref.borrow_mut();
            let Some(proc) = mgr.get_process_mut(&name) else {
                return;
            };

            proc.restart_count += 1;
            restart_count = proc.restart_count;

            if restart_count > MAX_RESTART_ATTEMPTS {
                log::error!(
                    "Process {name} exceeded max restart attempts ({MAX_RESTART_ATTEMPTS}), giving up"
                );
                proc.status = ProcessStatus::Crashed;
                mgr.notify_status_change(&name, ProcessStatus::Crashed);
                notifications::notify_crash(&name);
                return;
            }

            proc.status = ProcessStatus::Restarting;
            mgr.notify_status_change(&name, ProcessStatus::Restarting);
            notifications::notify_restart(&name, restart_count);
        }

        let delay = BASE_DELAY_MS * 2u32.pow(restart_count - 1);
        log::info!(
            "Restarting {name} in {delay}ms (attempt {restart_count}/{MAX_RESTART_ATTEMPTS})"
        );

        let manager_ref2 = manager_ref.clone();
        let name2 = name.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay as u64), move || {
            manager_ref2.borrow_mut().spawn(&name2);
        });
    });
}
