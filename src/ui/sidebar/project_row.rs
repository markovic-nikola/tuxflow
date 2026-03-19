use gtk4::prelude::*;

pub struct ProjectRow {
    container: gtk4::Box,
    expander_icon: gtk4::Image,
    revealer: gtk4::Revealer,
    content_box: gtk4::Box,
    controls_box: gtk4::Box,
}

impl ProjectRow {
    pub fn new(name: &str) -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

        // Project header row
        let header_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        header_row.set_margin_start(8);
        header_row.set_margin_end(8);
        header_row.set_margin_top(8);
        header_row.set_margin_bottom(4);
        header_row.add_css_class("project-row");

        let expander_icon = gtk4::Image::from_icon_name("pan-down-symbolic");
        header_row.append(&expander_icon);

        // Project icon (first 2 letters)
        let initials = name
            .chars()
            .take(2)
            .collect::<String>()
            .to_uppercase();
        let icon_label = gtk4::Label::builder()
            .label(&initials)
            .css_classes(["caption-heading"])
            .width_request(24)
            .build();
        header_row.append(&icon_label);

        // Project name
        let name_label = gtk4::Label::builder()
            .label(name)
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .css_classes(["heading"])
            .build();
        header_row.append(&name_label);

        // Control buttons (visible on hover)
        let controls_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
        controls_box.set_visible(false);

        let start_btn = gtk4::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text("Start auto-start")
            .css_classes(["flat", "circular"])
            .build();

        let restart_btn = gtk4::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Restart all")
            .css_classes(["flat", "circular"])
            .build();

        let stop_btn = gtk4::Button::builder()
            .icon_name("media-playback-stop-symbolic")
            .tooltip_text("Stop all")
            .css_classes(["flat", "circular"])
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
            controls_ref.set_visible(true);
        });
        let controls_ref2 = controls_box.clone();
        hover.connect_leave(move |_| {
            controls_ref2.set_visible(false);
        });
        header_row.add_controller(hover);

        // Collapsible content
        let revealer = gtk4::Revealer::builder()
            .reveal_child(true)
            .transition_type(gtk4::RevealerTransitionType::SlideDown)
            .build();

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        revealer.set_child(Some(&content_box));
        container.append(&revealer);

        // Toggle on click
        let gesture = gtk4::GestureClick::new();
        let revealer_ref = revealer.clone();
        let expander_icon_ref = expander_icon.clone();
        gesture.connect_released(move |_, _, _, _| {
            let revealed = revealer_ref.reveals_child();
            revealer_ref.set_reveal_child(!revealed);
            if revealed {
                expander_icon_ref.set_icon_name(Some("pan-end-symbolic"));
            } else {
                expander_icon_ref.set_icon_name(Some("pan-down-symbolic"));
            }
        });
        header_row.add_controller(gesture);

        Self {
            container,
            expander_icon,
            revealer,
            content_box,
            controls_box,
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
}
