use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::gio;
use gtk4::prelude::*;

use crate::process::manager::ProcessStatus;

type ActionCallback = Rc<RefCell<Option<Box<dyn Fn(&str, &str)>>>>;

pub struct ProcessRow {
    container: gtk4::Box,
    status_stack: gtk4::Stack,
    status_dot: gtk4::Label,
    status_spinner: gtk4::Image,
    is_terminal: bool,
    is_running: Cell<bool>,
    name_label: gtk4::Label,
    /// Shared name used by button callbacks so they track renames.
    action_name: Rc<RefCell<String>>,
    /// Shared qualified name (project::process) for context actions.
    pub qualified_name: Rc<RefCell<String>>,
    cpu_label: gtk4::Label,
    memory_label: gtk4::Label,
    port_label: gtk4::Label,
    browser_button: gtk4::Button,
    play_button: gtk4::Button,
    restart_button: gtk4::Button,
    stop_button: gtk4::Button,
    on_context_action: ActionCallback,
    url: Rc<RefCell<Option<String>>>,
    browser_menu_section: gio::Menu,
}

impl ProcessRow {
    pub fn new(name: &str, command: &str) -> Self {
        Self::new_with_options(name, command, false)
    }

    pub fn new_terminal(name: &str, command: &str) -> Self {
        Self::new_with_options(name, command, true)
    }

    fn new_with_options(name: &str, command: &str, is_terminal: bool) -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        container.set_margin_start(8);
        container.set_margin_end(12);
        container.set_margin_top(4);
        container.set_margin_bottom(4);
        container.add_css_class("process-row");
        container.set_tooltip_text(Some(command));

        // Status indicator: stack with spinner (running) and dot (stopped/crashed)
        let status_dot = gtk4::Label::builder()
            .label("\u{25CF}") // ●
            .css_classes(["caption", "status-stopped"])
            .build();

        let status_spinner = gtk4::Image::builder()
            .icon_name("process-working-symbolic")
            .pixel_size(10)
            .css_classes(["status-spinner"])
            .build();

        let status_stack = gtk4::Stack::builder()
            .transition_type(gtk4::StackTransitionType::None)
            .build();
        status_stack.add_named(&status_dot, Some("dot"));
        status_stack.add_named(&status_spinner, Some("spinner"));
        status_stack.set_visible_child_name("dot");
        container.append(&status_stack);

        // Process name
        let name_label = gtk4::Label::builder()
            .label(name)
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        container.append(&name_label);

        // CPU label (hidden by default)
        let cpu_label = gtk4::Label::builder()
            .css_classes(["caption", "dim-label", "resource-label"])
            .visible(false)
            .build();
        container.append(&cpu_label);

        // Memory label (hidden by default)
        let memory_label = gtk4::Label::builder()
            .css_classes(["caption", "dim-label", "resource-label"])
            .visible(false)
            .build();
        container.append(&memory_label);

        // Port label (hidden by default)
        let port_label = gtk4::Label::builder()
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();
        container.append(&port_label);

        // Browser button (hidden until URL is detected)
        let browser_button = gtk4::Button::builder()
            .icon_name("external-link-symbolic")
            .tooltip_text("Open in Browser")
            .css_classes(["flat", "circular", "browser-btn"])
            .visible(false)
            .build();
        container.append(&browser_button);

        let url: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        // Wire browser button to open URL
        let url_ref = url.clone();
        browser_button.connect_clicked(move |btn| {
            if let Some(ref url_str) = *url_ref.borrow() {
                let launcher = gtk4::UriLauncher::new(url_str);
                let window = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
                launcher.launch(window.as_ref(), gio::Cancellable::NONE, |_| {});
            }
        });

        // Action buttons (visible on hover via CSS)
        let play_button = gtk4::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text(command)
            .css_classes(["flat", "circular", "process-play-btn", "btn-play"])
            .build();
        container.append(&play_button);

        let restart_button = gtk4::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Restart")
            .css_classes(["flat", "circular", "process-play-btn"])
            .build();
        container.append(&restart_button);

        let stop_button = gtk4::Button::builder()
            .icon_name("media-playback-stop-symbolic")
            .tooltip_text("Stop")
            .css_classes(["flat", "circular", "process-play-btn", "btn-stop"])
            .build();
        container.append(&stop_button);

        let on_context_action: ActionCallback = Rc::new(RefCell::new(None));
        let action_name: Rc<RefCell<String>> = Rc::new(RefCell::new(name.to_string()));

        // Wire play button to trigger "toggle" action
        let on_action_ref = on_context_action.clone();
        let aname = action_name.clone();
        play_button.connect_clicked(move |_| {
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&aname.borrow(), "toggle");
            }
        });

        // Wire restart button
        let on_action_ref = on_context_action.clone();
        let aname = action_name.clone();
        restart_button.connect_clicked(move |_| {
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&aname.borrow(), "restart");
            }
        });

        // Wire stop button
        let on_action_ref = on_context_action.clone();
        let aname = action_name.clone();
        stop_button.connect_clicked(move |_| {
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&aname.borrow(), "stop");
            }
        });

        // Right-click context menu
        let (popover, _menu, browser_section) =
            Self::build_context_menu(name, command, &on_context_action, &url, &action_name);
        popover.set_parent(&container);

        let gesture = gtk4::GestureClick::builder()
            .button(3) // right click
            .build();
        let popover_ref = popover;
        gesture.connect_released(move |_, _, x, y| {
            popover_ref.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover_ref.popup();
        });
        container.add_controller(gesture);

        // Initially show play, hide stop and restart (default state is Stopped)
        stop_button.set_visible(false);
        restart_button.set_visible(false);

        Self {
            container,
            status_stack,
            status_dot,
            status_spinner,
            is_terminal,
            is_running: Cell::new(false),
            name_label,
            action_name,
            qualified_name: Rc::new(RefCell::new(String::new())),
            cpu_label,
            memory_label,
            port_label,
            browser_button,
            play_button,
            restart_button,
            stop_button,
            on_context_action,
            url,
            browser_menu_section: browser_section,
        }
    }

    fn build_context_menu(
        _process_name: &str,
        command: &str,
        on_action: &ActionCallback,
        url: &Rc<RefCell<Option<String>>>,
        action_name: &Rc<RefCell<String>>,
    ) -> (gtk4::PopoverMenu, gio::Menu, gio::Menu) {
        let menu = gio::Menu::new();

        let control_section = gio::Menu::new();
        control_section.append(Some("Start / Stop"), Some("proc.toggle"));
        control_section.append(Some("Restart"), Some("proc.restart"));
        menu.append_section(None, &control_section);

        // Browser section (initially empty, items added/removed dynamically)
        let browser_section = gio::Menu::new();
        menu.append_section(None, &browser_section);

        let terminal_section = gio::Menu::new();
        terminal_section.append(Some("Edit Command"), Some("proc.edit"));
        terminal_section.append(Some("Clear Output"), Some("proc.clear"));
        terminal_section.append(Some("Redraw Terminal"), Some("proc.redraw"));
        terminal_section.append(Some("Copy Command"), Some("proc.copy_command"));
        menu.append_section(None, &terminal_section);

        let danger_section = gio::Menu::new();
        let delete_item = gio::MenuItem::new(None, None);
        delete_item.set_attribute_value("custom", Some(&"delete-button".to_variant()));
        danger_section.append_item(&delete_item);
        menu.append_section(None, &danger_section);

        let popover = gtk4::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);

        // Custom red delete button
        let delete_btn = gtk4::Button::builder()
            .label("Delete Command")
            .css_classes(["flat", "destructive-menu-item"])
            .build();
        popover.add_child(&delete_btn, "delete-button");

        let action_group = gio::SimpleActionGroup::new();

        // Helper to create context actions — reads from shared action_name
        let add_action = |action_name_str: &str, action_str: &str| {
            let on_action_ref = on_action.clone();
            let aname = action_name.clone();
            let action_owned = action_str.to_string();
            let action = gio::SimpleAction::new(action_name_str, None);
            action.connect_activate(move |_, _| {
                if let Some(ref cb) = *on_action_ref.borrow() {
                    cb(&aname.borrow(), &action_owned);
                }
            });
            action_group.add_action(&action);
        };

        add_action("toggle", "toggle");
        add_action("restart", "restart");
        add_action("edit", "edit");
        add_action("clear", "clear");
        add_action("redraw", "redraw");

        // Wire delete button directly (custom widget, not in action group)
        let on_action_ref = on_action.clone();
        let aname = action_name.clone();
        let popover_ref = popover.clone();
        delete_btn.connect_clicked(move |_| {
            popover_ref.popdown();
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&aname.borrow(), "delete");
            }
        });

        // Copy command — uses clipboard directly
        let command_owned = command.to_string();
        let copy_action = gio::SimpleAction::new("copy_command", None);
        copy_action.connect_activate(move |_, _| {
            if let Some(display) = gtk4::gdk::Display::default() {
                display.clipboard().set_text(&command_owned);
            }
        });
        action_group.add_action(&copy_action);

        // Open in Browser action
        let url_ref = url.clone();
        let open_url_action = gio::SimpleAction::new("open_url", None);
        let popover_ref2 = popover.clone();
        open_url_action.connect_activate(move |_, _| {
            if let Some(ref url_str) = *url_ref.borrow() {
                let launcher = gtk4::UriLauncher::new(url_str);
                let window = popover_ref2
                    .root()
                    .and_then(|r| r.downcast::<gtk4::Window>().ok());
                launcher.launch(window.as_ref(), gio::Cancellable::NONE, |_| {});
            }
        });
        action_group.add_action(&open_url_action);

        popover.insert_action_group("proc", Some(&action_group));

        (popover, menu, browser_section)
    }

    pub fn set_status(&self, status: ProcessStatus) {
        // Remove old CSS classes from dot
        self.status_dot.remove_css_class("status-running");
        self.status_dot.remove_css_class("status-stopped");
        self.status_dot.remove_css_class("status-crashed");
        self.status_dot.remove_css_class("status-restarting");

        let is_running = matches!(status, ProcessStatus::Running | ProcessStatus::Restarting);
        self.play_button.set_visible(!is_running);
        self.stop_button.set_visible(is_running);
        self.restart_button.set_visible(is_running);

        match status {
            ProcessStatus::Running | ProcessStatus::Restarting => {
                self.is_running.set(true);
                self.status_spinner.remove_css_class("spinning");
                self.status_stack.set_visible_child_name("dot");
                self.status_dot.add_css_class("status-running");
            }
            ProcessStatus::Stopped => {
                self.is_running.set(false);
                self.status_spinner.remove_css_class("spinning");
                self.status_stack.set_visible_child_name("dot");
                self.status_dot.add_css_class("status-stopped");
                self.clear_resources();
                self.set_port(None);
                self.set_url(None);
            }
            ProcessStatus::Crashed => {
                self.is_running.set(false);
                self.status_spinner.remove_css_class("spinning");
                self.status_stack.set_visible_child_name("dot");
                self.status_dot.add_css_class("status-crashed");
                self.clear_resources();
                self.set_port(None);
                self.set_url(None);
            }
        }
    }

    pub fn set_resources(
        &self,
        cpu_percent: f64,
        memory_mb: f64,
        cpu_threshold: f64,
        mem_threshold: f64,
    ) {
        self.cpu_label.set_label(&format!("{cpu_percent:.1}%"));
        self.cpu_label
            .set_visible(cpu_threshold >= 0.0 && cpu_percent > cpu_threshold);

        let mem_str = if memory_mb >= 1024.0 {
            format!("{:.1}GB", memory_mb / 1024.0)
        } else {
            format!("{:.0}MB", memory_mb)
        };
        self.memory_label.set_label(&mem_str);
        self.memory_label
            .set_visible(mem_threshold >= 0.0 && memory_mb > mem_threshold);

        // Toggle spinner based on CPU activity (not for terminals)
        if self.is_running.get() {
            if cpu_percent > 1.0 {
                self.status_spinner.add_css_class("spinning");
                self.status_stack.set_visible_child_name("spinner");
            } else {
                self.status_spinner.remove_css_class("spinning");
                self.status_dot.remove_css_class("status-stopped");
                self.status_dot.remove_css_class("status-crashed");
                self.status_dot.add_css_class("status-running");
                self.status_stack.set_visible_child_name("dot");
            }
        }
    }

    pub fn clear_resources(&self) {
        self.cpu_label.set_visible(false);
        self.memory_label.set_visible(false);
        self.status_spinner.remove_css_class("spinning");
        self.status_stack.set_visible_child_name("dot");
    }

    pub fn set_port(&self, port: Option<u16>) {
        match port {
            Some(p) => {
                self.port_label.set_label(&format!(":{p}"));
                self.port_label.set_visible(true);
            }
            None => {
                if !self.port_label.is_visible() {
                    return;
                }
                self.port_label.set_visible(false);
            }
        }
    }

    pub fn set_url(&self, url: Option<&str>) {
        match url {
            Some(u) => {
                if self.url.borrow().as_deref() == Some(u) {
                    return;
                }
                *self.url.borrow_mut() = Some(u.to_string());
                self.browser_button.set_visible(true);
                self.browser_button
                    .set_tooltip_text(Some(&format!("Open {u}")));
                if self.browser_menu_section.n_items() == 0 {
                    self.browser_menu_section
                        .append(Some("Open in Browser"), Some("proc.open_url"));
                }
            }
            None => {
                if self.url.borrow().is_none() {
                    return;
                }
                *self.url.borrow_mut() = None;
                self.browser_button.set_visible(false);
                self.browser_menu_section.remove_all();
            }
        }
    }

    pub fn get_url(&self) -> Option<String> {
        self.url.borrow().clone()
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }

    pub fn set_on_context_action(&self, cb: impl Fn(&str, &str) + 'static) {
        *self.on_context_action.borrow_mut() = Some(Box::new(cb));
    }

    pub fn name(&self) -> String {
        self.name_label.label().to_string()
    }

    pub fn set_name(&self, name: &str) {
        self.name_label.set_label(name);
    }

    /// Update the internal process name used by button/menu actions.
    /// Call this when the process is renamed (not for display_name changes).
    pub fn set_action_name(&self, name: &str) {
        *self.action_name.borrow_mut() = name.to_string();
    }

    pub fn set_command_tooltip(&self, command: &str) {
        self.container.set_tooltip_text(Some(command));
    }
}
