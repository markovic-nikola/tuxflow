use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;

use crate::ui::git_changes_dialog::GitSeed;

pub struct StatusBar {
    container: gtk4::Box,
    remote_icon: gtk4::Image,
    update_btn: gtk4::Button,
    process_label: gtk4::Label,
    separator_label: gtk4::Label,
    global_label: gtk4::Label,
    status_label: gtk4::Label,
    focus_btn: gtk4::Button,
    git_btn: gtk4::Button,
    git_branch_label: gtk4::Label,
    git_sync_label: gtk4::Label,
    git_ahead: Cell<usize>,
    git_behind: Cell<usize>,
    git_dirty: Cell<usize>,
    browser_btn: gtk4::Button,
    clear_btn: gtk4::Button,
    stop_btn: gtk4::Button,
    restart_btn: gtk4::Button,
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

        // Remote-project indicator: shown when the active project lives on
        // an ssh host; tooltip carries host:dir.
        let remote_icon = gtk4::Image::from_icon_name("folder-remote-symbolic");
        remote_icon.set_pixel_size(14);
        remote_icon.add_css_class("dim-label");
        remote_icon.set_visible(false);
        info_box.append(&remote_icon);

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

        // Git button: icon + current branch + ahead/behind counters, all one
        // clickable chip. Filled by the periodic git poll (set_git_branch /
        // set_git_sync); the counters hide when the branch is in sync.
        let git_icon = gtk4::Image::from_icon_name("send-to-symbolic");
        let git_branch_label = gtk4::Label::builder()
            .label("")
            .visible(false)
            .ellipsize(gtk4::pango::EllipsizeMode::Middle)
            .max_width_chars(20)
            .css_classes(["caption"])
            .build();
        let git_sync_label = gtk4::Label::builder()
            .label("")
            .visible(false)
            .use_markup(true)
            .css_classes(["caption"])
            .build();
        let git_btn_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
        git_btn_content.append(&git_icon);
        git_btn_content.append(&git_branch_label);
        git_btn_content.append(&git_sync_label);
        let git_btn = gtk4::Button::builder()
            .child(&git_btn_content)
            .tooltip_text("Git Changes")
            .css_classes(["flat", "status-chip"])
            .visible(false)
            .build();

        let browser_btn = Self::make_button("Open in Browser", "external-link-symbolic");
        browser_btn.set_visible(false);
        let clear_btn = Self::make_button("Clear", "edit-clear-symbolic");
        let stop_btn = Self::make_button("Stop", "media-playback-stop-symbolic");
        stop_btn.add_css_class("btn-stop");
        let restart_btn = Self::make_button("Restart", "view-refresh-symbolic");

        actions.append(&git_btn);
        actions.append(&focus_btn);
        actions.append(&browser_btn);
        actions.append(&clear_btn);
        actions.append(&stop_btn);
        actions.append(&restart_btn);

        container.append(&actions);

        let url: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

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
            remote_icon,
            update_btn,
            process_label,
            separator_label,
            global_label,
            status_label,
            focus_btn,
            git_btn,
            git_branch_label,
            git_sync_label,
            git_ahead: Cell::new(0),
            git_behind: Cell::new(0),
            git_dirty: Cell::new(0),
            browser_btn,
            clear_btn,
            stop_btn,
            restart_btn,
            url,
        }
    }

    fn make_button(label: &str, icon: &str) -> gtk4::Button {
        gtk4::Button::builder()
            .icon_name(icon)
            .tooltip_text(label)
            .css_classes(["flat", "status-chip"])
            .build()
    }

    /// Show/hide the remote-project indicator. `Some(hint)` = remote,
    /// with `host:dir` as tooltip; `None` = local project or nothing open.
    pub fn set_remote_hint(&self, hint: Option<&str>) {
        self.remote_icon.set_visible(hint.is_some());
        self.remote_icon.set_tooltip_text(hint);
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

    /// Show the Stop button only when the selected process is running.
    /// When no process is selected, or it is stopped/crashed, hide it —
    /// you can't stop something that isn't running.
    pub fn set_process_running(&self, running: bool) {
        self.stop_btn.set_visible(running);
    }

    /// Last-known sync state, for seeding the Git Changes dialog so it
    /// opens with the Push/Pull buttons already correct.
    pub fn git_seed(&self) -> GitSeed {
        GitSeed {
            ahead: self.git_ahead.get(),
            behind: self.git_behind.get(),
            branch: self
                .git_branch_label
                .is_visible()
                .then(|| self.git_branch_label.label().to_string()),
        }
    }

    pub fn set_git_available(&self, available: bool) {
        self.git_btn.set_visible(available);
    }

    /// Show commits to push (↑, green) / pull (↓, amber) inside the git
    /// chip. Both 0 = in sync, counters hidden.
    pub fn set_git_sync(&self, ahead: usize, behind: usize) {
        self.git_ahead.set(ahead);
        self.git_behind.set(behind);
        if ahead == 0 && behind == 0 {
            self.git_sync_label.set_visible(false);
        } else {
            let mut parts = Vec::new();
            if behind > 0 {
                parts.push(format!(
                    "<span foreground='#d29922'>\u{2193}{behind}</span>"
                ));
            }
            if ahead > 0 {
                parts.push(format!("<span foreground='#73c991'>\u{2191}{ahead}</span>"));
            }
            self.git_sync_label.set_markup(&parts.join(" "));
            self.git_sync_label.set_visible(true);
        }
        self.update_git_tooltip();
    }

    /// Show the current branch inside the git chip. `None` (detached HEAD,
    /// or branch not yet known) hides the label, leaving just the icon.
    pub fn set_git_branch(&self, branch: Option<&str>) {
        match branch {
            Some(b) => {
                self.git_branch_label.set_label(b);
                self.git_branch_label.set_visible(true);
            }
            None => self.git_branch_label.set_visible(false),
        }
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
        let mut parts = Vec::new();
        let dirty = self.git_dirty.get();
        let ahead = self.git_ahead.get();
        let behind = self.git_behind.get();
        if dirty > 0 {
            parts.push(format!("{dirty} uncommitted"));
        }
        if ahead > 0 {
            parts.push(format!("{ahead} to push"));
        }
        if behind > 0 {
            parts.push(format!("{behind} to pull"));
        }
        let tip = if parts.is_empty() {
            "Git Changes".to_string()
        } else {
            format!("Git Changes ({})", parts.join(", "))
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
