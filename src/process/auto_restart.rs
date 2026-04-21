use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::glib;
use vte4::prelude::*;

use super::manager::{ProcessManagerRef, ProcessStatus};
use crate::config::settings::AppSettings;
use crate::util::notifications;
use crate::workspace;

const MAX_RESTART_ATTEMPTS: u32 = 5;
const BASE_DELAY_MS: u32 = 1000;

/// Shared name cell that allows the auto-restart handler to track renames.
pub type ProcessNameCell = Rc<RefCell<String>>;

/// Closure that returns true when a notification for the given qname should fire
/// (i.e. the user isn't already looking at that terminal). Optional — None means
/// never suppress.
pub type FocusGate = Rc<dyn Fn(&str) -> bool>;

/// Closure that resolves a project name to its current icon path. Kept behind a
/// closure so this module doesn't need to know about `Workspace`.
pub type IconResolver = Rc<dyn Fn(&str) -> Option<PathBuf>>;

/// Returns a closure that connects exit-detection signals to the given terminal,
/// plus a shared name cell. When the process is renamed, update the cell so the
/// signal handlers use the current name instead of the stale original.
pub fn build_auto_restart_handler(
    manager: &ProcessManagerRef,
    project_name: &str,
    process_name: &str,
    auto_restart: bool,
    focus_gate: Option<FocusGate>,
    icon_resolver: Option<IconResolver>,
) -> (Box<dyn Fn(&vte4::Terminal)>, ProcessNameCell) {
    let manager_ref = manager.clone();
    let name_cell: ProcessNameCell = Rc::new(RefCell::new(process_name.to_string()));
    let name_cell_ret = name_cell.clone();
    let project_name = project_name.to_string();

    let handler = Box::new(move |terminal: &vte4::Terminal| {
        // --- child_exited: primary exit handler (carries exit status) ---
        {
            let manager_ref = manager_ref.clone();
            let name_cell = name_cell.clone();
            let project_name = project_name.clone();
            let focus_gate = focus_gate.clone();
            let icon_resolver = icon_resolver.clone();

            terminal.connect_child_exited(move |_terminal, status| {
                let name = name_cell.borrow().clone();
                log::debug!("child_exited fired for {name} with status {status}");
                handle_process_exit(
                    &manager_ref,
                    &project_name,
                    &name,
                    Some(status),
                    auto_restart,
                    focus_gate.as_ref(),
                    icon_resolver.as_ref(),
                );
            });
        }

        // --- eof: fallback for when child_exited doesn't fire ---
        {
            let manager_ref = manager_ref.clone();
            let name_cell = name_cell.clone();
            let project_name = project_name.clone();
            let focus_gate = focus_gate.clone();
            let icon_resolver = icon_resolver.clone();

            terminal.connect_eof(move |_terminal| {
                let name = name_cell.borrow().clone();
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
                    handle_process_exit(
                        &manager_ref,
                        &project_name,
                        &name,
                        None,
                        auto_restart,
                        focus_gate.as_ref(),
                        icon_resolver.as_ref(),
                    );
                }
            });
        }
    });

    (handler, name_cell_ret)
}

/// Returns true when a notification for this qname should actually fire. Respects
/// the `suppress_when_focused` setting and the caller-supplied focus gate.
fn should_notify(
    settings: &AppSettings,
    project_name: &str,
    process_name: &str,
    focus_gate: Option<&FocusGate>,
) -> bool {
    if !settings.notifications.suppress_when_focused {
        return true;
    }
    let qname = workspace::qualified_name(project_name, process_name);
    focus_gate.map(|g| g(&qname)).unwrap_or(true)
}

/// Unified exit handler used by both `child_exited` and `eof` signals.
///
/// `status` is `Some(exit_status)` from `child_exited`, or `None` from `eof`
/// (where we don't know the exit code, so we treat it as clean exit).
fn handle_process_exit(
    manager_ref: &ProcessManagerRef,
    project_name: &str,
    name: &str,
    status: Option<i32>,
    auto_restart: bool,
    focus_gate: Option<&FocusGate>,
    icon_resolver: Option<&IconResolver>,
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

    let resolve_icon = || icon_resolver.and_then(|r| r(project_name));

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

        let settings = AppSettings::load();
        if settings.notifications.on_process_finish
            && should_notify(&settings, project_name, name, focus_gate)
        {
            notifications::notify_finish(project_name, name, resolve_icon().as_deref());
        }
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
        let settings = AppSettings::load();
        if settings.notifications.on_crash
            && should_notify(&settings, project_name, name, focus_gate)
        {
            notifications::notify_crash(project_name, name, resolve_icon().as_deref());
        }
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
        let settings = AppSettings::load();
        if settings.notifications.on_crash
            && should_notify(&settings, project_name, name, focus_gate)
        {
            notifications::notify_crash(project_name, name, resolve_icon().as_deref());
        }
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
    let settings = AppSettings::load();
    if settings.notifications.on_auto_restart
        && should_notify(&settings, project_name, name, focus_gate)
    {
        notifications::notify_restart(project_name, name, restart_count, resolve_icon().as_deref());
    }

    let delay = BASE_DELAY_MS * 2u32.pow(restart_count - 1);
    log::info!("Restarting {name} in {delay}ms (attempt {restart_count}/{MAX_RESTART_ATTEMPTS})");

    let manager_ref2 = manager_ref.clone();
    let name2 = name.to_string();
    glib::timeout_add_local_once(std::time::Duration::from_millis(delay as u64), move || {
        manager_ref2.borrow_mut().spawn(&name2);
    });
}
