use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;

use crate::workspace::Project;
use crate::process::manager::ProcessStatus;

pub struct ProjectDetail;

impl ProjectDetail {
    pub fn build(project: &Project) -> gtk4::Box {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
        container.set_margin_start(24);
        container.set_margin_end(24);
        container.set_margin_top(24);
        container.set_margin_bottom(24);
        container.set_vexpand(true);
        container.set_hexpand(true);
        container.add_css_class("project-detail");

        // Title
        let title = gtk4::Label::builder()
            .label(&project.name)
            .halign(gtk4::Align::Start)
            .css_classes(["title-1"])
            .build();
        container.append(&title);

        // Info cards
        let cards_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        cards_box.set_halign(gtk4::Align::Start);

        // Directory card
        let dir_card = Self::build_card(
            "Directory",
            &project.dir.to_string_lossy(),
            "folder-symbolic",
        );
        // Add copy path button
        let dir_str = project.dir.to_string_lossy().to_string();
        let copy_btn = gtk4::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy Path")
            .css_classes(["flat", "circular"])
            .build();
        let dir_owned = dir_str.clone();
        copy_btn.connect_clicked(move |_| {
            if let Some(display) = gtk4::gdk::Display::default() {
                display.clipboard().set_text(&dir_owned);
            }
        });
        if let Some(content) = dir_card.last_child() {
            if let Ok(content_box) = content.downcast::<gtk4::Box>() {
                content_box.append(&copy_btn);
            }
        }
        cards_box.append(&dir_card);

        // Config card
        let config_path = project.dir.join("tuxflow.toml");
        let config_status = if config_path.exists() { "Valid" } else { "Not found" };
        let config_card = Self::build_card(
            "Config",
            &format!("tuxflow.toml — {config_status}"),
            "document-properties-symbolic",
        );
        cards_box.append(&config_card);

        // Commands card
        let mgr = project.manager.borrow();
        let running = mgr.running_count();
        let total = mgr.total_count();
        let commands_card = Self::build_card(
            "Commands",
            &format!("{running} running / {total} total"),
            "view-list-symbolic",
        );
        cards_box.append(&commands_card);

        container.append(&cards_box);

        // Settings section
        let settings_group = adw::PreferencesGroup::builder()
            .title("Project Settings")
            .build();

        let start_with_project_row = adw::SwitchRow::builder()
            .title("Start with Project")
            .subtitle("Include when starting the project")
            .active(true)
            .build();
        settings_group.add(&start_with_project_row);

        container.append(settings_group.upcast_ref::<gtk4::Widget>());

        // Process list
        let proc_group = adw::PreferencesGroup::builder()
            .title("Processes")
            .build();

        for name in mgr.process_names() {
            if let Some(proc) = mgr.get_process(name) {
                let status_str = match proc.status {
                    ProcessStatus::Running => "Running",
                    ProcessStatus::Stopped => "Stopped",
                    ProcessStatus::Crashed => "Crashed",
                    ProcessStatus::Restarting => "Restarting",
                };
                let row = adw::ActionRow::builder()
                    .title(name)
                    .subtitle(&format!("{} — {status_str}", proc.config.command))
                    .build();
                proc_group.add(&row);
            }
        }

        container.append(proc_group.upcast_ref::<gtk4::Widget>());

        container
    }

    fn build_card(title: &str, subtitle: &str, icon_name: &str) -> gtk4::Box {
        let card = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        card.add_css_class("project-detail-card");
        card.set_width_request(200);

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let icon = gtk4::Image::from_icon_name(icon_name);
        icon.add_css_class("dim-label");
        header.append(&icon);

        let title_label = gtk4::Label::builder()
            .label(title)
            .css_classes(["heading"])
            .build();
        header.append(&title_label);
        card.append(&header);

        let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let subtitle_label = gtk4::Label::builder()
            .label(subtitle)
            .css_classes(["caption", "dim-label"])
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .hexpand(true)
            .halign(gtk4::Align::Start)
            .build();
        content.append(&subtitle_label);
        card.append(&content);

        card
    }
}
