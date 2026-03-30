use gtk4::glib;
use vte4::prelude::*;

use super::manager::{ProcessManagerRef, ProcessStatus};
use crate::util::notifications;

const MAX_RESTART_ATTEMPTS: u32 = 5;
const BASE_DELAY_MS: u32 = 1000;

/// Returns a closure that connects exit-detection signals to the given terminal.
/// Call this when the terminal is materialized (lazily created).
///
/// Connects both `child_exited` (primary) and `eof` (fallback) signals to
/// reliably detect when a spawned process finishes.
pub fn build_auto_restart_handler(
    manager: &ProcessManagerRef,
    process_name: &str,
    auto_restart: bool,
) -> Box<dyn Fn(&vte4::Terminal)> {
    let manager_ref = manager.clone();
    let name = process_name.to_string();

    Box::new(move |terminal: &vte4::Terminal| {
        // --- child_exited: primary exit handler (carries exit status) ---
        {
            let manager_ref = manager_ref.clone();
            let name = name.clone();

            terminal.connect_child_exited(move |_terminal, status| {
                log::debug!("child_exited fired for {name} with status {status}");
                handle_process_exit(&manager_ref, &name, Some(status), auto_restart);
            });
        }

        // --- eof: fallback for when child_exited doesn't fire ---
        {
            let manager_ref = manager_ref.clone();
            let name = name.clone();

            terminal.connect_eof(move |_terminal| {
                let mgr = manager_ref.borrow();
                let is_running = mgr
                    .get_process(&name)
                    .map(|p| p.status == ProcessStatus::Running)
                    .unwrap_or(false);
                drop(mgr);

                if is_running {
                    log::warn!(
                        "eof fired for {name} while still marked Running — \
                         child_exited was missed, handling exit via eof fallback"
                    );
                    handle_process_exit(&manager_ref, &name, None, auto_restart);
                }
            });
        }
    })
}

/// Unified exit handler used by both `child_exited` and `eof` signals.
///
/// `status` is `Some(exit_status)` from `child_exited`, or `None` from `eof`
/// (where we don't know the exit code, so we treat it as clean exit).
fn handle_process_exit(
    manager_ref: &ProcessManagerRef,
    name: &str,
    status: Option<i32>,
    auto_restart: bool,
) {
    // If already marked as Stopped (user-initiated kill or already handled), skip
    {
        let mgr = manager_ref.borrow();
        if let Some(proc) = mgr.get_process(name)
            && proc.status == ProcessStatus::Stopped
        {
            log::info!("Process {name} exited after user stop (status {status:?})");
            return;
        }
    }

    // Treat exit code 0 (or unknown from eof) as clean exit
    let is_clean = status.map(|s| s == 0).unwrap_or(true);

    if is_clean {
        log::info!("Process {name} exited cleanly (status {status:?})");
        {
            let mut mgr = manager_ref.borrow_mut();
            if let Some(proc) = mgr.get_process_mut(name) {
                proc.status = ProcessStatus::Stopped;
                proc.pid = None;
                proc.started_at = None;
            }
        }
        manager_ref
            .borrow()
            .notify_status_change(name, ProcessStatus::Stopped);
        return;
    }

    let status = status.unwrap();
    log::warn!("Process {name} crashed (exit status {status})");

    if !auto_restart {
        {
            let mut mgr = manager_ref.borrow_mut();
            if let Some(proc) = mgr.get_process_mut(name) {
                proc.status = ProcessStatus::Crashed;
                proc.pid = None;
                proc.started_at = None;
            }
        }
        manager_ref
            .borrow()
            .notify_status_change(name, ProcessStatus::Crashed);
        notifications::notify_crash(name);
        return;
    }

    let restart_count;
    {
        let mut mgr = manager_ref.borrow_mut();
        let Some(proc) = mgr.get_process_mut(name) else {
            return;
        };

        proc.restart_count += 1;
        restart_count = proc.restart_count;

        if restart_count > MAX_RESTART_ATTEMPTS {
            log::error!(
                "Process {name} exceeded max restart attempts ({MAX_RESTART_ATTEMPTS}), giving up"
            );
            proc.status = ProcessStatus::Crashed;
        }
    }

    if restart_count > MAX_RESTART_ATTEMPTS {
        manager_ref
            .borrow()
            .notify_status_change(name, ProcessStatus::Crashed);
        notifications::notify_crash(name);
        return;
    }

    {
        let mut mgr = manager_ref.borrow_mut();
        if let Some(proc) = mgr.get_process_mut(name) {
            proc.status = ProcessStatus::Restarting;
        }
    }
    manager_ref
        .borrow()
        .notify_status_change(name, ProcessStatus::Restarting);
    notifications::notify_restart(name, restart_count);

    let delay = BASE_DELAY_MS * 2u32.pow(restart_count - 1);
    log::info!("Restarting {name} in {delay}ms (attempt {restart_count}/{MAX_RESTART_ATTEMPTS})");

    let manager_ref2 = manager_ref.clone();
    let name2 = name.to_string();
    glib::timeout_add_local_once(std::time::Duration::from_millis(delay as u64), move || {
        manager_ref2.borrow_mut().spawn(&name2);
    });
}
