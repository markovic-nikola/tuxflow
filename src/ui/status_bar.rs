use gtk4::prelude::*;

pub struct StatusBar {
    container: gtk4::Box,
    process_label: gtk4::Label,
    status_label: gtk4::Label,
    stop_btn: gtk4::Button,
    restart_btn: gtk4::Button,
    clear_btn: gtk4::Button,
}

impl StatusBar {
    pub fn new() -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        container.add_css_class("status-bar");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_top(4);
        container.set_margin_bottom(4);

        // Left side: action buttons
        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);

        let focus_btn = Self::make_button("Focus", "focus-windows-symbolic");
        let clear_btn = Self::make_button("Clear", "edit-clear-symbolic");
        let stop_btn = Self::make_button("Stop", "media-playback-stop-symbolic");
        let restart_btn = Self::make_button("Restart", "view-refresh-symbolic");

        actions.append(&focus_btn);
        actions.append(&clear_btn);
        actions.append(&stop_btn);
        actions.append(&restart_btn);

        container.append(&actions);

        // Spacer
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        container.append(&spacer);

        // Right side: process info + status
        let right_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);

        let process_label = gtk4::Label::builder()
            .label("Ready")
            .css_classes(["caption"])
            .build();
        right_box.append(&process_label);

        let status_label = gtk4::Label::builder()
            .label("")
            .css_classes(["caption", "dim-label"])
            .build();
        right_box.append(&status_label);

        container.append(&right_box);

        Self {
            container,
            process_label,
            status_label,
            stop_btn,
            restart_btn,
            clear_btn,
        }
    }

    fn make_button(label: &str, icon: &str) -> gtk4::Button {
        gtk4::Button::builder()
            .icon_name(icon)
            .tooltip_text(label)
            .css_classes(["flat", "circular"])
            .build()
    }

    pub fn set_process_info(&self, name: &str, running: usize, total: usize) {
        if total > 0 {
            self.process_label
                .set_label(&format!("{name} \u{2014} {running}/{total}"));
        } else {
            self.process_label.set_label(name);
        }
    }

    pub fn connect_stop(&self, cb: impl Fn() + 'static) {
        self.stop_btn.connect_clicked(move |_| cb());
    }

    pub fn connect_restart(&self, cb: impl Fn() + 'static) {
        self.restart_btn.connect_clicked(move |_| cb());
    }

    pub fn connect_clear(&self, cb: impl Fn() + 'static) {
        self.clear_btn.connect_clicked(move |_| cb());
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }
}
