use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;

use crate::config::schema::{ProcessCategory, ProcessConfig};

pub struct AddCommandDialog;

impl AddCommandDialog {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        on_add: impl Fn(ProcessConfig) + 'static,
    ) {
        let dialog = adw::Dialog::builder()
            .title("Add Command")
            .content_width(450)
            .content_height(400)
            .build();

        let toolbar_view = adw::ToolbarView::new();

        // Header bar
        let headerbar = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&headerbar);

        // Content
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        // Name field
        let name_group = adw::PreferencesGroup::new();
        let name_row = adw::EntryRow::builder()
            .title("Name")
            .build();
        name_group.add(&name_row);
        content.append(&name_group);

        // Command field
        let cmd_group = adw::PreferencesGroup::new();
        cmd_group.set_margin_top(12);
        let cmd_row = adw::EntryRow::builder()
            .title("Command")
            .build();
        cmd_group.add(&cmd_row);
        content.append(&cmd_group);

        // Working directory
        let dir_group = adw::PreferencesGroup::new();
        dir_group.set_margin_top(12);
        let dir_row = adw::EntryRow::builder()
            .title("Working Directory (optional)")
            .build();
        dir_group.add(&dir_row);
        content.append(&dir_group);

        // Toggles
        let toggle_group = adw::PreferencesGroup::new();
        toggle_group.set_margin_top(12);

        let auto_start_row = adw::SwitchRow::builder()
            .title("Auto Start")
            .subtitle("Start when project opens")
            .build();
        toggle_group.add(&auto_start_row);

        let auto_restart_row = adw::SwitchRow::builder()
            .title("Auto Restart")
            .subtitle("Restart if process crashes")
            .build();
        toggle_group.add(&auto_restart_row);

        content.append(&toggle_group);

        // Category
        let cat_group = adw::PreferencesGroup::new();
        cat_group.set_margin_top(12);
        let cat_row = adw::ComboRow::builder()
            .title("Category")
            .model(&gtk4::StringList::new(&["Command", "Agent", "Terminal"]))
            .build();
        cat_group.add(&cat_row);
        content.append(&cat_group);

        // File watch patterns
        let watch_group = adw::PreferencesGroup::new();
        watch_group.set_margin_top(12);
        let watch_row = adw::EntryRow::builder()
            .title("Watch Patterns (comma-separated globs)")
            .build();
        watch_group.add(&watch_row);
        content.append(&watch_group);

        // Add button
        let add_btn = gtk4::Button::builder()
            .label("Add Command")
            .css_classes(["suggested-action", "pill"])
            .margin_top(24)
            .halign(gtk4::Align::Center)
            .build();
        content.append(&add_btn);

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        // Wire add button
        let dialog_ref = dialog.clone();
        add_btn.connect_clicked(move |_| {
            let name = name_row.text().to_string();
            let command = cmd_row.text().to_string();

            if name.is_empty() || command.is_empty() {
                return;
            }

            let working_dir = {
                let d = dir_row.text().to_string();
                if d.is_empty() { None } else { Some(d) }
            };

            let category = match cat_row.selected() {
                0 => ProcessCategory::Command,
                1 => ProcessCategory::Agent,
                2 => ProcessCategory::Terminal,
                _ => ProcessCategory::Command,
            };

            let watch_patterns: Vec<String> = watch_row
                .text()
                .to_string()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let config = ProcessConfig {
                name,
                command,
                working_dir,
                auto_start: auto_start_row.is_active(),
                auto_restart: auto_restart_row.is_active(),
                restart_when_changed: watch_patterns,
                env: std::collections::HashMap::new(),
                category,
            };

            on_add(config);
            dialog_ref.close();
        });

        dialog.present(Some(parent));
    }
}
