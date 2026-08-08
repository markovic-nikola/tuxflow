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
    git_sync_btn: gtk4::Button,
    git_sync_spinner: gtk4::Spinner,
    git_branch_label: gtk4::Label,
    git_sync_label: gtk4::Label,
    diff_added_label: gtk4::Label,
    diff_removed_label: gtk4::Label,
    git_available: Cell<bool>,
    git_ahead: Cell<usize>,
    git_behind: Cell<usize>,
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
        // Bundled copy of Adwaita's folder-remote-symbolic — app-namespaced
        // so the user's icon theme can't override the glyph.
        let remote_icon = gtk4::Image::from_icon_name("tuxflow-remote-symbolic");
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

        // Sync chip: branch name + ↓↑ counters. One click pulls (ff-only)
        // and pushes; a spinner replaces the counters while syncing.
        let git_sync_spinner = gtk4::Spinner::builder().visible(false).build();
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
        let sync_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
        sync_content.append(&git_sync_spinner);
        sync_content.append(&git_branch_label);
        sync_content.append(&git_sync_label);
        let git_sync_btn = gtk4::Button::builder()
            .child(&sync_content)
            .tooltip_text("Pull & Push")
            .css_classes(["flat", "status-chip"])
            .visible(false)
            .build();

        // Changes chip: icon + working-tree "+N −M" line counts. Click
        // opens the Git Changes dialog. Filled by the periodic git poll.
        let git_icon = gtk4::Image::from_icon_name("send-to-symbolic");
        let diff_added_label = gtk4::Label::builder()
            .label("")
            .visible(false)
            .css_classes(["caption", "diff-added"])
            .build();
        let diff_removed_label = gtk4::Label::builder()
            .label("")
            .visible(false)
            .css_classes(["caption", "diff-removed"])
            .build();
        let git_btn_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
        git_btn_content.append(&git_icon);
        git_btn_content.append(&diff_added_label);
        git_btn_content.append(&diff_removed_label);
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

        actions.append(&git_sync_btn);
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
            git_sync_btn,
            git_sync_spinner,
            git_branch_label,
            git_sync_label,
            diff_added_label,
            diff_removed_label,
            git_available: Cell::new(false),
            git_ahead: Cell::new(0),
            git_behind: Cell::new(0),
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

    pub fn show_update(&self, update: &crate::util::update_checker::UpdateInfo) {
        self.update_btn
            .set_label(&format!("Update available: v{}", update.latest_version));
        self.update_btn
            .set_tooltip_text(Some("See what changed and install"));
        self.update_btn.set_visible(true);

        let info = update.clone();
        self.update_btn.connect_clicked(move |btn| {
            let window = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            crate::ui::update_dialog::present(window.as_ref(), &info);
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
            branch: self.git_branch_label.get_visible().then(|| {
                // Label carries a "⎇ " display prefix — strip it back off.
                let label = self.git_branch_label.label();
                label
                    .strip_prefix("\u{2387} ")
                    .unwrap_or(label.as_str())
                    .to_string()
            }),
        }
    }

    pub fn set_git_available(&self, available: bool) {
        self.git_available.set(available);
        self.git_btn.set_visible(available);
        self.update_sync_visibility();
    }

    /// Sync chip needs both git and a branch (detached HEAD can't pull/push).
    fn update_sync_visibility(&self) {
        self.git_sync_btn
            .set_visible(self.git_available.get() && self.git_branch_label.get_visible());
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
        let tip = match (ahead, behind) {
            (0, 0) => "Pull & Push (in sync \u{2014} click to fetch)".to_string(),
            (a, 0) => format!("Pull & Push ({a} to push)"),
            (0, b) => format!("Pull & Push ({b} to pull)"),
            (a, b) => format!("Pull & Push ({a} to push, {b} to pull)"),
        };
        self.git_sync_btn.set_tooltip_text(Some(&tip));
    }

    /// Working-tree stats on the changes chip: "+N −M" line counts, amber
    /// icon tint while dirty. Untracked files don't show in line counts
    /// (git diff can't see them) — they go in the tooltip.
    /// Compact count for the chip labels: 931, 1.2K, 45K. Exact numbers
    /// stay in the tooltip.
    fn compact_count(n: usize) -> String {
        match n {
            0..=999 => n.to_string(),
            1_000..=9_999 => {
                let k = (n as f64 / 100.0).round() / 10.0;
                if k.fract() == 0.0 {
                    format!("{}K", k as usize)
                } else {
                    format!("{k:.1}K")
                }
            }
            _ => format!("{}K", (n as f64 / 1000.0).round() as usize),
        }
    }

    pub fn set_git_diffstat(&self, files: usize, added: usize, removed: usize, untracked: usize) {
        self.diff_added_label
            .set_label(&format!("+{}", Self::compact_count(added)));
        self.diff_added_label.set_visible(added > 0);
        self.diff_removed_label
            .set_label(&format!("\u{2212}{}", Self::compact_count(removed)));
        self.diff_removed_label.set_visible(removed > 0);
        let mut parts = Vec::new();
        if files > 0 {
            parts.push(format!("{files} files: +{added} \u{2212}{removed}"));
        }
        if untracked > 0 {
            parts.push(format!("{untracked} untracked"));
        }
        let tip = if parts.is_empty() {
            "Git Changes".to_string()
        } else {
            format!("Git Changes ({})", parts.join(", "))
        };
        self.git_btn.set_tooltip_text(Some(&tip));
    }

    /// Spinner + insensitive while the one-click sync runs.
    pub fn set_git_syncing(&self, syncing: bool) {
        self.git_sync_spinner.set_visible(syncing);
        self.git_sync_spinner.set_spinning(syncing);
        self.git_sync_btn.set_sensitive(!syncing);
    }

    pub fn connect_git_sync(&self, cb: impl Fn(&gtk4::Button) + 'static) {
        self.git_sync_btn.connect_clicked(move |btn| cb(btn));
    }

    /// Show the current branch on the sync chip. `None` (detached HEAD, or
    /// branch not yet known) hides the whole chip — nothing to pull/push.
    pub fn set_git_branch(&self, branch: Option<&str>) {
        match branch {
            Some(b) => {
                self.git_branch_label.set_label(&format!("\u{2387} {b}"));
                self.git_branch_label.set_visible(true);
            }
            None => self.git_branch_label.set_visible(false),
        }
        self.update_sync_visibility();
    }

    pub fn connect_git_changes(&self, cb: impl Fn(&gtk4::Button) + 'static) {
        self.git_btn.connect_clicked(move |btn| cb(btn));
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }
}
