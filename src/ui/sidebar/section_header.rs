use gtk4::prelude::*;

pub struct SectionHeader {
    container: gtk4::Box,
    count_label: gtk4::Label,
    revealer: gtk4::Revealer,
    content_box: gtk4::Box,
}

impl SectionHeader {
    pub fn new(title: &str, icon_name: &str) -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

        // Header row (clickable to expand/collapse)
        let header_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        header_row.set_margin_start(0);
        header_row.set_margin_end(12);
        header_row.set_margin_top(8);
        header_row.set_margin_bottom(4);

        let expander_icon = gtk4::Image::from_icon_name("pan-down-symbolic");
        expander_icon.add_css_class("dim-label");
        header_row.append(&expander_icon);

        let icon = gtk4::Image::from_icon_name(icon_name);
        icon.add_css_class("dim-label");
        header_row.append(&icon);

        let title_label = gtk4::Label::builder()
            .label(title)
            .css_classes(["caption-heading", "dim-label"])
            .build();
        header_row.append(&title_label);

        // Separator line
        let separator = gtk4::Separator::new(gtk4::Orientation::Horizontal);
        separator.set_hexpand(true);
        separator.set_valign(gtk4::Align::Center);
        header_row.append(&separator);

        // Running/total count
        let count_label = gtk4::Label::builder()
            .label("0/0")
            .css_classes(["caption", "dim-label"])
            .build();
        header_row.append(&count_label);

        container.append(&header_row);

        // Content (collapsible)
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
            count_label,
            revealer,
            content_box,
        }
    }

    pub fn set_count(&self, running: usize, total: usize) {
        self.count_label.set_label(&format!("{running}/{total}"));
    }

    pub fn content_box(&self) -> &gtk4::Box {
        &self.content_box
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }
}
