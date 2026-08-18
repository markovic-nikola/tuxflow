use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::ssh::parse_ssh_config;
use crate::remote::fs::list_remote_dirs;
use crate::ui::path_completion;

/// Debounce for path autocompletion — long enough to skip probing on every
/// keystroke, short enough to feel live over a warm ControlMaster connection.
const SUGGEST_DEBOUNCE_MS: u64 = 250;

pub struct AddRemoteProjectDialog;

impl AddRemoteProjectDialog {
    /// Collect host + remote path, verify them over ssh (worker thread,
    /// BatchMode so it never hangs on an auth prompt), then hand the
    /// validated pair to `on_verified`.
    pub fn show(parent: &impl IsA<gtk4::Widget>, on_verified: impl Fn(String, String) + 'static) {
        // Dialog heights: compact for the plain form, expanded while the
        // path-suggestion list is showing (it needs room for ~5 rows).
        const COMPACT_HEIGHT: i32 = 360;
        const EXPANDED_HEIGHT: i32 = 560;

        let on_verified = Rc::new(on_verified);
        let ssh_hosts = parse_ssh_config();

        let dialog = adw::Dialog::builder()
            .title("Add Remote Project")
            .content_width(450)
            .content_height(COMPACT_HEIGHT)
            .build();
        crate::ui::guard_dialog_maximize(&dialog);

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        headerbar.set_show_start_title_buttons(false);
        headerbar.set_show_end_title_buttons(false);

        let cancel_btn = gtk4::Button::builder().label("Cancel").build();
        headerbar.pack_start(&cancel_btn);

        let add_btn = gtk4::Button::builder()
            .label("Add")
            .css_classes(["suggested-action"])
            .sensitive(false)
            .build();
        headerbar.pack_end(&add_btn);

        toolbar_view.add_top_bar(&headerbar);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        // SSH config host picker
        let host_picker_group = adw::PreferencesGroup::new();
        let mut picker_labels = vec!["Custom...".to_string()];
        picker_labels.extend(ssh_hosts.iter().map(|h| h.name.clone()));
        let picker_list =
            gtk4::StringList::new(&picker_labels.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let host_picker_row = adw::ComboRow::builder()
            .title("SSH Config Host")
            .subtitle("Pick from ~/.ssh/config or enter custom")
            .model(&picker_list)
            .build();
        host_picker_group.add(&host_picker_row);
        content.append(&host_picker_group);

        // Host + remote path
        let fields_group = adw::PreferencesGroup::new();
        fields_group.set_margin_top(12);

        let host_row = adw::EntryRow::builder()
            .title("Host (alias or user@host)")
            .build();
        fields_group.add(&host_row);

        let path_row = adw::EntryRow::builder()
            .title("Remote directory (absolute path)")
            .build();
        fields_group.add(&path_row);

        content.append(&fields_group);

        // Directory suggestions, filled asynchronously while typing the path.
        // The scroll cap fits ~5 rows; the dialog grows to EXPANDED_HEIGHT
        // while they're shown so the list isn't clipped by the dialog edge.
        let suggestions_list = gtk4::ListBox::new();
        suggestions_list.add_css_class("boxed-list");
        suggestions_list.set_selection_mode(gtk4::SelectionMode::None);
        let suggestions_scroll = gtk4::ScrolledWindow::builder()
            .child(&suggestions_list)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(210)
            .margin_top(6)
            .visible(false)
            .build();
        content.append(&suggestions_scroll);

        // Status / error label
        let status_label = gtk4::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .margin_top(12)
            .css_classes(["dim-label", "caption"])
            .visible(false)
            .build();
        content.append(&status_label);

        // Enable Add only with a host and an absolute path
        let validate = {
            let add_btn = add_btn.clone();
            let host_row = host_row.clone();
            let path_row = path_row.clone();
            move || {
                let host_ok = !host_row.text().trim().is_empty();
                let path_ok = path_row.text().trim().starts_with('/');
                add_btn.set_sensitive(host_ok && path_ok);
            }
        };
        let v = validate.clone();
        host_row.connect_changed(move |_| v());
        let v = validate.clone();
        path_row.connect_changed(move |_| v());

        // Path autocompletion. Only fires once the host field is filled —
        // over the warm ControlMaster connection each probe is a few tens
        // of ms.
        {
            let host_row = host_row.clone();
            let scroll = suggestions_scroll.clone();
            let dialog = dialog.clone();
            path_completion::attach(
                &path_row,
                &suggestions_list,
                Duration::from_millis(SUGGEST_DEBOUNCE_MS),
                move |prefix| {
                    let host = host_row.text().trim().to_string();
                    if host.is_empty() || !prefix.starts_with('/') {
                        return None;
                    }
                    Some(move || list_remote_dirs(&host, &prefix))
                },
                move |dirs| {
                    scroll.set_visible(!dirs.is_empty());
                    dialog.set_content_height(if dirs.is_empty() {
                        COMPACT_HEIGHT
                    } else {
                        EXPANDED_HEIGHT
                    });
                },
            );
        }

        // Clicking a suggestion fills the path field (which re-triggers the
        // completion one level deeper).
        {
            let path_row = path_row.clone();
            suggestions_list.connect_row_activated(move |_, row| {
                if let Some(dir) = path_completion::row_path(row) {
                    path_row.set_text(&dir);
                    path_row.set_position(-1);
                    path_row.grab_focus();
                }
            });
        }

        // Picking a config host fills the host field with the alias —
        // ssh resolves the alias itself, preserving ProxyJump/identity/etc.
        let host_row_ref = host_row.clone();
        let ssh_hosts_ref = ssh_hosts.clone();
        host_picker_row.connect_selected_notify(move |picker| {
            let idx = picker.selected() as usize;
            if idx == 0 {
                host_row_ref.set_text("");
            } else if let Some(ssh_host) = ssh_hosts_ref.get(idx - 1) {
                host_row_ref.set_text(&ssh_host.name);
            }
        });

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });

        // Verify on a worker thread, then hand off. One in-flight check max.
        let verifying: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
        let dialog_ref = dialog.clone();
        add_btn.connect_clicked(move |btn| {
            if *verifying.borrow() {
                return;
            }
            let host = host_row.text().trim().to_string();
            let dir = path_row.text().trim().trim_end_matches('/').to_string();
            if host.is_empty() || !dir.starts_with('/') {
                return;
            }

            *verifying.borrow_mut() = true;
            btn.set_sensitive(false);
            status_label.set_visible(true);
            status_label.remove_css_class("error");
            status_label.set_text(&format!("Connecting to {host}…"));

            let verifying = verifying.clone();
            let btn = btn.clone();
            let status_label = status_label.clone();
            let dialog_ref = dialog_ref.clone();
            let on_verified = on_verified.clone();
            let probe_host = host.clone();
            let probe_dir = dir.clone();
            crate::util::worker::run(
                move || crate::remote::fs::remote_dir_exists(&probe_host, &probe_dir),
                move |result| {
                    *verifying.borrow_mut() = false;
                    btn.set_sensitive(true);
                    match result {
                        Ok(true) => {
                            on_verified(host.clone(), dir.clone());
                            dialog_ref.close();
                        }
                        Ok(false) => {
                            status_label.add_css_class("error");
                            status_label.set_text(&format!("No such directory on {host}: {dir}"));
                        }
                        Err(e) => {
                            status_label.add_css_class("error");
                            status_label.set_text(&format!(
                                "{e}\n\nIf this host needs a password or first-time host-key \
                                 confirmation, connect once in a terminal (ssh {host}), then retry."
                            ));
                        }
                    }
                },
            );
        });

        dialog.present(Some(parent));
    }
}
