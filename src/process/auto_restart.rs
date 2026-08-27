use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gtk4::glib;
use vte4::prelude::*;

use super::manager::{ProcessManagerRef, ProcessStatus};
use crate::config::settings::AppSettings;
use crate::util::notifications::{self, AgentKind};
use crate::workspace;

const MAX_RESTART_ATTEMPTS: u32 = 5;
const BASE_DELAY_MS: u32 = 1000;
/// Backoff doubles up to this exponent (2^5 → 32 s), then stays flat.
/// Matters for connection-loss reconnects, which retry indefinitely.
const MAX_BACKOFF_EXPONENT: u32 = 5;
/// A run at least this long before exiting counts as a new incident:
/// the attempt counter resets instead of accumulating across hours.
const STABLE_RUN_SECS: u64 = 60;

/// Shared name cell that allows the auto-restart handler to track renames.
pub type ProcessNameCell = Rc<RefCell<String>>;

/// VTE's `child-exited` delivers the raw waitpid status (exit 127 arrives as
/// 32512). Decode it to the conventional shell exit code: `exit N` for normal
/// exits, `128 + signal` for signal deaths — everything downstream (crash vs
/// connection-loss classification, "command not found" hints) expects that.
fn decode_exit_status(raw: i32) -> i32 {
    if raw & 0x7f == 0 {
        (raw >> 8) & 0xff
    } else {
        128 + (raw & 0x7f)
    }
}

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
                let code = decode_exit_status(status);
                log::debug!("child_exited fired for {name}: raw status {status}, exit code {code}");
                handle_process_exit(
                    &manager_ref,
                    &project_name,
                    &name,
                    Some(code),
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

    // Exit 255 is ssh's own "connection failed/lost" exit code. For a process
    // of a remote project that means the link dropped, not that the command
    // crashed — surface it as a disconnect/reconnect instead of a crash.
    // (SSH-category terminals are user-managed ssh sessions; leave those be.)
    let is_connection_loss = status == 255 && {
        let mgr = manager_ref.borrow();
        mgr.location().is_remote()
            && mgr
                .get_process(name)
                .is_some_and(|p| p.config.category != crate::config::schema::ProcessCategory::SSH)
    };
    if is_connection_loss {
        log::warn!("Process {name} lost its ssh connection (exit 255)");
    } else {
        log::warn!("Process {name} crashed (exit status {status})");
    }

    // Explain the exit inside the terminal itself (the wording is core's,
    // shared with the iced shell — both keep a process's terminal across
    // runs, so both need the same marker for why a run ended).
    {
        let mgr = manager_ref.borrow();
        if let Some(proc) = mgr.get_process(name)
            && let Some(term) = proc.terminal.as_ref()
            && let Some(msg) = tuxflow_core::util::banner::exit_banner(
                status,
                is_connection_loss,
                &proc.config.command,
                mgr.location().host(),
            )
        {
            term.feed(msg.as_bytes());
        }
    }

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
            if is_connection_loss {
                notifications::notify_disconnect(project_name, name, resolve_icon().as_deref());
            } else {
                notifications::notify_crash(project_name, name, resolve_icon().as_deref());
            }
        }
        return;
    }

    let restart_count;
    {
        let mut mgr = manager_ref.borrow_mut();
        let Some(proc) = mgr.get_process_mut(name) else {
            return;
        };

        // A stable run before this exit starts a new incident — don't let
        // attempts from failures hours apart accumulate into a give-up.
        if proc
            .started_at
            .is_some_and(|t| t.elapsed().as_secs() >= STABLE_RUN_SECS)
        {
            proc.restart_count = 0;
        }
        proc.restart_count += 1;
        restart_count = proc.restart_count;

        // Connection loss never gives up: the process (tmux-wrapped) is
        // still running detached on the host — keep trying to reattach
        // until the link comes back or the user stops it.
        if !is_connection_loss && restart_count > MAX_RESTART_ATTEMPTS {
            log::error!(
                "Process {name} exceeded max restart attempts ({MAX_RESTART_ATTEMPTS}), giving up"
            );
            proc.status = ProcessStatus::Crashed;
        }
    }

    if !is_connection_loss && restart_count > MAX_RESTART_ATTEMPTS {
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
        if is_connection_loss {
            // One notification per outage — reconnect attempts are unlimited
            // and a long outage would otherwise fire one every backoff tick.
            if restart_count == 1 {
                notifications::notify_reconnect(
                    project_name,
                    name,
                    restart_count,
                    resolve_icon().as_deref(),
                );
            }
        } else {
            notifications::notify_restart(
                project_name,
                name,
                restart_count,
                resolve_icon().as_deref(),
            );
        }
    }

    let delay = BASE_DELAY_MS * 2u32.pow((restart_count - 1).min(MAX_BACKOFF_EXPONENT));
    if is_connection_loss {
        log::info!("Reconnecting {name} in {delay}ms (attempt {restart_count})");
    } else {
        log::info!(
            "Restarting {name} in {delay}ms (attempt {restart_count}/{MAX_RESTART_ATTEMPTS})"
        );
    }

    let manager_ref2 = manager_ref.clone();
    let name2 = name.to_string();
    glib::timeout_add_local_once(std::time::Duration::from_millis(delay as u64), move || {
        manager_ref2.borrow_mut().spawn(&name2);
    });
}

/// Connects VTE `bell` + `contents-changed` handlers for Agent-category
/// processes, feeding the per-project idle-silence ticker via `last_activity`
/// and firing `notify_agent_idle` on BEL.
///
/// The returned closure is meant to run after `build_auto_restart_handler`'s
/// closure on the same terminal — both attach signals, neither replaces the
/// other.
pub fn build_agent_idle_handler(
    project_name: &str,
    process_name: &str,
    kind: AgentKind,
    last_activity: Rc<Cell<Instant>>,
    activity_burst: Rc<Cell<u32>>,
    is_idle: Rc<Cell<bool>>,
    focus_gate: Option<FocusGate>,
    icon_resolver: Option<IconResolver>,
) -> Box<dyn Fn(&vte4::Terminal)> {
    let project_name = project_name.to_string();
    let process_name = process_name.to_string();

    Box::new(move |terminal: &vte4::Terminal| {
        // contents-changed: stamp activity + count the burst + reset the
        // idle edge-trigger.
        {
            let la = last_activity.clone();
            let burst = activity_burst.clone();
            let idle = is_idle.clone();
            terminal.connect_contents_changed(move |_| {
                la.set(Instant::now());
                burst.set(burst.get().saturating_add(1));
                idle.set(false);
            });
        }

        // bell: primary "agent waiting for input" signal.
        {
            let project_name = project_name.clone();
            let process_name = process_name.clone();
            let focus_gate = focus_gate.clone();
            let icon_resolver = icon_resolver.clone();
            terminal.connect_bell(move |_| {
                let settings = AppSettings::load();
                if !settings.notifications.on_agent_idle {
                    return;
                }
                if !should_notify(&settings, &project_name, &process_name, focus_gate.as_ref()) {
                    return;
                }
                let icon = icon_resolver.as_ref().and_then(|r| r(&project_name));
                notifications::notify_agent_idle(
                    &project_name,
                    &process_name,
                    icon.as_deref(),
                    kind,
                );
            });
        }
    })
}

/// Per-tick check for the silence-fallback. Called by the project's
/// `glib::timeout_add_local` ticker once per agent. Returns true when a
/// notification was fired (caller just needs to know the edge-trigger state
/// was flipped).
pub fn check_agent_silence(
    project_name: &str,
    process_name: &str,
    kind: AgentKind,
    last_activity: &Cell<Instant>,
    is_idle: &Cell<bool>,
    threshold_seconds: u32,
    focus_gate: Option<&FocusGate>,
    icon_resolver: Option<&IconResolver>,
) -> bool {
    if is_idle.get() {
        return false;
    }
    let elapsed = last_activity.get().elapsed();
    if elapsed.as_secs() < threshold_seconds as u64 {
        return false;
    }
    let settings = AppSettings::load();
    if !settings.notifications.on_agent_idle
        || !settings.notifications.on_agent_idle_silence_fallback
    {
        return false;
    }
    is_idle.set(true);
    if !should_notify(&settings, project_name, process_name, focus_gate) {
        return false;
    }
    let icon = icon_resolver.and_then(|r| r(project_name));
    notifications::notify_agent_idle(project_name, process_name, icon.as_deref(), kind);
    true
}

#[cfg(test)]
mod tests {
    use super::decode_exit_status;

    #[test]
    fn decodes_raw_waitpid_statuses() {
        assert_eq!(decode_exit_status(0), 0); // clean exit
        assert_eq!(decode_exit_status(32512), 127); // command not found
        assert_eq!(decode_exit_status(32256), 126); // not executable
        assert_eq!(decode_exit_status(65280), 255); // ssh connection loss
        assert_eq!(decode_exit_status(256), 1); // plain crash, exit 1
        assert_eq!(decode_exit_status(15), 128 + 15); // SIGTERM death
    }
}
