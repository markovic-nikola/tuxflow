use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gio;
use gtk4::prelude::*;

type ActionCallback = Rc<RefCell<Option<Box<dyn Fn(&str)>>>>;
type ToggleCallback = Rc<RefCell<Option<Box<dyn Fn(&str, bool)>>>>;

pub struct ProjectRow {
    container: gtk4::Box,
    header_row: gtk4::Box,
    expander_icon: gtk4::Image,
    icon_area: gtk4::Box,
    name_label: gtk4::Label,
    memory_label: gtk4::Label,
    revealer: gtk4::Revealer,
    content_box: gtk4::Box,
    controls_box: gtk4::Box,
    on_context_action: ActionCallback,
    on_toggled: ToggleCallback,
}

impl ProjectRow {
    pub fn new(name: &str, icon_path: Option<&str>) -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

        // Project header row
        let header_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
        header_row.set_margin_top(1);
        header_row.set_margin_bottom(1);
        header_row.add_css_class("project-row");

        let expander_icon = gtk4::Image::from_icon_name("pan-down-symbolic");
        header_row.append(&expander_icon);

        // Project icon area — shows image if icon_path set, else initials
        let icon_area = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        icon_area.set_size_request(24, 24);
        icon_area.set_halign(gtk4::Align::Center);
        icon_area.set_valign(gtk4::Align::Center);
        icon_area.set_overflow(gtk4::Overflow::Hidden);
        icon_area.add_css_class("project-icon-area");
        Self::update_icon_widget(&icon_area, name, icon_path);
        header_row.append(&icon_area);

        // Project name
        let name_label = gtk4::Label::builder()
            .label(name)
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .css_classes(["project-name"])
            .build();
        header_row.append(&name_label);

        // Memory label (hidden by default)
        let memory_label = gtk4::Label::builder()
            .css_classes(["caption", "dim-label", "resource-label"])
            .visible(false)
            .build();
        header_row.append(&memory_label);

        // Control buttons (visible on hover)
        let controls_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
        controls_box.set_opacity(0.0);

        let start_btn = gtk4::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text("Start all marked processes")
            .css_classes(["flat", "circular", "btn-play"])
            .build();

        let restart_btn = gtk4::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Restart all running processes")
            .css_classes(["flat", "circular"])
            .build();

        let stop_btn = gtk4::Button::builder()
            .icon_name("media-playback-stop-symbolic")
            .tooltip_text("Stop all")
            .css_classes(["flat", "circular", "btn-stop"])
            .build();

        controls_box.append(&start_btn);
        controls_box.append(&restart_btn);
        controls_box.append(&stop_btn);
        header_row.append(&controls_box);

        container.append(&header_row);

        // Hover controller to show/hide controls
        let hover = gtk4::EventControllerMotion::new();
        let controls_ref = controls_box.clone();
        hover.connect_enter(move |_, _, _| {
            controls_ref.set_opacity(1.0);
        });
        let controls_ref2 = controls_box.clone();
        hover.connect_leave(move |_| {
            controls_ref2.set_opacity(0.0);
        });
        header_row.add_controller(hover);

        // Collapsible content
        let revealer = gtk4::Revealer::builder()
            .reveal_child(true)
            .transition_type(gtk4::RevealerTransitionType::SlideDown)
            .build();

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content_box.add_css_class("project-content");
        revealer.set_child(Some(&content_box));
        container.add_css_class("project-container");
        container.add_css_class("project-expanded");
        container.append(&revealer);

        // Toggle on left-click only (button 1)
        let on_toggled: ToggleCallback = Rc::new(RefCell::new(None));
        let gesture = gtk4::GestureClick::builder().button(1).build();
        let revealer_ref = revealer.clone();
        let expander_icon_ref = expander_icon.clone();
        let name_label_ref = name_label.clone();
        let on_toggled_ref = on_toggled.clone();
        let container_ref = container.clone();
        gesture.connect_released(move |_, _, _, _| {
            let revealed = revealer_ref.reveals_child();
            let new_state = !revealed;
            revealer_ref.set_reveal_child(new_state);
            if new_state {
                expander_icon_ref.set_icon_name(Some("pan-down-symbolic"));
                container_ref.add_css_class("project-expanded");
            } else {
                expander_icon_ref.set_icon_name(Some("pan-end-symbolic"));
                container_ref.remove_css_class("project-expanded");
            }
            if let Some(ref cb) = *on_toggled_ref.borrow() {
                cb(&name_label_ref.label(), new_state);
            }
        });
        header_row.add_controller(gesture);

        // Right-click context menu
        let on_context_action: ActionCallback = Rc::new(RefCell::new(None));
        let popover = Self::build_context_menu(name, &on_context_action);
        popover.set_parent(&header_row);

        let right_click = gtk4::GestureClick::builder().button(3).build();
        let popover_ref = popover;
        right_click.connect_released(move |_, _, x, y| {
            popover_ref.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover_ref.popup();
        });
        header_row.add_controller(right_click);

        Self {
            container,
            header_row,
            expander_icon,
            icon_area,
            name_label,
            memory_label,
            revealer,
            content_box,
            controls_box,
            on_context_action,
            on_toggled,
        }
    }

    fn build_context_menu(_project_name: &str, on_action: &ActionCallback) -> gtk4::PopoverMenu {
        let menu = gio::Menu::new();

        let control_section = gio::Menu::new();
        control_section.append(Some("Start All"), Some("project.start_all"));
        control_section.append(Some("Stop All"), Some("project.stop_all"));
        control_section.append(Some("Restart All"), Some("project.restart_all"));
        menu.append_section(None, &control_section);

        let project_section = gio::Menu::new();
        project_section.append(Some("Open in Editor"), Some("project.open_in_editor"));
        project_section.append(Some("Edit Project"), Some("project.edit"));
        project_section.append(Some("Copy Path"), Some("project.copy_path"));
        menu.append_section(None, &project_section);

        let danger_section = gio::Menu::new();
        let remove_item = gio::MenuItem::new(None, None);
        remove_item.set_attribute_value("custom", Some(&"remove-button".to_variant()));
        danger_section.append_item(&remove_item);
        menu.append_section(None, &danger_section);

        let popover = gtk4::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);

        let remove_btn = gtk4::Button::builder()
            .label("Remove Project")
            .css_classes(["flat", "destructive-menu-item"])
            .build();
        popover.add_child(&remove_btn, "remove-button");

        let on_action_remove = on_action.clone();
        let popover_remove = popover.clone();
        remove_btn.connect_clicked(move |_| {
            popover_remove.popdown();
            if let Some(ref cb) = *on_action_remove.borrow() {
                cb("remove");
            }
        });

        let action_group = gio::SimpleActionGroup::new();

        let actions = [
            "start_all",
            "stop_all",
            "restart_all",
            "open_in_editor",
            "edit",
            "copy_path",
        ];
        for action_name in &actions {
            let on_action_ref = on_action.clone();
            let action_owned = action_name.to_string();
            let action = gio::SimpleAction::new(action_name, None);
            action.connect_activate(move |_, _| {
                if let Some(ref cb) = *on_action_ref.borrow() {
                    cb(&action_owned);
                }
            });
            action_group.add_action(&action);
        }

        popover.insert_action_group("project", Some(&action_group));
        popover
    }

    fn update_icon_widget(icon_area: &gtk4::Box, name: &str, icon_path: Option<&str>) {
        // Clear existing children
        while let Some(child) = icon_area.first_child() {
            icon_area.remove(&child);
        }

        if let Some(path) = icon_path {
            let image = gtk4::Image::from_file(path);
            image.set_pixel_size(24);
            icon_area.append(&image);
        } else {
            let initials = name.chars().take(2).collect::<String>().to_uppercase();
            let label = gtk4::Label::builder()
                .label(&initials)
                .css_classes(["project-icon"])
                .width_request(28)
                .height_request(28)
                .halign(gtk4::Align::Center)
                .valign(gtk4::Align::Center)
                .build();
            icon_area.append(&label);
        }
    }

    pub fn set_icon(&self, icon_path: Option<&str>) {
        let name = self.name_label.label().to_string();
        Self::update_icon_widget(&self.icon_area, &name, icon_path);
    }

    pub fn set_name(&self, name: &str) {
        self.name_label.set_label(name);
        // Update icon initials if no custom icon
        if self
            .icon_area
            .first_child()
            .and_then(|w| w.downcast::<gtk4::Image>().ok())
            .is_none()
        {
            Self::update_icon_widget(&self.icon_area, name, None);
        }
    }

    pub fn set_memory(&self, total_mb: f64) {
        if total_mb > 0.1 {
            let mem_str = if total_mb >= 1024.0 {
                format!("{:.1}GB", total_mb / 1024.0)
            } else {
                format!("{:.0}MB", total_mb)
            };
            self.memory_label.set_label(&mem_str);
            self.memory_label.set_visible(true);
        } else {
            self.memory_label.set_visible(false);
        }
    }

    pub fn content_box(&self) -> &gtk4::Box {
        &self.content_box
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }

    pub fn start_button(&self) -> gtk4::Button {
        self.controls_box
            .first_child()
            .and_then(|w| w.downcast::<gtk4::Button>().ok())
            .unwrap()
    }

    pub fn set_start_enabled(&self, enabled: bool) {
        let btn = self.start_button();
        btn.set_sensitive(enabled);
        if enabled {
            btn.set_tooltip_text(Some("Start all marked processes"));
        } else {
            btn.set_tooltip_text(Some(
                "No processes marked \"Start with project\" — toggle it on a process to enable",
            ));
        }
    }

    pub fn restart_button(&self) -> gtk4::Button {
        self.controls_box
            .first_child()
            .and_then(|w| w.next_sibling())
            .and_then(|w| w.downcast::<gtk4::Button>().ok())
            .unwrap()
    }

    pub fn stop_button(&self) -> gtk4::Button {
        self.controls_box
            .last_child()
            .and_then(|w| w.downcast::<gtk4::Button>().ok())
            .unwrap()
    }

    pub fn header_row(&self) -> &gtk4::Box {
        &self.header_row
    }

    pub fn set_on_context_action(&self, cb: impl Fn(&str) + 'static) {
        *self.on_context_action.borrow_mut() = Some(Box::new(cb));
    }

    pub fn set_on_toggled(&self, cb: impl Fn(&str, bool) + 'static) {
        *self.on_toggled.borrow_mut() = Some(Box::new(cb));
    }

    /// Set expanded state without triggering the on_toggled callback.
    pub fn set_expanded(&self, expanded: bool) {
        self.revealer.set_reveal_child(expanded);
        if expanded {
            self.expander_icon.set_icon_name(Some("pan-down-symbolic"));
            self.container.add_css_class("project-expanded");
        } else {
            self.expander_icon.set_icon_name(Some("pan-end-symbolic"));
            self.container.remove_css_class("project-expanded");
        }
    }

    pub fn is_expanded(&self) -> bool {
        self.revealer.reveals_child()
    }

    pub fn set_active(&self, active: bool) {
        if active {
            self.header_row.add_css_class("project-active");
        } else {
            self.header_row.remove_css_class("project-active");
        }
    }

    pub fn set_has_running(&self, has_running: bool) {
        if has_running {
            self.container.add_css_class("project-has-running");
        } else {
            self.container.remove_css_class("project-has-running");
        }
    }

    pub fn name(&self) -> String {
        self.name_label.label().to_string()
    }
}
