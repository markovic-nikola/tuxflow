use std::path::Path;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::gdk;
use libadwaita as adw;
use adw::prelude::*;
use vte4::prelude::*;

use crate::config::loader;
use crate::detect::detector;
use crate::process::auto_restart;
use crate::process::manager::{ProcessManager, ProcessManagerRef};
use crate::ui::add_command_dialog::AddCommandDialog;
use crate::ui::command_palette::CommandPalette;
use crate::ui::sidebar::project_list::ProjectList;
use crate::ui::status_bar::StatusBar;
use crate::watcher::file_watcher::FileWatcher;

pub struct TuxFlowWindow;

impl TuxFlowWindow {
    pub fn new(app: &adw::Application, project_dir: Option<&Path>) -> adw::ApplicationWindow {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("TuxFlow")
            .default_width(1200)
            .default_height(800)
            .build();

        Self::load_css();

        let manager = ProcessManager::new();
        let mut project_name = String::from("TuxFlow");
        let mut detected = false;

        // Load config or auto-detect
        if let Some(dir) = project_dir {
            if let Some(config_path) = loader::find_config(dir) {
                match loader::load_config(&config_path) {
                    Ok(config) => {
                        project_name = config.project.name.clone();
                        let mut mgr = manager.borrow_mut();
                        for proc_config in config.process {
                            mgr.add_process(proc_config);
                        }
                        log::info!("Loaded config from {}", config_path.display());
                    }
                    Err(e) => log::error!("Failed to load config: {e}"),
                }
            } else {
                log::info!("No tuxflow.toml found, running stack detection in {}", dir.display());
                let stacks = detector::detect_stacks(dir);
                if !stacks.is_empty() {
                    detected = true;
                    project_name = dir
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "project".to_string());
                    let mut mgr = manager.borrow_mut();
                    for stack in &stacks {
                        log::info!("Detected stack: {} ({} commands)", stack.name, stack.suggested_processes.len());
                        for proc_config in &stack.suggested_processes {
                            mgr.add_process(proc_config.clone());
                        }
                    }
                }
            }
        }

        // Build UI
        let content = Self::build_content(&window, &manager, &project_name, detected);
        window.set_content(Some(&content));

        // Setup auto-restart
        {
            let names: Vec<String> = manager.borrow().process_names().to_vec();
            for name in &names {
                auto_restart::setup_auto_restart(&manager, name);
            }
        }

        // Spawn auto-start processes
        manager.borrow_mut().spawn_auto_start();

        // Start file watcher
        if let Some(dir) = project_dir {
            let _watcher = FileWatcher::new(dir, &manager);
        }

        // Start MCP server
        let process_state = crate::mcp::server::create_process_state(&manager);
        crate::mcp::server::start_mcp_server(&project_name, process_state);

        window
    }

    fn load_css() {
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(include_str!("../../data/style.css"));
        gtk4::style_context_add_provider_for_display(
            &gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    fn build_content(
        window: &adw::ApplicationWindow,
        manager: &ProcessManagerRef,
        project_name: &str,
        detected: bool,
    ) -> gtk4::Widget {
        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

        // Terminal stack
        let terminal_stack = gtk4::Stack::new();
        terminal_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        terminal_stack.set_transition_duration(150);
        terminal_stack.set_vexpand(true);
        terminal_stack.set_hexpand(true);

        {
            let mgr = manager.borrow();
            for name in mgr.process_names() {
                if let Some(proc) = mgr.get_process(name) {
                    terminal_stack.add_named(&proc.terminal, Some(name));
                }
            }
        }

        if manager.borrow().total_count() == 0 {
            let default_terminal = crate::ui::terminal_view::TerminalView::new();
            terminal_stack.add_named(default_terminal.widget(), Some("__default_shell__"));
        }

        // Sidebar
        let sidebar = ProjectList::new();
        sidebar.populate(manager, project_name);

        let stack_ref = terminal_stack.clone();
        sidebar.set_on_process_selected(move |name| {
            stack_ref.set_visible_child_name(name);
        });

        // Command palette
        let palette = Rc::new(CommandPalette::new());
        palette.add_navigation_items(manager.borrow().process_names());

        // Wire palette actions
        let manager_ref = manager.clone();
        let stack_ref = terminal_stack.clone();
        let window_ref = window.clone();
        let palette_ref = palette.clone();
        palette.set_on_action(move |action| {
            match action {
                "stop_all" => manager_ref.borrow_mut().stop_all(),
                "restart_all" => manager_ref.borrow_mut().restart_all(),
                "start_all" => manager_ref.borrow_mut().spawn_auto_start(),
                "add_process" => {
                    let mgr = manager_ref.clone();
                    let stack = stack_ref.clone();
                    AddCommandDialog::show(&window_ref, move |config| {
                        let name = config.name.clone();
                        let mut m = mgr.borrow_mut();
                        m.add_process(config);
                        if let Some(proc) = m.get_process(&name) {
                            stack.add_named(&proc.terminal, Some(&name));
                        }
                        m.spawn(&name);
                    });
                }
                _ if action.starts_with("switch:") => {
                    let name = &action[7..];
                    stack_ref.set_visible_child_name(name);
                }
                _ => log::warn!("Unknown palette action: {action}"),
            }
            palette_ref.hide();
        });

        // Status bar
        let status_bar = StatusBar::new();

        let manager_ref = manager.clone();
        status_bar.connect_stop(move || manager_ref.borrow_mut().stop_all());

        let manager_ref = manager.clone();
        status_bar.connect_restart(move || manager_ref.borrow_mut().restart_all());

        let stack_ref = terminal_stack.clone();
        status_bar.connect_clear(move || {
            if let Some(child) = stack_ref.visible_child() {
                if let Ok(terminal) = child.downcast::<vte4::Terminal>() {
                    terminal.reset(true, true);
                }
            }
        });

        let running = manager.borrow().running_count();
        let total = manager.borrow().total_count();
        status_bar.set_process_info(project_name, running, total);

        // Headerbar
        let headerbar = Self::build_headerbar(window, manager, &terminal_stack, project_name, running, total);

        // Split view
        let split_view = adw::OverlaySplitView::new();

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

        // Content area: overlay terminal stack with command palette
        let content_overlay = gtk4::Overlay::new();
        content_overlay.set_child(Some(&terminal_stack));
        content_overlay.add_overlay(palette.widget());
        split_view.set_content(Some(&content_overlay));

        // Assemble
        vbox.append(&headerbar);

        if detected {
            let banner = adw::Banner::new("Stack auto-detected \u{2014} processes loaded from project files. Run with a tuxflow.toml for full control.");
            banner.set_revealed(true);
            banner.set_button_label(Some("Dismiss"));
            banner.connect_button_clicked(|b| b.set_revealed(false));
            vbox.append(&banner);
        }

        vbox.append(&split_view);
        vbox.append(status_bar.widget());

        // Keyboard shortcuts
        Self::setup_keyboard_shortcuts(window, &palette, manager, &terminal_stack);

        vbox.upcast()
    }

    fn setup_keyboard_shortcuts(
        window: &adw::ApplicationWindow,
        palette: &Rc<CommandPalette>,
        manager: &ProcessManagerRef,
        terminal_stack: &gtk4::Stack,
    ) {
        let key_controller = gtk4::EventControllerKey::new();

        let palette_ref = palette.clone();
        let manager_ref = manager.clone();
        let stack_ref = terminal_stack.clone();

        key_controller.connect_key_pressed(move |_, keyval, _keycode, state| {
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);

            if ctrl {
                match keyval {
                    // Ctrl+K — command palette
                    gdk::Key::k => {
                        palette_ref.toggle();
                        return gtk4::glib::Propagation::Stop;
                    }
                    // Ctrl+1..9 — switch to process N
                    gdk::Key::_1 => { Self::switch_to_nth(&manager_ref, &stack_ref, 0); return gtk4::glib::Propagation::Stop; }
                    gdk::Key::_2 => { Self::switch_to_nth(&manager_ref, &stack_ref, 1); return gtk4::glib::Propagation::Stop; }
                    gdk::Key::_3 => { Self::switch_to_nth(&manager_ref, &stack_ref, 2); return gtk4::glib::Propagation::Stop; }
                    gdk::Key::_4 => { Self::switch_to_nth(&manager_ref, &stack_ref, 3); return gtk4::glib::Propagation::Stop; }
                    gdk::Key::_5 => { Self::switch_to_nth(&manager_ref, &stack_ref, 4); return gtk4::glib::Propagation::Stop; }
                    gdk::Key::_6 => { Self::switch_to_nth(&manager_ref, &stack_ref, 5); return gtk4::glib::Propagation::Stop; }
                    gdk::Key::_7 => { Self::switch_to_nth(&manager_ref, &stack_ref, 6); return gtk4::glib::Propagation::Stop; }
                    gdk::Key::_8 => { Self::switch_to_nth(&manager_ref, &stack_ref, 7); return gtk4::glib::Propagation::Stop; }
                    gdk::Key::_9 => { Self::switch_to_nth(&manager_ref, &stack_ref, 8); return gtk4::glib::Propagation::Stop; }
                    _ => {}
                }
            }

            // Escape closes palette
            if keyval == gdk::Key::Escape && palette_ref.is_visible() {
                palette_ref.hide();
                return gtk4::glib::Propagation::Stop;
            }

            gtk4::glib::Propagation::Proceed
        });

        window.add_controller(key_controller);
    }

    fn build_headerbar(
        window: &adw::ApplicationWindow,
        manager: &ProcessManagerRef,
        terminal_stack: &gtk4::Stack,
        project_name: &str,
        running: usize,
        total: usize,
    ) -> adw::HeaderBar {
        let headerbar = adw::HeaderBar::new();

        // Sidebar toggle
        let sidebar_btn = gtk4::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .active(true)
            .tooltip_text("Toggle Sidebar (Ctrl+\\)")
            .build();
        headerbar.pack_start(&sidebar_btn);

        // Title with running status
        let title_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        title_box.set_halign(gtk4::Align::Center);

        let title_label = gtk4::Label::builder()
            .label(&format!("TuxFlow \u{2014} {project_name}"))
            .css_classes(["title"])
            .build();
        title_box.append(&title_label);

        if total > 0 {
            let status_dot = gtk4::Label::builder()
                .label("\u{25CF}")
                .css_classes(if running > 0 { vec!["status-running"] } else { vec!["status-stopped"] })
                .build();
            title_box.append(&status_dot);

            let count_label = gtk4::Label::builder()
                .label(&format!("{running}/{total} Running"))
                .css_classes(["dim-label"])
                .build();
            title_box.append(&count_label);
        }

        headerbar.set_title_widget(Some(&title_box));

        // Settings button
        let settings_btn = gtk4::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("Settings (Ctrl+,)")
            .build();
        let window_ref2 = window.clone();
        settings_btn.connect_clicked(move |_| {
            crate::ui::settings::settings_window::SettingsWindow::show(&window_ref2);
        });
        headerbar.pack_end(&settings_btn);

        // Add process button
        let add_btn = gtk4::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add Process (Ctrl+T)")
            .build();

        let window_ref = window.clone();
        let manager_ref = manager.clone();
        let stack_ref = terminal_stack.clone();
        add_btn.connect_clicked(move |_| {
            let mgr = manager_ref.clone();
            let stack = stack_ref.clone();
            AddCommandDialog::show(&window_ref, move |config| {
                let name = config.name.clone();
                let mut m = mgr.borrow_mut();
                m.add_process(config);
                if let Some(proc) = m.get_process(&name) {
                    stack.add_named(&proc.terminal, Some(&name));
                }
                m.spawn(&name);
            });
        });

        headerbar.pack_end(&add_btn);

        headerbar
    }

    fn switch_to_nth(manager: &ProcessManagerRef, stack: &gtk4::Stack, n: usize) {
        let mgr = manager.borrow();
        let names = mgr.process_names();
        if n < names.len() {
            stack.set_visible_child_name(&names[n]);
        }
    }
}
