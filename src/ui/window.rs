use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use vte4::prelude::*;

use crate::config::keybindings::{KeybindingMap, ShortcutAction, is_modifier_key};
use crate::config::settings::AppSettings;
use crate::process::manager::{ProcessManagerRef, ProcessStatus};
use crate::process::pid_file::PidFile;
use crate::ui::add_command_dialog::AddCommandDialog;
use crate::ui::add_ssh_dialog::AddSshDialog;
use crate::ui::command_palette::CommandPalette;
use crate::ui::git_changes_dialog::{
    GitChangesDialog, commits_ahead, commits_behind, current_branch, diff_shortstat, git_fetch,
    has_git_repo, sync_with_remote, untracked_count,
};
use crate::ui::sidebar::project_list::ProjectList;
use crate::ui::status_bar::StatusBar;
use crate::ui::terminal_search::TerminalSearch;
use crate::util::port_detector::PortDetector;
use crate::workspace::{self, Workspace, WorkspaceRef};

/// Maps current project name → shared cell holding the same name.
/// Closures inside `wire_project` capture the cell instead of a `String` so
/// they keep working after the project is renamed. The window-level
/// `on_project_renamed` callback is responsible for updating the cell's
/// contents and rekeying this registry.
type ProjectNameCells = Rc<RefCell<HashMap<String, Rc<RefCell<String>>>>>;

/// Decides which terminal mouse gestures may publish a tmux selection.
///
/// tmux only stores a paste buffer for a drag (`MouseDragEnd1Pane`) or a
/// double/triple click (its word and line copies). Every other button release
/// — a click to focus a pane, a right-click, the click that dismisses a menu
/// — leaves tmux's buffer exactly as it was, so treating one as a selection
/// republishes text the user chose minutes ago over whatever they have
/// selected *now*. That is the whole bug: the gesture that overwrote the
/// selection wasn't a selection.
///
/// GTK4 dropped the multi-press event types, and GtkGestureClick (where the
/// click counting moved) is unusable here — VTE claims mouse sequences for
/// mouse-tracking apps and a claimed sequence cancels other gestures. So a
/// click sequence is recognised the way GTK recognises one: same spot, within
/// the double-click interval.
/// Why the bridge is fetching a tmux buffer, which decides what it may do
/// with it — see `tmux_buffer_to_clipboard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipRoute {
    /// A selection gesture just finished in a remote pane. Publishes only a
    /// buffer tmux made for it, to CLIPBOARD and PRIMARY both.
    Selection,
    /// The user pressed copy. Takes the newest buffer at any age — a
    /// copy-mode `y`, or an OSC 52 the agent in the pane sent.
    ExplicitCopy,
}

#[derive(Debug)]
struct SelectionGesture {
    /// Where and when the last button press landed.
    press: Option<(f64, f64, u32)>,
    /// That press hasn't been released yet.
    armed: bool,
    /// ...and it continued a click sequence (double/triple click).
    multi: bool,
    double_click_ms: u32,
    double_click_px: f64,
}

impl SelectionGesture {
    /// Pointer travel that separates a drag from a click that wobbled. Mouse
    /// reporting is per cell, so tmux cannot have begun a selection below one
    /// cell of movement — this only has to clear jitter.
    const DRAG_PX: f64 = 4.0;

    fn new(double_click_ms: u32, double_click_px: f64) -> Self {
        Self {
            press: None,
            armed: false,
            multi: false,
            double_click_ms,
            double_click_px,
        }
    }

    fn press(&mut self, x: f64, y: f64, time: u32) {
        let previous = self.press.replace((x, y, time));
        self.multi = previous.is_some_and(|(px, py, pt)| {
            time.wrapping_sub(pt) <= self.double_click_ms
                && (x - px).abs() <= self.double_click_px
                && (y - py).abs() <= self.double_click_px
        });
        self.armed = true;
    }

    /// Whether the gesture ending here could have left a new tmux selection.
    /// Consumes the press, so one press publishes at most once.
    fn release(&mut self, x: f64, y: f64) -> bool {
        let Some((px, py, _)) = self.press else {
            return false;
        };
        std::mem::replace(&mut self.armed, false)
            && (self.multi || (x - px).abs() > Self::DRAG_PX || (y - py).abs() > Self::DRAG_PX)
    }
}

pub struct TuxFlowWindow;

impl TuxFlowWindow {
    pub fn new(app: &adw::Application, project_dir: Option<&Path>) -> adw::ApplicationWindow {
        // Load persisted settings
        let settings = Rc::new(RefCell::new(AppSettings::load()));
        // Must reach the mic module before any project registers its host,
        // or the first load would skip bridging.
        crate::remote::mic::set_enabled(settings.borrow().tools.remote_microphone);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("TuxFlow")
            .default_width(settings.borrow().window.width)
            .default_height(settings.borrow().window.height)
            .build();

        // Restore window monitor and position/maximize state
        {
            let maximized = settings.borrow().window.maximized;
            let saved_x = settings.borrow().window.x;
            let saved_y = settings.borrow().window.y;
            let saved_monitor = settings.borrow().window.monitor.clone();
            if let Some(ref connector) = saved_monitor {
                let connector = connector.clone();
                let do_maximize = maximized;
                window.connect_realize(move |win| {
                    if !do_maximize {
                        set_x11_position_hint(win, saved_x, saved_y);
                    }
                });
                let connector2 = settings.borrow().window.monitor.clone();
                window.connect_map(move |win| {
                    let win = win.clone();
                    let connector = connector2.clone();
                    let do_maximize = do_maximize;
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(200),
                        move || {
                            restore_window_placement(
                                &win,
                                saved_x,
                                saved_y,
                                connector.as_deref(),
                                do_maximize,
                            );
                        },
                    );
                });
            } else if maximized {
                window.maximize();
            }
        }

        Self::load_css();
        let keybinding_map = Rc::new(RefCell::new(KeybindingMap::from_settings(
            &settings.borrow().keybindings,
        )));
        let single_expand = Rc::new(Cell::new(settings.borrow().sidebar.single_project_expand));

        // Set guard env var before spawning any children
        // SAFETY: called on the main thread before spawning any child processes
        unsafe {
            std::env::set_var("TUXFLOW_CHILD", "1");
            // Tell child programs about the terminal's fg/bg so they pick
            // colors with enough contrast (e.g. Claude Code / Ink / chalk).
            let theme_name = &settings.borrow().appearance.terminal_theme;
            if crate::ui::terminal_theme::is_dark_theme(theme_name) {
                std::env::set_var("COLORFGBG", "15;0");
            } else {
                std::env::set_var("COLORFGBG", "0;15");
            }
        }

        // Check for orphaned processes from a previous crash
        let orphans = PidFile::orphaned_pids();
        if !orphans.is_empty() {
            Self::show_orphan_dialog(&window, orphans);
        }

        let pid_file: Rc<RefCell<PidFile>> = Rc::new(RefCell::new(PidFile::new()));

        let ws = Workspace::new();
        let terminal_stack = gtk4::Stack::new();
        terminal_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        terminal_stack.set_transition_duration(150);
        terminal_stack.set_vexpand(true);
        terminal_stack.set_hexpand(true);

        // Focus-gate for notifications: returns true when the given qname's terminal
        // is NOT what the user is currently looking at. Callers combine this with
        // the `suppress_when_focused` setting to decide whether to notify.
        let focus_gate: crate::process::auto_restart::FocusGate = {
            let window_ref = window.clone();
            let stack_ref = terminal_stack.clone();
            Rc::new(move |qname: &str| {
                let window_focused = window_ref.is_active();
                let terminal_visible = stack_ref
                    .visible_child_name()
                    .map(|n| n.as_str() == qname)
                    .unwrap_or(false);
                !(window_focused && terminal_visible)
            })
        };

        // Resolver from project name → current icon path, for attaching project
        // icons to desktop notifications. Kept behind a closure so the auto-restart
        // module stays decoupled from the workspace type.
        let icon_resolver: crate::process::auto_restart::IconResolver = {
            let ws_ref = ws.clone();
            Rc::new(move |project_name: &str| {
                ws_ref
                    .borrow()
                    .get_project_icon(project_name)
                    .map(std::path::PathBuf::from)
            })
        };

        let sidebar = Rc::new(ProjectList::new(single_expand.clone()));
        sidebar.set_workspace(&ws);
        sidebar.set_window(&window);

        let welcome = Self::build_welcome_page();
        terminal_stack.add_named(&welcome, Some("__welcome__"));

        // Track selected process and last-used project for quick re-selection
        let selected_process: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let last_selected_project: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let project_name_cells: ProjectNameCells = Rc::new(RefCell::new(HashMap::new()));
        let status_bar = Rc::new(StatusBar::new());

        // Local message composer under agent terminals; visibility follows
        // the selected process (agents only) and the tools.agent_composer
        // setting, tracked in a Cell so the settings toggle applies live.
        let composer = Rc::new(crate::ui::composer_bar::ComposerBar::new());
        {
            let s = settings.borrow();
            composer.apply_terminal_style(
                s.appearance.font_size,
                &crate::ui::terminal_theme::background_css(&s.appearance.terminal_theme),
            );
        }
        let composer_enabled = Rc::new(Cell::new(AppSettings::load().tools.agent_composer));

        // Check for updates in background
        {
            let status_bar_ref = status_bar.clone();
            crate::util::worker::run(
                crate::util::update_checker::check_for_update,
                move |update| {
                    if let Some(update) = update {
                        status_bar_ref.show_update(&update);
                    }
                },
            );
        }

        // Watch for the binary being replaced underneath us. The update check
        // above runs once per launch, so a release installed by the system's
        // software manager while this window is open would otherwise be
        // invisible — leaving the old code running with no hint, and any
        // "update available" chip stale and pointing at a download we no
        // longer need. Polling is a readlink; the source stops on first hit.
        //
        // Debug builds are exempt: `cargo run` replaces its own binary on
        // every rebuild, which would leave the chip permanently lit.
        if !cfg!(debug_assertions) {
            let status_bar_ref = status_bar.clone();
            glib::timeout_add_seconds_local(30, move || {
                if !crate::util::update_checker::binary_replaced() {
                    return glib::ControlFlow::Continue;
                }
                log::info!("binary replaced on disk; prompting for restart");
                status_bar_ref.show_restart_required();
                glib::ControlFlow::Break
            });
        }

        // Load projects progressively on idle ticks so the window paints first.
        // Each load_project call wires ~250 ms of GTK widget construction per
        // process row; serialising 15-20 projects on the main thread before
        // present() was costing many seconds of "blank screen" on launch.
        {
            // Saved entries are location keys: local paths or ssh://host/path
            let mut queue: Vec<String> = ws
                .borrow()
                .saved_directories()
                .into_iter()
                .filter(|key| match crate::remote::ProjectLocation::parse(key) {
                    crate::remote::ProjectLocation::Local(p) => p.is_dir(),
                    crate::remote::ProjectLocation::Ssh { .. } => true,
                })
                .collect();
            if let Some(dir) = project_dir {
                let key = dir.to_string_lossy().to_string();
                if !queue.iter().any(|k| k == &key) {
                    queue.push(key);
                }
            }
            let ws_load = ws.clone();
            let sidebar_load = sidebar.clone();
            let stack_load = terminal_stack.clone();
            let pid_file_load = pid_file.clone();
            let status_bar_load = status_bar.clone();
            let selected_load = selected_process.clone();
            let cells_load = project_name_cells.clone();
            let focus_gate_load = focus_gate.clone();
            let icon_resolver_load = icon_resolver.clone();
            let queue_cell: Rc<RefCell<std::vec::IntoIter<String>>> =
                Rc::new(RefCell::new(queue.into_iter()));
            glib::idle_add_local(move || {
                let next = queue_cell.borrow_mut().next();
                match next {
                    Some(key) => {
                        Self::load_project(
                            &ws_load,
                            &sidebar_load,
                            &stack_load,
                            &key,
                            &pid_file_load,
                            &status_bar_load,
                            &selected_load,
                            &cells_load,
                            Some(focus_gate_load.clone()),
                            Some(icon_resolver_load.clone()),
                        );
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                }
            });
        }

        // Wire sidebar selection → terminal switch + status bar URL update
        let stack_ref = terminal_stack.clone();
        let selected_ref = selected_process.clone();
        let sidebar_ref = sidebar.clone();
        let status_bar_ref = status_bar.clone();
        let last_proj_ref = last_selected_project.clone();
        let ws_select = ws.clone();
        let composer_sel = composer.clone();
        let composer_enabled_sel = composer_enabled.clone();
        sidebar.set_on_process_selected(move |qname| {
            // Materialize the VTE terminal lazily on first selection.
            // Use try_borrow to avoid panic when this fires during a manager borrow_mut
            // (e.g. spawn triggers status change which triggers sidebar selection).
            if let Some((proj, proc_name)) = qname.split_once("::") {
                let ws_borrow = ws_select.borrow();
                if let Some(project) = ws_borrow.projects().iter().find(|p| p.name == proj) {
                    if let Ok(mut mgr) = project.manager.try_borrow_mut() {
                        mgr.materialize_process(proc_name);
                    }
                }
                drop(ws_borrow);
            }
            stack_ref.set_visible_child_name(qname);
            // Focus the newly-visible terminal so Enter/typing goes
            // straight to the PTY instead of being swallowed by whatever
            // widget had focus before the click (the sidebar row itself,
            // or a status-bar button like the git button).
            if let Some(child) = stack_ref.visible_child() {
                child.grab_focus();
            }
            *selected_ref.borrow_mut() = Some(qname.to_string());
            if let Some((proj, _)) = qname.split_once("::") {
                *last_proj_ref.borrow_mut() = Some(proj.to_string());
                sidebar_ref.set_active_project(proj);

                // Defer status bar update to idle to avoid RefCell conflict
                // when this callback fires during a manager borrow_mut (e.g. spawn)
                let ws_idle = ws_select.clone();
                let sb_idle = status_bar_ref.clone();
                let proj_owned = proj.to_string();
                let qname_owned = qname.to_string();
                let composer_idle = composer_sel.clone();
                let composer_enabled_idle = composer_enabled_sel.clone();
                glib::idle_add_local_once(move || {
                    let ws_borrow = ws_idle.borrow();
                    let running = is_qualified_process_running(&ws_borrow, Some(&qname_owned));
                    sb_idle.set_process_running(running);
                    let mut global_r = 0usize;
                    let mut global_t = 0usize;
                    let mut proj_r = 0usize;
                    let mut proj_t = 0usize;
                    let mut running_names = Vec::new();
                    for project in ws_borrow.projects() {
                        let mgr = project.manager.borrow();
                        let r = mgr.running_count();
                        let t = mgr.total_count();
                        global_r += r;
                        global_t += t;
                        if project.name == proj_owned {
                            proj_r = r;
                            proj_t = t;
                        }
                        let names: Vec<String> =
                            mgr.running_names().into_iter().map(String::from).collect();
                        if !names.is_empty() {
                            running_names.push((project.name.clone(), names));
                        }
                    }
                    sb_idle.set_project_info(Some(&proj_owned), proj_r, proj_t);
                    sb_idle.set_global_info(global_r, global_t, true, &running_names);

                    // Composer only under agent terminals (and only when
                    // enabled). try_borrow: this can fire mid-spawn.
                    let is_agent = ws_borrow
                        .projects()
                        .iter()
                        .find(|p| p.name == proj_owned)
                        .and_then(|project| project.manager.try_borrow().ok())
                        .and_then(|mgr| {
                            qname_owned.split_once("::").map(|(_, pname)| {
                                mgr.get_process(pname).is_some_and(|p| {
                                    p.config.category
                                        == crate::config::schema::ProcessCategory::Agent
                                })
                            })
                        })
                        .unwrap_or(false);
                    // Switching terminals clears pending attachments (their
                    // paths are machine-specific).
                    composer_idle.set_context(&qname_owned);
                    composer_idle.set_visible(composer_enabled_idle.get() && is_agent);
                });
            }
            let url = sidebar_ref.get_process_url(qname);
            status_bar_ref.set_url(url.as_deref());
        });

        // Wire process deletion → remove terminal from stack and handle selected fallback
        let stack_ref = terminal_stack.clone();
        let selected_ref = selected_process.clone();
        let composer_del = composer.clone();
        sidebar.set_on_process_deleted(move |qname| {
            if let Some(child) = stack_ref.child_by_name(qname) {
                stack_ref.remove(&child);
            }
            let mut sel = selected_ref.borrow_mut();
            if sel.as_deref() == Some(qname) {
                stack_ref.set_visible_child_name("__welcome__");
                composer_del.set_visible(false);
                *sel = None;
            }
        });

        // Wire process rename → update terminal stack child name
        let stack_ref = terminal_stack.clone();
        let selected_ref = selected_process.clone();
        sidebar.set_on_process_renamed(move |old_qname, new_qname| {
            if let Some(child) = stack_ref.child_by_name(old_qname) {
                let page = stack_ref.page(&child);
                page.set_name(new_qname);
            }
            let mut sel = selected_ref.borrow_mut();
            if sel.as_deref() == Some(old_qname) {
                *sel = Some(new_qname.to_string());
            }
        });

        // Build settings change callback for accordion mode
        let sidebar_for_cb = sidebar.clone();
        let single_expand_for_cb = single_expand.clone();
        let on_single_expand_changed: Rc<dyn Fn(bool)> = Rc::new(move |enabled: bool| {
            sidebar_for_cb.set_single_project_expand(enabled);
            single_expand_for_cb.set(enabled);
        });

        // Auto-hide sidebar runtime flag + callback
        let auto_hide = Rc::new(Cell::new(settings.borrow().sidebar.auto_hide_sidebar));
        let auto_hide_for_cb = auto_hide.clone();
        let on_auto_hide_changed: Rc<dyn Fn(bool)> = Rc::new(move |enabled: bool| {
            auto_hide_for_cb.set(enabled);
        });

        // Toggle the sidebar's Ctrl+N keybind hints live when the setting changes
        let sidebar_for_keybind = sidebar.clone();
        let on_keybind_hints_changed: Rc<dyn Fn(bool)> = Rc::new(move |enabled: bool| {
            sidebar_for_keybind.set_show_keybind_hints(enabled);
        });

        // Re-sort the sidebar live when "running projects first" is toggled
        let sidebar_for_running = sidebar.clone();
        let on_recent_first_changed: Rc<dyn Fn(bool)> = Rc::new(move |enabled: bool| {
            sidebar_for_running.set_recent_first(enabled);
        });

        // Build theme-change callback that applies to all existing terminals
        // (and the composer bar, which blends into the terminal background)
        let ws_for_theme = ws.clone();
        let composer_theme = composer.clone();
        let settings_theme = settings.clone();
        let on_terminal_theme_changed: Rc<dyn Fn(&str)> = Rc::new(move |theme_name: &str| {
            let ws_borrow = ws_for_theme.borrow();
            for project in ws_borrow.projects() {
                project
                    .manager
                    .borrow_mut()
                    .apply_terminal_theme(theme_name);
            }
            composer_theme.apply_terminal_style(
                settings_theme.borrow().appearance.font_size,
                &crate::ui::terminal_theme::background_css(theme_name),
            );
            // Update COLORFGBG so newly spawned processes pick the right colors
            // SAFETY: called on the main GTK thread
            unsafe {
                if crate::ui::terminal_theme::is_dark_theme(theme_name) {
                    std::env::set_var("COLORFGBG", "15;0");
                } else {
                    std::env::set_var("COLORFGBG", "0;15");
                }
            }
        });

        // Build font-change callback that applies to all existing terminals
        // (and the composer input, which mirrors the terminal font size)
        let ws_for_font = ws.clone();
        let composer_font = composer.clone();
        let on_font_changed: Rc<dyn Fn()> = Rc::new(move || {
            let settings = AppSettings::load();
            let ws_borrow = ws_for_font.borrow();
            for project in ws_borrow.projects() {
                project.manager.borrow_mut().apply_font_settings(&settings);
            }
            composer_font.apply_terminal_style(
                settings.appearance.font_size,
                &crate::ui::terminal_theme::background_css(&settings.appearance.terminal_theme),
            );
        });

        // Composer send → attachments are delivered natively first (staged
        // as the agent's clipboard + Ctrl+V, so Claude shows [Image #N]
        // instead of a path), then the text, then Enter.
        {
            let ws_send = ws.clone();
            let stack_send = terminal_stack.clone();
            composer.set_on_send(move |text, attachments| {
                let Some(terminal) = Self::visible_terminal(&ws_send, &stack_send) else {
                    return;
                };
                let host = Self::remote_paste_target(&ws_send, &stack_send).map(|(h, _)| h);
                Self::deliver_composed(terminal, host, attachments.into(), text, false);
            });
        }

        // Composer image paste → materialize the image as a /tmp file on
        // whichever machine the agent runs on (nothing persists past a
        // reboot), then show a pending-attachment chip. The path is spliced
        // into the message on send — agents treat image paths as attachments.
        {
            let ws_img = ws.clone();
            let stack_img = terminal_stack.clone();
            let composer_img = composer.clone();
            composer.set_on_image_paste(move || {
                let remote_host = Self::remote_paste_target(&ws_img, &stack_img).map(|(h, _)| h);
                let composer_add = composer_img.clone();
                let clipboard = composer_img.widget().clipboard();
                clipboard.read_texture_async(
                    gtk4::gio::Cancellable::NONE,
                    move |res: Result<Option<gtk4::gdk::Texture>, _>| {
                        let Ok(Some(texture)) = res else {
                            log::warn!("composer image paste: couldn't read clipboard texture");
                            return;
                        };
                        let stamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        match remote_host {
                            Some(host) => {
                                let png = texture.save_to_png_bytes();
                                let upload_host = host.clone();
                                crate::util::worker::run(
                                    move || {
                                        crate::remote::upload_temp_image(
                                            &upload_host,
                                            png.as_ref(),
                                            stamp,
                                        )
                                    },
                                    move |result| match result {
                                        Ok(path) => {
                                            composer_add.add_attachment(&path, Some(&texture))
                                        }
                                        Err(e) => log::error!(
                                            "composer image paste: upload to {host} failed: {e}"
                                        ),
                                    },
                                );
                            }
                            None => {
                                let path = format!("/tmp/.tuxflow-img-{stamp}.png");
                                match texture.save_to_png(&path) {
                                    Ok(()) => composer_add.add_attachment(&path, Some(&texture)),
                                    Err(e) => {
                                        log::error!("composer image paste: save failed: {e}")
                                    }
                                }
                            }
                        }
                    },
                );
            });
        }

        // Settings toggle applies live to the current selection
        let on_composer_changed: Rc<dyn Fn(bool)> = {
            let composer_ref = composer.clone();
            let enabled_ref = composer_enabled.clone();
            let ws_ref = ws.clone();
            let stack_ref = terminal_stack.clone();
            Rc::new(move |enabled| {
                enabled_ref.set(enabled);
                composer_ref
                    .set_visible(enabled && Self::visible_terminal_is_agent(&ws_ref, &stack_ref));
            })
        };

        // Build UI
        let content = Self::build_content(
            &window,
            &ws,
            &sidebar,
            &terminal_stack,
            &selected_process,
            &last_selected_project,
            &project_name_cells,
            &on_single_expand_changed,
            &on_auto_hide_changed,
            &on_keybind_hints_changed,
            &on_recent_first_changed,
            &on_terminal_theme_changed,
            &on_font_changed,
            &on_composer_changed,
            &pid_file,
            &status_bar,
            &composer,
            &keybinding_map,
            &auto_hide,
            &focus_gate,
            &icon_resolver,
        );
        window.set_content(Some(&content));

        // Sync the global MCP-enabled flag so per-project startup inside
        // `load_project` (which fires after this on idle ticks) can decide
        // whether to spin up the server. Previously this block also looped
        // over `ws.projects()` to start servers eagerly, but with the
        // progressive-load change projects haven't been added to the
        // workspace yet at this point — startup is now driven from
        // `load_project` itself.
        crate::mcp::bridge::set_mcp_enabled(settings.borrow().integrations.mcp_enabled);

        // Kill all child processes and save window state when the window closes
        let ws_shutdown = ws.clone();
        let pid_file_shutdown = pid_file.clone();
        let settings_shutdown = settings.clone();
        window.connect_close_request(move |win| {
            // Save window size, position, and maximized state.
            // Re-load from disk first so we don't overwrite changes made by the
            // settings dialog (which uses its own AppSettings instance).
            {
                *settings_shutdown.borrow_mut() = AppSettings::load();
                let mut s = settings_shutdown.borrow_mut();
                s.window.maximized = win.is_maximized();
                if !win.is_maximized() {
                    let w = win.width();
                    let h = win.height();
                    if w > 0 && h > 0 {
                        s.window.width = w;
                        s.window.height = h;
                    }
                }
                // Save monitor name (works on both X11 and Wayland)
                if let Some(surface) = win.surface() {
                    let display = surface.display();
                    if let Some(monitor) = display.monitor_at_surface(&surface) {
                        s.window.monitor = monitor.connector().map(|c| c.to_string());
                    }
                }
                // Save absolute position (X11 only)
                if !win.is_maximized() {
                    save_window_position(win, &mut s);
                }
                s.save();
            }
            let ws_borrow = ws_shutdown.borrow();
            for project in ws_borrow.projects() {
                let mut mgr = project.manager.borrow_mut();
                if project.location.is_remote() {
                    // Remote processes live in tmux sessions on the host —
                    // quitting only detaches them; the next launch reattaches.
                    mgr.detach_all();
                } else {
                    mgr.stop_all();
                }
            }
            pid_file_shutdown.borrow_mut().clear();
            // Remote processes deliberately outlive us, but the microphone
            // bridge must not: it is the one thing that stays pointed at this
            // machine's hardware.
            crate::remote::mic::shutdown();
            glib::Propagation::Proceed
        });

        window
    }

    fn show_orphan_dialog(window: &adw::ApplicationWindow, orphans: Vec<i32>) {
        let count = orphans.len();
        let dialog = adw::AlertDialog::builder()
            .heading("Orphaned Processes Detected")
            .body(format!(
                "TuxFlow found {} process{} from a previous session still running. \
                 These may be consuming resources.",
                count,
                if count == 1 { "" } else { "es" },
            ))
            .build();
        dialog.add_response("ignore", "Ignore");
        dialog.add_response("kill", "Kill All");
        dialog.set_response_appearance("kill", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("kill"));
        dialog.set_close_response("ignore");

        let parent = window.clone().upcast::<gtk4::Widget>();
        dialog.choose(
            Some(&parent),
            gtk4::gio::Cancellable::NONE,
            move |response| {
                if response == "kill" {
                    PidFile::kill_orphans(&orphans);
                    log::info!("Killed {} orphaned process(es)", count);
                } else {
                    // Clear the stale pid file so we don't prompt again
                    PidFile::new().clear();
                    log::info!("Ignored {} orphaned process(es)", count);
                }
            },
        );
    }

    /// Parse a shell window title into a short display name.
    /// Shells often set titles like "user@host: command" or "command - /path".
    /// We extract the most useful part.
    /// `(host, is_agent)` for the currently visible terminal when it belongs
    /// to a remote project — what image-paste bridging needs to decide how
    /// (and whether) to act. None for local projects or no visible terminal.
    /// Deliver a composed message to an agent terminal: each attachment is
    /// staged as the agent's clipboard (shim file remotely, real clipboard
    /// locally) and announced with Ctrl+V — the native attachment route —
    /// paced so the agent ingests one image before the next arrives. The
    /// text follows in one bracketed paste (multi-line stays in the input),
    /// then Enter submits.
    fn deliver_composed(
        terminal: vte4::Terminal,
        host: Option<String>,
        mut attachments: std::collections::VecDeque<crate::ui::composer_bar::Attachment>,
        text: String,
        after_attachments: bool,
    ) {
        /// Pause after each Ctrl+V so the agent reads the staged image
        /// before the clipboard is overwritten or the text lands.
        const INGEST_MS: u64 = 400;
        let Some(att) = attachments.pop_front() else {
            if !text.is_empty() {
                let mut buf = Vec::with_capacity(text.len() + 17);
                buf.extend_from_slice(b"\x1b[200~");
                // Keep the [Image #N] tokens on their own line; the prompt
                // starts below (a newline inside bracketed paste inserts —
                // it can't submit).
                if after_attachments {
                    buf.push(b'\n');
                }
                buf.extend_from_slice(text.as_bytes());
                buf.extend_from_slice(b"\x1b[201~");
                terminal.feed_child(&buf);
            }
            glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
                terminal.feed_child(b"\r");
            });
            return;
        };
        match host.clone() {
            Some(h) => {
                let path = att.path.clone();
                let stage_host = h.clone();
                crate::util::worker::run(
                    move || crate::remote::stage_clipboard_image(&stage_host, &path),
                    move |result| {
                        match result {
                            Ok(()) => terminal.feed_child(&[0x16]),
                            Err(e) => {
                                log::error!("composer send: staging attachment on {h} failed: {e}")
                            }
                        }
                        let t = terminal.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_millis(INGEST_MS),
                            move || Self::deliver_composed(t, host, attachments, text, true),
                        );
                    },
                );
            }
            None => {
                if let Some(ref texture) = att.texture {
                    terminal.clipboard().set_texture(texture);
                }
                terminal.feed_child(&[0x16]);
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(INGEST_MS),
                    move || Self::deliver_composed(terminal, host, attachments, text, true),
                );
            }
        }
    }

    /// The VTE terminal of the currently visible process, if materialized.
    fn visible_terminal(ws: &WorkspaceRef, stack: &gtk4::Stack) -> Option<vte4::Terminal> {
        let name = stack.visible_child_name()?;
        let (proj, pname) = name.as_str().split_once("::")?;
        let ws_b = ws.borrow();
        let project = ws_b.projects().iter().find(|p| p.name == proj)?;
        let mgr = project.manager.try_borrow().ok()?;
        mgr.get_process(pname).and_then(|p| p.terminal.clone())
    }

    /// Whether the currently visible terminal belongs to an Agent process.
    fn visible_terminal_is_agent(ws: &WorkspaceRef, stack: &gtk4::Stack) -> bool {
        let Some(name) = stack.visible_child_name() else {
            return false;
        };
        let Some((proj, pname)) = name.as_str().split_once("::") else {
            return false;
        };
        let ws_b = ws.borrow();
        ws_b.projects()
            .iter()
            .find(|p| p.name == proj)
            .and_then(|project| project.manager.try_borrow().ok())
            .and_then(|mgr| {
                mgr.get_process(pname)
                    .map(|p| p.config.category == crate::config::schema::ProcessCategory::Agent)
            })
            .unwrap_or(false)
    }

    fn remote_paste_target(ws: &WorkspaceRef, stack: &gtk4::Stack) -> Option<(String, bool)> {
        let (proj, pname) = stack.visible_child_name().and_then(|n| {
            n.split_once("::")
                .map(|(p, pr)| (p.to_string(), pr.to_string()))
        })?;
        let ws_b = ws.borrow();
        let project = ws_b.projects().iter().find(|p| p.name == proj)?;
        let host = project.location.host()?.to_string();
        let is_agent = project
            .manager
            .borrow()
            .get_process(&pname)
            .is_some_and(|p| p.config.category == crate::config::schema::ProcessCategory::Agent);
        Some((host, is_agent))
    }

    /// If the clipboard holds an image, upload it to the remote host's
    /// TuxFlow clipboard file (provisioning the `xclip` shim on first use) —
    /// the bytes exist only in this machine's clipboard. Agent terminals
    /// then receive a real Ctrl+V so the agent "reads the clipboard" through
    /// the shim and shows its native attachment UI; other terminals get the
    /// path typed. Returns whether the paste was handled (false = clipboard
    /// has no image; caller should paste normally).
    fn paste_image_to_remote(terminal: &vte4::Terminal, host: &str, is_agent: bool) -> bool {
        let clipboard = terminal.clipboard();
        if !clipboard
            .formats()
            .contains_type(gtk4::gdk::Texture::static_type())
        {
            return false;
        }
        let host = host.to_string();
        let terminal = terminal.clone();
        clipboard.read_texture_async(
            gtk4::gio::Cancellable::NONE,
            move |res: Result<Option<gtk4::gdk::Texture>, _>| {
                let Ok(Some(texture)) = res else {
                    log::warn!("image paste: couldn't read clipboard texture");
                    return;
                };
                let png = texture.save_to_png_bytes();
                let upload_host = host.clone();
                crate::util::worker::run(
                    move || crate::remote::upload_clipboard_image(&upload_host, png.as_ref()),
                    move |result| match result {
                        Ok(path) => {
                            log::info!("image paste: uploaded to {host}:{path}");
                            if is_agent {
                                // Ctrl+V: the agent reads "the clipboard"
                                // (our shim) and attaches the image natively
                                terminal.feed_child(&[0x16]);
                            } else {
                                terminal.feed_child(format!("{path} ").as_bytes());
                            }
                        }
                        Err(e) => log::error!("image paste: upload to {host} failed: {e}"),
                    },
                );
            },
        );
        true
    }

    /// Hand the newest tmux paste buffer on `host` to the local clipboard.
    ///
    /// The hard part isn't fetching it, it's knowing whether it's *ours*.
    /// `tmux show-buffer` always answers, and answers with the newest buffer
    /// on that server — which might be the drag that just finished, or might
    /// be an OSC 52 an agent sent half an hour ago that nothing has
    /// displaced since. The old bridge couldn't tell, so it published the
    /// second as if it were the first, and the user's clipboard reverted to
    /// scrollback they never selected. Hence `route`.
    fn tmux_buffer_to_clipboard(host: &str, route: ClipRoute, seen: Option<Rc<Cell<u64>>>) {
        /// How recently tmux must have made a buffer for a selection gesture
        /// to claim it. Generous next to the ~0.5 s the gesture's own delay
        /// and ssh round trip cost, but far below the minutes-old buffers
        /// this exists to reject.
        const SELECTION_MAX_AGE: Duration = Duration::from_secs(5);

        let host = host.to_string();
        crate::util::worker::run(
            move || crate::remote::fetch_tmux_buffer(&host),
            move |buf| {
                let Some(buf) = buf else {
                    log::debug!("clipboard bridge: no tmux buffer");
                    return;
                };
                // A gesture only gets to publish a buffer tmux made *for
                // that gesture*. An explicit copy asks for the newest buffer
                // whatever its age — that is also how an agent's OSC 52 copy
                // is collected, since nothing else knows it happened.
                if route == ClipRoute::Selection && buf.age > SELECTION_MAX_AGE {
                    log::debug!(
                        "clipboard bridge: newest buffer is {}s old, not this selection",
                        buf.age.as_secs()
                    );
                    return;
                }
                let hash = Self::tmux_buffer_hash(&buf.text);
                if let Some(seen) = seen
                    && seen.replace(hash) == hash
                {
                    log::debug!("clipboard bridge: buffer unchanged");
                    return;
                }
                let Some(display) = gtk4::gdk::Display::default() else {
                    return;
                };
                log::debug!(
                    "clipboard bridge: copied {} bytes ({route:?})",
                    buf.text.len()
                );
                display.clipboard().set_text(&buf.text);
                // A selection belongs in PRIMARY too — that's where a local
                // VTE drag puts it, so middle-click paste behaves the same
                // on a remote pane as on a local one.
                if route == ClipRoute::Selection {
                    display.primary_clipboard().set_text(&buf.text);
                }
            },
        );
    }

    /// Change-detection hash for the tmux clipboard bridge — only stability
    /// within one app run matters.
    fn tmux_buffer_hash(text: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    fn parse_window_title(title: &str) -> Option<String> {
        let title = title.trim();
        if title.is_empty() {
            return None;
        }
        // If it contains ": ", take the part after the last ": "
        let name = if let Some(pos) = title.rfind(": ") {
            &title[pos + 2..]
        } else if let Some(pos) = title.find(" - ") {
            // fish style: "command - /path"
            &title[..pos]
        } else {
            title
        };
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        // Truncate if too long
        let truncated = if name.len() > 30 {
            format!("{}...", &name[..27])
        } else {
            name.to_string()
        };
        Some(truncated)
    }

    /// Connect a VTE terminal's `window-title` property to auto-rename
    /// the sidebar row, but only while the process has `auto_named: true`.
    /// `pname_cell` and `qname_cell` are read on each title change, so the
    /// closure keeps using the current project / qualified name even after a
    /// rename.
    fn connect_window_title_auto_rename(
        terminal: &vte4::Terminal,
        manager: &ProcessManagerRef,
        process_name: &str,
        sidebar: &Rc<ProjectList>,
        qname_cell: &Rc<RefCell<String>>,
        workspace: &WorkspaceRef,
        pname_cell: &Rc<RefCell<String>>,
    ) {
        let mgr_ref = manager.clone();
        let proc_name = process_name.to_string();
        let sidebar_ref = sidebar.clone();
        let qname_cell = qname_cell.clone();
        let ws_ref = workspace.clone();
        let pname_cell = pname_cell.clone();
        terminal.connect_window_title_changed(move |term| {
            let is_auto = mgr_ref
                .borrow()
                .get_process(&proc_name)
                .map(|p| p.config.auto_named)
                .unwrap_or(false);
            if !is_auto {
                return;
            }
            if let Some(title) = term.window_title()
                && let Some(display_name) = Self::parse_window_title(&title)
            {
                let qname = qname_cell.borrow().clone();
                sidebar_ref.set_process_name(&qname, &display_name);
                if let Some(proc) = mgr_ref.borrow_mut().get_process_mut(&proc_name) {
                    proc.config.display_name = Some(display_name.clone());
                }
                ws_ref.borrow_mut().set_display_name(
                    &pname_cell.borrow(),
                    &proc_name,
                    &display_name,
                );
            }
        });
    }

    /// Resolve the best project to pre-select in dialogs:
    /// 1. Active terminal's project
    /// 2. Last selected project
    /// 3. First expanded project in sidebar
    fn resolve_active_project(
        stack: &gtk4::Stack,
        last_project: &Rc<RefCell<Option<String>>>,
        sidebar: &Rc<ProjectList>,
    ) -> Option<String> {
        last_project
            .borrow()
            .clone()
            .or_else(|| {
                stack
                    .visible_child_name()
                    .and_then(|name| name.split_once("::").map(|(proj, _)| proj.to_string()))
            })
            .or_else(|| sidebar.last_expanded_project())
    }

    fn refresh_status_bar_git(
        location: crate::remote::ProjectLocation,
        status_bar: Rc<StatusBar>,
        do_fetch: bool,
    ) {
        Self::refresh_status_bar_git_then(location, status_bar, do_fetch, None)
    }

    /// `on_done` (when given) runs once the refreshed numbers are on the chip.
    /// The periodic poller uses it to clear an in-flight flag, so a slow
    /// refresh (remote project with the link down: up to 10 s per git call)
    /// makes it skip ticks instead of stacking a new blocked worker thread
    /// every minute; the sync button uses it to drop its spinner.
    ///
    /// A fetching refresh reports twice: local counts first, then again after
    /// the fetch. Only `behind` needs the network, and the fetch is the whole
    /// cost of the call (seconds against a remote host) — reporting after it
    /// alone would leave the chip claiming "1 to push" for those seconds
    /// after a push already cleared it.
    fn refresh_status_bar_git_then(
        location: crate::remote::ProjectLocation,
        status_bar: Rc<StatusBar>,
        do_fetch: bool,
        on_done: Option<Box<dyn FnOnce()>>,
    ) {
        type GitPoll = (usize, usize, Option<String>, (usize, usize, usize), usize);
        let token = status_bar.begin_git_refresh();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<GitPoll>();
        std::thread::spawn(move || {
            let read = |location: &crate::remote::ProjectLocation| {
                (
                    commits_ahead(location),
                    commits_behind(location),
                    current_branch(location),
                    diff_shortstat(location),
                    untracked_count(location),
                )
            };
            let _ = tx.send(read(&location));
            if do_fetch {
                git_fetch(&location);
                let _ = tx.send(read(&location));
            }
        });
        glib::spawn_future_local(async move {
            // Drain to the end even once superseded, so `on_done` still marks
            // the *worker* as finished — that's what the poller's in-flight
            // flag is counting, not who owns the chip.
            while let Some((ahead, behind, branch, (files, added, removed), untracked)) =
                rx.recv().await
            {
                if !status_bar.git_refresh_current(token) {
                    continue;
                }
                status_bar.set_git_sync(ahead, behind);
                status_bar.set_git_branch(branch.as_deref());
                status_bar.set_git_diffstat(files, added, removed, untracked);
            }
            // After the chip is updated, never before — `on_done` is what
            // hides the sync spinner, and the swap has to be same-frame.
            if let Some(done) = on_done {
                done();
            }
        });
    }

    fn pick_project(
        parent: &adw::ApplicationWindow,
        project_names: &[String],
        best_project: Option<&str>,
        last_project: &Rc<RefCell<Option<String>>>,
        on_selected: impl Fn(&str) + 'static,
    ) {
        if project_names.is_empty() {
            return;
        }
        if project_names.len() == 1 {
            *last_project.borrow_mut() = Some(project_names[0].clone());
            on_selected(&project_names[0]);
            return;
        }

        let dialog = adw::Dialog::builder()
            .title("Select Project")
            .content_width(350)
            .content_height(200)
            .build();
        crate::ui::guard_dialog_maximize(&dialog);

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&headerbar);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        let project_group = adw::PreferencesGroup::new();
        let project_list =
            gtk4::StringList::new(&project_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let project_row = adw::ComboRow::builder()
            .title("Project")
            .model(&project_list)
            .build();

        // Pre-select best project
        if let Some(best) = best_project
            && let Some(idx) = project_names.iter().position(|n| n == best)
        {
            project_row.set_selected(idx as u32);
        }

        project_group.add(&project_row);
        content.append(&project_group);

        let select_btn = gtk4::Button::builder()
            .label("Select")
            .css_classes(["suggested-action", "pill"])
            .margin_top(24)
            .halign(gtk4::Align::Center)
            .build();
        content.append(&select_btn);

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        let dialog_ref = dialog.clone();
        let names = project_names.to_vec();
        let last_ref = last_project.clone();
        select_btn.connect_clicked(move |_| {
            let selected = names
                .get(project_row.selected() as usize)
                .cloned()
                .unwrap_or_default();
            *last_ref.borrow_mut() = Some(selected.clone());
            on_selected(&selected);
            dialog_ref.close();
        });

        dialog.present(Some(parent));
    }

    fn load_project(
        ws: &WorkspaceRef,
        sidebar: &Rc<ProjectList>,
        terminal_stack: &gtk4::Stack,
        key: &str,
        pid_file: &Rc<RefCell<PidFile>>,
        status_bar: &Rc<StatusBar>,
        selected_process: &Rc<RefCell<Option<String>>>,
        project_name_cells: &ProjectNameCells,
        focus_gate: Option<crate::process::auto_restart::FocusGate>,
        icon_resolver: Option<crate::process::auto_restart::IconResolver>,
    ) {
        match crate::remote::ProjectLocation::parse(key) {
            crate::remote::ProjectLocation::Local(path) => {
                let mut ws_mut = ws.borrow_mut();
                if let Some(project) = ws_mut.add_project_from_dir(&path) {
                    let project_name = project.name.clone();
                    let manager = project.manager.clone();
                    let icon_path = project.icon_path.clone();
                    let dir_str = project.key();
                    let saved_expanded = ws_mut.is_project_expanded(&project_name);
                    drop(ws_mut);
                    Self::wire_project(
                        &project_name,
                        &manager,
                        icon_path.as_deref(),
                        saved_expanded,
                        ws,
                        sidebar,
                        terminal_stack,
                        pid_file,
                        status_bar,
                        selected_process,
                        project_name_cells,
                        focus_gate,
                        icon_resolver,
                    );
                    if crate::mcp::bridge::is_mcp_enabled() {
                        Self::start_mcp_for_project(&manager, &project_name, &dir_str, ws);
                    }
                }
            }
            location @ crate::remote::ProjectLocation::Ssh { .. } => {
                Self::load_remote_project(
                    ws,
                    sidebar,
                    terminal_stack,
                    location,
                    pid_file,
                    status_bar,
                    selected_process,
                    project_name_cells,
                    focus_gate,
                    icon_resolver,
                    1,
                );
            }
        }
    }

    /// Load a remote project: ssh probing runs on a worker thread, then the
    /// project is assembled and wired on the main thread. When the host is
    /// unreachable the load retries itself with capped backoff until it
    /// succeeds (notifying once per outage), so a laptop that comes online
    /// after TuxFlow does still gets its remote projects. `attempt` starts
    /// at 1 and tracks the backoff/notification state across retries.
    #[allow(clippy::too_many_arguments)]
    fn load_remote_project(
        ws: &WorkspaceRef,
        sidebar: &Rc<ProjectList>,
        terminal_stack: &gtk4::Stack,
        location: crate::remote::ProjectLocation,
        pid_file: &Rc<RefCell<PidFile>>,
        status_bar: &Rc<StatusBar>,
        selected_process: &Rc<RefCell<Option<String>>>,
        project_name_cells: &ProjectNameCells,
        focus_gate: Option<crate::process::auto_restart::FocusGate>,
        icon_resolver: Option<crate::process::auto_restart::IconResolver>,
        attempt: u32,
    ) {
        let crate::remote::ProjectLocation::Ssh { host, dir } = location.clone() else {
            return;
        };

        let ws = ws.clone();
        let sidebar = sidebar.clone();
        let terminal_stack = terminal_stack.clone();
        let pid_file = pid_file.clone();
        let status_bar = status_bar.clone();
        let selected_process = selected_process.clone();
        let project_name_cells = project_name_cells.clone();
        let probe_host = host.clone();
        let fetch_icon = !ws.borrow().has_saved_icon(&location.key());
        crate::util::worker::run(
            // Full detection: the conservative trim happens in
            // auto_select_processes, so Edit Project can still list and
            // restore every command detection found on the host.
            move || workspace::probe_remote(&probe_host, &dir, false, fetch_icon),
            move |result| match result {
                Ok(probe) => {
                    let live_sessions = probe.live_sessions.clone();
                    let mut ws_mut = ws.borrow_mut();
                    let Some(prepared) = ws_mut.prepare_project_probed(location.clone(), probe)
                    else {
                        return;
                    };
                    let selected = ws_mut.auto_select_processes(&prepared);
                    if let Some(project) = ws_mut.finalize_project(prepared, selected) {
                        let project_name = project.name.clone();
                        let manager = project.manager.clone();
                        let icon_path = project.icon_path.clone();
                        let saved_expanded = ws_mut.is_project_expanded(&project_name);
                        drop(ws_mut);
                        Self::wire_project(
                            &project_name,
                            &manager,
                            icon_path.as_deref(),
                            saved_expanded,
                            &ws,
                            &sidebar,
                            &terminal_stack,
                            &pid_file,
                            &status_bar,
                            &selected_process,
                            &project_name_cells,
                            focus_gate.clone(),
                            icon_resolver.clone(),
                        );
                        // MCP stays off for remote projects in v1: the Unix
                        // socket is local, agent processes run remotely.

                        // Reattach processes whose tmux sessions are still
                        // running from a previous app run — otherwise they'd
                        // show Stopped while actually alive on the host.
                        // spawn() reattaches via `new-session -A` and is a
                        // no-op for anything already Running.
                        if !live_sessions.is_empty() {
                            let key = location.key();
                            let manager = manager.clone();
                            let reattach = move || {
                                let mut mgr = manager.borrow_mut();
                                let names: Vec<String> = mgr.process_names().to_vec();
                                for pname in names {
                                    let session = crate::remote::remote_session_name(&key, &pname);
                                    if live_sessions.contains(&session) {
                                        log::info!(
                                            "Reattaching {pname}: session {session} alive on host"
                                        );
                                        mgr.spawn_quiet(&pname);
                                    }
                                }
                            };
                            // Claude Code probes for a microphone once and
                            // caches the answer for the life of the agent
                            // process, so an agent started before the bridge
                            // is up can never see it — and the user has no
                            // way to tell that from a broken bridge. Wait for
                            // it (bounded), then reattach either way.
                            if crate::remote::mic::is_enabled() {
                                let host_for_wait = host.clone();
                                let host_for_notify = host.clone();
                                crate::util::worker::run(
                                    move || crate::remote::mic::wait_ready(&host_for_wait),
                                    move |ready| {
                                        if let Err(e) = ready {
                                            crate::util::notifications::notify_mic_bridge_failed(
                                                &host_for_notify,
                                                &e,
                                            );
                                        }
                                        reattach();
                                    },
                                );
                            } else {
                                reattach();
                            }
                        }
                    }
                }
                Err(workspace::ProbeError::Invalid(e)) => {
                    // Missing dir / broken config — retrying won't change it
                    log::error!("Failed to load remote project {}: {e}", location.key());
                }
                Err(workspace::ProbeError::Unreachable(e)) => {
                    log::warn!(
                        "Remote project {} unreachable (attempt {attempt}): {e}",
                        location.key()
                    );
                    if attempt == 1 {
                        crate::util::notifications::notify_remote_unreachable(
                            &location.base_name(),
                            &host,
                        );
                    }
                    // 3 s doubling to a 60 s cap, forever — a dead host costs
                    // one idle worker thread per minute.
                    let delay = (3u64 << (attempt - 1).min(5)).min(60);
                    glib::timeout_add_local_once(Duration::from_secs(delay), move || {
                        Self::load_remote_project(
                            &ws,
                            &sidebar,
                            &terminal_stack,
                            location,
                            &pid_file,
                            &status_bar,
                            &selected_process,
                            &project_name_cells,
                            focus_gate,
                            icon_resolver,
                            attempt + 1,
                        );
                    });
                }
            },
        );
    }

    /// Interactive add of a remote project: probe over ssh on a worker thread
    /// (full detector), then run the same confirm/select flow as local adds.
    #[allow(clippy::too_many_arguments)]
    fn load_remote_project_interactive(
        parent: &(impl IsA<gtk4::Widget> + Clone + 'static),
        ws: &WorkspaceRef,
        sidebar: &Rc<ProjectList>,
        terminal_stack: &gtk4::Stack,
        location: crate::remote::ProjectLocation,
        pid_file: &Rc<RefCell<PidFile>>,
        status_bar: &Rc<StatusBar>,
        selected_process: &Rc<RefCell<Option<String>>>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        project_name_cells: &ProjectNameCells,
        focus_gate: Option<crate::process::auto_restart::FocusGate>,
        icon_resolver: Option<crate::process::auto_restart::IconResolver>,
    ) {
        let crate::remote::ProjectLocation::Ssh { host, dir } = location.clone() else {
            return;
        };

        let parent = parent.clone();
        let ws = ws.clone();
        let sidebar = sidebar.clone();
        let terminal_stack = terminal_stack.clone();
        let pid_file = pid_file.clone();
        let status_bar = status_bar.clone();
        let selected_process = selected_process.clone();
        let last_selected_project = last_selected_project.clone();
        let project_name_cells = project_name_cells.clone();
        let fetch_icon = !ws.borrow().has_saved_icon(&location.key());
        crate::util::worker::run(
            move || workspace::probe_remote(&host, &dir, false, fetch_icon),
            move |result| match result {
                Ok(probe) => {
                    let prepared = ws
                        .borrow_mut()
                        .prepare_project_probed(location.clone(), probe);
                    if let Some(prepared) = prepared {
                        Self::present_prepared_interactive(
                            &parent,
                            &ws,
                            &sidebar,
                            &terminal_stack,
                            &pid_file,
                            &status_bar,
                            &selected_process,
                            &last_selected_project,
                            &project_name_cells,
                            focus_gate,
                            icon_resolver,
                            prepared,
                        );
                    }
                }
                Err(e) => {
                    log::error!("Failed to add remote project {}: {e}", location.key());
                    let alert = adw::AlertDialog::builder()
                        .heading("Couldn't add remote project")
                        .body(e.to_string())
                        .build();
                    alert.add_response("ok", "OK");
                    alert.present(Some(&parent));
                }
            },
        );
    }

    fn load_project_interactive(
        parent: &impl IsA<gtk4::Widget>,
        ws: &WorkspaceRef,
        sidebar: &Rc<ProjectList>,
        terminal_stack: &gtk4::Stack,
        dir: &Path,
        pid_file: &Rc<RefCell<PidFile>>,
        status_bar: &Rc<StatusBar>,
        selected_process: &Rc<RefCell<Option<String>>>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        project_name_cells: &ProjectNameCells,
        focus_gate: Option<crate::process::auto_restart::FocusGate>,
        icon_resolver: Option<crate::process::auto_restart::IconResolver>,
    ) {
        let prepared = {
            let mut ws_mut = ws.borrow_mut();
            ws_mut.prepare_project(dir)
        };
        let Some(prepared) = prepared else { return };
        Self::present_prepared_interactive(
            parent,
            ws,
            sidebar,
            terminal_stack,
            pid_file,
            status_bar,
            selected_process,
            last_selected_project,
            project_name_cells,
            focus_gate,
            icon_resolver,
            prepared,
        );
    }

    /// Shared interactive continuation of adding a project (local or remote):
    /// confirm/select-commands dialog, finalize, wire into the UI.
    #[allow(clippy::too_many_arguments)]
    fn present_prepared_interactive(
        parent: &impl IsA<gtk4::Widget>,
        ws: &WorkspaceRef,
        sidebar: &Rc<ProjectList>,
        terminal_stack: &gtk4::Stack,
        pid_file: &Rc<RefCell<PidFile>>,
        status_bar: &Rc<StatusBar>,
        selected_process: &Rc<RefCell<Option<String>>>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        project_name_cells: &ProjectNameCells,
        focus_gate: Option<crate::process::auto_restart::FocusGate>,
        icon_resolver: Option<crate::process::auto_restart::IconResolver>,
        prepared: workspace::PreparedProject,
    ) {
        // MCP stays local-only in v1: the Unix socket can't be reached by
        // processes running on the remote host.
        let is_remote = prepared.location.is_remote();

        if !crate::detect::detector::needs_command_selection(
            prepared.config_loaded,
            &prepared.stacks,
        ) {
            // Show a small dialog to let the user rename before adding
            let all_processes: Vec<crate::config::schema::ProcessConfig> = prepared
                .stacks
                .iter()
                .flat_map(|s| s.suggested_processes.clone())
                .collect();
            let project_name = prepared.name.clone();
            let dir_string = prepared.key.clone();
            let ws = ws.clone();
            let sidebar = sidebar.clone();
            let terminal_stack = terminal_stack.clone();
            let pid_file = pid_file.clone();
            let status_bar = status_bar.clone();
            let selected_process = selected_process.clone();
            let last_selected_project = last_selected_project.clone();
            let project_name_cells = project_name_cells.clone();

            Self::show_confirm_project_dialog(parent, &project_name, move |custom_name| {
                let mut ws_mut = ws.borrow_mut();
                let mut prepared = prepared;
                if custom_name != prepared.name {
                    ws_mut.set_project_name(&dir_string, &custom_name);
                    prepared.name = custom_name;
                }
                if let Some(project) = ws_mut.finalize_project(prepared, all_processes) {
                    let project_name = project.name.clone();
                    let manager = project.manager.clone();
                    let icon_path = project.icon_path.clone();
                    let dir_str = project.key();
                    let saved_expanded = ws_mut.is_project_expanded(&project_name);
                    drop(ws_mut);
                    Self::wire_project(
                        &project_name,
                        &manager,
                        icon_path.as_deref(),
                        saved_expanded,
                        &ws,
                        &sidebar,
                        &terminal_stack,
                        &pid_file,
                        &status_bar,
                        &selected_process,
                        &project_name_cells,
                        focus_gate.clone(),
                        icon_resolver.clone(),
                    );
                    if !is_remote && crate::mcp::bridge::is_mcp_enabled() {
                        Self::start_mcp_for_project(&manager, &project_name, &dir_str, &ws);
                    }
                    *last_selected_project.borrow_mut() = Some(project_name.clone());
                    sidebar.expand_project(&project_name);
                }
            });
        } else {
            // Show selection dialog
            let project_name = prepared.name.clone();
            let dir_string = prepared.key.clone();
            let stacks_for_dialog = prepared.stacks.clone();
            let all_detected_names: Vec<String> = prepared
                .stacks
                .iter()
                .flat_map(|s| s.suggested_processes.iter().map(|p| p.name.clone()))
                .collect();
            let ws = ws.clone();
            let sidebar = sidebar.clone();
            let terminal_stack = terminal_stack.clone();
            let pid_file = pid_file.clone();
            let status_bar = status_bar.clone();
            let selected_process = selected_process.clone();
            let last_selected_project = last_selected_project.clone();
            let project_name_cells = project_name_cells.clone();

            crate::ui::select_commands_dialog::SelectCommandsDialog::show(
                parent,
                &project_name,
                &stacks_for_dialog,
                move |custom_name, selected| {
                    // Mark deselected processes as deleted so they stay hidden on restart
                    let selected_names: std::collections::HashSet<&str> =
                        selected.iter().map(|p| p.name.as_str()).collect();
                    let mut ws_mut = ws.borrow_mut();
                    for name in &all_detected_names {
                        if !selected_names.contains(name.as_str()) {
                            ws_mut.mark_process_deleted_by_dir(&dir_string, name);
                        }
                    }
                    // Apply custom name if user changed it
                    let mut prepared = prepared;
                    if custom_name != prepared.name {
                        ws_mut.set_project_name(&dir_string, &custom_name);
                        prepared.name = custom_name;
                    }
                    if let Some(project) = ws_mut.finalize_project(prepared, selected) {
                        let project_name = project.name.clone();
                        let manager = project.manager.clone();
                        let icon_path = project.icon_path.clone();
                        let dir_str = project.key();
                        let saved_expanded = ws_mut.is_project_expanded(&project_name);
                        drop(ws_mut);
                        Self::wire_project(
                            &project_name,
                            &manager,
                            icon_path.as_deref(),
                            saved_expanded,
                            &ws,
                            &sidebar,
                            &terminal_stack,
                            &pid_file,
                            &status_bar,
                            &selected_process,
                            &project_name_cells,
                            focus_gate.clone(),
                            icon_resolver.clone(),
                        );
                        if !is_remote && crate::mcp::bridge::is_mcp_enabled() {
                            Self::start_mcp_for_project(&manager, &project_name, &dir_str, &ws);
                        }
                        *last_selected_project.borrow_mut() = Some(project_name.clone());
                        sidebar.expand_project(&project_name);
                    }
                },
            );
        }
    }

    fn show_confirm_project_dialog(
        parent: &impl IsA<gtk4::Widget>,
        project_name: &str,
        on_confirm: impl FnOnce(String) + 'static,
    ) {
        let dialog = adw::Dialog::builder()
            .title("Add Project")
            .content_width(350)
            .content_height(150)
            .build();
        crate::ui::guard_dialog_maximize(&dialog);

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&headerbar);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        let name_group = adw::PreferencesGroup::new();
        let name_row = adw::EntryRow::builder()
            .title("Project Name")
            .text(project_name)
            .build();
        name_group.add(&name_row);
        content.append(&name_group);

        let add_btn = gtk4::Button::builder()
            .label("Add Project")
            .css_classes(["suggested-action", "pill"])
            .margin_top(24)
            .halign(gtk4::Align::Center)
            .build();
        content.append(&add_btn);

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        let dialog_ref = dialog.clone();
        let on_confirm = std::cell::Cell::new(Some(on_confirm));
        add_btn.connect_clicked(move |_| {
            let name = name_row.text().to_string();
            if name.is_empty() {
                return;
            }
            if let Some(cb) = on_confirm.take() {
                cb(name);
            }
            dialog_ref.close();
        });

        dialog.present(Some(parent));
    }

    fn wire_project(
        project_name: &str,
        manager: &ProcessManagerRef,
        icon_path: Option<&str>,
        saved_expanded: Option<bool>,
        ws: &WorkspaceRef,
        sidebar: &Rc<ProjectList>,
        terminal_stack: &gtk4::Stack,
        pid_file: &Rc<RefCell<PidFile>>,
        status_bar: &Rc<StatusBar>,
        selected_process: &Rc<RefCell<Option<String>>>,
        project_name_cells: &ProjectNameCells,
        focus_gate: Option<crate::process::auto_restart::FocusGate>,
        icon_resolver: Option<crate::process::auto_restart::IconResolver>,
    ) {
        // Shared cell holding this project's current display name. The window's
        // `on_project_renamed` handler updates the cell and the registry on
        // rename, so closures below keep building correct qualified names.
        let pname_cell: Rc<RefCell<String>> = Rc::new(RefCell::new(project_name.to_string()));
        project_name_cells
            .borrow_mut()
            .insert(project_name.to_string(), pname_cell.clone());

        // Add placeholders to the stack (real terminals are created lazily)
        let detector = Rc::new(RefCell::new(PortDetector::new()));

        // Remote projects get a tunnel manager: every detected port is
        // forwarded locally over the shared ssh connection, so the browser
        // button and Ctrl+click on localhost URLs work exactly like local.
        let tunnels: Rc<RefCell<Option<crate::remote::tunnel::TunnelManager>>> =
            Rc::new(RefCell::new(
                manager
                    .borrow()
                    .location()
                    .host()
                    .map(crate::remote::tunnel::TunnelManager::new),
            ));

        // A remote run's ports often never reach the terminal at all:
        // `php artisan dev` draws `@laravel/multiplex`, a tabbed TUI that
        // renders only the selected tab, so a project parked on `vite` never
        // shows its server URL and one parked on `server` never shows Vite's.
        // Output scanning cannot recover what was never printed, so ask the
        // host what the run is *listening* on instead — true whichever tab is
        // drawn, and whatever runner started it. Everything found is forwarded
        // 1:1: a remote dev server hands the browser its own address (Vite
        // bakes its port into `public/hot`), so a remapped forward listens
        // where nothing knocks. Scanning still decides the badge URL; this
        // only decides what gets tunnelled.
        if let (Some(host), dir) = (
            manager.borrow().location().host().map(str::to_owned),
            manager.borrow().location().dir_str(),
        ) {
            let manager_probe = manager.clone();
            let tunnels_probe = tunnels.clone();
            // Ports we forwarded, so they can be dropped when the project goes
            // idle and rediscovered on the next run.
            let forwarded: Rc<RefCell<HashSet<u16>>> = Rc::new(RefCell::new(HashSet::new()));
            let in_flight = Rc::new(Cell::new(false));
            // Ticks to skip before the next probe. A project can sit with
            // agents running and no dev server for hours; back off so that
            // costs one ssh round trip a minute rather than one every 2 s.
            let cooldown: Rc<Cell<u32>> = Rc::new(Cell::new(0));
            let was_idle = Rc::new(Cell::new(true));
            glib::timeout_add_local(Duration::from_secs(2), move || {
                // Sessions of everything currently running: the probe needs a
                // live tmux pane to walk down from.
                let sessions: Vec<String> = {
                    let mgr = manager_probe.borrow();
                    mgr.process_names()
                        .iter()
                        .filter_map(|n| mgr.get_process(n))
                        .filter(|p| p.status == ProcessStatus::Running)
                        .filter_map(|p| p.remote_session.clone())
                        .collect()
                };
                if sessions.is_empty() {
                    // The servers died with the run; drop the forwards so the
                    // next one rediscovers instead of inheriting stale ports.
                    if let Some(tm) = tunnels_probe.borrow_mut().as_mut() {
                        for port in forwarded.borrow_mut().drain() {
                            tm.close(port);
                        }
                    }
                    was_idle.set(true);
                    return glib::ControlFlow::Continue;
                }
                // Something just started — probe promptly however long the
                // project idled before it.
                if was_idle.replace(false) {
                    cooldown.set(0);
                }
                if in_flight.get() {
                    return glib::ControlFlow::Continue;
                }
                if let Some(remaining) = cooldown.get().checked_sub(1) {
                    cooldown.set(remaining);
                    return glib::ControlFlow::Continue;
                }
                in_flight.set(true);
                let host_probe = host.clone();
                let fs = crate::remote::fs::SshFs::new(host.clone(), dir.clone());
                let tunnels_done = tunnels_probe.clone();
                let forwarded_done = forwarded.clone();
                let in_flight_done = in_flight.clone();
                let cooldown_done = cooldown.clone();
                crate::util::worker::run(
                    move || {
                        let by_session =
                            crate::remote::ports::session_ports(&host_probe, &sessions);
                        let mut ports: Vec<u16> = by_session.into_values().flatten().collect();
                        // No live pane means no tmux on the host (the fallback
                        // exec's the command directly), so there is no tree to
                        // walk — Vite's hot file still names its port.
                        if ports.is_empty() {
                            ports.extend(crate::remote::vite::hot_port(&fs));
                        }
                        ports.sort_unstable();
                        ports.dedup();
                        ports
                    },
                    move |ports| {
                        in_flight_done.set(false);
                        let mut guard = tunnels_done.borrow_mut();
                        let Some(tm) = guard.as_mut() else {
                            return;
                        };
                        let mut open = forwarded_done.borrow_mut();
                        let mut changed = false;
                        for port in ports {
                            if open.contains(&port) {
                                continue;
                            }
                            if tm.ensure_exact(port).is_some() {
                                open.insert(port);
                                changed = true;
                            }
                        }
                        // Stay responsive while a run is still opening ports,
                        // go quiet once it has settled. Cap at 30 s: 1, 3, 7,
                        // 15 ticks.
                        if changed {
                            cooldown_done.set(0);
                        } else {
                            cooldown_done.set((cooldown_done.get() * 2 + 1).min(15));
                        }
                    },
                );
                glib::ControlFlow::Continue
            });
        }

        // Microphone bridge: headless hosts have no capture device, so an
        // agent's voice input (Claude Code's hold-to-talk) records through a
        // fake `arecord` that reads this machine's mic over an ssh -R socket.
        // Registered unconditionally — `mic` decides whether to bridge based
        // on the setting, so toggling it later can reach already-open projects.
        if let Some(host) = manager.borrow().location().host() {
            crate::remote::mic::register_host(host);
        }

        // Clipboard bridge for tmux mouse-selections (no released VTE
        // implements OSC 52): after a selection gesture on a remote terminal,
        // fetch the newest tmux paste buffer and copy it locally if it is
        // both new and this gesture's. Primed at load so a buffer left over
        // from a previous session can't be published as a fresh selection.
        let clip_host: Option<String> = manager.borrow().location().host().map(str::to_string);
        let clip_hash: Rc<Cell<u64>> = Rc::new(Cell::new(0));
        if let Some(host) = clip_host.clone() {
            let clip_hash = clip_hash.clone();
            crate::util::worker::run(
                move || crate::remote::fetch_tmux_buffer(&host),
                move |buf| {
                    if let Some(buf) = buf {
                        clip_hash.set(Self::tmux_buffer_hash(&buf.text));
                    }
                },
            );
        }
        {
            let mgr = manager.borrow();
            for name in mgr.process_names() {
                let qname = workspace::qualified_name(project_name, name);
                let placeholder = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                placeholder.set_vexpand(true);
                placeholder.set_hexpand(true);
                terminal_stack.add_named(&placeholder, Some(&qname));
            }
        }

        // Wire status change → sidebar update + MCP state sync
        {
            let sidebar_ref = sidebar.clone();
            let pname_cell_status = pname_cell.clone();
            let mcp_state = crate::mcp::bridge::MCP_PROCESS_STATE.clone();
            let detector_status = detector.clone();
            let tunnels_status = tunnels.clone();
            let mut mgr = manager.borrow_mut();
            mgr.set_on_status_change(move |process_name, status| {
                let qname = workspace::qualified_name(&pname_cell_status.borrow(), process_name);
                sidebar_ref.update_process_status(&qname, status);

                // Clear locked port on stop/crash/restart so the next run re-detects.
                if !matches!(status, ProcessStatus::Running) {
                    // Tear down every tunnel this process's output spawned
                    // (badge port + secondary ports like a vite asset server)
                    if let Some(tm) = tunnels_status.borrow_mut().as_mut() {
                        let det = detector_status.borrow();
                        if let Some(port) = det.get_port(process_name) {
                            tm.close(port);
                        }
                        for port in det.all_local_ports(process_name) {
                            tm.close(port);
                        }
                    }
                    detector_status.borrow_mut().clear(process_name);
                    sidebar_ref.set_process_port(&qname, None);
                    sidebar_ref.set_process_url(&qname, None);
                }

                // Update MCP shared state
                if let Ok(mut state) = mcp_state.lock()
                    && let Some(snapshot) = state.get_mut(process_name)
                {
                    snapshot.status = format!("{:?}", status);
                }
            });

            let pf = pid_file.clone();
            mgr.set_on_pid_change(move |pid, acquired| {
                let mut pf = pf.borrow_mut();
                if acquired {
                    pf.add(pid);
                } else {
                    pf.remove(pid);
                }
            });

            // File-watch restart notification. Respects on_file_watch_restart
            // setting + focus-gate + icon resolver, same pattern as clean-exit
            // notifications in auto_restart.rs.
            let pname_cell_fw = pname_cell.clone();
            let focus_gate_fw = focus_gate.clone();
            let icon_resolver_fw = icon_resolver.clone();
            mgr.set_on_file_watch_restart(move |process_name| {
                let settings = AppSettings::load();
                if !settings.notifications.on_file_watch_restart {
                    return;
                }
                let pname = pname_cell_fw.borrow().clone();
                if settings.notifications.suppress_when_focused
                    && let Some(gate) = &focus_gate_fw
                {
                    let qname = workspace::qualified_name(&pname, process_name);
                    if !gate(&qname) {
                        return;
                    }
                }
                let icon = icon_resolver_fw.as_ref().and_then(|r| r(&pname));
                crate::util::notifications::notify_file_watch_restart(
                    &pname,
                    process_name,
                    icon.as_deref(),
                );
            });
        }

        // Per-process wiring, shared by load-time processes and everything
        // added later (Edit Project restore, add dialogs): auto-restart
        // handler, agent idle handlers, and the on_materialized closure that
        // hooks up the terminal (stack swap, clipboard bridge, MCP log
        // capture, port detection + tunnels, auto-open, auto-rename). Stored
        // on the manager as the wiring factory so dynamic-add paths wire new
        // processes identically to load-time ones.
        let wire_process: Rc<dyn Fn(&str)> = {
            let manager = manager.clone();
            let project_name = project_name.to_string();
            let ws = ws.clone();
            let sidebar = sidebar.clone();
            let status_bar = status_bar.clone();
            let selected_process = selected_process.clone();
            let terminal_stack = terminal_stack.clone();
            let detector = detector.clone();
            let tunnels = tunnels.clone();
            let clip_host = clip_host.clone();
            let clip_hash = clip_hash.clone();
            let pname_cell = pname_cell.clone();
            let focus_gate = focus_gate.clone();
            let icon_resolver = icon_resolver.clone();
            Rc::new(move |name: &str| {
                let (skip_port_detection, is_agent, is_auto_named, auto_restart_cfg, command) = {
                    let mgr = manager.borrow();
                    let Some(proc) = mgr.get_process(name) else {
                        return;
                    };
                    (
                        matches!(
                            proc.config.category,
                            crate::config::schema::ProcessCategory::Agent
                                | crate::config::schema::ProcessCategory::SSH
                        ),
                        matches!(
                            proc.config.category,
                            crate::config::schema::ProcessCategory::Agent
                        ),
                        proc.config.auto_named,
                        proc.config.auto_restart,
                        proc.config.command.clone(),
                    )
                };

                // Build the auto-restart handler (returns a shared name cell for rename tracking)
                let (auto_restart_handler, name_cell) =
                    crate::process::auto_restart::build_auto_restart_handler(
                        &manager,
                        &project_name,
                        name,
                        auto_restart_cfg,
                        focus_gate.clone(),
                        icon_resolver.clone(),
                    );

                // For Agent-category processes, also build the idle handler
                // (BEL + activity stamp) and remember the cells so the
                // per-project silence ticker can read them.
                let (agent_idle_handler, last_activity_cell, activity_burst_cell, is_idle_cell) =
                    if is_agent {
                        let kind = crate::util::notifications::AgentKind::from_command(&command);
                        let last_activity = Rc::new(Cell::new(Instant::now()));
                        let activity_burst = Rc::new(Cell::new(0u32));
                        let is_idle = Rc::new(Cell::new(false));
                        let handler = crate::process::auto_restart::build_agent_idle_handler(
                            &project_name,
                            name,
                            kind,
                            last_activity.clone(),
                            activity_burst.clone(),
                            is_idle.clone(),
                            focus_gate.clone(),
                            icon_resolver.clone(),
                        );
                        (
                            Some(handler),
                            Some(last_activity),
                            Some(activity_burst),
                            Some(is_idle),
                        )
                    } else {
                        (None, None, None, None)
                    };

                // Capture refs for the on_materialized closure
                let detector_ref = detector.clone();
                let tunnels_ref = tunnels.clone();
                let clip_host_mat = clip_host.clone();
                let clip_hash_mat = clip_hash.clone();
                let manager_mat = manager.clone();
                let sidebar_ref = sidebar.clone();
                let sb_ref = status_bar.clone();
                let sel_ref = selected_process.clone();
                let proc_name = name.to_string();
                let qname = workspace::qualified_name(&pname_cell.borrow(), name);
                let stack_ref = terminal_stack.clone();
                let mgr_ref = manager.clone();
                let ws_ref = ws.clone();
                let sidebar_rename = sidebar.clone();
                let pname_cell_rename = pname_cell.clone();
                let proc_name_rename = name.to_string();

                let qname_cell: Rc<RefCell<String>> = Rc::new(RefCell::new(qname.clone()));
                let qname_cell_mat = qname_cell.clone();

                let mut mgr = manager.borrow_mut();
                let Some(proc) = mgr.get_process_mut(name) else {
                    return;
                };
                proc.name_cell = Some(name_cell);
                proc.qname_cell = Some(qname_cell);
                proc.last_activity = last_activity_cell;
                proc.activity_burst = activity_burst_cell;
                proc.is_idle = is_idle_cell;
                // Remote: Ctrl+click on a localhost URL must open the local
                // end of its tunnel, not the literal port — the badge shows
                // the remapped port but the terminal text can't be edited.
                // ensure() rather than a bare lookup, so a click also
                // revives a dead forward or creates a missing one (the
                // detector normally has it up already; this is the miss
                // path, and ssh may still be binding when the browser
                // fires — a reload later is the cost of the race).
                if clip_host.is_some() {
                    let tunnels_url = tunnels.clone();
                    proc.url_rewriter = Some(Rc::new(move |url: &str| {
                        crate::util::port_detector::rewrite_clicked_url(url, |port| {
                            tunnels_url
                                .borrow_mut()
                                .as_mut()
                                .and_then(|tm| tm.ensure(port))
                        })
                    }));
                }
                proc.on_materialized = Some(Box::new(move |terminal: &vte4::Terminal| {
                    // Replace placeholder in stack with real terminal
                    let current_qname = qname_cell_mat.borrow().clone();
                    if let Some(old_child) = stack_ref.child_by_name(&current_qname) {
                        stack_ref.remove(&old_child);
                    }
                    stack_ref.add_named(terminal, Some(&current_qname));

                    // Connect auto-restart handler
                    auto_restart_handler(terminal);

                    // Agent-only: connect BEL + activity-stamp handlers.
                    if let Some(ref handler) = agent_idle_handler {
                        handler(terminal);
                    }

                    // Remote: bridge tmux mouse-selections to the local
                    // PRIMARY selection, where a local terminal's drag-
                    // selection also lands (see `tmux_buffer_to_selection`
                    // for why it must stay off CLIPBOARD), and only for
                    // gestures that could have made one (`SelectionGesture`).
                    //
                    // EventControllerLegacy, not a gesture: VTE claims mouse
                    // sequences (always, for tmux/mouse-tracking apps) and a
                    // claimed sequence cancels other gestures — their
                    // `released` never fires. The legacy controller observes
                    // raw events without joining gesture claiming.
                    if let Some(host) = clip_host_mat.clone() {
                        let clip_hash = clip_hash_mat.clone();
                        let gesture = Rc::new(RefCell::new(SelectionGesture::new(
                            gtk4::Settings::default()
                                .map(|s| s.gtk_double_click_time() as u32)
                                .unwrap_or(400),
                            gtk4::Settings::default()
                                .map(|s| s.gtk_double_click_distance() as f64)
                                .unwrap_or(5.0),
                        )));
                        let ctrl = gtk4::EventControllerLegacy::new();
                        ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
                        ctrl.connect_event(move |_, event| {
                            let Some((x, y)) = event.position() else {
                                return glib::Propagation::Proceed;
                            };
                            match event.event_type() {
                                gdk::EventType::ButtonPress => {
                                    gesture.borrow_mut().press(x, y, event.time());
                                }
                                gdk::EventType::ButtonRelease
                                    if gesture.borrow_mut().release(x, y) =>
                                {
                                    log::debug!(
                                        "clipboard bridge: selection gesture, fetching tmux buffer"
                                    );
                                    let host = host.clone();
                                    let clip_hash = clip_hash.clone();
                                    // Give tmux a beat to store the buffer
                                    glib::timeout_add_local_once(
                                        Duration::from_millis(150),
                                        move || {
                                            Self::tmux_buffer_to_clipboard(
                                                &host,
                                                ClipRoute::Selection,
                                                Some(clip_hash),
                                            );
                                        },
                                    );
                                }
                                _ => {}
                            }
                            glib::Propagation::Proceed
                        });
                        terminal.add_controller(ctrl);
                    }

                    // Connect port detection + MCP log capture
                    let log_buffers = crate::mcp::bridge::MCP_LOG_BUFFERS.clone();
                    let log_proc_name = proc_name.clone();
                    let last_row: Rc<Cell<i64>> = Rc::new(Cell::new(0));
                    let detector_ref = detector_ref.clone();
                    let tunnels_cc = tunnels_ref.clone();
                    let mgr_cc = manager_mat.clone();
                    let sidebar_ref = sidebar_ref.clone();
                    let sb_ref = sb_ref.clone();
                    let sel_ref = sel_ref.clone();
                    let proc_name = proc_name.clone();
                    let qname_cell_cc = qname_cell_mat.clone();
                    let clip_host_cc = clip_host_mat.clone();
                    let history_seeded: Rc<Cell<bool>> = Rc::new(Cell::new(false));

                    terminal.connect_contents_changed(move |terminal| {
                        let qname_contents = qname_cell_cc.borrow().clone();
                        let row = terminal.cursor_position().1;

                        // Capture new output lines into the MCP log buffer.
                        // The delta text is kept: the port scanner reuses it
                        // after the badge locks (see below).
                        let delta_text: Option<String> = {
                            let prev_row = last_row.get();
                            if row > prev_row {
                                let cols = terminal.column_count();
                                let (text_opt, _) = terminal.text_range_format(
                                    vte4::Format::Text,
                                    prev_row,
                                    0,
                                    row,
                                    cols,
                                );
                                if let Some(ref text) = text_opt
                                    && let Ok(mut buffers) = log_buffers.lock()
                                {
                                    let buffer = buffers
                                        .entry(log_proc_name.clone())
                                        .or_insert_with(crate::mcp::bridge::LogBuffer::new);
                                    for line in text.lines() {
                                        if !line.trim().is_empty() {
                                            buffer.push(line.to_string());
                                        }
                                    }
                                }
                                last_row.set(row);
                                text_opt.map(|t| t.to_string())
                            } else {
                                None
                            }
                        };

                        // Port detection — skip for agents and stopped processes.
                        // Scans continue until the badge is final: a *local* port
                        // locks, or a labeled preview URL wins (`shopify app dev`).
                        // A plain remote-URL fallback (e.g. an OAuth link during
                        // `shopify theme dev` login) is provisional and
                        // upgradeable. The tunnel ensure() runs on every tick so
                        // a forward that died gets respawned.
                        if !skip_port_detection && sidebar_ref.is_process_running(&qname_contents) {
                            if !detector_ref.borrow().badge_final(&proc_name) {
                                // Remote, once per run: also scan the tmux pane
                                // *history*. After reattaching to a live session
                                // the startup banner (ports, URLs) has usually
                                // scrolled out of the visible screen — without
                                // this, tunnels don't come back until the
                                // process is restarted.
                                if let Some(host) = clip_host_cc.clone()
                                    && !history_seeded.replace(true)
                                    && let Some(session) = mgr_cc
                                        .borrow()
                                        .get_process(&proc_name)
                                        .and_then(|p| p.remote_session.clone())
                                {
                                    let det_seed = detector_ref.clone();
                                    let seed_name = proc_name.clone();
                                    let term_seed = terminal.clone();
                                    crate::util::worker::run(
                                        move || crate::remote::fetch_pane_history(&host, &session),
                                        move |text| {
                                            if text.is_empty() {
                                                return;
                                            }
                                            // -J joins tmux's own wraps, but
                                            // Ink-style CLIs hard-wrap at
                                            // width-1 themselves — rejoin.
                                            det_seed.borrow_mut().scan_output_wrapped(
                                                &seed_name,
                                                &text,
                                                term_seed.column_count() as usize,
                                            );
                                            // Re-fire the handler so the seeded
                                            // ports get applied (tunnels/badges)
                                            // immediately. An idle server emits
                                            // no output on its own — and can't
                                            // receive requests until the tunnel
                                            // exists.
                                            term_seed.emit_by_name::<()>("contents-changed", &[]);
                                        },
                                    );
                                }

                                // Wide enough that the app's startup line isn't
                                // pushed out of view by later log spam before
                                // detection locks in.
                                const PORT_SCAN_LOOKBACK_ROWS: i64 = 200;
                                // Anchor at the end of the buffer content, NOT
                                // the cursor: full-screen apps (shopify's Ink
                                // panel in tmux) park the cursor above their
                                // last lines, and a cursor-bounded scan then
                                // never sees the rows below it — the wrapped
                                // "Preview URL:" continuation row lives there.
                                let content_end = terminal
                                    .vadjustment()
                                    .map(|adj| adj.upper() as i64)
                                    .unwrap_or(row)
                                    .max(row);
                                let start_row = (content_end - PORT_SCAN_LOOKBACK_ROWS).max(0);
                                let cols = terminal.column_count();
                                let (text_opt, _len) = terminal.text_range_format(
                                    vte4::Format::Text,
                                    start_row,
                                    0,
                                    content_end,
                                    cols,
                                );
                                if let Some(text) = text_opt {
                                    let mut det = detector_ref.borrow_mut();
                                    if clip_host_cc.is_some() {
                                        // Remote: tmux redraws long lines as
                                        // separate hard rows, truncating
                                        // wrapped URLs — re-join at the
                                        // terminal width before scanning.
                                        det.scan_output_wrapped(&proc_name, &text, cols as usize);
                                    } else {
                                        det.scan_output(&proc_name, &text);
                                    }
                                }
                            } else if let Some(ref dt) = delta_text {
                                // Badge locked, but secondary servers can boot
                                // later — `php artisan dev` locks the badge on
                                // artisan serve's :8000 seconds before vite
                                // prints its :5174, and without a tunnel for
                                // it every asset URL in the page is dead on a
                                // remote project. The badge can't change
                                // anymore (scan_output early-returns), but
                                // port *harvesting* is monotonic — feed it the
                                // already-extracted delta, which costs no
                                // extra VTE work.
                                let mut det = detector_ref.borrow_mut();
                                if clip_host_cc.is_some() {
                                    det.scan_output_wrapped(
                                        &proc_name,
                                        dt,
                                        terminal.column_count() as usize,
                                    );
                                } else {
                                    det.scan_output(&proc_name, dt);
                                }
                            }

                            let det = detector_ref.borrow();
                            // Remote project: forward the port locally as soon
                            // as it's detected, so the URL is clickable the
                            // moment the badge appears. Only genuinely local
                            // ports are forwarded — a public-host URL has
                            // nothing listening on the ssh host to forward to.
                            // The forward may land on a different local port if
                            // the same one is already taken here — badge and
                            // URL then show the remapped local port.
                            let ports = det.get_port(&proc_name).map(|port| {
                                let mut local = port;
                                if det.has_local_port(&proc_name)
                                    && let Some(tm) = tunnels_cc.borrow_mut().as_mut()
                                    && let Some(lp) = tm.ensure(port)
                                {
                                    local = lp;
                                }
                                (port, local)
                            });
                            // Also forward every other local port seen in the
                            // output (e.g. the vite asset server the theme
                            // proxy loads CSS/JS from) — best-effort, since
                            // page-internal asset URLs can't be remapped.
                            if let Some(tm) = tunnels_cc.borrow_mut().as_mut() {
                                let badge_port = ports.map(|(p, _)| p);
                                for port in det.all_local_ports(&proc_name) {
                                    if badge_port != Some(port) {
                                        let _ = tm.ensure(port);
                                    }
                                }
                            }
                            // Port badge only for genuinely local ports — a
                            // public URL's :443 is noise (the URL button and
                            // status-bar link still appear).
                            if let Some((_, local)) = ports
                                && det.has_local_port(&proc_name)
                            {
                                sidebar_ref.set_process_port(&qname_contents, Some(local));
                            }
                            let url = det.get_url(&proc_name).map(|u| u.to_string());
                            if let Some(mut url_str) = url {
                                if let Some((remote, local)) = ports
                                    && local != remote
                                {
                                    url_str = crate::util::port_detector::remap_url_port(
                                        &url_str, remote, local,
                                    );
                                }
                                sidebar_ref
                                    .set_process_url(&qname_contents, Some(url_str.as_str()));
                                if sel_ref.borrow().as_deref() == Some(qname_contents.as_str()) {
                                    sb_ref.set_url(Some(url_str.as_str()));
                                }
                                // open_in_browser: one-shot armed by a
                                // user-initiated start. The first URL in the
                                // output isn't always the right one —
                                // `shopify app dev` prints proxy/tunnel URLs
                                // seconds before its "Preview URL:" panel —
                                // so fire once the badge is final (local port
                                // locked or labeled preview URL won). A badge
                                // that never finalizes (e.g. only an OAuth
                                // login link) still opens after a short grace.
                                const AUTO_OPEN_GRACE: std::time::Duration =
                                    std::time::Duration::from_secs(5);
                                let (armed, first_seen) = {
                                    let m = mgr_cc.borrow();
                                    m.get_process(&proc_name)
                                        .map(|p| (p.auto_open_armed, p.auto_open_first_url))
                                        .unwrap_or((false, None))
                                };
                                if armed {
                                    let fire = det.badge_final(&proc_name)
                                        || first_seen
                                            .is_some_and(|t| t.elapsed() >= AUTO_OPEN_GRACE);
                                    if fire {
                                        if let Some(p) =
                                            mgr_cc.borrow_mut().get_process_mut(&proc_name)
                                        {
                                            p.auto_open_armed = false;
                                        }
                                        // Local URLs: wait until the server
                                        // actually answers non-5xx before
                                        // opening — `php artisan dev` serves
                                        // :8000 seconds before vite writes
                                        // the manifest, and the tab would
                                        // show the 500 forever. Public URLs
                                        // (admin preview, OAuth) open as-is.
                                        let probe = det.has_local_port(&proc_name)
                                            && url_str.starts_with("http://");
                                        let term_open = terminal.clone();
                                        let name_open = proc_name.clone();
                                        let open = move |url: String| {
                                            log::info!("Opening {url} for {name_open}");
                                            let launcher = gtk4::UriLauncher::new(&url);
                                            let window = term_open
                                                .root()
                                                .and_then(|r| r.downcast::<gtk4::Window>().ok());
                                            launcher.launch(
                                                window.as_ref(),
                                                gtk4::gio::Cancellable::NONE,
                                                |_| {},
                                            );
                                        };
                                        if probe {
                                            let url_probe = url_str.clone();
                                            crate::util::worker::run(
                                                move || {
                                                    wait_http_ready(
                                                        &url_probe,
                                                        std::time::Duration::from_secs(20),
                                                    );
                                                    url_probe
                                                },
                                                open,
                                            );
                                        } else {
                                            open(url_str.clone());
                                        }
                                    } else if first_seen.is_none() {
                                        if let Some(p) =
                                            mgr_cc.borrow_mut().get_process_mut(&proc_name)
                                        {
                                            p.auto_open_first_url = Some(std::time::Instant::now());
                                        }
                                        // Re-poke the handler when the grace
                                        // expires — an idle process emits no
                                        // further output to trigger it.
                                        let term_poke = terminal.clone();
                                        glib::timeout_add_local_once(AUTO_OPEN_GRACE, move || {
                                            term_poke.emit_by_name::<()>("contents-changed", &[]);
                                        });
                                    }
                                }
                            }
                        }
                    });

                    // Wire auto-rename for auto_named processes
                    if is_auto_named {
                        Self::connect_window_title_auto_rename(
                            terminal,
                            &mgr_ref,
                            &proc_name_rename,
                            &sidebar_rename,
                            &qname_cell_mat,
                            &ws_ref,
                            &pname_cell_rename,
                        );
                    }
                }));
            })
        };
        manager
            .borrow_mut()
            .set_wiring_factory(wire_process.clone());
        // Bind the names first — a `for` over `manager.borrow()...` would
        // hold the borrow across iterations while wire_process re-borrows.
        let names: Vec<String> = manager.borrow().process_names().to_vec();
        for name in &names {
            wire_process(name);
        }

        // Populate sidebar
        sidebar.add_project(manager, project_name, icon_path, saved_expanded);
        if let crate::remote::ProjectLocation::Ssh { host, dir } = manager.borrow().location() {
            sidebar.set_project_remote_hint(project_name, &format!("{host}:{dir}"));
        }

        // Per-project ticker for the idle-silence fallback. Walks Agent-category
        // processes every 2 s and fires a "waiting for input" notification when
        // a process has been silent longer than the configured threshold. The
        // ticker itself is always alive (cheap no-op when the fallback setting
        // is off — `check_agent_silence` short-circuits), but only does per-
        // process work when a `last_activity` cell is present.
        {
            let manager_ref = manager.clone();
            let pname_cell_tick = pname_cell.clone();
            let focus_gate_tick = focus_gate.clone();
            let icon_resolver_tick = icon_resolver.clone();
            let sidebar_tick = sidebar.clone();
            glib::timeout_add_local(tuxflow_core::util::activity::SAMPLE_INTERVAL, move || {
                let cells: Vec<(
                    String,
                    crate::util::notifications::AgentKind,
                    Rc<Cell<Instant>>,
                    Rc<Cell<u32>>,
                    Rc<Cell<bool>>,
                )> = {
                    let mgr = manager_ref.borrow();
                    mgr.processes_by_category(crate::config::schema::ProcessCategory::Agent)
                        .into_iter()
                        .filter(|p| p.status == ProcessStatus::Running)
                        .filter_map(
                            |p| match (&p.last_activity, &p.activity_burst, &p.is_idle) {
                                (Some(la), Some(burst), Some(idle)) => Some((
                                    p.id.clone(),
                                    crate::util::notifications::AgentKind::from_command(
                                        &p.config.command,
                                    ),
                                    la.clone(),
                                    burst.clone(),
                                    idle.clone(),
                                )),
                                _ => None,
                            },
                        )
                        .collect()
                };
                if cells.is_empty() {
                    return glib::ControlFlow::Continue;
                }
                let settings = AppSettings::load();
                let threshold = settings.notifications.agent_idle_silence_seconds;
                let project_name = pname_cell_tick.borrow().clone();
                for (name, kind, la, burst, idle) in cells {
                    // Working/waiting dot, on core's shared hysteresis (the
                    // iced shell's card sweep reads the same rule).
                    let events = burst.replace(0);
                    let qname = workspace::qualified_name(&project_name, &name);
                    let working = tuxflow_core::util::activity::next_working(
                        sidebar_tick.is_process_working(&qname),
                        events,
                        la.get().elapsed(),
                    );
                    sidebar_tick.set_process_working(&qname, working);
                    crate::process::auto_restart::check_agent_silence(
                        &project_name,
                        &name,
                        kind,
                        &la,
                        &idle,
                        threshold,
                        focus_gate_tick.as_ref(),
                        icon_resolver_tick.as_ref(),
                    );
                }
                glib::ControlFlow::Continue
            });
        }
    }

    fn load_css() {
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(include_str!("../../data/style.css"));
        gtk4::style_context_add_provider_for_display(
            &gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let settings = AppSettings::load();
        crate::ui::accent::apply(&settings.appearance);
    }

    fn build_content(
        window: &adw::ApplicationWindow,
        ws: &WorkspaceRef,
        sidebar: &Rc<ProjectList>,
        terminal_stack: &gtk4::Stack,
        selected_process: &Rc<RefCell<Option<String>>>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        project_name_cells: &ProjectNameCells,
        on_single_expand_changed: &Rc<dyn Fn(bool)>,
        on_auto_hide_changed: &Rc<dyn Fn(bool)>,
        on_keybind_hints_changed: &Rc<dyn Fn(bool)>,
        on_recent_first_changed: &Rc<dyn Fn(bool)>,
        on_terminal_theme_changed: &Rc<dyn Fn(&str)>,
        on_font_changed: &Rc<dyn Fn()>,
        on_composer_changed: &Rc<dyn Fn(bool)>,
        pid_file: &Rc<RefCell<PidFile>>,
        status_bar: &Rc<StatusBar>,
        composer: &Rc<crate::ui::composer_bar::ComposerBar>,
        keybinding_map: &Rc<RefCell<KeybindingMap>>,
        auto_hide: &Rc<Cell<bool>>,
        focus_gate: &crate::process::auto_restart::FocusGate,
        icon_resolver: &crate::process::auto_restart::IconResolver,
    ) -> gtk4::Widget {
        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

        // Command palette
        let palette = Rc::new(CommandPalette::new());

        // Refresh navigation items dynamically (only running processes)
        let ws_refresh = ws.clone();
        palette.set_on_refresh(move |p| {
            let ws_borrow = ws_refresh.borrow();
            let project_names: Vec<String> = ws_borrow
                .projects()
                .iter()
                .map(|proj| proj.name.clone())
                .collect();
            p.add_project_items(&project_names);
            for project in ws_borrow.projects() {
                let mgr = project.manager.borrow();
                for name in mgr.process_names() {
                    if let Some(proc) = mgr.get_process(name)
                        && proc.status == ProcessStatus::Running
                    {
                        let qname = workspace::qualified_name(&project.name, name);
                        p.add_navigation_items(&[qname]);
                    }
                }
            }
        });

        // Wire palette actions
        let ws_ref = ws.clone();
        let stack_ref = terminal_stack.clone();
        let window_ref = window.clone();
        let palette_ref = palette.clone();
        let sidebar_ref = sidebar.clone();
        let pf_ref = pid_file.clone();
        let sb_ref = status_bar.clone();
        let sel_ref = selected_process.clone();
        let last_proj_ref = last_selected_project.clone();
        let pname_cells_ref = project_name_cells.clone();
        let focus_gate_ref = focus_gate.clone();
        let icon_resolver_ref = icon_resolver.clone();
        palette.set_on_action(move |action| {
            match action {
                "stop_all" => {
                    let ws_borrow = ws_ref.borrow();
                    for project in ws_borrow.projects() {
                        project.manager.borrow_mut().stop_all();
                    }
                }
                "restart_all" => {
                    let ws_borrow = ws_ref.borrow();
                    for project in ws_borrow.projects() {
                        project.manager.borrow_mut().restart_all();
                    }
                }
                "add_project" => {
                    let win = window_ref.clone();
                    let ws2 = ws_ref.clone();
                    let sidebar2 = sidebar_ref.clone();
                    let stack2 = stack_ref.clone();
                    let pf2 = pf_ref.clone();
                    let sb2 = sb_ref.clone();
                    let sel2 = sel_ref.clone();
                    let last_proj2 = last_proj_ref.clone();
                    let pname_cells2 = pname_cells_ref.clone();
                    let focus_gate2 = focus_gate_ref.clone();
                    let icon_resolver2 = icon_resolver_ref.clone();
                    let dialog = gtk4::FileDialog::builder()
                        .title("Open Project Directory")
                        .build();
                    let win2 = win.clone();
                    dialog.select_folder(Some(&win), gtk4::gio::Cancellable::NONE, move |result| {
                        if let Ok(file) = result
                            && let Some(path) = file.path()
                        {
                            Self::load_project_interactive(
                                &win2,
                                &ws2,
                                &sidebar2,
                                &stack2,
                                &path,
                                &pf2,
                                &sb2,
                                &sel2,
                                &last_proj2,
                                &pname_cells2,
                                Some(focus_gate2.clone()),
                                Some(icon_resolver2.clone()),
                            );
                        }
                    });
                }
                "add_remote_project" => {
                    let win = window_ref.clone();
                    let ws2 = ws_ref.clone();
                    let sidebar2 = sidebar_ref.clone();
                    let stack2 = stack_ref.clone();
                    let pf2 = pf_ref.clone();
                    let sb2 = sb_ref.clone();
                    let sel2 = sel_ref.clone();
                    let last_proj2 = last_proj_ref.clone();
                    let pname_cells2 = pname_cells_ref.clone();
                    let focus_gate2 = focus_gate_ref.clone();
                    let icon_resolver2 = icon_resolver_ref.clone();
                    let win_cb = win.clone();
                    crate::ui::add_remote_project_dialog::AddRemoteProjectDialog::show(
                        &win,
                        move |host, dir| {
                            Self::load_remote_project_interactive(
                                &win_cb,
                                &ws2,
                                &sidebar2,
                                &stack2,
                                crate::remote::ProjectLocation::Ssh { host, dir },
                                &pf2,
                                &sb2,
                                &sel2,
                                &last_proj2,
                                &pname_cells2,
                                Some(focus_gate2.clone()),
                                Some(icon_resolver2.clone()),
                            );
                        },
                    );
                }
                "new_custom_agent" => {
                    let ws2 = ws_ref.clone();
                    let stack2 = stack_ref.clone();
                    let sidebar2 = sidebar_ref.clone();
                    let project_names: Vec<String> = ws_ref
                        .borrow()
                        .projects()
                        .iter()
                        .map(|p| p.name.clone())
                        .collect();
                    let best =
                        Self::resolve_active_project(&stack_ref, &last_proj_ref, &sidebar_ref);
                    let last_proj_cb = last_proj_ref.clone();
                    AddCommandDialog::show_add_agent(
                        &window_ref,
                        &project_names,
                        best.as_deref(),
                        move |selected_project, mut config| {
                            *last_proj_cb.borrow_mut() = Some(selected_project.to_string());
                            let name = config.name.clone();
                            {
                                let ws_borrow = ws2.borrow();
                                if let Some(project) = ws_borrow
                                    .projects()
                                    .iter()
                                    .find(|p| p.name == selected_project)
                                    && config.working_dir.is_none()
                                {
                                    config.working_dir = Some(project.location.dir_str());
                                }
                            }
                            ws2.borrow_mut()
                                .save_custom_command(selected_project, config.clone());

                            let ws_borrow = ws2.borrow();
                            if let Some(project) = ws_borrow
                                .projects()
                                .iter()
                                .find(|p| p.name == selected_project)
                            {
                                let project_name = project.name.clone();
                                let qname = workspace::qualified_name(&project_name, &name);
                                project.manager.borrow_mut().add_process(config);
                                // Full wiring via the project's factory (see
                                // wire_project) — must run before materialize
                                // so on_materialized is in place.
                                let factory = project.manager.borrow().wiring_factory();
                                if let Some(factory) = factory {
                                    factory(&name);
                                }
                                project.manager.borrow_mut().materialize_process(&name);
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &name,
                                    ProcessStatus::Stopped,
                                    crate::config::schema::ProcessCategory::Agent,
                                );
                                sidebar2.expand_project(&project_name);
                                project.manager.borrow_mut().spawn(&name);
                                stack2.set_visible_child_name(&qname);
                            }
                        },
                    );
                }
                "new_ssh" => {
                    let ws2 = ws_ref.clone();
                    let stack2 = stack_ref.clone();
                    let sidebar2 = sidebar_ref.clone();
                    let project_names: Vec<String> = ws_ref
                        .borrow()
                        .projects()
                        .iter()
                        .map(|p| p.name.clone())
                        .collect();
                    let best =
                        Self::resolve_active_project(&stack_ref, &last_proj_ref, &sidebar_ref);
                    let last_proj_cb = last_proj_ref.clone();
                    AddSshDialog::show(
                        &window_ref,
                        &project_names,
                        best.as_deref(),
                        move |selected_project, mut config| {
                            *last_proj_cb.borrow_mut() = Some(selected_project.to_string());
                            let name = config.name.clone();
                            let start_with_project = config.start_with_project;
                            {
                                let ws_borrow = ws2.borrow();
                                if let Some(project) = ws_borrow
                                    .projects()
                                    .iter()
                                    .find(|p| p.name == selected_project)
                                    && config.working_dir.is_none()
                                {
                                    config.working_dir = Some(project.location.dir_str());
                                }
                            }
                            ws2.borrow_mut()
                                .save_custom_command(selected_project, config.clone());

                            let ws_borrow = ws2.borrow();
                            if let Some(project) = ws_borrow
                                .projects()
                                .iter()
                                .find(|p| p.name == selected_project)
                            {
                                let project_name = project.name.clone();
                                let qname = workspace::qualified_name(&project_name, &name);
                                project.manager.borrow_mut().add_process(config);
                                let factory = project.manager.borrow().wiring_factory();
                                if let Some(factory) = factory {
                                    factory(&name);
                                }
                                project.manager.borrow_mut().materialize_process(&name);
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &name,
                                    ProcessStatus::Stopped,
                                    crate::config::schema::ProcessCategory::SSH,
                                );
                                sidebar2.expand_project(&project_name);
                                if start_with_project {
                                    project.manager.borrow_mut().spawn(&name);
                                }
                                stack2.set_visible_child_name(&qname);
                            }
                        },
                    );
                }
                "add_process" => {
                    let ws2 = ws_ref.clone();
                    let stack = stack_ref.clone();
                    let sidebar2 = sidebar_ref.clone();
                    let project_names: Vec<String> = ws_ref
                        .borrow()
                        .projects()
                        .iter()
                        .map(|p| p.name.clone())
                        .collect();
                    let best =
                        Self::resolve_active_project(&stack_ref, &last_proj_ref, &sidebar_ref);
                    let last_proj_cb = last_proj_ref.clone();
                    AddCommandDialog::show(
                        &window_ref,
                        &project_names,
                        best.as_deref(),
                        move |selected_project, mut config| {
                            *last_proj_cb.borrow_mut() = Some(selected_project.to_string());
                            let category = config.category.clone();
                            let start_with_project = config.start_with_project;
                            let name = config.name.clone();
                            // Default working_dir to project directory before persisting
                            {
                                let ws_borrow = ws2.borrow();
                                if let Some(project) = ws_borrow
                                    .projects()
                                    .iter()
                                    .find(|p| p.name == selected_project)
                                    && config.working_dir.is_none()
                                {
                                    config.working_dir = Some(project.location.dir_str());
                                }
                            }
                            // Persist the custom command and clear any stale deletion marker
                            // so it doesn't get filtered out on next startup.
                            {
                                let mut ws_mut = ws2.borrow_mut();
                                ws_mut.unmark_process_deleted(selected_project, &name);
                                ws_mut.save_custom_command(selected_project, config.clone());
                            }

                            let ws_borrow = ws2.borrow();
                            if let Some(project) = ws_borrow
                                .projects()
                                .iter()
                                .find(|p| p.name == selected_project)
                            {
                                let project_name = project.name.clone();
                                let qname = workspace::qualified_name(&project_name, &name);
                                project.manager.borrow_mut().add_process(config);
                                let factory = project.manager.borrow().wiring_factory();
                                if let Some(factory) = factory {
                                    factory(&name);
                                }
                                let status = {
                                    let mut mgr = project.manager.borrow_mut();
                                    mgr.materialize_process(&name);
                                    mgr.get_process(&name)
                                        .map(|p| p.status)
                                        .unwrap_or(ProcessStatus::Stopped)
                                };
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &name,
                                    status,
                                    category,
                                );
                                sidebar2.expand_project(&project_name);
                                if start_with_project {
                                    project.manager.borrow_mut().spawn(&name);
                                }
                                stack.set_visible_child_name(&qname);
                            }
                        },
                    );
                }
                "new_terminal" => {
                    let ws2 = ws_ref.clone();
                    let stack2 = stack_ref.clone();
                    let sidebar2 = sidebar_ref.clone();
                    let win2 = window_ref.clone();
                    let project_names: Vec<String> = ws_ref
                        .borrow()
                        .projects()
                        .iter()
                        .map(|p| p.name.clone())
                        .collect();
                    let best =
                        Self::resolve_active_project(&stack_ref, &last_proj_ref, &sidebar_ref);
                    Self::pick_project(
                        &win2,
                        &project_names,
                        best.as_deref(),
                        &last_proj_ref,
                        move |selected_project| {
                            let term_name = format!(
                                "terminal-{}",
                                uuid::Uuid::new_v4()
                                    .to_string()
                                    .split('-')
                                    .next()
                                    .unwrap_or("0")
                            );
                            let mut config = crate::config::schema::ProcessConfig {
                                name: term_name.clone(),
                                command: std::env::var("SHELL")
                                    .unwrap_or_else(|_| "/bin/bash".to_string()),
                                working_dir: None,
                                start_with_project: true,
                                auto_restart: false,
                                open_in_browser: false,
                                restart_when_changed: Vec::new(),
                                env: std::collections::BTreeMap::new(),
                                category: crate::config::schema::ProcessCategory::Terminal,
                                auto_named: true,
                                display_name: None,
                            };
                            // Set working_dir and persist before borrowing workspace immutably
                            {
                                let ws_borrow = ws2.borrow();
                                if let Some(project) = ws_borrow
                                    .projects()
                                    .iter()
                                    .find(|p| p.name == selected_project)
                                {
                                    config.working_dir = Some(project.location.dir_str());
                                }
                            }
                            ws2.borrow_mut()
                                .save_custom_command(selected_project, config.clone());

                            let ws_borrow = ws2.borrow();
                            if let Some(project) = ws_borrow
                                .projects()
                                .iter()
                                .find(|p| p.name == selected_project)
                            {
                                let project_name = project.name.clone();
                                let qname = workspace::qualified_name(&project_name, &term_name);
                                project.manager.borrow_mut().add_process(config);
                                let factory = project.manager.borrow().wiring_factory();
                                if let Some(factory) = factory {
                                    factory(&term_name);
                                }
                                project.manager.borrow_mut().materialize_process(&term_name);
                                // Add sidebar row before spawning so status updates are received
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &term_name,
                                    ProcessStatus::Stopped,
                                    crate::config::schema::ProcessCategory::Terminal,
                                );
                                sidebar2.expand_project(&project_name);
                                project.manager.borrow_mut().spawn(&term_name);
                                stack2.set_visible_child_name(&qname);
                            }
                        },
                    );
                }
                _ if action.starts_with("new_agent:") => {
                    let agent_type = action[10..].to_string();
                    let ws2 = ws_ref.clone();
                    let stack2 = stack_ref.clone();
                    let sidebar2 = sidebar_ref.clone();
                    let win2 = window_ref.clone();
                    let project_names: Vec<String> = ws_ref
                        .borrow()
                        .projects()
                        .iter()
                        .map(|p| p.name.clone())
                        .collect();
                    let best =
                        Self::resolve_active_project(&stack_ref, &last_proj_ref, &sidebar_ref);
                    Self::pick_project(
                        &win2,
                        &project_names,
                        best.as_deref(),
                        &last_proj_ref,
                        move |selected_project| {
                            let agent_name = format!(
                                "{agent_type}-{}",
                                uuid::Uuid::new_v4()
                                    .to_string()
                                    .split('-')
                                    .next()
                                    .unwrap_or("0")
                            );
                            // The palette items are generated from
                            // AGENT_PRESETS, so the command comes from the
                            // same row — a preset whose command ever
                            // diverges from its slug must not silently run
                            // the slug here.
                            let command = tuxflow_core::util::agents::AGENT_PRESETS
                                .iter()
                                .find(|p| p.slug == agent_type)
                                .map(|p| p.command.to_string())
                                .unwrap_or_else(|| agent_type.to_string());
                            let mut config = crate::config::schema::ProcessConfig {
                                name: agent_name.clone(),
                                command,
                                working_dir: None,
                                start_with_project: false,
                                auto_restart: false,
                                open_in_browser: false,
                                restart_when_changed: Vec::new(),
                                env: std::collections::BTreeMap::new(),
                                category: crate::config::schema::ProcessCategory::Agent,
                                auto_named: true,
                                display_name: None,
                            };
                            // Set working_dir and persist
                            {
                                let ws_borrow = ws2.borrow();
                                if let Some(project) = ws_borrow
                                    .projects()
                                    .iter()
                                    .find(|p| p.name == selected_project)
                                {
                                    config.working_dir = Some(project.location.dir_str());
                                }
                            }
                            ws2.borrow_mut()
                                .save_custom_command(selected_project, config.clone());

                            let ws_borrow = ws2.borrow();
                            if let Some(project) = ws_borrow
                                .projects()
                                .iter()
                                .find(|p| p.name == selected_project)
                            {
                                let project_name = project.name.clone();
                                let qname = workspace::qualified_name(&project_name, &agent_name);
                                project.manager.borrow_mut().add_process(config);
                                let factory = project.manager.borrow().wiring_factory();
                                if let Some(factory) = factory {
                                    factory(&agent_name);
                                }
                                project
                                    .manager
                                    .borrow_mut()
                                    .materialize_process(&agent_name);
                                // Add sidebar row before spawning so status updates are received
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &agent_name,
                                    ProcessStatus::Stopped,
                                    crate::config::schema::ProcessCategory::Agent,
                                );
                                sidebar2.expand_project(&project_name);
                                project.manager.borrow_mut().spawn(&agent_name);
                                stack2.set_visible_child_name(&qname);
                            }
                        },
                    );
                }
                _ if action.starts_with("project:") => {
                    let pname = &action[8..];
                    let idx = ws_ref
                        .borrow()
                        .projects()
                        .iter()
                        .position(|p| p.name == pname);
                    if let Some(idx) = idx {
                        Self::switch_to_project(
                            &ws_ref,
                            &stack_ref,
                            &sidebar_ref,
                            &sb_ref,
                            &last_proj_ref,
                            idx,
                        );
                    }
                }
                _ if action.starts_with("switch:") => {
                    let qname = &action[7..];
                    stack_ref.set_visible_child_name(qname);
                    *sel_ref.borrow_mut() = Some(qname.to_string());
                    sidebar_ref.select_process(qname);
                    if let Some((proj, _)) = qname.split_once("::") {
                        sidebar_ref.set_active_project(proj);
                        Self::refresh_status_bar_for_project(
                            &ws_ref,
                            &sb_ref,
                            &last_proj_ref,
                            proj,
                        );
                    }
                    let url = sidebar_ref.get_process_url(qname);
                    sb_ref.set_url(url.as_deref());
                }
                _ => log::warn!("Unknown palette action: {action}"),
            }
            palette_ref.hide();
            if let Some(child) = stack_ref.visible_child() {
                child.grab_focus();
            }
        });

        // Status bar actions
        // Stop selected process (or all if none selected)
        let ws_ref = ws.clone();
        let selected_ref = selected_process.clone();
        status_bar.connect_stop(move || {
            let ws_borrow = ws_ref.borrow();
            if let Some(ref qname) = *selected_ref.borrow()
                && let Some((proj, proc_name)) = qname.split_once("::")
                && let Some(mgr) = ws_borrow.get_manager_for_project(proj)
            {
                mgr.borrow_mut().kill(proc_name);
                return;
            }
            for project in ws_borrow.projects() {
                project.manager.borrow_mut().stop_all();
            }
        });

        // Restart selected process (or all if none selected)
        let ws_ref = ws.clone();
        let selected_ref = selected_process.clone();
        status_bar.connect_restart(move || {
            let ws_borrow = ws_ref.borrow();
            if let Some(ref qname) = *selected_ref.borrow()
                && let Some((proj, proc_name)) = qname.split_once("::")
                && let Some(mgr) = ws_borrow.get_manager_for_project(proj)
            {
                mgr.borrow_mut().restart(proc_name);
                return;
            }
            for project in ws_borrow.projects() {
                project.manager.borrow_mut().restart_all();
            }
        });

        let stack_ref = terminal_stack.clone();
        status_bar.connect_clear(move || {
            if let Some(child) = stack_ref.visible_child()
                && let Ok(terminal) = child.downcast::<vte4::Terminal>()
            {
                terminal.reset(true, true);
            }
        });

        // Split view
        let split_view = adw::OverlaySplitView::new();

        // Headerbar
        let (headerbar, title_label) = Self::build_headerbar(
            window,
            &split_view,
            &palette,
            sidebar,
            on_single_expand_changed,
            on_auto_hide_changed,
            on_keybind_hints_changed,
            on_recent_first_changed,
            on_terminal_theme_changed,
            on_font_changed,
            on_composer_changed,
            keybinding_map,
        );

        // Shared closure to refresh status bar counts
        // Deferred to idle so it never collides with an in-progress borrow_mut on a manager
        let refresh_counts: Rc<dyn Fn()> = {
            let ws_ref = ws.clone();
            let sb_ref = status_bar.clone();
            let last_proj = last_selected_project.clone();
            let selected_ref = selected_process.clone();
            Rc::new(move || {
                let ws_inner = ws_ref.clone();
                let sb = sb_ref.clone();
                let proj = last_proj.clone();
                let selected = selected_ref.clone();
                glib::idle_add_local_once(move || {
                    let ws_borrow = ws_inner.borrow();
                    let selected_proj = proj.borrow();
                    let running =
                        is_qualified_process_running(&ws_borrow, selected.borrow().as_deref());
                    sb.set_process_running(running);
                    let mut global_r = 0usize;
                    let mut global_t = 0usize;
                    let mut proj_r = 0usize;
                    let mut proj_t = 0usize;
                    let mut running_names = Vec::new();
                    for project in ws_borrow.projects() {
                        let mgr = project.manager.borrow();
                        let r = mgr.running_count();
                        let t = mgr.total_count();
                        global_r += r;
                        global_t += t;
                        if selected_proj.as_deref() == Some(&project.name) {
                            proj_r = r;
                            proj_t = t;
                        }
                        let names: Vec<String> =
                            mgr.running_names().into_iter().map(String::from).collect();
                        if !names.is_empty() {
                            running_names.push((project.name.clone(), names));
                        }
                    }
                    let has_project = selected_proj.is_some();
                    sb.set_project_info(selected_proj.as_deref(), proj_r, proj_t);
                    sb.set_global_info(global_r, global_t, has_project, &running_names);
                });
            })
        };
        sidebar.set_on_counts_changed({
            let refresh = refresh_counts.clone();
            let sb = sidebar.clone();
            move || {
                refresh();
                sb.refresh_all_project_start_states();
            }
        });
        sidebar.set_on_project_commands_changed({
            let ws_ref = ws.clone();
            let stack_ref = terminal_stack.clone();
            let sidebar_ref = sidebar.clone();
            let selected_ref = selected_process.clone();
            let refresh = refresh_counts.clone();
            move |project_name, enabled, disabled| {
                // Apply disables first: kill, persist deletion, remove sidebar row,
                // remove terminal from stack.
                for name in disabled {
                    let qname = workspace::qualified_name(project_name, name);
                    let mgr_ref = ws_ref
                        .borrow()
                        .get_manager_for_project(project_name)
                        .cloned();
                    if let Some(mgr) = mgr_ref {
                        mgr.borrow_mut().remove_process(name);
                    }
                    ws_ref.borrow_mut().mark_process_deleted(project_name, name);
                    sidebar_ref.remove_process_row(&qname);
                    if let Some(child) = stack_ref.child_by_name(&qname) {
                        stack_ref.remove(&child);
                    }
                    let mut sel = selected_ref.borrow_mut();
                    if sel.as_deref() == Some(&qname) {
                        stack_ref.set_visible_child_name("__welcome__");
                        *sel = None;
                    }
                }

                // Apply enables: persist as custom command, unmark deleted, add to
                // manager, run the project's wiring factory, materialize, add
                // sidebar row.
                for cfg in enabled {
                    let name = cfg.name.clone();
                    let category = cfg.category.clone();

                    // Default working_dir to the project directory so commands resolve correctly.
                    let mut cfg = cfg.clone();
                    if cfg.working_dir.is_none() {
                        cfg.working_dir = ws_ref
                            .borrow()
                            .get_project_location(project_name)
                            .map(|l| l.dir_str());
                    }

                    ws_ref
                        .borrow_mut()
                        .unmark_process_deleted(project_name, &name);
                    ws_ref
                        .borrow_mut()
                        .save_custom_command(project_name, cfg.clone());

                    let mgr_ref = ws_ref
                        .borrow()
                        .get_manager_for_project(project_name)
                        .cloned();
                    let Some(mgr) = mgr_ref else { continue };

                    mgr.borrow_mut().add_process(cfg.clone());
                    // Full per-process wiring via the project's factory
                    // (auto-restart, port detection, clipboard bridge, stack
                    // insertion on materialize) — identical to load-time
                    // processes. Must run before materialize_process so
                    // on_materialized is in place when the terminal appears.
                    let factory = mgr.borrow().wiring_factory();
                    if let Some(factory) = factory {
                        factory(&name);
                    }
                    let status = {
                        let mut m = mgr.borrow_mut();
                        m.materialize_process(&name);
                        m.get_process(&name)
                            .map(|p| p.status)
                            .unwrap_or(ProcessStatus::Stopped)
                    };

                    sidebar_ref.add_process_to_project(&mgr, project_name, &name, status, category);
                }

                refresh();
                sidebar_ref.refresh_all_project_start_states();
            }
        });
        sidebar.set_on_project_renamed({
            let last_proj = last_selected_project.clone();
            let refresh = refresh_counts.clone();
            let pname_cells = project_name_cells.clone();
            let ws_rename = ws.clone();
            let stack_rename = terminal_stack.clone();
            move |old_name, new_name| {
                let mut lp = last_proj.borrow_mut();
                if lp.as_deref() == Some(old_name) {
                    *lp = Some(new_name.to_string());
                }
                drop(lp);

                // Update the shared project-name cell so closures captured in
                // wire_project (status callback, file-watch callback, silence
                // ticker, window-title rename) keep producing the current
                // qualified names.
                let cell = {
                    let mut reg = pname_cells.borrow_mut();
                    let cell = reg.remove(old_name);
                    if let Some(ref c) = cell {
                        *c.borrow_mut() = new_name.to_string();
                        reg.insert(new_name.to_string(), c.clone());
                    }
                    cell
                };
                if cell.is_none() {
                    log::warn!(
                        "on_project_renamed: no project-name cell registered for '{old_name}'"
                    );
                }

                // Rename terminal stack pages for every process in the project,
                // so `set_visible_child_name(new_qname)` finds them. Also update
                // each process's `qname_cell` so per-terminal closures
                // (on_materialized, contents-changed) produce the new qname.
                let old_prefix = format!("{old_name}::");
                let new_prefix = format!("{new_name}::");
                let ws_borrow = ws_rename.borrow();
                if let Some(project) = ws_borrow.projects().iter().find(|p| p.name == new_name) {
                    let mut mgr = project.manager.borrow_mut();
                    let proc_names: Vec<String> = mgr.process_names().to_vec();
                    for proc_name in &proc_names {
                        let old_qname = format!("{old_prefix}{proc_name}");
                        let new_qname = format!("{new_prefix}{proc_name}");
                        if let Some(child) = stack_rename.child_by_name(&old_qname) {
                            stack_rename.page(&child).set_name(&new_qname);
                        }
                        if let Some(proc) = mgr.get_process_mut(proc_name)
                            && let Some(ref qcell) = proc.qname_cell
                        {
                            *qcell.borrow_mut() = new_qname;
                        }
                    }
                }
                drop(ws_borrow);

                refresh();
            }
        });
        // Initial status bar refresh
        refresh_counts();

        let sidebar_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .width_request(260)
            .child(sidebar.widget())
            .build();

        split_view.set_sidebar(Some(&sidebar_scroll));
        split_view.set_show_sidebar(true);
        split_view.set_collapsed(false);
        split_view.set_min_sidebar_width(220.0);
        split_view.set_max_sidebar_width(400.0);

        // Focus mode: toggle sidebar
        let sv_ref = split_view.clone();
        status_bar.connect_focus(move || {
            sv_ref.set_show_sidebar(!sv_ref.shows_sidebar());
        });

        // Git changes button
        let ws_git = ws.clone();
        let stack_git = terminal_stack.clone();
        let last_proj_git = last_selected_project.clone();
        let sidebar_git = sidebar.clone();
        let status_bar_git = status_bar.clone();
        status_bar.connect_git_changes(move |btn| {
            let project_name =
                Self::resolve_active_project(&stack_git, &last_proj_git, &sidebar_git);
            if let Some(proj_name) = project_name {
                let ws_borrow = ws_git.borrow();
                if let Some(location) = ws_borrow.get_project_location(&proj_name) {
                    // Refresh the badge as the dialog opens — cheap, no fetch.
                    Self::refresh_status_bar_git(location.clone(), status_bar_git.clone(), false);

                    let cb = {
                        let ws = ws_git.clone();
                        let sb = status_bar_git.clone();
                        let stack = stack_git.clone();
                        let last_proj = last_proj_git.clone();
                        let sidebar = sidebar_git.clone();
                        move || {
                            if let Some(proj) =
                                Self::resolve_active_project(&stack, &last_proj, &sidebar)
                                && let Some(location) = ws.borrow().get_project_location(&proj)
                            {
                                Self::refresh_status_bar_git(location, sb.clone(), true);
                            }
                        }
                    };
                    GitChangesDialog::show(btn, &location, status_bar_git.git_seed(), cb);
                }
            }
        });

        // Sync button: one click = fetch + ff-only pull + push, off-thread.
        // Diverged histories error out (no merge attempt) and point at the
        // Git Changes dialog. When already in sync it's just a fetch, which
        // refreshes the counters.
        let ws_sync = ws.clone();
        let stack_sync = terminal_stack.clone();
        let last_proj_sync = last_selected_project.clone();
        let sidebar_sync = sidebar.clone();
        let status_bar_sync = status_bar.clone();
        status_bar.connect_git_sync(move |btn| {
            let project_name =
                Self::resolve_active_project(&stack_sync, &last_proj_sync, &sidebar_sync);
            let Some(proj_name) = project_name else {
                return;
            };
            let Some(location) = ws_sync.borrow().get_project_location(&proj_name) else {
                return;
            };
            status_bar_sync.set_git_syncing(true);

            let sb_done = status_bar_sync.clone();
            let location_done = location.clone();
            let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            crate::util::worker::run(
                move || sync_with_remote(&location),
                move |result| {
                    // Counters refresh regardless — even a failed sync may
                    // have fetched, and a partial sync changed them. The
                    // spinner stays up across that refresh and hands straight
                    // over to the new numbers.
                    let sb_spinner = sb_done.clone();
                    Self::refresh_status_bar_git_then(
                        location_done,
                        sb_done.clone(),
                        false,
                        Some(Box::new(move || sb_spinner.set_git_syncing(false))),
                    );
                    if let Err(err) = result {
                        let dialog = adw::AlertDialog::builder()
                            .heading("Sync failed")
                            .body(format!("{err}\n\nOpen Git Changes to resolve it manually."))
                            .build();
                        dialog.add_response("ok", "OK");
                        dialog.set_default_response(Some("ok"));
                        dialog.present(parent.as_ref());
                    }
                },
            );
        });

        // Terminal search bar
        let search_bar = Rc::new(TerminalSearch::new());

        // Update search bar terminal and window title when stack child changes
        let search_ref = search_bar.clone();
        let title_ref = title_label.clone();
        let ws_vis = ws.clone();
        let sb_vis = status_bar.clone();
        terminal_stack.connect_visible_child_notify(move |stack| {
            if let Some(child) = stack.visible_child()
                && let Ok(terminal) = child.downcast::<vte4::Terminal>()
            {
                search_ref.set_terminal(&terminal);
            }
            // Whatever git refresh is in flight belongs to the terminal we're
            // leaving; retire it so it can't paint its project's counters onto
            // the chip after the switch. Every branch below either starts its
            // own refresh (which takes a fresh token) or clears the chip.
            sb_vis.begin_git_refresh();
            if let Some(name) = stack.visible_child_name() {
                if let Some((proj, _)) = name.split_once("::") {
                    title_ref.set_label(proj);
                    let loc_opt = ws_vis.borrow().get_project_location(proj);
                    match loc_opt {
                        Some(location @ crate::remote::ProjectLocation::Local(_)) => {
                            sb_vis.set_remote_hint(None);
                            let has_git = has_git_repo(&location);
                            sb_vis.set_git_available(has_git);
                            if has_git {
                                Self::refresh_status_bar_git(location, sb_vis.clone(), true);
                            } else {
                                sb_vis.set_git_sync(0, 0);
                                sb_vis.set_git_diffstat(0, 0, 0, 0);
                                sb_vis.set_git_branch(None);
                            }
                        }
                        Some(location @ crate::remote::ProjectLocation::Ssh { .. }) => {
                            if let crate::remote::ProjectLocation::Ssh { host, dir } = &location {
                                sb_vis.set_remote_hint(Some(&format!("{host}:{dir}")));
                            }
                            // Probing .git needs an ssh round trip — do it off
                            // the main thread, hide the button until it answers.
                            sb_vis.set_git_available(false);
                            sb_vis.set_git_sync(0, 0);
                            sb_vis.set_git_diffstat(0, 0, 0, 0);
                            sb_vis.set_git_branch(None);
                            let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
                            {
                                let location = location.clone();
                                std::thread::spawn(move || {
                                    let _ = tx.send(has_git_repo(&location));
                                });
                            }
                            let sb = sb_vis.clone();
                            let stack = stack.clone();
                            let proj = proj.to_string();
                            glib::spawn_future_local(async move {
                                if rx.await != Ok(true) {
                                    return;
                                }
                                // Ignore if the user already switched tabs
                                let still_current = stack
                                    .visible_child_name()
                                    .and_then(|n| n.split_once("::").map(|(p, _)| p.to_string()))
                                    .is_some_and(|p| p == proj);
                                if still_current {
                                    sb.set_git_available(true);
                                    TuxFlowWindow::refresh_status_bar_git(
                                        location,
                                        sb.clone(),
                                        true,
                                    );
                                }
                            });
                        }
                        None => {
                            sb_vis.set_remote_hint(None);
                            sb_vis.set_git_available(false);
                            sb_vis.set_git_sync(0, 0);
                            sb_vis.set_git_diffstat(0, 0, 0, 0);
                            sb_vis.set_git_branch(None);
                        }
                    }
                }
            } else {
                title_ref.set_label("TuxFlow");
                sb_vis.set_remote_hint(None);
                sb_vis.set_git_available(false);
                sb_vis.set_git_sync(0, 0);
                sb_vis.set_git_diffstat(0, 0, 0, 0);
                sb_vis.set_git_branch(None);
            }
        });

        // Poll git pull indicator every 60 seconds
        {
            let ws_poll = ws.clone();
            let sb_poll = status_bar.clone();
            let stack_poll = terminal_stack.clone();
            let last_proj_poll = last_selected_project.clone();
            let sidebar_poll = sidebar.clone();
            // One refresh in flight at a time: when a remote host is down,
            // each git call blocks its worker thread for up to 10 s — without
            // this, every tick would stack another blocked thread.
            let in_flight = Rc::new(Cell::new(false));
            glib::timeout_add_seconds_local(60, move || {
                if in_flight.get() {
                    return glib::ControlFlow::Continue;
                }
                let project_name = TuxFlowWindow::resolve_active_project(
                    &stack_poll,
                    &last_proj_poll,
                    &sidebar_poll,
                );
                if let Some(proj_name) = project_name {
                    let loc_opt = ws_poll.borrow().get_project_location(&proj_name);
                    if let Some(location) = loc_opt {
                        // Local: skip non-repos via a cheap stat. Remote: refresh
                        // unconditionally — the worker thread's git calls just
                        // return zero counts when there's no repo.
                        let do_refresh = match &location {
                            crate::remote::ProjectLocation::Local(d) => d.join(".git").exists(),
                            crate::remote::ProjectLocation::Ssh { .. } => true,
                        };
                        if do_refresh {
                            in_flight.set(true);
                            let flag = in_flight.clone();
                            TuxFlowWindow::refresh_status_bar_git_then(
                                location,
                                sb_poll.clone(),
                                true,
                                Some(Box::new(move || flag.set(false))),
                            );
                        }
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        // Terminal area + composer bar under it (visible for agents only)
        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content_box.append(terminal_stack);
        content_box.append(composer.widget());

        let content_overlay = gtk4::Overlay::new();
        content_overlay.set_child(Some(&content_box));
        content_overlay.add_overlay(palette.widget());
        content_overlay.add_overlay(search_bar.widget());

        // Auto-hide sidebar when clicking the terminal area
        {
            let gesture = gtk4::GestureClick::new();
            gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let sv = split_view.clone();
            let ah = auto_hide.clone();
            let palette_ref = palette.clone();
            let search_ref = search_bar.clone();
            gesture.connect_pressed(move |g, _, _, _| {
                // Never claim — let VTE handle the click normally
                g.set_state(gtk4::EventSequenceState::None);
                // Skip auto-hide when command palette or search bar is open
                if palette_ref.is_visible() || search_ref.is_visible() {
                    return;
                }
                if ah.get() && sv.shows_sidebar() {
                    // Defer to idle so VTE finishes processing the click
                    // before the layout shifts from sidebar hiding
                    let sv = sv.clone();
                    glib::idle_add_local_once(move || {
                        sv.set_show_sidebar(false);
                    });
                }
            });
            content_overlay.add_controller(gesture);
        }

        split_view.set_content(Some(&content_overlay));

        vbox.append(&headerbar);
        vbox.append(&split_view);
        vbox.append(status_bar.widget());

        // No process is selected at startup (welcome screen) — hide the Stop button
        // until a running process is selected.
        status_bar.set_process_running(false);

        Self::setup_keyboard_shortcuts(
            window,
            &palette,
            ws,
            terminal_stack,
            &split_view,
            selected_process,
            &search_bar,
            sidebar,
            on_single_expand_changed,
            on_auto_hide_changed,
            on_keybind_hints_changed,
            on_recent_first_changed,
            on_terminal_theme_changed,
            on_font_changed,
            on_composer_changed,
            keybinding_map,
            last_selected_project,
            status_bar,
            composer,
        );

        vbox.upcast()
    }

    fn setup_keyboard_shortcuts(
        window: &adw::ApplicationWindow,
        palette: &Rc<CommandPalette>,
        ws: &WorkspaceRef,
        terminal_stack: &gtk4::Stack,
        split_view: &adw::OverlaySplitView,
        selected_process: &Rc<RefCell<Option<String>>>,
        search_bar: &Rc<TerminalSearch>,
        sidebar: &Rc<ProjectList>,
        on_single_expand_changed: &Rc<dyn Fn(bool)>,
        on_auto_hide_changed: &Rc<dyn Fn(bool)>,
        on_keybind_hints_changed: &Rc<dyn Fn(bool)>,
        on_recent_first_changed: &Rc<dyn Fn(bool)>,
        on_terminal_theme_changed: &Rc<dyn Fn(&str)>,
        on_font_changed: &Rc<dyn Fn()>,
        on_composer_changed: &Rc<dyn Fn(bool)>,
        keybinding_map: &Rc<RefCell<KeybindingMap>>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        status_bar: &Rc<StatusBar>,
        composer: &Rc<crate::ui::composer_bar::ComposerBar>,
    ) {
        let key_controller = gtk4::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);

        let palette_ref = palette.clone();
        let ws_ref = ws.clone();
        let stack_ref = terminal_stack.clone();
        let sv_ref = split_view.clone();
        let selected_ref = selected_process.clone();
        let window_ref = window.clone();
        let search_ref = search_bar.clone();
        let sidebar_ref = sidebar.clone();
        let single_expand_cb = on_single_expand_changed.clone();
        let auto_hide_cb = on_auto_hide_changed.clone();
        let keybind_hints_cb = on_keybind_hints_changed.clone();
        let recent_first_cb = on_recent_first_changed.clone();
        let theme_cb = on_terminal_theme_changed.clone();
        let font_cb = on_font_changed.clone();
        let composer_cb = on_composer_changed.clone();
        let composer_ref = composer.clone();
        let kb_map = keybinding_map.clone();
        let last_proj_ref = last_selected_project.clone();
        let sb_ref = status_bar.clone();

        key_controller.connect_key_pressed(move |_, keyval, _keycode, state| {
            // Skip all shortcuts while settings key capture is active
            if kb_map.borrow().is_capturing() {
                return gtk4::glib::Propagation::Proceed;
            }

            // Skip all shortcuts when an adw::Dialog is focused (e.g. git
            // changes dialog) so dialog-local shortcuts like Ctrl+Enter work.
            if let Some(fw) =
                gtk4::prelude::GtkWindowExt::focus(window_ref.upcast_ref::<gtk4::Window>())
            {
                let mut w: gtk4::Widget = fw;
                loop {
                    if w.downcast_ref::<adw::Dialog>().is_some() {
                        return gtk4::glib::Propagation::Proceed;
                    }
                    match w.parent() {
                        Some(p) => w = p,
                        None => break,
                    }
                }
            }

            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = state.contains(gdk::ModifierType::ALT_MASK);

            // Skip modifier-only key presses
            if is_modifier_key(&keyval) {
                return gtk4::glib::Propagation::Proceed;
            }

            // Check configurable keybindings
            if let Some(action) = kb_map.borrow().action_for(keyval, state) {
                match action {
                    ShortcutAction::Copy => {
                        if let Some(child) = stack_ref.visible_child()
                            && let Ok(terminal) = child.downcast::<vte4::Terminal>()
                        {
                            // In a remote pane tmux owns the mouse, so the
                            // selection the user is looking at lives in a
                            // tmux paste buffer and VTE has none of its own
                            // — which is why this asks VTE first: a
                            // Shift-drag bypasses mouse reporting and does
                            // select locally. Being explicit, this route
                            // accepts a buffer of any age, so it collects a
                            // copy-mode `y` (no mouse gesture to hang off)
                            // and an agent's OSC 52 copy alike.
                            if !terminal.has_selection()
                                && let Some((host, _)) =
                                    Self::remote_paste_target(&ws_ref, &stack_ref)
                            {
                                Self::tmux_buffer_to_clipboard(
                                    &host,
                                    ClipRoute::ExplicitCopy,
                                    None,
                                );
                            } else {
                                terminal.copy_clipboard_format(vte4::Format::Text);
                            }
                        }
                    }
                    ShortcutAction::Paste => {
                        // Composer focused: the paste belongs to it (images
                        // become attachment chips, text inserts locally) —
                        // never to the terminal underneath.
                        if composer_ref.input_has_focus() {
                            composer_ref.paste();
                        } else if let Some(child) = stack_ref.visible_child()
                            && let Ok(terminal) = child.downcast::<vte4::Terminal>()
                        {
                            // Remote terminal + image in the clipboard: the
                            // image only exists on this machine — upload it
                            // to the host's TuxFlow clipboard file first.
                            let handled = Self::remote_paste_target(&ws_ref, &stack_ref)
                                .is_some_and(|(host, is_agent)| {
                                    Self::paste_image_to_remote(&terminal, &host, is_agent)
                                });
                            if !handled {
                                terminal.paste_clipboard();
                            }
                        }
                    }
                    ShortcutAction::TerminalSearch => {
                        search_ref.toggle();
                    }
                    ShortcutAction::CommandPalette => {
                        palette_ref.toggle();
                    }
                    ShortcutAction::AddNew => {
                        palette_ref.show_with_text("New ");
                    }
                    ShortcutAction::FilterProcesses => {
                        sidebar_ref.toggle_filter();
                    }
                    ShortcutAction::Settings => {
                        crate::ui::settings::settings_window::SettingsWindow::show(
                            &window_ref,
                            Some(single_expand_cb.clone()),
                            Some(auto_hide_cb.clone()),
                            Some(keybind_hints_cb.clone()),
                            Some(recent_first_cb.clone()),
                            Some(theme_cb.clone()),
                            Some(font_cb.clone()),
                            Some(composer_cb.clone()),
                            Some(kb_map.clone()),
                        );
                    }
                    ShortcutAction::FocusSidebar => {
                        sv_ref.set_show_sidebar(true);
                    }
                    ShortcutAction::FocusTerminal => {
                        if palette_ref.is_visible() {
                            palette_ref.hide();
                        }
                        if let Some(child) = stack_ref.visible_child() {
                            child.grab_focus();
                        }
                    }
                    ShortcutAction::PrevProcess => {
                        Self::switch_relative(
                            &ws_ref,
                            &stack_ref,
                            &selected_ref,
                            &sidebar_ref,
                            &sb_ref,
                            &last_proj_ref,
                            -1,
                        );
                    }
                    ShortcutAction::NextProcess => {
                        Self::switch_relative(
                            &ws_ref,
                            &stack_ref,
                            &selected_ref,
                            &sidebar_ref,
                            &sb_ref,
                            &last_proj_ref,
                            1,
                        );
                    }
                    ShortcutAction::FontIncrease => {
                        Self::adjust_font_size(&stack_ref, 1);
                    }
                    ShortcutAction::FontDecrease => {
                        Self::adjust_font_size(&stack_ref, -1);
                    }
                    ShortcutAction::QuickJump => {
                        palette_ref.show_with_text("Switch ");
                    }
                    ShortcutAction::ClearOutput => {
                        if let Some(child) = stack_ref.visible_child()
                            && let Ok(terminal) = child.downcast::<vte4::Terminal>()
                        {
                            terminal.reset(true, true);
                        }
                    }
                    ShortcutAction::ToggleProcess => {
                        Self::toggle_current_process(&ws_ref, &stack_ref);
                    }
                    ShortcutAction::RestartProcess => {
                        Self::restart_current_process(&ws_ref, &stack_ref);
                    }
                    ShortcutAction::CloseProcess => {
                        Self::close_current_process(&ws_ref, &stack_ref, &sidebar_ref);
                    }
                    ShortcutAction::PrevProject => {
                        Self::switch_project_relative(
                            &ws_ref,
                            &stack_ref,
                            &sidebar_ref,
                            &sb_ref,
                            &last_proj_ref,
                            -1,
                        );
                    }
                    ShortcutAction::NextProject => {
                        Self::switch_project_relative(
                            &ws_ref,
                            &stack_ref,
                            &sidebar_ref,
                            &sb_ref,
                            &last_proj_ref,
                            1,
                        );
                    }
                    ShortcutAction::ToggleSidebar => {
                        sv_ref.set_show_sidebar(!sv_ref.shows_sidebar());
                    }
                    ShortcutAction::NewTerminal => {
                        Self::create_terminal_in_current_project(
                            &ws_ref,
                            &stack_ref,
                            &sidebar_ref,
                            &last_proj_ref,
                        );
                    }
                }
                return gtk4::glib::Propagation::Stop;
            }

            // Hardcoded: plain Ctrl+V in a remote *agent* terminal. That's
            // the agent's image-paste chord, but the clipboard lives on this
            // machine — a raw ^V would only earn Claude's "No image found in
            // clipboard". Bridge images (native attachment via the shim) and
            // paste text normally instead. Non-agent terminals keep raw ^V
            // (shell literal-insert).
            if ctrl
                && !state.contains(gdk::ModifierType::SHIFT_MASK)
                && keyval == gdk::Key::v
                // Composer focused: let the event reach it (its own handler
                // turns image pastes into attachment chips).
                && !composer_ref.input_has_focus()
                && let Some((host, true)) = Self::remote_paste_target(&ws_ref, &stack_ref)
                && let Some(child) = stack_ref.visible_child()
                && let Ok(terminal) = child.downcast::<vte4::Terminal>()
            {
                if !Self::paste_image_to_remote(&terminal, &host, true) {
                    terminal.paste_clipboard();
                }
                return gtk4::glib::Propagation::Stop;
            }

            // Hardcoded: Ctrl+Return — focus terminal (convenience alias)
            if ctrl && keyval == gdk::Key::Return {
                if palette_ref.is_visible() {
                    palette_ref.hide();
                }
                if let Some(child) = stack_ref.visible_child() {
                    child.grab_focus();
                }
                return gtk4::glib::Propagation::Stop;
            }

            // Hardcoded: Ctrl+1..9 — switch to Nth process globally
            if ctrl {
                let idx = match keyval {
                    gdk::Key::_1 => Some(0usize),
                    gdk::Key::_2 => Some(1),
                    gdk::Key::_3 => Some(2),
                    gdk::Key::_4 => Some(3),
                    gdk::Key::_5 => Some(4),
                    gdk::Key::_6 => Some(5),
                    gdk::Key::_7 => Some(6),
                    gdk::Key::_8 => Some(7),
                    gdk::Key::_9 => Some(8),
                    _ => None,
                };
                if let Some(i) = idx {
                    Self::switch_to_nth_global(
                        &ws_ref,
                        &stack_ref,
                        &sidebar_ref,
                        &sb_ref,
                        &last_proj_ref,
                        i,
                    );
                    return gtk4::glib::Propagation::Stop;
                }
            }

            // Hardcoded: Alt+1..9 — switch to project N
            if alt {
                let project_idx = match keyval {
                    gdk::Key::_1 => Some(0usize),
                    gdk::Key::_2 => Some(1),
                    gdk::Key::_3 => Some(2),
                    gdk::Key::_4 => Some(3),
                    gdk::Key::_5 => Some(4),
                    gdk::Key::_6 => Some(5),
                    gdk::Key::_7 => Some(6),
                    gdk::Key::_8 => Some(7),
                    gdk::Key::_9 => Some(8),
                    _ => None,
                };
                if let Some(idx) = project_idx {
                    Self::switch_to_project(
                        &ws_ref,
                        &stack_ref,
                        &sidebar_ref,
                        &sb_ref,
                        &last_proj_ref,
                        idx,
                    );
                    return gtk4::glib::Propagation::Stop;
                }
            }

            // Hardcoded: Escape — close palette
            if keyval == gdk::Key::Escape && palette_ref.is_visible() {
                palette_ref.hide();
                if let Some(child) = stack_ref.visible_child() {
                    child.grab_focus();
                }
                return gtk4::glib::Propagation::Stop;
            }

            gtk4::glib::Propagation::Proceed
        });

        window.add_controller(key_controller);
    }

    fn build_headerbar(
        window: &adw::ApplicationWindow,
        split_view: &adw::OverlaySplitView,
        palette: &Rc<CommandPalette>,
        sidebar: &Rc<ProjectList>,
        on_single_expand_changed: &Rc<dyn Fn(bool)>,
        on_auto_hide_changed: &Rc<dyn Fn(bool)>,
        on_keybind_hints_changed: &Rc<dyn Fn(bool)>,
        on_recent_first_changed: &Rc<dyn Fn(bool)>,
        on_terminal_theme_changed: &Rc<dyn Fn(&str)>,
        on_font_changed: &Rc<dyn Fn()>,
        on_composer_changed: &Rc<dyn Fn(bool)>,
        keybinding_map: &Rc<RefCell<KeybindingMap>>,
    ) -> (adw::HeaderBar, gtk4::Label) {
        let headerbar = adw::HeaderBar::new();

        let sidebar_tooltip = format!(
            "Toggle Sidebar ({})",
            keybinding_map
                .borrow()
                .display_string(ShortcutAction::ToggleSidebar)
        );
        let sidebar_btn = gtk4::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .active(true)
            .tooltip_text(&sidebar_tooltip)
            .build();

        let sv = split_view.clone();
        sidebar_btn.connect_toggled(move |btn| {
            sv.set_show_sidebar(btn.is_active());
        });

        // Keep headerbar toggle in sync when sidebar is hidden by other means (auto-hide, shortcuts)
        let btn_sync = sidebar_btn.clone();
        split_view.connect_show_sidebar_notify(move |sv| {
            let showing = sv.shows_sidebar();
            if btn_sync.is_active() != showing {
                btn_sync.set_active(showing);
            }
        });

        // Settings button
        let settings_btn = gtk4::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("Settings (Ctrl+,)")
            .build();
        let window_ref = window.clone();
        let single_expand_cb = on_single_expand_changed.clone();
        let auto_hide_cb = on_auto_hide_changed.clone();
        let keybind_hints_cb = on_keybind_hints_changed.clone();
        let recent_first_cb = on_recent_first_changed.clone();
        let theme_cb = on_terminal_theme_changed.clone();
        let font_cb = on_font_changed.clone();
        let composer_cb = on_composer_changed.clone();
        let kb_map = keybinding_map.clone();
        settings_btn.connect_clicked(move |_| {
            crate::ui::settings::settings_window::SettingsWindow::show(
                &window_ref,
                Some(single_expand_cb.clone()),
                Some(auto_hide_cb.clone()),
                Some(keybind_hints_cb.clone()),
                Some(recent_first_cb.clone()),
                Some(theme_cb.clone()),
                Some(font_cb.clone()),
                Some(composer_cb.clone()),
                Some(kb_map.clone()),
            );
        });

        // Add button
        let add_btn = gtk4::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add Project or Process (Ctrl+P)")
            .build();
        let palette_ref = palette.clone();
        add_btn.connect_clicked(move |_| {
            palette_ref.show_with_text("New ");
        });

        headerbar.pack_start(&sidebar_btn);
        headerbar.pack_start(sidebar.search_button());
        headerbar.pack_start(&settings_btn);
        headerbar.pack_start(&add_btn);

        // Title
        let title_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        title_box.set_halign(gtk4::Align::Center);

        let title_label = gtk4::Label::builder()
            .label("TuxFlow")
            .css_classes(["title"])
            .build();
        title_box.append(&title_label);

        headerbar.set_title_widget(Some(&title_box));

        (headerbar, title_label)
    }

    fn all_qualified_names(ws: &WorkspaceRef) -> Vec<String> {
        use crate::config::schema::ProcessCategory;
        let category_order = [
            ProcessCategory::Agent,
            ProcessCategory::Command,
            ProcessCategory::Terminal,
            ProcessCategory::SSH,
        ];
        let ws_borrow = ws.borrow();
        let mut names = Vec::new();
        for project in ws_borrow.projects() {
            let mgr = project.manager.borrow();
            for cat in &category_order {
                for proc in mgr.processes_by_category(cat.clone()) {
                    names.push(workspace::qualified_name(&project.name, &proc.config.name));
                }
            }
        }
        names
    }

    fn toggle_current_process(ws: &WorkspaceRef, stack: &gtk4::Stack) {
        let qname = match stack.visible_child_name() {
            Some(name) if name != "__welcome__" => name.to_string(),
            _ => return,
        };
        let (proj_name, proc_name) = match qname.split_once("::") {
            Some(parts) => parts,
            None => return,
        };
        let ws_borrow = ws.borrow();
        if let Some(project) = ws_borrow.projects().iter().find(|p| p.name == proj_name) {
            let mut mgr = project.manager.borrow_mut();
            if let Some(proc) = mgr.get_process(proc_name) {
                if proc.status == ProcessStatus::Running {
                    mgr.kill(proc_name);
                } else {
                    mgr.spawn(proc_name);
                }
            }
        }
    }

    fn restart_current_process(ws: &WorkspaceRef, stack: &gtk4::Stack) {
        let qname = match stack.visible_child_name() {
            Some(name) if name != "__welcome__" => name.to_string(),
            _ => return,
        };
        let (proj_name, proc_name) = match qname.split_once("::") {
            Some(parts) => parts,
            None => return,
        };
        let ws_borrow = ws.borrow();
        if let Some(project) = ws_borrow.projects().iter().find(|p| p.name == proj_name) {
            project.manager.borrow_mut().restart(proc_name);
        }
    }

    fn close_current_process(ws: &WorkspaceRef, stack: &gtk4::Stack, sidebar: &Rc<ProjectList>) {
        let qname = match stack.visible_child_name() {
            Some(name) if name != "__welcome__" => name.to_string(),
            _ => return,
        };

        let (proj_name, proc_name) = match qname.split_once("::") {
            Some(parts) => parts,
            None => return,
        };

        let ws_borrow = ws.borrow();
        let project = match ws_borrow.projects().iter().find(|p| p.name == proj_name) {
            Some(p) => p,
            None => return,
        };

        let category = {
            let mgr = project.manager.borrow();
            match mgr.get_process(proc_name) {
                Some(proc) => proc.config.category.clone(),
                None => return,
            }
        };

        match category {
            crate::config::schema::ProcessCategory::Terminal
            | crate::config::schema::ProcessCategory::SSH => {
                // Stop, remove from manager, persist deletion, remove from sidebar
                project.manager.borrow_mut().remove_process(proc_name);
                drop(ws_borrow);
                ws.borrow_mut().mark_process_deleted(proj_name, proc_name);
                sidebar.remove_process(&qname);
            }
            _ => {
                // Agent or Command: just stop
                project.manager.borrow_mut().kill(proc_name);
            }
        }
    }

    fn create_terminal_in_current_project(
        ws: &WorkspaceRef,
        stack: &gtk4::Stack,
        sidebar: &Rc<ProjectList>,
        last_project: &Rc<RefCell<Option<String>>>,
    ) {
        let ws_borrow = ws.borrow();
        let project_name = match Self::resolve_active_project(stack, last_project, sidebar) {
            Some(name) => name,
            None => return,
        };

        let project = match ws_borrow.projects().iter().find(|p| p.name == project_name) {
            Some(p) => p,
            None => return,
        };

        let term_name = format!(
            "terminal-{}",
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("0")
        );
        let config = crate::config::schema::ProcessConfig {
            name: term_name.clone(),
            // Remote terminals must resolve the shell on the host — the
            // local $SHELL path may not exist there.
            command: if project.location.is_remote() {
                "exec \"$SHELL\"".to_string()
            } else {
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
            },
            working_dir: Some(project.location.dir_str()),
            start_with_project: true,
            auto_restart: false,
            open_in_browser: false,
            restart_when_changed: Vec::new(),
            env: std::collections::BTreeMap::new(),
            category: crate::config::schema::ProcessCategory::Terminal,
            auto_named: true,
            display_name: None,
        };

        drop(ws_borrow);
        ws.borrow_mut()
            .save_custom_command(&project_name, config.clone());

        let ws_borrow = ws.borrow();
        if let Some(project) = ws_borrow.projects().iter().find(|p| p.name == project_name) {
            project.manager.borrow_mut().add_process(config);
            let factory = project.manager.borrow().wiring_factory();
            if let Some(factory) = factory {
                factory(&term_name);
            }
            project.manager.borrow_mut().materialize_process(&term_name);
            sidebar.add_process_to_project(
                &project.manager,
                &project_name,
                &term_name,
                ProcessStatus::Stopped,
                crate::config::schema::ProcessCategory::Terminal,
            );
            project.manager.borrow_mut().spawn(&term_name);
        }
    }

    fn refresh_status_bar_for_project(
        ws: &WorkspaceRef,
        status_bar: &Rc<StatusBar>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        project_name: &str,
    ) {
        *last_selected_project.borrow_mut() = Some(project_name.to_string());
        let ws_inner = ws.clone();
        let sb = status_bar.clone();
        let proj_owned = project_name.to_string();
        glib::idle_add_local_once(move || {
            let ws_borrow = ws_inner.borrow();
            let mut global_r = 0usize;
            let mut global_t = 0usize;
            let mut proj_r = 0usize;
            let mut proj_t = 0usize;
            let mut running_names = Vec::new();
            for project in ws_borrow.projects() {
                let mgr = project.manager.borrow();
                let r = mgr.running_count();
                let t = mgr.total_count();
                global_r += r;
                global_t += t;
                if project.name == proj_owned {
                    proj_r = r;
                    proj_t = t;
                }
                let names: Vec<String> =
                    mgr.running_names().into_iter().map(String::from).collect();
                if !names.is_empty() {
                    running_names.push((project.name.clone(), names));
                }
            }
            sb.set_project_info(Some(&proj_owned), proj_r, proj_t);
            sb.set_global_info(global_r, global_t, true, &running_names);
        });
    }

    fn switch_relative(
        ws: &WorkspaceRef,
        stack: &gtk4::Stack,
        selected: &Rc<RefCell<Option<String>>>,
        sidebar: &Rc<ProjectList>,
        status_bar: &Rc<StatusBar>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        delta: i32,
    ) {
        use crate::config::schema::ProcessCategory;
        let category_order = [
            ProcessCategory::Agent,
            ProcessCategory::Command,
            ProcessCategory::Terminal,
            ProcessCategory::SSH,
        ];
        let names: Vec<String> = {
            let ws_borrow = ws.borrow();
            let mut names = Vec::new();
            for project in ws_borrow.projects() {
                let mgr = project.manager.borrow();
                for cat in &category_order {
                    for proc in mgr.processes_by_category(cat.clone()) {
                        if proc.status == ProcessStatus::Running {
                            names.push(workspace::qualified_name(&project.name, &proc.config.name));
                        }
                    }
                }
            }
            names
        };
        if names.is_empty() {
            return;
        }

        let current = selected.borrow();
        let current_idx = current
            .as_ref()
            .and_then(|c| names.iter().position(|n| n == c))
            .unwrap_or(0);
        drop(current);

        let new_idx = (current_idx as i32 + delta).rem_euclid(names.len() as i32) as usize;
        stack.set_visible_child_name(&names[new_idx]);
        *selected.borrow_mut() = Some(names[new_idx].clone());
        sidebar.select_process(&names[new_idx]);
        if let Some((proj, _)) = names[new_idx].split_once("::") {
            sidebar.set_active_project(proj);
            Self::refresh_status_bar_for_project(ws, status_bar, last_selected_project, proj);
        }
        if let Some(child) = stack.visible_child() {
            child.grab_focus();
        }
    }

    fn switch_project_relative(
        ws: &WorkspaceRef,
        stack: &gtk4::Stack,
        sidebar: &Rc<ProjectList>,
        status_bar: &Rc<StatusBar>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        delta: i32,
    ) {
        // Extract target project info, then drop the workspace borrow
        // before calling sidebar.expand_project (which needs ws.borrow_mut)
        let (target_name, target_qname) = {
            let ws_borrow = ws.borrow();
            let projects = ws_borrow.projects();
            if projects.is_empty() {
                return;
            }

            let current_project = stack
                .visible_child_name()
                .and_then(|name| name.split_once("::").map(|(proj, _)| proj.to_string()));

            let current_idx = current_project
                .and_then(|name| projects.iter().position(|p| p.name == name))
                .unwrap_or(0);

            let count = projects.len() as i32;
            let new_idx = ((current_idx as i32 + delta).rem_euclid(count)) as usize;

            match projects.get(new_idx) {
                Some(project) => {
                    let mgr = project.manager.borrow();
                    let qname = mgr
                        .process_names()
                        .first()
                        .map(|first_name| workspace::qualified_name(&project.name, first_name));
                    (project.name.clone(), qname)
                }
                None => return,
            }
        };

        if let Some(qname) = target_qname {
            stack.set_visible_child_name(&qname);
        }
        sidebar.expand_project(&target_name);
        sidebar.set_active_project(&target_name);
        sidebar.scroll_to_project(&target_name);
        Self::refresh_status_bar_for_project(ws, status_bar, last_selected_project, &target_name);
        if let Some(child) = stack.visible_child() {
            child.grab_focus();
        }
    }

    fn switch_to_project(
        ws: &WorkspaceRef,
        stack: &gtk4::Stack,
        sidebar: &Rc<ProjectList>,
        status_bar: &Rc<StatusBar>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        project_idx: usize,
    ) {
        let project_name = {
            let ws_borrow = ws.borrow();
            if let Some(project) = ws_borrow.projects().get(project_idx) {
                let mgr = project.manager.borrow();
                if let Some(first_name) = mgr.process_names().first() {
                    let qname = workspace::qualified_name(&project.name, first_name);
                    stack.set_visible_child_name(&qname);
                }
                Some(project.name.clone())
            } else {
                None
            }
        };
        if let Some(name) = project_name {
            sidebar.expand_project(&name);
            sidebar.set_active_project(&name);
            sidebar.scroll_to_project(&name);
            Self::refresh_status_bar_for_project(ws, status_bar, last_selected_project, &name);
        }
        if let Some(child) = stack.visible_child() {
            child.grab_focus();
        }
    }

    fn adjust_font_size(stack: &gtk4::Stack, delta: i32) {
        if let Some(child) = stack.visible_child()
            && let Ok(terminal) = child.downcast::<vte4::Terminal>()
            && let Some(font) = terminal.font()
        {
            let current_size = font.size() / gtk4::pango::SCALE;
            let new_size = (current_size + delta).max(6).min(48);
            let new_desc = gtk4::pango::FontDescription::from_string(&format!(
                "{} {new_size}",
                font.family().unwrap_or("Monospace".into())
            ));
            terminal.set_font(Some(&new_desc));
        }
    }

    fn switch_to_nth_global(
        ws: &WorkspaceRef,
        stack: &gtk4::Stack,
        sidebar: &Rc<ProjectList>,
        status_bar: &Rc<StatusBar>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        n: usize,
    ) {
        let found = {
            let ws_borrow = ws.borrow();
            let mut idx = 0;
            let mut found = None;
            for project in ws_borrow.projects() {
                let mgr = project.manager.borrow();
                for name in mgr.running_names_in_sidebar_order() {
                    if idx == n {
                        let qname = workspace::qualified_name(&project.name, name);
                        stack.set_visible_child_name(&qname);
                        found = Some((project.name.clone(), qname));
                        break;
                    }
                    idx += 1;
                }
                if found.is_some() {
                    break;
                }
            }
            found
        };
        if let Some((name, qname)) = found {
            sidebar.select_process(&qname);
            sidebar.set_active_project(&name);
            Self::refresh_status_bar_for_project(ws, status_bar, last_selected_project, &name);
        }
        if let Some(child) = stack.visible_child() {
            child.grab_focus();
        }
    }

    fn build_welcome_page() -> adw::StatusPage {
        adw::StatusPage::builder()
            .icon_name("tuxflow-logo-symbolic")
            .title("TuxFlow")
            .description("Select a process from the sidebar to view its output\nCtrl+Shift+P to open the command palette")
            .css_classes(["welcome-page"])
            .vexpand(true)
            .hexpand(true)
            .build()
    }

    fn start_mcp_for_project(
        manager: &ProcessManagerRef,
        project_name: &str,
        project_dir: &str,
        ws: &WorkspaceRef,
    ) {
        use crate::mcp::bridge::{self, MCP_PROCESS_STATE, McpCommand, ProcessSnapshot};

        // Populate initial process state
        {
            let mgr = manager.borrow();
            let mut state = MCP_PROCESS_STATE.lock().unwrap();
            for name in mgr.process_names() {
                if let Some(proc) = mgr.get_process(name) {
                    state.insert(
                        name.clone(),
                        ProcessSnapshot {
                            name: proc.config.name.clone(),
                            status: format!("{:?}", proc.status),
                            command: proc.config.command.clone(),
                            category: format!("{:?}", proc.config.category),
                            pid: proc.pid_cell.as_ref().and_then(|c| *c.borrow()),
                            restart_count: proc.restart_count,
                            uptime_secs: proc.started_at.map(|t| t.elapsed().as_secs()),
                        },
                    );
                }
            }
        }

        // Create bridge and start MCP server
        let (mcp_bridge, mut command_rx) = bridge::create_mcp_bridge();
        crate::mcp::server::start_mcp_server(project_name, project_dir, mcp_bridge);

        // Poll MCP commands on the GTK main loop
        let ws_for_mcp = ws.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            while let Ok(cmd) = command_rx.try_recv() {
                let ws_borrow = ws_for_mcp.borrow();
                match cmd {
                    McpCommand::RestartProcess { name, reply } => {
                        let result = Self::execute_mcp_command(&ws_borrow, &name, "restart");
                        let _ = reply.send(result);
                    }
                    McpCommand::StopProcess { name, reply } => {
                        let result = Self::execute_mcp_command(&ws_borrow, &name, "stop");
                        let _ = reply.send(result);
                    }
                    McpCommand::StartProcess { name, reply } => {
                        let result = Self::execute_mcp_command(&ws_borrow, &name, "start");
                        let _ = reply.send(result);
                    }
                    McpCommand::ReadLogs { name, lines, reply } => {
                        let result = Self::read_terminal_logs(&ws_borrow, &name, lines);
                        let _ = reply.send(result);
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    fn execute_mcp_command(
        ws: &crate::workspace::Workspace,
        process_name: &str,
        action: &str,
    ) -> crate::mcp::bridge::CommandResult {
        use crate::mcp::bridge::CommandResult;

        for project in ws.projects() {
            let mgr = project.manager.borrow();
            if mgr.get_process(process_name).is_some() {
                drop(mgr);
                let mut mgr = project.manager.borrow_mut();
                match action {
                    "restart" => mgr.restart(process_name),
                    "stop" => mgr.kill(process_name),
                    "start" => mgr.spawn(process_name),
                    _ => {}
                }
                let past = match action {
                    "stop" => "stopped",
                    "start" => "started",
                    "restart" => "restarted",
                    _ => "updated",
                };
                return CommandResult::Ok(format!(
                    "Process '{}' {} successfully",
                    process_name, past
                ));
            }
        }
        CommandResult::Error(format!("Process '{}' not found", process_name))
    }

    fn read_terminal_logs(
        ws: &crate::workspace::Workspace,
        process_name: &str,
        max_lines: usize,
    ) -> crate::mcp::bridge::CommandResult {
        use crate::mcp::bridge::CommandResult;
        use vte4::prelude::*;

        for project in ws.projects() {
            let mgr = project.manager.borrow();
            if let Some(proc) = mgr.get_process(process_name)
                && let Some(ref terminal) = proc.terminal
            {
                let row = terminal.cursor_position().1;
                let cols = terminal.column_count();
                let start_row = (row - max_lines as i64).max(0);
                let (text_opt, _) =
                    terminal.text_range_format(vte4::Format::Text, start_row, 0, row, cols);
                let text = text_opt.map(|t| t.to_string()).unwrap_or_default();
                // Filter out blank lines and take last N
                let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                let start = lines.len().saturating_sub(max_lines);
                let result = lines[start..].join("\n");
                return CommandResult::Ok(result);
            }
        }
        CommandResult::Error(format!("Process '{}' not found", process_name))
    }
}

/// Whether the qualified process (`proj::proc`) is currently running.
/// Uses the same `Running | Restarting` predicate as the sidebar rows so the
/// status bar and the row agree on what counts as "running".
fn is_qualified_process_running(ws: &Workspace, qname: Option<&str>) -> bool {
    let Some(qname) = qname else {
        return false;
    };
    let Some((proj, proc_name)) = qname.split_once("::") else {
        return false;
    };
    let Some(project) = ws.projects().iter().find(|p| p.name == proj) else {
        return false;
    };
    let Ok(mgr) = project.manager.try_borrow() else {
        return false;
    };
    matches!(
        mgr.get_process(proc_name).map(|p| p.status),
        Some(ProcessStatus::Running | ProcessStatus::Restarting)
    )
}

/// Set X11 WM position hints before the window is mapped, so the WM respects our position.
/// Must be called from connect_realize (before the window is mapped). No-op on Wayland.
fn set_x11_position_hint(win: &adw::ApplicationWindow, saved_x: Option<i32>, saved_y: Option<i32>) {
    let (Some(x), Some(y)) = (saved_x, saved_y) else {
        return;
    };
    let Some(surface) = win.surface() else {
        return;
    };
    let Ok(x11_surface) = surface.downcast::<gdk4_x11::X11Surface>() else {
        return;
    };
    let x11_display = x11_surface
        .display()
        .downcast::<gdk4_x11::X11Display>()
        .expect("X11Surface must have X11Display");
    unsafe {
        let xdisplay = x11_display.xdisplay() as *mut x11::xlib::Display;
        let xwindow = x11_surface.xid();
        let hints = x11::xlib::XAllocSizeHints();
        if !hints.is_null() {
            (*hints).flags = x11::xlib::PPosition | x11::xlib::USPosition;
            (*hints).x = x;
            (*hints).y = y;
            x11::xlib::XSetWMNormalHints(xdisplay, xwindow, hints);
            x11::xlib::XFree(hints as *mut _);
        }
    }
}

/// Save window position using X11 APIs. No-op on Wayland.
fn save_window_position(win: &adw::ApplicationWindow, s: &mut AppSettings) {
    let Some(surface) = win.surface() else {
        return;
    };
    let Ok(x11_surface) = surface.downcast::<gdk4_x11::X11Surface>() else {
        return;
    };
    let x11_display = x11_surface
        .display()
        .downcast::<gdk4_x11::X11Display>()
        .expect("X11Surface must have X11Display");
    unsafe {
        let xdisplay = x11_display.xdisplay();
        let xwindow = x11_surface.xid();
        let root = x11::xlib::XDefaultRootWindow(xdisplay as *mut _);
        let mut x: i32 = 0;
        let mut y: i32 = 0;
        let mut child: x11::xlib::Window = 0;
        x11::xlib::XTranslateCoordinates(
            xdisplay as *mut _,
            xwindow,
            root,
            0,
            0,
            &mut x,
            &mut y,
            &mut child,
        );
        s.window.x = Some(x);
        s.window.y = Some(y);
    }
}

/// Restore window placement: exact position on X11, monitor hint on Wayland.
/// If `do_maximize` is true, the window will be maximized after moving to the correct monitor.
fn restore_window_placement(
    win: &adw::ApplicationWindow,
    saved_x: Option<i32>,
    saved_y: Option<i32>,
    saved_monitor: Option<&str>,
    do_maximize: bool,
) {
    let Some(surface) = win.surface() else {
        return;
    };
    let display = surface.display();

    // Find the target monitor by connector name
    let target_monitor = saved_monitor.and_then(|c| find_monitor_by_connector(&display, c));

    // Check if already on the correct monitor
    let already_correct = match (&target_monitor, saved_monitor) {
        (Some(_), Some(connector)) => {
            display
                .monitor_at_surface(&surface)
                .and_then(|m| m.connector().map(|c| c.to_string()))
                .as_deref()
                == Some(connector)
        }
        _ => true,
    };

    // X11: try exact positioning for non-maximized windows
    if !do_maximize {
        if let Ok(x11_surface) = surface.clone().downcast::<gdk4_x11::X11Surface>() {
            if let (Some(x), Some(y)) = (saved_x, saved_y) {
                let monitors = display.monitors();
                let on_screen = (0..monitors.n_items()).any(|i| {
                    monitors
                        .item(i)
                        .and_then(|m| m.downcast::<gdk::Monitor>().ok())
                        .is_some_and(|monitor| {
                            let geo = monitor.geometry();
                            x >= geo.x()
                                && x < geo.x() + geo.width()
                                && y >= geo.y()
                                && y < geo.y() + geo.height()
                        })
                });
                if on_screen {
                    log::debug!("X11: Moving window to ({x}, {y})");
                    let x11_display = display
                        .downcast::<gdk4_x11::X11Display>()
                        .expect("X11Surface must have X11Display");
                    unsafe {
                        let xdisplay = x11_display.xdisplay() as *mut x11::xlib::Display;
                        let xwindow = x11_surface.xid();
                        x11::xlib::XMoveWindow(xdisplay, xwindow, x, y);
                        x11::xlib::XFlush(xdisplay);
                    }
                    return;
                }
                log::info!("Saved window position ({x}, {y}) is off-screen, ignoring");
            }
            return;
        }
    }

    // Move to the correct monitor (works on both X11 and Wayland)
    if !already_correct {
        if let Some(ref monitor) = target_monitor {
            let connector = saved_monitor.unwrap_or("unknown");
            log::debug!("Moving window to monitor '{connector}' via fullscreen toggle");
            let gtk_win: &gtk4::Window = win.upcast_ref();
            gtk_win.fullscreen_on_monitor(monitor);
            let win = win.clone();
            let do_maximize = do_maximize;
            glib::idle_add_local_once(move || {
                win.unfullscreen();
                if do_maximize {
                    win.maximize();
                }
            });
            return;
        }
        if let Some(connector) = saved_monitor {
            log::info!("Saved monitor '{connector}' not found, letting WM decide placement");
        }
    }

    // If already on the correct monitor, just maximize if needed
    if do_maximize {
        win.maximize();
    }
}

/// Find a monitor by its connector name (e.g. "HDMI-1", "DP-2", "eDP-1").
fn find_monitor_by_connector(display: &gdk::Display, connector: &str) -> Option<gdk::Monitor> {
    let monitors = display.monitors();
    (0..monitors.n_items()).find_map(|i| {
        monitors
            .item(i)
            .and_then(|m| m.downcast::<gdk::Monitor>().ok())
            .filter(|m| m.connector().as_deref() == Some(connector))
    })
}

/// Block until the HTTP server behind `url` answers with a non-5xx status,
/// polling every 400 ms up to `timeout` (then give up and let the caller
/// open the URL anyway — something visible beats nothing). Runs on a worker
/// thread; keeps the auto-opened tab from capturing a mid-startup 500
/// (artisan serve answers seconds before vite writes the manifest).
fn wait_http_ready(url: &str, timeout: std::time::Duration) {
    use std::io::{Read, Write};
    let Some(rest) = url.strip_prefix("http://") else {
        return;
    };
    let (host_port, path) = match rest.split_once('/') {
        Some((hp, p)) => (hp.to_string(), format!("/{p}")),
        None => (rest.to_string(), "/".to_string()),
    };
    let addr = if host_port.contains(':') {
        host_port.clone()
    } else {
        format!("{host_port}:80")
    };
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok(mut stream) = std::net::TcpStream::connect(&addr) {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
            let req =
                format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
            if stream.write_all(req.as_bytes()).is_ok() {
                let mut buf = [0u8; 32];
                if let Ok(n) = stream.read(&mut buf)
                    && let Some(code) = String::from_utf8_lossy(&buf[..n])
                        .split_whitespace()
                        .nth(1)
                        .and_then(|c| c.parse::<u16>().ok())
                    && code < 500
                {
                    return;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    log::info!("auto-open: {url} still not healthy after {timeout:?}, opening anyway");
}

#[cfg(test)]
mod tests {
    use super::SelectionGesture;

    /// GTK defaults, so the numbers below read like real interactions.
    fn gesture() -> SelectionGesture {
        SelectionGesture::new(400, 5.0)
    }

    /// The regression that started all this: a click that merely focuses a
    /// pane changes nothing in tmux, so publishing on it re-asserts an old
    /// selection over whatever the user has selected since.
    #[test]
    fn a_plain_click_publishes_nothing() {
        let mut g = gesture();
        g.press(100.0, 100.0, 1_000);
        assert!(!g.release(100.0, 100.0));
        // Even with the hand shaking a little.
        g.press(100.0, 100.0, 5_000);
        assert!(!g.release(102.0, 97.0));
    }

    #[test]
    fn a_drag_publishes() {
        let mut g = gesture();
        g.press(100.0, 100.0, 1_000);
        assert!(g.release(240.0, 100.0), "horizontal drag");
        g.press(100.0, 100.0, 5_000);
        assert!(g.release(100.0, 134.0), "drag down a few rows");
    }

    /// tmux copies a word on double-click and a line on triple-click, both
    /// without moving the pointer — the gate has to let those through or
    /// they'd never reach the local selection.
    #[test]
    fn a_click_sequence_publishes_from_the_second_click() {
        let mut g = gesture();
        g.press(100.0, 100.0, 1_000);
        assert!(!g.release(100.0, 100.0), "first click of the pair");
        g.press(101.0, 100.0, 1_200);
        assert!(g.release(101.0, 100.0), "double click");
        g.press(101.0, 100.0, 1_400);
        assert!(g.release(101.0, 100.0), "triple click");
    }

    /// Two separate clicks in the same spot are not a double click, however
    /// patient the user is — nor are two quick clicks in different spots.
    #[test]
    fn clicks_far_apart_in_time_or_space_stay_clicks() {
        let mut g = gesture();
        g.press(100.0, 100.0, 1_000);
        assert!(!g.release(100.0, 100.0));
        g.press(100.0, 100.0, 1_401);
        assert!(!g.release(100.0, 100.0), "past the double-click interval");

        let mut g = gesture();
        g.press(100.0, 100.0, 1_000);
        assert!(!g.release(100.0, 100.0));
        g.press(140.0, 100.0, 1_100);
        assert!(!g.release(140.0, 100.0), "past the double-click distance");
    }

    /// A release the press of which we never saw — pointer pressed in another
    /// window and dragged in, or a grab broken mid-gesture — decides nothing.
    /// Nor does a second release for one press.
    #[test]
    fn a_release_needs_its_own_press() {
        let mut g = gesture();
        assert!(!g.release(400.0, 400.0), "no press at all");
        g.press(100.0, 100.0, 1_000);
        assert!(g.release(300.0, 100.0));
        assert!(!g.release(300.0, 100.0), "press already consumed");
    }

    /// X server timestamps are milliseconds in a u32 and do wrap (~49 days of
    /// uptime). A wrap must not turn every click into a double click.
    #[test]
    fn timestamps_may_wrap() {
        let mut g = gesture();
        g.press(100.0, 100.0, u32::MAX - 100);
        assert!(!g.release(100.0, 100.0));
        // 200ms later, on the other side of the wrap: still a double click.
        g.press(100.0, 100.0, 99);
        assert!(g.release(100.0, 100.0));
    }
}
