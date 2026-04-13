use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::schema::ProcessConfig;
use crate::config::settings::AppSettings;
use crate::workspace::CommandToggleEntry;

pub struct EditProjectResult {
    pub name: String,
    pub icon_path: Option<String>,
    pub remove: bool,
    pub enabled_commands: Vec<ProcessConfig>,
    pub disabled_commands: Vec<String>,
}

pub struct EditProjectDialog;

impl EditProjectDialog {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        project_name: &str,
        project_dir: &str,
        current_icon: Option<&str>,
        commands: Vec<CommandToggleEntry>,
        on_save: impl Fn(EditProjectResult) + 'static,
    ) {
        let dialog = adw::Dialog::builder()
            .title("Edit Project")
            .content_width(520)
            .content_height(640)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        headerbar.set_show_end_title_buttons(false);
        headerbar.set_show_start_title_buttons(false);

        // Header: cancel (left), save (right), overflow menu with Remove (right).
        let cancel_btn = gtk4::Button::builder().label("Cancel").build();
        headerbar.pack_start(&cancel_btn);

        let save_btn = gtk4::Button::builder()
            .label("Save")
            .css_classes(["suggested-action"])
            .build();
        headerbar.pack_end(&save_btn);

        let menu_btn = gtk4::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More actions")
            .build();
        let menu_popover = gtk4::Popover::new();
        let menu_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let remove_btn = gtk4::Button::builder()
            .label("Remove Project")
            .css_classes(["flat", "destructive-action"])
            .build();
        menu_box.append(&remove_btn);
        menu_popover.set_child(Some(&menu_box));
        menu_btn.set_popover(Some(&menu_popover));
        headerbar.pack_end(&menu_btn);

        toolbar_view.add_top_bar(&headerbar);

        // Content uses an AdwPreferencesPage for consistent spacing.
        let page = adw::PreferencesPage::new();

        // --- Project group: name + directory (with quick-action suffixes) ---
        let project_group = adw::PreferencesGroup::builder().title("Project").build();

        let name_row = adw::EntryRow::builder()
            .title("Name")
            .text(project_name)
            .build();
        project_group.add(&name_row);

        let dir_row = adw::ActionRow::builder()
            .title("Directory")
            .subtitle(project_dir)
            .subtitle_selectable(true)
            .build();
        dir_row.add_css_class("property");

        let copy_path_btn = gtk4::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy Path")
            .css_classes(["flat"])
            .valign(gtk4::Align::Center)
            .build();
        let reveal_btn = gtk4::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Reveal in File Manager")
            .css_classes(["flat"])
            .valign(gtk4::Align::Center)
            .build();
        let terminal_btn = gtk4::Button::builder()
            .icon_name("utilities-terminal-symbolic")
            .tooltip_text("Open Terminal Here")
            .css_classes(["flat"])
            .valign(gtk4::Align::Center)
            .build();
        let editor_btn = gtk4::Button::builder()
            .icon_name("text-editor-symbolic")
            .tooltip_text("Open in Editor")
            .css_classes(["flat"])
            .valign(gtk4::Align::Center)
            .build();

        dir_row.add_suffix(&copy_path_btn);
        dir_row.add_suffix(&reveal_btn);
        dir_row.add_suffix(&terminal_btn);
        dir_row.add_suffix(&editor_btn);
        project_group.add(&dir_row);

        page.add(&project_group);

        // --- Icon section: single ActionRow with prefix preview + suffix menu ---
        let icon_group = adw::PreferencesGroup::new();
        let icon_row = adw::ActionRow::builder().title("Icon").build();

        let icon_preview = gtk4::Image::builder()
            .pixel_size(32)
            .margin_start(4)
            .margin_end(4)
            .build();
        let initials_label = gtk4::Label::builder()
            .css_classes(["project-icon", "caption"])
            .width_request(32)
            .height_request(32)
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .build();

        let icon_path_store: Rc<RefCell<Option<String>>> =
            Rc::new(RefCell::new(current_icon.map(|s| s.to_string())));

        let update_preview = {
            let icon_preview = icon_preview.clone();
            let initials_label = initials_label.clone();
            let icon_row = icon_row.clone();
            Rc::new(move |path: Option<&str>, pname: &str| {
                if let Some(p) = path {
                    icon_preview.set_from_file(Some(p));
                    icon_preview.set_visible(true);
                    initials_label.set_visible(false);
                    icon_row.set_subtitle("Custom image");
                } else {
                    icon_preview.set_visible(false);
                    let initials = pname.chars().take(2).collect::<String>().to_uppercase();
                    initials_label.set_label(&initials);
                    initials_label.set_visible(true);
                    icon_row.set_subtitle("Default initials");
                }
            })
        };
        update_preview(current_icon, project_name);

        let preview_wrap = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        preview_wrap.set_valign(gtk4::Align::Center);
        preview_wrap.append(&icon_preview);
        preview_wrap.append(&initials_label);
        icon_row.add_prefix(&preview_wrap);

        let icon_menu_btn = gtk4::MenuButton::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("Change icon")
            .css_classes(["flat"])
            .valign(gtk4::Align::Center)
            .build();
        let icon_menu_popover = gtk4::Popover::new();
        let icon_menu_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let auto_detect_btn = gtk4::Button::builder()
            .label("Auto-detect")
            .css_classes(["flat"])
            .build();
        let choose_file_btn = gtk4::Button::builder()
            .label("Choose File…")
            .css_classes(["flat"])
            .build();
        let clear_icon_btn = gtk4::Button::builder()
            .label("Reset to Initials")
            .css_classes(["flat"])
            .build();
        icon_menu_box.append(&auto_detect_btn);
        icon_menu_box.append(&choose_file_btn);
        icon_menu_box.append(&clear_icon_btn);
        icon_menu_popover.set_child(Some(&icon_menu_box));
        icon_menu_btn.set_popover(Some(&icon_menu_popover));
        icon_row.add_suffix(&icon_menu_btn);

        icon_group.add(&icon_row);
        page.add(&icon_group);

        // --- Commands section: grouped into Active / Hidden / Detected ---
        let switches: Rc<RefCell<Vec<(adw::SwitchRow, ProcessConfig, bool, bool)>>> =
            Rc::new(RefCell::new(Vec::with_capacity(commands.len())));

        if !commands.is_empty() {
            let mut active_entries = Vec::new();
            let mut hidden_entries = Vec::new();
            let mut new_entries = Vec::new();
            for entry in commands {
                match entry.source_label {
                    "hidden" => hidden_entries.push(entry),
                    "new" => new_entries.push(entry),
                    _ => active_entries.push(entry),
                }
            }

            let add_group =
                |title: &str,
                 description: Option<&str>,
                 entries: Vec<CommandToggleEntry>,
                 switches: &Rc<RefCell<Vec<(adw::SwitchRow, ProcessConfig, bool, bool)>>>|
                 -> Option<adw::PreferencesGroup> {
                    if entries.is_empty() {
                        return None;
                    }
                    let builder = adw::PreferencesGroup::builder().title(title);
                    let group = match description {
                        Some(d) => builder.description(d).build(),
                        None => builder.build(),
                    };
                    for entry in entries {
                        let title = entry
                            .config
                            .display_name
                            .clone()
                            .unwrap_or_else(|| entry.config.name.clone());
                        let row = adw::SwitchRow::builder()
                            .title(&title)
                            .subtitle(&entry.config.command)
                            .active(entry.initial_on)
                            .build();
                        group.add(&row);
                        switches.borrow_mut().push((
                            row,
                            entry.config,
                            entry.initial_on,
                            entry.is_custom,
                        ));
                    }
                    Some(group)
                };

            let active_count = active_entries.len();
            let hidden_count = hidden_entries.len();
            let new_count = new_entries.len();

            if let Some(g) = add_group(
                &format!("Active ({active_count})"),
                Some("Currently part of this project. Toggle off to stop and hide."),
                active_entries,
                &switches,
            ) {
                page.add(&g);
            }
            if let Some(g) = add_group(
                &format!("Hidden ({hidden_count})"),
                Some("Previously removed. Toggle on to restore."),
                hidden_entries,
                &switches,
            ) {
                page.add(&g);
            }
            if let Some(g) = add_group(
                &format!("Detected ({new_count})"),
                Some("Found in this project but not yet added. Toggle on to include."),
                new_entries,
                &switches,
            ) {
                page.add(&g);
            }
        }

        toolbar_view.set_content(Some(&page));
        dialog.set_child(Some(&toolbar_view));

        // --- Wire directory quick actions ---
        let dir_for_copy = project_dir.to_string();
        copy_path_btn.connect_clicked(move |_| {
            if let Some(display) = gtk4::gdk::Display::default() {
                display.clipboard().set_text(&dir_for_copy);
            }
        });

        let dir_for_reveal = project_dir.to_string();
        reveal_btn.connect_clicked(move |_| {
            let _ = std::process::Command::new("xdg-open")
                .arg(&dir_for_reveal)
                .spawn();
        });

        let dir_for_terminal = project_dir.to_string();
        terminal_btn.connect_clicked(move |_| {
            let settings = AppSettings::load();
            let terminal = &settings.tools.default_terminal;
            if terminal == "xdg-open" {
                for candidate in [
                    "gnome-terminal",
                    "konsole",
                    "xfce4-terminal",
                    "alacritty",
                    "kitty",
                    "foot",
                    "wezterm",
                    "xterm",
                ] {
                    if std::process::Command::new("which")
                        .arg(candidate)
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
                    {
                        let _ = std::process::Command::new(candidate)
                            .current_dir(&dir_for_terminal)
                            .spawn();
                        return;
                    }
                }
            } else {
                let _ = std::process::Command::new(terminal)
                    .current_dir(&dir_for_terminal)
                    .spawn();
            }
        });

        let dir_for_editor = project_dir.to_string();
        editor_btn.connect_clicked(move |_| {
            let settings = AppSettings::load();
            let editor = &settings.tools.default_editor;
            if editor == "xdg-open" {
                for candidate in [
                    "code",
                    "codium",
                    "zed",
                    "gnome-text-editor",
                    "gedit",
                    "kate",
                ] {
                    if std::process::Command::new("which")
                        .arg(candidate)
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
                    {
                        let _ = std::process::Command::new(candidate)
                            .arg(&dir_for_editor)
                            .spawn();
                        return;
                    }
                }
            } else {
                let _ = std::process::Command::new(editor)
                    .arg(&dir_for_editor)
                    .spawn();
            }
        });

        // --- Wire icon actions ---
        let pname_owned = project_name.to_string();
        let dir_owned = project_dir.to_string();
        let store_ref = icon_path_store.clone();
        let pname_ref = pname_owned.clone();
        let update_preview_ad = update_preview.clone();
        let popover_close = icon_menu_popover.clone();
        auto_detect_btn.connect_clicked(move |_| {
            popover_close.popdown();
            if let Some(icon) = detect_project_icon(Path::new(&dir_owned)) {
                update_preview_ad(Some(&icon), &pname_ref);
                *store_ref.borrow_mut() = Some(icon);
            } else {
                update_preview_ad(None, &pname_ref);
                *store_ref.borrow_mut() = None;
            }
        });

        let store_ref = icon_path_store.clone();
        let pname_ref = pname_owned.clone();
        let update_preview_cf = update_preview.clone();
        let popover_close = icon_menu_popover.clone();
        choose_file_btn.connect_clicked(move |btn| {
            popover_close.popdown();
            let file_dialog = gtk4::FileDialog::builder()
                .title("Select Project Icon")
                .build();

            let filter = gtk4::FileFilter::new();
            filter.add_mime_type("image/png");
            filter.add_mime_type("image/svg+xml");
            filter.add_mime_type("image/jpeg");
            filter.add_mime_type("image/x-icon");
            filter.set_name(Some("Images"));

            let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
            filters.append(&filter);
            file_dialog.set_filters(Some(&filters));

            let win = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());

            let store_ref = store_ref.clone();
            let pname_ref = pname_ref.clone();
            let update_preview_cf = update_preview_cf.clone();
            file_dialog.open(win.as_ref(), gtk4::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    let path_str = path.to_string_lossy().to_string();
                    update_preview_cf(Some(&path_str), &pname_ref);
                    *store_ref.borrow_mut() = Some(path_str);
                }
            });
        });

        let store_ref = icon_path_store.clone();
        let pname_ref = pname_owned.clone();
        let update_preview_cl = update_preview.clone();
        let popover_close = icon_menu_popover.clone();
        clear_icon_btn.connect_clicked(move |_| {
            popover_close.popdown();
            update_preview_cl(None, &pname_ref);
            *store_ref.borrow_mut() = None;
        });

        // --- Wire Save / Cancel / Remove ---
        let on_save = Rc::new(on_save);

        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });

        let dialog_save = dialog.clone();
        let on_save_ref = on_save.clone();
        let store_ref = icon_path_store.clone();
        let switches_save = switches.clone();
        let name_row_ref = name_row.clone();
        save_btn.connect_clicked(move |_| {
            let name = name_row_ref.text().to_string();
            if name.is_empty() {
                return;
            }
            let switches_borrow = switches_save.borrow();
            let enabled_commands: Vec<ProcessConfig> = switches_borrow
                .iter()
                .filter(|(row, _, initial_on, _)| row.is_active() && !initial_on)
                .map(|(_, cfg, _, _)| cfg.clone())
                .collect();
            let disabled_commands: Vec<String> = switches_borrow
                .iter()
                .filter(|(row, _, initial_on, _)| !row.is_active() && *initial_on)
                .map(|(_, cfg, _, _)| cfg.name.clone())
                .collect();
            drop(switches_borrow);
            on_save_ref(EditProjectResult {
                name,
                icon_path: store_ref.borrow().clone(),
                remove: false,
                enabled_commands,
                disabled_commands,
            });
            dialog_save.close();
        });

        let dialog_remove = dialog.clone();
        let menu_popover_close = menu_popover.clone();
        remove_btn.connect_clicked(move |_| {
            menu_popover_close.popdown();
            on_save(EditProjectResult {
                name: String::new(),
                icon_path: None,
                remove: true,
                enabled_commands: Vec::new(),
                disabled_commands: Vec::new(),
            });
            dialog_remove.close();
        });

        dialog.present(Some(parent));
    }
}

/// Scan project directory for common icon files
pub fn detect_project_icon(dir: &Path) -> Option<String> {
    let candidates = [
        "favicon.svg",
        "favicon.png",
        "favicon.ico",
        "logo.svg",
        "logo.png",
        ".icon.png",
        ".icon.svg",
        "public/favicon.svg",
        "public/favicon.png",
        "public/favicon.ico",
        "public/logo.svg",
        "public/logo.png",
        "resources/icon.svg",
        "resources/icon.png",
        "assets/icon.svg",
        "assets/icon.png",
    ];

    for candidate in &candidates {
        let path = dir.join(candidate);
        if path.exists() {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}
