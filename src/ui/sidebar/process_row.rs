use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::gio;

use crate::process::manager::ProcessStatus;

type ActionCallback = Rc<RefCell<Option<Box<dyn Fn(&str, &str)>>>>;

pub struct ProcessRow {
    container: gtk4::Box,
    status_dot: gtk4::Label,
    name_label: gtk4::Label,
    port_label: gtk4::Label,
    on_context_action: ActionCallback,
}

impl ProcessRow {
    pub fn new(name: &str) -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        container.set_margin_start(32);
        container.set_margin_end(12);
        container.set_margin_top(4);
        container.set_margin_bottom(4);
        container.add_css_class("process-row");

        // Status dot
        let status_dot = gtk4::Label::builder()
            .label("\u{25CF}") // ●
            .css_classes(["caption", "status-stopped"])
            .build();
        container.append(&status_dot);

        // Process name
        let name_label = gtk4::Label::builder()
            .label(name)
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        container.append(&name_label);

        // Port label (hidden by default)
        let port_label = gtk4::Label::builder()
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();
        container.append(&port_label);

        let on_context_action: ActionCallback = Rc::new(RefCell::new(None));

        // Right-click context menu
        let popover = Self::build_context_menu(name, &on_context_action);
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

        Self {
            container,
            status_dot,
            name_label,
            port_label,
            on_context_action,
        }
    }

    fn build_context_menu(process_name: &str, on_action: &ActionCallback) -> gtk4::PopoverMenu {
        let menu = gio::Menu::new();
        menu.append(Some("Start / Stop"), Some("proc.toggle"));
        menu.append(Some("Restart"), Some("proc.restart"));
        menu.append(Some("Clear Output"), Some("proc.clear"));

        let popover = gtk4::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);

        let action_group = gio::SimpleActionGroup::new();
        let name = process_name.to_string();

        let on_action_ref = on_action.clone();
        let name_ref = name.clone();
        let toggle_action = gio::SimpleAction::new("toggle", None);
        toggle_action.connect_activate(move |_, _| {
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&name_ref, "toggle");
            }
        });
        action_group.add_action(&toggle_action);

        let on_action_ref = on_action.clone();
        let name_ref = name.clone();
        let restart_action = gio::SimpleAction::new("restart", None);
        restart_action.connect_activate(move |_, _| {
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&name_ref, "restart");
            }
        });
        action_group.add_action(&restart_action);

        let on_action_ref = on_action.clone();
        let name_ref = name.clone();
        let clear_action = gio::SimpleAction::new("clear", None);
        clear_action.connect_activate(move |_, _| {
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&name_ref, "clear");
            }
        });
        action_group.add_action(&clear_action);

        popover.insert_action_group("process", Some(&action_group));

        popover
    }

    pub fn set_status(&self, status: ProcessStatus) {
        // Remove old CSS classes
        self.status_dot.remove_css_class("status-running");
        self.status_dot.remove_css_class("status-stopped");
        self.status_dot.remove_css_class("status-crashed");
        self.status_dot.remove_css_class("status-restarting");

        match status {
            ProcessStatus::Running => {
                self.status_dot.add_css_class("status-running");
            }
            ProcessStatus::Stopped => {
                self.status_dot.add_css_class("status-stopped");
            }
            ProcessStatus::Crashed => {
                self.status_dot.add_css_class("status-crashed");
            }
            ProcessStatus::Restarting => {
                self.status_dot.add_css_class("status-restarting");
            }
        }
    }

    pub fn set_port(&self, port: Option<u16>) {
        match port {
            Some(p) => {
                self.port_label.set_label(&p.to_string());
                self.port_label.set_visible(true);
            }
            None => {
                self.port_label.set_visible(false);
            }
        }
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
}
