use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;

use crate::config::schema::{ProcessCategory, ProcessConfig};

pub enum EditCommandResult {
    Save(ProcessConfig),
    Delete,
}

pub struct AddCommandDialog;

struct FormFields {
    name_row: adw::EntryRow,
    cmd_row: adw::EntryRow,
    start_with_project_row: adw::SwitchRow,
    auto_restart_row: adw::SwitchRow,
    watch_row: adw::EntryRow,
}

fn build_form_fields(content: &gtk4::Box) -> FormFields {
    // Name field
    let name_group = adw::PreferencesGroup::new();
    name_group.set_margin_top(12);
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

    // Toggles
    let toggle_group = adw::PreferencesGroup::new();
    toggle_group.set_margin_top(12);

    let start_with_project_row = adw::SwitchRow::builder()
        .title("Start with Project")
        .subtitle("Include when starting the project")
        .build();
    toggle_group.add(&start_with_project_row);

    let auto_restart_row = adw::SwitchRow::builder()
        .title("Auto Restart")
        .subtitle("Restart if process crashes")
        .build();
    toggle_group.add(&auto_restart_row);

    content.append(&toggle_group);

    // File watch patterns
    let watch_group = adw::PreferencesGroup::new();
    watch_group.set_margin_top(12);
    let watch_row = adw::EntryRow::builder()
        .title("Watch Patterns (comma-separated globs)")
        .build();
    watch_group.add(&watch_row);
    content.append(&watch_group);

    FormFields { name_row, cmd_row, start_with_project_row, auto_restart_row, watch_row }
}

fn parse_watch_patterns(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

impl AddCommandDialog {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        project_names: &[String],
        last_project: Option<&str>,
        on_add: impl Fn(&str, ProcessConfig) + 'static,
    ) {
        let dialog = adw::Dialog::builder()
            .title("Add Command")
            .content_width(450)
            .content_height(400)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&headerbar);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        // Project selector
        let project_group = adw::PreferencesGroup::new();
        let project_list = gtk4::StringList::new(&project_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let project_row = adw::ComboRow::builder()
            .title("Project")
            .model(&project_list)
            .build();
        // Pre-select last used project
        if let Some(last) = last_project {
            if let Some(idx) = project_names.iter().position(|n| n == last) {
                project_row.set_selected(idx as u32);
            }
        }

        project_group.add(&project_row);
        content.append(&project_group);

        let fields = build_form_fields(&content);

        let add_btn = gtk4::Button::builder()
            .label("Add Command")
            .css_classes(["suggested-action", "pill"])
            .margin_top(24)
            .halign(gtk4::Align::Center)
            .build();
        content.append(&add_btn);

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        let dialog_ref = dialog.clone();
        let names = project_names.to_vec();
        add_btn.connect_clicked(move |_| {
            let name = fields.name_row.text().to_string();
            let command = fields.cmd_row.text().to_string();

            if name.is_empty() || command.is_empty() {
                return;
            }

            let selected_project = names
                .get(project_row.selected() as usize)
                .cloned()
                .unwrap_or_default();

            let config = ProcessConfig {
                name,
                command,
                working_dir: None,
                start_with_project: fields.start_with_project_row.is_active(),
                auto_restart: fields.auto_restart_row.is_active(),
                restart_when_changed: parse_watch_patterns(&fields.watch_row.text()),
                env: std::collections::HashMap::new(),
                category: ProcessCategory::Command,
                auto_named: false,
                display_name: None,
            };

            on_add(&selected_project, config);
            dialog_ref.close();
        });

        dialog.present(Some(parent));
    }

    pub fn show_edit(
        parent: &impl IsA<gtk4::Widget>,
        current: &ProcessConfig,
        on_result: impl Fn(EditCommandResult) + 'static,
    ) {
        let dialog = adw::Dialog::builder()
            .title("Edit Command")
            .content_width(450)
            .content_height(400)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&headerbar);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        let fields = build_form_fields(&content);

        // Pre-fill from current config
        fields.name_row.set_text(&current.name);
        fields.cmd_row.set_text(&current.command);
        fields.start_with_project_row.set_active(current.start_with_project);
        fields.auto_restart_row.set_active(current.auto_restart);
        if !current.restart_when_changed.is_empty() {
            fields.watch_row.set_text(&current.restart_when_changed.join(", "));
        }

        // Button row: Save + Delete
        let button_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        button_box.set_margin_top(24);
        button_box.set_halign(gtk4::Align::Center);

        let save_btn = gtk4::Button::builder()
            .label("Save")
            .css_classes(["suggested-action", "pill"])
            .build();
        button_box.append(&save_btn);

        let delete_btn = gtk4::Button::builder()
            .label("Delete Command")
            .css_classes(["destructive-action", "pill"])
            .build();
        button_box.append(&delete_btn);

        content.append(&button_box);

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        // Preserve working_dir, env, and category from original
        let working_dir = current.working_dir.clone();
        let env = current.env.clone();
        let category = current.category.clone();

        let on_result = std::rc::Rc::new(on_result);

        let dialog_ref = dialog.clone();
        let on_result_ref = on_result.clone();
        save_btn.connect_clicked(move |_| {
            let name = fields.name_row.text().to_string();
            let command = fields.cmd_row.text().to_string();

            if name.is_empty() || command.is_empty() {
                return;
            }

            let config = ProcessConfig {
                name,
                command,
                working_dir: working_dir.clone(),
                start_with_project: fields.start_with_project_row.is_active(),
                auto_restart: fields.auto_restart_row.is_active(),
                restart_when_changed: parse_watch_patterns(&fields.watch_row.text()),
                env: env.clone(),
                category: category.clone(),
                auto_named: false,
                display_name: None,
            };

            on_result_ref(EditCommandResult::Save(config));
            dialog_ref.close();
        });

        let dialog_ref = dialog.clone();
        delete_btn.connect_clicked(move |_| {
            on_result(EditCommandResult::Delete);
            dialog_ref.close();
        });

        dialog.present(Some(parent));
    }

    pub fn show_add_agent(
        parent: &impl IsA<gtk4::Widget>,
        project_names: &[String],
        last_project: Option<&str>,
        on_add: impl Fn(&str, ProcessConfig) + 'static,
    ) {
        let dialog = adw::Dialog::builder()
            .title("New Custom Agent")
            .content_width(450)
            .content_height(300)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&headerbar);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        // Project selector
        let project_group = adw::PreferencesGroup::new();
        let project_list = gtk4::StringList::new(&project_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let project_row = adw::ComboRow::builder()
            .title("Project")
            .model(&project_list)
            .build();
        if let Some(last) = last_project {
            if let Some(idx) = project_names.iter().position(|n| n == last) {
                project_row.set_selected(idx as u32);
            }
        }
        project_group.add(&project_row);
        content.append(&project_group);

        // Name field
        let name_group = adw::PreferencesGroup::new();
        name_group.set_margin_top(12);
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

        let add_btn = gtk4::Button::builder()
            .label("Add Agent")
            .css_classes(["suggested-action", "pill"])
            .margin_top(24)
            .halign(gtk4::Align::Center)
            .build();
        content.append(&add_btn);

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        let dialog_ref = dialog.clone();
        let names = project_names.to_vec();
        add_btn.connect_clicked(move |_| {
            let name = name_row.text().to_string();
            let command = cmd_row.text().to_string();

            if name.is_empty() || command.is_empty() {
                return;
            }

            let selected_project = names
                .get(project_row.selected() as usize)
                .cloned()
                .unwrap_or_default();

            let config = ProcessConfig {
                name,
                command,
                working_dir: None,
                start_with_project: false,
                auto_restart: false,
                restart_when_changed: Vec::new(),
                env: std::collections::HashMap::new(),
                category: ProcessCategory::Agent,
                auto_named: false,
                display_name: None,
            };

            on_add(&selected_project, config);
            dialog_ref.close();
        });

        dialog.present(Some(parent));
    }
}
