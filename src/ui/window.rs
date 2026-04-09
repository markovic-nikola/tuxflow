use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

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
use crate::ui::git_changes_dialog::{GitChangesDialog, commits_behind, git_fetch};
use crate::ui::sidebar::project_list::ProjectList;
use crate::ui::status_bar::StatusBar;
use crate::ui::terminal_search::TerminalSearch;
use crate::util::port_detector::PortDetector;
use crate::util::resource_monitor;
use crate::workspace::{self, Workspace, WorkspaceRef};

pub struct TuxFlowWindow;

impl TuxFlowWindow {
    pub fn new(app: &adw::Application, project_dir: Option<&Path>) -> adw::ApplicationWindow {
        // Load persisted settings
        let settings = Rc::new(RefCell::new(AppSettings::load()));

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

        let sidebar = Rc::new(ProjectList::new(single_expand.clone()));
        sidebar.set_workspace(&ws);
        sidebar.set_window(&window);

        let welcome = Self::build_welcome_page();
        terminal_stack.add_named(&welcome, Some("__welcome__"));

        // Track selected process and last-used project for quick re-selection
        let selected_process: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let last_selected_project: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let status_bar = Rc::new(StatusBar::new());

        // Check for updates in background
        {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                if let Some(update) = crate::util::update_checker::check_for_update() {
                    let _ = tx.send(update);
                }
            });
            let status_bar_ref = status_bar.clone();
            glib::idle_add_local(move || {
                if let Ok(update) = rx.try_recv() {
                    status_bar_ref.show_update(&update.latest_version, &update.release_url);
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
        }

        // Load saved projects
        {
            let saved_dirs = ws.borrow().saved_directories();
            for dir_str in &saved_dirs {
                let path = std::path::PathBuf::from(dir_str);
                if path.is_dir() {
                    Self::load_project(
                        &ws,
                        &sidebar,
                        &terminal_stack,
                        &path,
                        &pid_file,
                        &status_bar,
                        &selected_process,
                    );
                }
            }
        }

        // Load CLI project if given (and not already loaded from saved)
        if let Some(dir) = project_dir {
            Self::load_project(
                &ws,
                &sidebar,
                &terminal_stack,
                dir,
                &pid_file,
                &status_bar,
                &selected_process,
            );
        }

        // Wire sidebar selection → terminal switch + status bar URL update
        let stack_ref = terminal_stack.clone();
        let selected_ref = selected_process.clone();
        let sidebar_ref = sidebar.clone();
        let status_bar_ref = status_bar.clone();
        let last_proj_ref = last_selected_project.clone();
        let ws_select = ws.clone();
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
            *selected_ref.borrow_mut() = Some(qname.to_string());
            if let Some((proj, _)) = qname.split_once("::") {
                *last_proj_ref.borrow_mut() = Some(proj.to_string());
                sidebar_ref.set_active_project(proj);

                // Defer status bar update to idle to avoid RefCell conflict
                // when this callback fires during a manager borrow_mut (e.g. spawn)
                let ws_idle = ws_select.clone();
                let sb_idle = status_bar_ref.clone();
                let proj_owned = proj.to_string();
                glib::idle_add_local_once(move || {
                    let ws_borrow = ws_idle.borrow();
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
                });
            }
            let url = sidebar_ref.get_process_url(qname);
            status_bar_ref.set_url(url.as_deref());
        });

        // Wire process deletion → remove terminal from stack and handle selected fallback
        let stack_ref = terminal_stack.clone();
        let selected_ref = selected_process.clone();
        sidebar.set_on_process_deleted(move |qname| {
            if let Some(child) = stack_ref.child_by_name(qname) {
                stack_ref.remove(&child);
            }
            let mut sel = selected_ref.borrow_mut();
            if sel.as_deref() == Some(qname) {
                stack_ref.set_visible_child_name("__welcome__");
                *sel = None;
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

        // Build theme-change callback that applies to all existing terminals
        let ws_for_theme = ws.clone();
        let on_terminal_theme_changed: Rc<dyn Fn(&str)> = Rc::new(move |theme_name: &str| {
            let ws_borrow = ws_for_theme.borrow();
            for project in ws_borrow.projects() {
                project
                    .manager
                    .borrow_mut()
                    .apply_terminal_theme(theme_name);
            }
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
        let ws_for_font = ws.clone();
        let on_font_changed: Rc<dyn Fn()> = Rc::new(move || {
            let settings = AppSettings::load();
            let ws_borrow = ws_for_font.borrow();
            for project in ws_borrow.projects() {
                project.manager.borrow_mut().apply_font_settings(&settings);
            }
        });

        // Build UI
        let content = Self::build_content(
            &window,
            &ws,
            &sidebar,
            &terminal_stack,
            &selected_process,
            &last_selected_project,
            &on_single_expand_changed,
            &on_auto_hide_changed,
            &on_terminal_theme_changed,
            &on_font_changed,
            &pid_file,
            &status_bar,
            &keybinding_map,
            &auto_hide,
        );
        window.set_content(Some(&content));

        // Start MCP servers for loaded projects (if enabled in settings)
        {
            let mcp_enabled = settings.borrow().integrations.mcp_enabled;
            crate::mcp::bridge::set_mcp_enabled(mcp_enabled);
            if mcp_enabled {
                let ws_borrow = ws.borrow();
                for project in ws_borrow.projects() {
                    let dir_str = project.dir.to_string_lossy().to_string();
                    Self::start_mcp_for_project(&project.manager, &project.name, &dir_str, &ws);
                }
            }
        }

        // Start resource monitoring
        {
            let ws_ref = ws.clone();
            let sidebar_ref = sidebar.clone();
            resource_monitor::start_monitoring(
                move || {
                    let ws_borrow = ws_ref.borrow();
                    let mut pids = Vec::new();
                    for project in ws_borrow.projects() {
                        let mgr = project.manager.borrow();
                        for (name, pid) in mgr.running_pids() {
                            let qname = workspace::qualified_name(&project.name, &name);
                            pids.push((qname, pid));
                        }
                    }
                    pids
                },
                move |qname, resources| {
                    sidebar_ref.set_process_resources(
                        qname,
                        resources.cpu_percent,
                        resources.memory_mb,
                    );
                },
            );
        }

        // Kill all child processes and save window state when the window closes
        let ws_shutdown = ws.clone();
        let pid_file_shutdown = pid_file.clone();
        let settings_shutdown = settings.clone();
        window.connect_close_request(move |win| {
            // Save window size, position, and maximized state
            {
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
                project.manager.borrow_mut().stop_all();
            }
            pid_file_shutdown.borrow_mut().clear();
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

    /// Connect auto-restart handler to a dynamically added process's terminal.
    fn setup_auto_restart_for_process(manager: &ProcessManagerRef, name: &str) {
        let auto_restart = manager
            .borrow()
            .get_process(name)
            .map(|p| p.config.auto_restart)
            .unwrap_or(false);
        let handler =
            crate::process::auto_restart::build_auto_restart_handler(manager, name, auto_restart);
        let mgr = manager.borrow();
        if let Some(proc) = mgr.get_process(name)
            && let Some(ref terminal) = proc.terminal
        {
            handler(terminal);
        }
    }

    /// Connect a VTE terminal's `window-title` property to auto-rename
    /// the sidebar row, but only while the process has `auto_named: true`.
    fn connect_window_title_auto_rename(
        terminal: &vte4::Terminal,
        manager: &ProcessManagerRef,
        process_name: &str,
        sidebar: &Rc<ProjectList>,
        qualified_name: &str,
        workspace: &WorkspaceRef,
        project_name: &str,
    ) {
        let mgr_ref = manager.clone();
        let proc_name = process_name.to_string();
        let sidebar_ref = sidebar.clone();
        let qname = qualified_name.to_string();
        let ws_ref = workspace.clone();
        let proj_name = project_name.to_string();
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
                sidebar_ref.set_process_name(&qname, &display_name);
                if let Some(proc) = mgr_ref.borrow_mut().get_process_mut(&proc_name) {
                    proc.config.display_name = Some(display_name.clone());
                }
                ws_ref
                    .borrow_mut()
                    .set_display_name(&proj_name, &proc_name, &display_name);
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
        dir: &Path,
        pid_file: &Rc<RefCell<PidFile>>,
        status_bar: &Rc<StatusBar>,
        selected_process: &Rc<RefCell<Option<String>>>,
    ) {
        let mut ws_mut = ws.borrow_mut();
        if let Some(project) = ws_mut.add_project_from_dir(dir) {
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
                ws,
                sidebar,
                terminal_stack,
                pid_file,
                status_bar,
                selected_process,
            );
        }
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
    ) {
        let prepared = {
            let mut ws_mut = ws.borrow_mut();
            ws_mut.prepare_project(dir)
        };
        let Some(prepared) = prepared else { return };

        let total_detected: usize = prepared
            .stacks
            .iter()
            .map(|s| s.suggested_processes.len())
            .sum();

        if prepared.config_loaded || total_detected <= 5 {
            // No dialog needed — add all detected processes directly
            let all_processes: Vec<crate::config::schema::ProcessConfig> = prepared
                .stacks
                .iter()
                .flat_map(|s| s.suggested_processes.clone())
                .collect();
            let mut ws_mut = ws.borrow_mut();
            if let Some(project) = ws_mut.finalize_project(prepared, all_processes) {
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
                    ws,
                    sidebar,
                    terminal_stack,
                    pid_file,
                    status_bar,
                    selected_process,
                );
                *last_selected_project.borrow_mut() = Some(project_name.clone());
                sidebar.expand_project(&project_name);
            }
        } else {
            // Show selection dialog
            let project_name = prepared.name.clone();
            let dir_string = prepared.dir_string.clone();
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

            crate::ui::select_commands_dialog::SelectCommandsDialog::show(
                parent,
                &project_name,
                &stacks_for_dialog,
                move |selected| {
                    // Mark deselected processes as deleted so they stay hidden on restart
                    let selected_names: std::collections::HashSet<&str> =
                        selected.iter().map(|p| p.name.as_str()).collect();
                    let mut ws_mut = ws.borrow_mut();
                    for name in &all_detected_names {
                        if !selected_names.contains(name.as_str()) {
                            ws_mut.mark_process_deleted_by_dir(&dir_string, name);
                        }
                    }
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
                        );
                        *last_selected_project.borrow_mut() = Some(project_name.clone());
                        sidebar.expand_project(&project_name);
                    }
                },
            );
        }
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
    ) {
        // Add placeholders to the stack (real terminals are created lazily)
        let detector = Rc::new(RefCell::new(PortDetector::new()));
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
            let pname = project_name.to_string();
            let mcp_state = crate::mcp::bridge::MCP_PROCESS_STATE.clone();
            let mut mgr = manager.borrow_mut();
            mgr.set_on_status_change(move |process_name, status| {
                let qname = workspace::qualified_name(&pname, process_name);
                sidebar_ref.update_process_status(&qname, status);

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
        }

        // Build per-process on_materialized callbacks (deferred signal connections)
        {
            let mut mgr = manager.borrow_mut();
            let names: Vec<String> = mgr.process_names().to_vec();
            for name in &names {
                let Some(proc) = mgr.get_process(name) else {
                    continue;
                };
                let skip_port_detection = matches!(
                    proc.config.category,
                    crate::config::schema::ProcessCategory::Agent
                        | crate::config::schema::ProcessCategory::SSH
                );
                let is_auto_named = proc.config.auto_named;
                let auto_restart_cfg = proc.config.auto_restart;

                // Build the auto-restart handler
                let auto_restart_handler = crate::process::auto_restart::build_auto_restart_handler(
                    manager,
                    name,
                    auto_restart_cfg,
                );

                // Capture refs for the on_materialized closure
                let detector_ref = detector.clone();
                let sidebar_ref = sidebar.clone();
                let sb_ref = status_bar.clone();
                let sel_ref = selected_process.clone();
                let proc_name = name.to_string();
                let qname = workspace::qualified_name(project_name, name);
                let stack_ref = terminal_stack.clone();
                let mgr_ref = manager.clone();
                let ws_ref = ws.clone();
                let sidebar_rename = sidebar.clone();
                let proj_name = project_name.to_string();
                let proc_name_rename = name.to_string();
                let qname_rename = qname.clone();

                let Some(proc) = mgr.get_process_mut(name) else {
                    continue;
                };
                proc.on_materialized = Some(Box::new(move |terminal: &vte4::Terminal| {
                    // Replace placeholder in stack with real terminal
                    if let Some(old_child) = stack_ref.child_by_name(&qname) {
                        stack_ref.remove(&old_child);
                    }
                    stack_ref.add_named(terminal, Some(&qname));

                    // Connect auto-restart handler
                    auto_restart_handler(terminal);

                    // Connect port detection + MCP log capture
                    let log_buffers = crate::mcp::bridge::MCP_LOG_BUFFERS.clone();
                    let log_proc_name = proc_name.clone();
                    let last_row: Rc<Cell<i64>> = Rc::new(Cell::new(0));
                    let detector_ref = detector_ref.clone();
                    let sidebar_ref = sidebar_ref.clone();
                    let sb_ref = sb_ref.clone();
                    let sel_ref = sel_ref.clone();
                    let proc_name = proc_name.clone();
                    let qname_contents = qname.clone();

                    terminal.connect_contents_changed(move |terminal| {
                        let row = terminal.cursor_position().1;

                        // Capture new output lines into MCP log buffer
                        {
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
                                if let Some(text) = text_opt
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
                            }
                        }

                        // Port detection — skip for agents, skip when not running
                        if !skip_port_detection && sidebar_ref.is_process_running(&qname_contents) {
                            let start_row = (row - 5).max(0);
                            let cols = terminal.column_count();
                            let (text_opt, _len) = terminal.text_range_format(
                                vte4::Format::Text,
                                start_row,
                                0,
                                row,
                                cols,
                            );
                            if let Some(text) = text_opt {
                                let mut det = detector_ref.borrow_mut();
                                det.scan_output(&proc_name, &text);
                                if let Some(port) = det.get_port(&proc_name) {
                                    sidebar_ref.set_process_port(&qname_contents, Some(port));
                                }
                                let url = det.get_url(&proc_name).map(|u| u.to_string());
                                if let Some(ref url_str) = url {
                                    sidebar_ref.set_process_url(&qname_contents, Some(url_str));
                                    if sel_ref.borrow().as_deref() == Some(qname_contents.as_str())
                                    {
                                        sb_ref.set_url(Some(url_str));
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
                            &qname_rename,
                            &ws_ref,
                            &proj_name,
                        );
                    }
                }));
            }
        }

        // Populate sidebar
        sidebar.add_project(manager, project_name, icon_path, saved_expanded);
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
        crate::ui::accent::apply(&settings.appearance.accent_color);
    }

    fn build_content(
        window: &adw::ApplicationWindow,
        ws: &WorkspaceRef,
        sidebar: &Rc<ProjectList>,
        terminal_stack: &gtk4::Stack,
        selected_process: &Rc<RefCell<Option<String>>>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
        on_single_expand_changed: &Rc<dyn Fn(bool)>,
        on_auto_hide_changed: &Rc<dyn Fn(bool)>,
        on_terminal_theme_changed: &Rc<dyn Fn(&str)>,
        on_font_changed: &Rc<dyn Fn()>,
        pid_file: &Rc<RefCell<PidFile>>,
        status_bar: &Rc<StatusBar>,
        keybinding_map: &Rc<RefCell<KeybindingMap>>,
        auto_hide: &Rc<Cell<bool>>,
    ) -> gtk4::Widget {
        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

        // Command palette
        let palette = Rc::new(CommandPalette::new());

        // Refresh navigation items dynamically (only running processes)
        let ws_refresh = ws.clone();
        palette.set_on_refresh(move |p| {
            let ws_borrow = ws_refresh.borrow();
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
                            );
                        }
                    });
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
                                    config.working_dir =
                                        Some(project.dir.to_string_lossy().to_string());
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
                                let mut mgr = project.manager.borrow_mut();
                                mgr.add_process(config);
                                mgr.materialize_process(&name);
                                let terminal =
                                    mgr.get_process(&name).and_then(|p| p.terminal.clone());
                                if let Some(ref term) = terminal {
                                    stack2.add_named(term, Some(&qname));
                                }
                                drop(mgr);
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &name,
                                    ProcessStatus::Stopped,
                                    crate::config::schema::ProcessCategory::Agent,
                                );
                                sidebar2.expand_project(&project_name);
                                Self::setup_auto_restart_for_process(&project.manager, &name);
                                project.manager.borrow_mut().spawn(&name);
                                stack2.set_visible_child_name(&qname);
                                if let Some(ref term) = terminal {
                                    Self::connect_window_title_auto_rename(
                                        term,
                                        &project.manager,
                                        &name,
                                        &sidebar2,
                                        &qname,
                                        &ws2,
                                        &project_name,
                                    );
                                }
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
                                    config.working_dir =
                                        Some(project.dir.to_string_lossy().to_string());
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
                                let mut mgr = project.manager.borrow_mut();
                                mgr.add_process(config);
                                mgr.materialize_process(&name);
                                let terminal =
                                    mgr.get_process(&name).and_then(|p| p.terminal.clone());
                                if let Some(ref term) = terminal {
                                    stack2.add_named(term, Some(&qname));
                                }
                                drop(mgr);
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &name,
                                    ProcessStatus::Stopped,
                                    crate::config::schema::ProcessCategory::SSH,
                                );
                                sidebar2.expand_project(&project_name);
                                Self::setup_auto_restart_for_process(&project.manager, &name);
                                if start_with_project {
                                    project.manager.borrow_mut().spawn(&name);
                                }
                                stack2.set_visible_child_name(&qname);
                                if let Some(ref term) = terminal {
                                    Self::connect_window_title_auto_rename(
                                        term,
                                        &project.manager,
                                        &name,
                                        &sidebar2,
                                        &qname,
                                        &ws2,
                                        &project_name,
                                    );
                                }
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
                                    config.working_dir =
                                        Some(project.dir.to_string_lossy().to_string());
                                }
                            }
                            // Persist the custom command
                            ws2.borrow_mut()
                                .save_custom_command(selected_project, config.clone());

                            let ws_borrow = ws2.borrow();
                            if let Some(project) = ws_borrow
                                .projects()
                                .iter()
                                .find(|p| p.name == selected_project)
                            {
                                let project_name = project.name.clone();
                                let mut mgr = project.manager.borrow_mut();
                                mgr.add_process(config);
                                mgr.materialize_process(&name);
                                let qname = workspace::qualified_name(&project_name, &name);
                                if let Some(proc) = mgr.get_process(&name)
                                    && let Some(ref terminal) = proc.terminal
                                {
                                    stack.add_named(terminal, Some(&qname));
                                }
                                let status = mgr
                                    .get_process(&name)
                                    .map(|p| p.status)
                                    .unwrap_or(ProcessStatus::Stopped);
                                drop(mgr);
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &name,
                                    status,
                                    category,
                                );
                                sidebar2.expand_project(&project_name);
                                Self::setup_auto_restart_for_process(&project.manager, &name);
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
                                restart_when_changed: Vec::new(),
                                env: std::collections::HashMap::new(),
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
                                    config.working_dir =
                                        Some(project.dir.to_string_lossy().to_string());
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
                                let mut mgr = project.manager.borrow_mut();
                                mgr.add_process(config);
                                let terminal = {
                                    mgr.materialize_process(&term_name);
                                    mgr.get_process(&term_name).and_then(|p| p.terminal.clone())
                                };
                                if let Some(ref term) = terminal {
                                    stack2.add_named(term, Some(&qname));
                                }
                                drop(mgr);
                                // Add sidebar row before spawning so status updates are received
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &term_name,
                                    ProcessStatus::Stopped,
                                    crate::config::schema::ProcessCategory::Terminal,
                                );
                                sidebar2.expand_project(&project_name);
                                Self::setup_auto_restart_for_process(&project.manager, &term_name);
                                project.manager.borrow_mut().spawn(&term_name);
                                stack2.set_visible_child_name(&qname);
                                if let Some(ref term) = terminal {
                                    Self::connect_window_title_auto_rename(
                                        term,
                                        &project.manager,
                                        &term_name,
                                        &sidebar2,
                                        &qname,
                                        &ws2,
                                        &project_name,
                                    );
                                }
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
                            let command = match agent_type.as_str() {
                                "claude" => "claude".to_string(),
                                "codex" => "codex".to_string(),
                                "gemini" => "gemini".to_string(),
                                _ => agent_type.to_string(),
                            };
                            let mut config = crate::config::schema::ProcessConfig {
                                name: agent_name.clone(),
                                command,
                                working_dir: None,
                                start_with_project: false,
                                auto_restart: false,
                                restart_when_changed: Vec::new(),
                                env: std::collections::HashMap::new(),
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
                                    config.working_dir =
                                        Some(project.dir.to_string_lossy().to_string());
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
                                let mut mgr = project.manager.borrow_mut();
                                mgr.add_process(config);
                                let terminal = {
                                    mgr.materialize_process(&agent_name);
                                    mgr.get_process(&agent_name)
                                        .and_then(|p| p.terminal.clone())
                                };
                                if let Some(ref term) = terminal {
                                    stack2.add_named(term, Some(&qname));
                                }
                                drop(mgr);
                                // Add sidebar row before spawning so status updates are received
                                sidebar2.add_process_to_project(
                                    &project.manager,
                                    &project_name,
                                    &agent_name,
                                    ProcessStatus::Stopped,
                                    crate::config::schema::ProcessCategory::Agent,
                                );
                                sidebar2.expand_project(&project_name);
                                Self::setup_auto_restart_for_process(&project.manager, &agent_name);
                                project.manager.borrow_mut().spawn(&agent_name);
                                stack2.set_visible_child_name(&qname);
                                if let Some(ref term) = terminal {
                                    Self::connect_window_title_auto_rename(
                                        term,
                                        &project.manager,
                                        &agent_name,
                                        &sidebar2,
                                        &qname,
                                        &ws2,
                                        &project_name,
                                    );
                                }
                            }
                        },
                    );
                }
                _ if action.starts_with("switch:") => {
                    let qname = &action[7..];
                    stack_ref.set_visible_child_name(qname);
                    *sel_ref.borrow_mut() = Some(qname.to_string());
                    sidebar_ref.select_process(qname);
                    if let Some((proj, _)) = qname.split_once("::") {
                        *last_proj_ref.borrow_mut() = Some(proj.to_string());
                        sidebar_ref.set_active_project(proj);
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
            on_terminal_theme_changed,
            on_font_changed,
            keybinding_map,
        );

        // Shared closure to refresh status bar counts
        // Deferred to idle so it never collides with an in-progress borrow_mut on a manager
        let refresh_counts: Rc<dyn Fn()> = {
            let ws_ref = ws.clone();
            let sb_ref = status_bar.clone();
            let last_proj = last_selected_project.clone();
            Rc::new(move || {
                let ws_inner = ws_ref.clone();
                let sb = sb_ref.clone();
                let proj = last_proj.clone();
                glib::idle_add_local_once(move || {
                    let ws_borrow = ws_inner.borrow();
                    let selected_proj = proj.borrow();
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
            move || refresh()
        });
        sidebar.set_on_project_renamed({
            let last_proj = last_selected_project.clone();
            let refresh = refresh_counts.clone();
            move |old_name, new_name| {
                let mut lp = last_proj.borrow_mut();
                if lp.as_deref() == Some(old_name) {
                    *lp = Some(new_name.to_string());
                }
                drop(lp);
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
        status_bar.connect_git_changes(move |btn| {
            let project_name =
                Self::resolve_active_project(&stack_git, &last_proj_git, &sidebar_git);
            if let Some(proj_name) = project_name {
                let ws_borrow = ws_git.borrow();
                if let Some(dir) = ws_borrow.get_project_dir(&proj_name) {
                    GitChangesDialog::show(btn, &dir);
                }
            }
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
            if let Some(name) = stack.visible_child_name() {
                if let Some((proj, _)) = name.split_once("::") {
                    title_ref.set_label(proj);
                    let ws_borrow = ws_vis.borrow();
                    let dir_opt = ws_borrow.get_project_dir(proj);
                    let has_git = dir_opt.as_ref().is_some_and(|d| d.join(".git").exists());
                    sb_vis.set_git_available(has_git);
                    if has_git {
                        if let Some(dir) = dir_opt {
                            let dir = dir.clone();
                            let sb = sb_vis.clone();
                            let (tx, rx) = std::sync::mpsc::channel::<usize>();
                            std::thread::spawn(move || {
                                git_fetch(&dir);
                                let _ = tx.send(commits_behind(&dir));
                            });
                            glib::idle_add_local(move || {
                                if let Ok(behind) = rx.try_recv() {
                                    sb.set_git_pull_indicator(behind);
                                    return glib::ControlFlow::Break;
                                }
                                glib::ControlFlow::Continue
                            });
                        }
                    } else {
                        sb_vis.set_git_pull_indicator(0);
                    }
                }
            } else {
                title_ref.set_label("TuxFlow");
                sb_vis.set_git_available(false);
                sb_vis.set_git_pull_indicator(0);
            }
        });

        // Poll git pull indicator every 60 seconds
        {
            let ws_poll = ws.clone();
            let sb_poll = status_bar.clone();
            let stack_poll = terminal_stack.clone();
            let last_proj_poll = last_selected_project.clone();
            let sidebar_poll = sidebar.clone();
            glib::timeout_add_seconds_local(60, move || {
                let project_name = TuxFlowWindow::resolve_active_project(
                    &stack_poll,
                    &last_proj_poll,
                    &sidebar_poll,
                );
                if let Some(proj_name) = project_name {
                    let ws_borrow = ws_poll.borrow();
                    if let Some(dir) = ws_borrow.get_project_dir(&proj_name) {
                        if dir.join(".git").exists() {
                            let dir = dir.clone();
                            let sb = sb_poll.clone();
                            let (tx, rx) = std::sync::mpsc::channel::<usize>();
                            std::thread::spawn(move || {
                                git_fetch(&dir);
                                let _ = tx.send(commits_behind(&dir));
                            });
                            glib::idle_add_local(move || {
                                if let Ok(behind) = rx.try_recv() {
                                    sb.set_git_pull_indicator(behind);
                                    return glib::ControlFlow::Break;
                                }
                                glib::ControlFlow::Continue
                            });
                        }
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        let content_overlay = gtk4::Overlay::new();
        content_overlay.set_child(Some(terminal_stack));
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
            on_terminal_theme_changed,
            on_font_changed,
            keybinding_map,
            last_selected_project,
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
        on_terminal_theme_changed: &Rc<dyn Fn(&str)>,
        on_font_changed: &Rc<dyn Fn()>,
        keybinding_map: &Rc<RefCell<KeybindingMap>>,
        last_selected_project: &Rc<RefCell<Option<String>>>,
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
        let theme_cb = on_terminal_theme_changed.clone();
        let font_cb = on_font_changed.clone();
        let kb_map = keybinding_map.clone();
        let last_proj_ref = last_selected_project.clone();

        key_controller.connect_key_pressed(move |_, keyval, _keycode, state| {
            // Skip all shortcuts while settings key capture is active
            if kb_map.borrow().is_capturing() {
                return gtk4::glib::Propagation::Proceed;
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
                            terminal.copy_clipboard_format(vte4::Format::Text);
                        }
                    }
                    ShortcutAction::Paste => {
                        if let Some(child) = stack_ref.visible_child()
                            && let Ok(terminal) = child.downcast::<vte4::Terminal>()
                        {
                            terminal.paste_clipboard();
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
                            Some(theme_cb.clone()),
                            Some(font_cb.clone()),
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
                        Self::switch_relative(&ws_ref, &stack_ref, &selected_ref, &sidebar_ref, -1);
                    }
                    ShortcutAction::NextProcess => {
                        Self::switch_relative(&ws_ref, &stack_ref, &selected_ref, &sidebar_ref, 1);
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
                        Self::switch_project_relative(&ws_ref, &stack_ref, &sidebar_ref, -1);
                    }
                    ShortcutAction::NextProject => {
                        Self::switch_project_relative(&ws_ref, &stack_ref, &sidebar_ref, 1);
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
                    Self::switch_to_nth_global(&ws_ref, &stack_ref, i);
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
                    Self::switch_to_project(&ws_ref, &stack_ref, idx);
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
        on_terminal_theme_changed: &Rc<dyn Fn(&str)>,
        on_font_changed: &Rc<dyn Fn()>,
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
        let theme_cb = on_terminal_theme_changed.clone();
        let font_cb = on_font_changed.clone();
        let kb_map = keybinding_map.clone();
        settings_btn.connect_clicked(move |_| {
            crate::ui::settings::settings_window::SettingsWindow::show(
                &window_ref,
                Some(single_expand_cb.clone()),
                Some(auto_hide_cb.clone()),
                Some(theme_cb.clone()),
                Some(font_cb.clone()),
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
            command: std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
            working_dir: Some(project.dir.to_string_lossy().to_string()),
            start_with_project: true,
            auto_restart: false,
            restart_when_changed: Vec::new(),
            env: std::collections::HashMap::new(),
            category: crate::config::schema::ProcessCategory::Terminal,
            auto_named: true,
            display_name: None,
        };

        drop(ws_borrow);
        ws.borrow_mut()
            .save_custom_command(&project_name, config.clone());

        let ws_borrow = ws.borrow();
        if let Some(project) = ws_borrow.projects().iter().find(|p| p.name == project_name) {
            let qname = workspace::qualified_name(&project_name, &term_name);
            let mut mgr = project.manager.borrow_mut();
            mgr.add_process(config);
            let terminal = {
                mgr.materialize_process(&term_name);
                mgr.get_process(&term_name).and_then(|p| p.terminal.clone())
            };
            if let Some(ref term) = terminal {
                stack.add_named(term, Some(&qname));
            }
            drop(mgr);
            sidebar.add_process_to_project(
                &project.manager,
                &project_name,
                &term_name,
                ProcessStatus::Stopped,
                crate::config::schema::ProcessCategory::Terminal,
            );
            Self::setup_auto_restart_for_process(&project.manager, &term_name);
            project.manager.borrow_mut().spawn(&term_name);
            if let Some(ref term) = terminal {
                Self::connect_window_title_auto_rename(
                    term,
                    &project.manager,
                    &term_name,
                    sidebar,
                    &qname,
                    ws,
                    &project_name,
                );
            }
        }
    }

    fn switch_relative(
        ws: &WorkspaceRef,
        stack: &gtk4::Stack,
        selected: &Rc<RefCell<Option<String>>>,
        sidebar: &Rc<ProjectList>,
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
        if let Some(child) = stack.visible_child() {
            child.grab_focus();
        }
    }

    fn switch_project_relative(
        ws: &WorkspaceRef,
        stack: &gtk4::Stack,
        sidebar: &Rc<ProjectList>,
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
    }

    fn switch_to_project(ws: &WorkspaceRef, stack: &gtk4::Stack, project_idx: usize) {
        let ws_borrow = ws.borrow();
        if let Some(project) = ws_borrow.projects().get(project_idx) {
            let mgr = project.manager.borrow();
            if let Some(first_name) = mgr.process_names().first() {
                let qname = workspace::qualified_name(&project.name, first_name);
                stack.set_visible_child_name(&qname);
            }
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

    fn switch_to_nth_global(ws: &WorkspaceRef, stack: &gtk4::Stack, n: usize) {
        let ws_borrow = ws.borrow();
        let mut idx = 0;
        for project in ws_borrow.projects() {
            let mgr = project.manager.borrow();
            for name in mgr.process_names() {
                if idx == n {
                    let qname = workspace::qualified_name(&project.name, name);
                    stack.set_visible_child_name(&qname);
                    return;
                }
                idx += 1;
            }
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
