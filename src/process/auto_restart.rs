use gtk4::glib;
use vte4::prelude::*;

use super::manager::{ProcessManagerRef, ProcessStatus};

const MAX_RESTART_ATTEMPTS: u32 = 5;
const BASE_DELAY_MS: u32 = 1000;

pub fn setup_auto_restart(manager: &ProcessManagerRef, process_name: &str) {
    let mgr = manager.borrow();
    let Some(proc) = mgr.get_process(process_name) else {
        return;
    };

    if !proc.config.auto_restart {
        return;
    }

    let manager_ref = manager.clone();
    let name = process_name.to_string();

    proc.terminal.connect_child_exited(move |_terminal, status| {
        if status == 0 {
            log::info!("Process {name} exited cleanly (status 0), not restarting");
            return;
        }

        log::warn!("Process {name} crashed (exit status {status})");

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
                return;
            }

            proc.status = ProcessStatus::Restarting;
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
