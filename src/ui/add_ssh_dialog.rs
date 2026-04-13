use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::schema::{ProcessCategory, ProcessConfig};
use crate::config::ssh::{SshHost, parse_ssh_config};

pub struct AddSshDialog;

impl AddSshDialog {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        project_names: &[String],
        last_project: Option<&str>,
        on_add: impl Fn(&str, ProcessConfig) + 'static,
    ) {
        let ssh_hosts = parse_ssh_config();

        let dialog = adw::Dialog::builder()
            .title("New SSH Connection")
            .content_width(450)
            .content_height(480)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        headerbar.set_show_start_title_buttons(false);
        headerbar.set_show_end_title_buttons(false);

        let cancel_btn = gtk4::Button::builder().label("Cancel").build();
        headerbar.pack_start(&cancel_btn);

        let add_btn = gtk4::Button::builder()
            .label("Add")
            .css_classes(["suggested-action"])
            .build();
        headerbar.pack_end(&add_btn);

        toolbar_view.add_top_bar(&headerbar);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        // Project selector
        let project_group = adw::PreferencesGroup::new();
        let project_list =
            gtk4::StringList::new(&project_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let project_row = adw::ComboRow::builder()
            .title("Project")
            .model(&project_list)
            .build();
        if let Some(last) = last_project
            && let Some(idx) = project_names.iter().position(|n| n == last)
        {
            project_row.set_selected(idx as u32);
        }
        project_group.add(&project_row);
        content.append(&project_group);

        // SSH config host picker
        let host_picker_group = adw::PreferencesGroup::new();
        host_picker_group.set_margin_top(12);

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

        // Connection fields
        let fields_group = adw::PreferencesGroup::new();
        fields_group.set_margin_top(12);

        let name_row = adw::EntryRow::builder().title("Name").build();
        fields_group.add(&name_row);

        let host_row = adw::EntryRow::builder().title("Host").build();
        fields_group.add(&host_row);

        let user_row = adw::EntryRow::builder().title("User").build();
        fields_group.add(&user_row);

        let port_row = adw::EntryRow::builder().title("Port").text("22").build();
        fields_group.add(&port_row);

        let identity_row = adw::EntryRow::builder()
            .title("Identity File (optional)")
            .build();
        fields_group.add(&identity_row);

        content.append(&fields_group);

        // Toggles
        let toggle_group = adw::PreferencesGroup::new();
        toggle_group.set_margin_top(12);

        let auto_connect_row = adw::SwitchRow::builder()
            .title("Auto-connect")
            .subtitle("Connect when project starts")
            .build();
        toggle_group.add(&auto_connect_row);

        let auto_reconnect_row = adw::SwitchRow::builder()
            .title("Auto-reconnect")
            .subtitle("Reconnect if connection drops")
            .build();
        toggle_group.add(&auto_reconnect_row);

        content.append(&toggle_group);

        // When picking from ssh config, populate fields
        let name_row_ref = name_row.clone();
        let host_row_ref = host_row.clone();
        let user_row_ref = user_row.clone();
        let port_row_ref = port_row.clone();
        let identity_row_ref = identity_row.clone();
        let ssh_hosts_ref = ssh_hosts.clone();
        host_picker_row.connect_selected_notify(move |picker| {
            let idx = picker.selected() as usize;
            if idx == 0 {
                // Custom — clear fields
                name_row_ref.set_text("");
                host_row_ref.set_text("");
                user_row_ref.set_text("");
                port_row_ref.set_text("22");
                identity_row_ref.set_text("");
            } else if let Some(ssh_host) = ssh_hosts_ref.get(idx - 1) {
                name_row_ref.set_text(&ssh_host.name);
                host_row_ref.set_text(ssh_host.hostname.as_deref().unwrap_or(&ssh_host.name));
                user_row_ref.set_text(ssh_host.user.as_deref().unwrap_or(""));
                port_row_ref.set_text(&ssh_host.port.unwrap_or(22).to_string());
                identity_row_ref.set_text(ssh_host.identity_file.as_deref().unwrap_or(""));
            }
        });

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });

        let dialog_ref = dialog.clone();
        let names = project_names.to_vec();
        add_btn.connect_clicked(move |_| {
            let host = host_row.text().to_string().trim().to_string();
            if host.is_empty() {
                return;
            }

            let display_name = name_row.text().to_string().trim().to_string();
            let user = user_row.text().to_string().trim().to_string();
            let port: u16 = port_row.text().to_string().trim().parse().unwrap_or(22);
            let identity = identity_row.text().to_string().trim().to_string();

            // Build the ssh command
            let ssh_host = SshHost {
                name: host.clone(),
                hostname: Some(host.clone()),
                user: if user.is_empty() {
                    None
                } else {
                    Some(user.clone())
                },
                port: if port == 22 { None } else { Some(port) },
                identity_file: if identity.is_empty() {
                    None
                } else {
                    Some(identity)
                },
            };
            let command = ssh_host.to_ssh_command();

            let selected_project = names
                .get(project_row.selected() as usize)
                .cloned()
                .unwrap_or_default();

            let conn_name = format!(
                "ssh-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("0")
            );

            // Use user-provided name, fall back to user@host or just host
            let display = if !display_name.is_empty() {
                display_name
            } else if user.is_empty() {
                host.clone()
            } else {
                format!("{user}@{host}")
            };

            let config = ProcessConfig {
                name: conn_name,
                command,
                working_dir: None,
                start_with_project: auto_connect_row.is_active(),
                auto_restart: auto_reconnect_row.is_active(),
                restart_when_changed: Vec::new(),
                env: std::collections::HashMap::new(),
                category: ProcessCategory::SSH,
                auto_named: false,
                display_name: Some(display),
            };

            on_add(&selected_project, config);
            dialog_ref.close();
        });

        dialog.present(Some(parent));
    }
}
