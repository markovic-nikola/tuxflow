use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;

pub struct StatusBar {
    container: gtk4::Box,
    update_btn: gtk4::Button,
    process_label: gtk4::Label,
    separator_label: gtk4::Label,
    global_label: gtk4::Label,
    status_label: gtk4::Label,
    cpu_label: gtk4::Label,
    memory_label: gtk4::Label,
    follow_btn: gtk4::Button,
    focus_btn: gtk4::Button,
    git_btn: gtk4::Button,
    git_pull_dot: gtk4::DrawingArea,
    git_behind: Cell<usize>,
    git_dirty: Cell<usize>,
    browser_btn: gtk4::Button,
    clear_btn: gtk4::Button,
    stop_btn: gtk4::Button,
    restart_btn: gtk4::Button,
    following: Rc<RefCell<bool>>,
    url: Rc<RefCell<Option<String>>>,
}

impl StatusBar {
    pub fn new() -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        container.add_css_class("status-bar");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_top(4);
        container.set_margin_bottom(4);
        container.set_valign(gtk4::Align::Center);

        // Left side: resource info + process info
        let info_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);

        let cpu_label = gtk4::Label::builder()
            .label("")
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();
        info_box.append(&cpu_label);

        let memory_label = gtk4::Label::builder()
            .label("")
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();
        info_box.append(&memory_label);

        let process_label = gtk4::Label::builder()
            .label("")
            .css_classes(["caption"])
            .visible(false)
            .build();
        info_box.append(&process_label);

        let separator_label = gtk4::Label::builder()
            .label("\u{00b7}")
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();
        info_box.append(&separator_label);

        let global_label = gtk4::Label::builder()
            .label("")
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();
        info_box.append(&global_label);

        // Update available button (hidden by default)
        let update_btn = gtk4::Button::builder()
            .label("Update available")
            .css_classes(["flat", "caption", "update-label"])
            .visible(false)
            .build();
        info_box.append(&update_btn);

        let status_label = gtk4::Label::builder()
            .label("")
            .css_classes(["caption", "dim-label"])
            .build();
        info_box.append(&status_label);

        container.append(&info_box);

        // Spacer
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        container.append(&spacer);

        // Right side: action buttons
        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);

        let focus_btn = Self::make_button("Focus", "focus-windows-symbolic");
        let follow_btn = Self::make_button("Follow Output", "go-bottom-symbolic");
        let git_btn = Self::make_button("Git Changes", "send-to-symbolic");

        let git_pull_dot = gtk4::DrawingArea::builder()
            .visible(false)
            .content_width(8)
            .content_height(8)
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Start)
            .margin_top(5)
            .can_target(false)
            .build();
        git_pull_dot.set_draw_func(|_, cr, w, h| {
            cr.set_source_rgb(0.824, 0.600, 0.133); // #d29922
            cr.arc(
                w as f64 / 2.0,
                h as f64 / 2.0,
                4.0,
                0.0,
                2.0 * std::f64::consts::PI,
            );
            let _ = cr.fill();
        });

        let git_box = gtk4::Overlay::new();
        git_box.set_child(Some(&git_btn));
        git_box.add_overlay(&git_pull_dot);
        git_box.set_visible(false);

        let browser_btn = Self::make_button("Open in Browser", "external-link-symbolic");
        browser_btn.set_visible(false);
        let clear_btn = Self::make_button("Clear", "edit-clear-symbolic");
        let stop_btn = Self::make_button("Stop", "media-playback-stop-symbolic");
        stop_btn.add_css_class("btn-stop");
        let restart_btn = Self::make_button("Restart", "view-refresh-symbolic");

        actions.append(&git_box);
        actions.append(&focus_btn);
        actions.append(&follow_btn);
        actions.append(&browser_btn);
        actions.append(&clear_btn);
        actions.append(&stop_btn);
        actions.append(&restart_btn);

        container.append(&actions);

        let following = Rc::new(RefCell::new(true));
        let url: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        // Follow button toggle
        let following_ref = following.clone();
        let follow_btn_ref = follow_btn.clone();
        follow_btn.connect_clicked(move |_| {
            let mut f = following_ref.borrow_mut();
            *f = !*f;
            if *f {
                follow_btn_ref.set_icon_name("go-bottom-symbolic");
                follow_btn_ref.set_tooltip_text(Some("Follow Output"));
            } else {
                follow_btn_ref.set_icon_name("media-playback-pause-symbolic");
                follow_btn_ref.set_tooltip_text(Some("Paused — Click to Follow"));
            }
        });

        // Browser button opens the stored URL
        let url_ref = url.clone();
        browser_btn.connect_clicked(move |btn| {
            if let Some(ref url_str) = *url_ref.borrow() {
                let launcher = gtk4::UriLauncher::new(url_str);
                let window = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
                launcher.launch(window.as_ref(), gtk4::gio::Cancellable::NONE, |_| {});
            }
        });

        Self {
            container,
            update_btn,
            process_label,
            separator_label,
            global_label,
            status_label,
            cpu_label,
            memory_label,
            follow_btn,
            focus_btn,
            git_btn,
            git_pull_dot,
            git_behind: Cell::new(0),
            git_dirty: Cell::new(0),
            browser_btn,
            clear_btn,
            stop_btn,
            restart_btn,
            following,
            url,
        }
    }

    fn make_button(label: &str, icon: &str) -> gtk4::Button {
        gtk4::Button::builder()
            .icon_name(icon)
            .tooltip_text(label)
            .css_classes(["flat", "circular"])
            .build()
    }

    pub fn set_project_info(&self, project_name: Option<&str>, running: usize, total: usize) {
        match project_name {
            Some(name) if total > 0 => {
                self.process_label
                    .set_label(&format!("{name} {running}/{total}"));
                self.process_label.set_visible(true);
            }
            Some(name) => {
                self.process_label.set_label(name);
                self.process_label.set_visible(true);
            }
            None => {
                self.process_label.set_visible(false);
            }
        }
    }

    pub fn set_global_info(
        &self,
        running: usize,
        total: usize,
        has_project: bool,
        running_names: &[(String, Vec<String>)],
    ) {
        if total > 0 {
            self.global_label
                .set_label(&format!("Total {running}/{total}"));
            self.global_label.set_visible(true);
            self.separator_label.set_visible(has_project);

            if running > 0 {
                let tooltip: Vec<String> = running_names
                    .iter()
                    .filter(|(_, procs)| !procs.is_empty())
                    .map(|(project, procs)| {
                        let list = procs.join(", ");
                        format!("{project}: {list}")
                    })
                    .collect();
                self.global_label
                    .set_tooltip_text(Some(&tooltip.join("\n")));
            } else {
                self.global_label.set_tooltip_text(None);
            }
        } else {
            self.global_label.set_visible(false);
            self.separator_label.set_visible(false);
        }
    }

    pub fn set_resource_info(&self, cpu_percent: f64, memory_mb: f64) {
        self.cpu_label.set_label(&format!("CPU {cpu_percent:.1}%"));
        self.cpu_label.set_visible(true);

        let mem_str = if memory_mb >= 1024.0 {
            format!("MEM {:.1}GB", memory_mb / 1024.0)
        } else {
            format!("MEM {:.0}MB", memory_mb)
        };
        self.memory_label.set_label(&mem_str);
        self.memory_label.set_visible(true);
    }

    pub fn clear_resource_info(&self) {
        self.cpu_label.set_visible(false);
        self.memory_label.set_visible(false);
    }

    pub fn is_following(&self) -> bool {
        *self.following.borrow()
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

    pub fn connect_focus(&self, cb: impl Fn() + 'static) {
        self.focus_btn.connect_clicked(move |_| cb());
    }

    pub fn set_url(&self, url: Option<&str>) {
        match url {
            Some(u) => {
                *self.url.borrow_mut() = Some(u.to_string());
                self.browser_btn.set_visible(true);
                self.browser_btn
                    .set_tooltip_text(Some(&format!("Open {u}")));
            }
            None => {
                *self.url.borrow_mut() = None;
                self.browser_btn.set_visible(false);
            }
        }
    }

    pub fn show_update(&self, version: &str, url: &str) {
        self.update_btn
            .set_label(&format!("Update available: v{version}"));
        self.update_btn
            .set_tooltip_text(Some("Click to download the latest version"));
        self.update_btn.set_visible(true);

        let release_url = url.to_string();
        self.update_btn.connect_clicked(move |btn| {
            let launcher = gtk4::UriLauncher::new(&release_url);
            let window = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            launcher.launch(window.as_ref(), gtk4::gio::Cancellable::NONE, |_| {});
        });
    }

    pub fn set_git_available(&self, available: bool) {
        if let Some(parent) = self.git_btn.parent() {
            parent.set_visible(available);
        }
    }

    pub fn set_git_pull_indicator(&self, behind: usize) {
        self.git_behind.set(behind);
        self.git_pull_dot.set_visible(behind > 0);
        self.update_git_tooltip();
    }

    pub fn set_git_dirty(&self, dirty: usize) {
        self.git_dirty.set(dirty);
        if dirty > 0 {
            self.git_btn.add_css_class("git-dirty");
        } else {
            self.git_btn.remove_css_class("git-dirty");
        }
        self.update_git_tooltip();
    }

    fn update_git_tooltip(&self) {
        let behind = self.git_behind.get();
        let dirty = self.git_dirty.get();
        let tip = match (dirty, behind) {
            (0, 0) => "Git Changes".to_string(),
            (d, 0) => format!("Git Changes ({d} uncommitted)"),
            (0, b) => format!("Git Changes ({b} to pull)"),
            (d, b) => format!("Git Changes ({d} uncommitted, {b} to pull)"),
        };
        self.git_btn.set_tooltip_text(Some(&tip));
    }

    pub fn connect_git_changes(&self, cb: impl Fn(&gtk4::Button) + 'static) {
        self.git_btn.connect_clicked(move |btn| cb(btn));
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }
}
