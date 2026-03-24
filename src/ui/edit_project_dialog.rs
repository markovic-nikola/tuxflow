use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;

use crate::config::settings::AppSettings;

pub struct EditProjectResult {
    pub name: String,
    pub icon_path: Option<String>,
    pub remove: bool,
}

pub struct EditProjectDialog;

impl EditProjectDialog {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        project_name: &str,
        project_dir: &str,
        current_icon: Option<&str>,
        on_save: impl Fn(EditProjectResult) + 'static,
    ) {
        let dialog = adw::Dialog::builder()
            .title("Edit Project")
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

        // Name field
        let name_group = adw::PreferencesGroup::new();
        let name_row = adw::EntryRow::builder()
            .title("Project Name")
            .text(project_name)
            .build();
        name_group.add(&name_row);
        content.append(&name_group);

        // Directory field (read-only)
        let dir_group = adw::PreferencesGroup::new();
        dir_group.set_margin_top(12);
        let dir_row = adw::EntryRow::builder()
            .title("Project Directory")
            .text(project_dir)
            .editable(false)
            .build();
        dir_group.add(&dir_row);
        content.append(&dir_group);

        // Quick actions row
        let actions_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        actions_box.set_margin_top(8);
        actions_box.set_halign(gtk4::Align::Start);
        actions_box.set_margin_start(12);

        let copy_path_btn = gtk4::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy Path")
            .css_classes(["flat", "circular"])
            .build();

        let reveal_btn = gtk4::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Reveal in File Manager")
            .css_classes(["flat", "circular"])
            .build();

        let terminal_btn = gtk4::Button::builder()
            .icon_name("utilities-terminal-symbolic")
            .tooltip_text("Open Terminal")
            .css_classes(["flat", "circular"])
            .build();

        let editor_btn = gtk4::Button::builder()
            .icon_name("text-editor-symbolic")
            .tooltip_text("Open in Editor")
            .css_classes(["flat", "circular"])
            .build();

        actions_box.append(&copy_path_btn);
        actions_box.append(&reveal_btn);
        actions_box.append(&terminal_btn);
        actions_box.append(&editor_btn);
        content.append(&actions_box);

        // Copy path
        let dir_for_copy = project_dir.to_string();
        copy_path_btn.connect_clicked(move |_| {
            if let Some(display) = gtk4::gdk::Display::default() {
                display.clipboard().set_text(&dir_for_copy);
            }
        });

        // Reveal in file manager
        let dir_for_reveal = project_dir.to_string();
        reveal_btn.connect_clicked(move |_| {
            let _ = std::process::Command::new("xdg-open")
                .arg(&dir_for_reveal)
                .spawn();
        });

        // Open terminal in project directory
        let dir_for_terminal = project_dir.to_string();
        terminal_btn.connect_clicked(move |_| {
            let settings = AppSettings::load();
            let terminal = &settings.tools.default_terminal;
            if terminal == "xdg-open" {
                // xdg-open on a dir opens file manager, so pick a real terminal
                for candidate in ["gnome-terminal", "konsole", "xfce4-terminal", "alacritty", "kitty", "foot", "wezterm", "xterm"] {
                    if std::process::Command::new("which").arg(candidate).output().map(|o| o.status.success()).unwrap_or(false) {
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

        // Open in editor
        let dir_for_editor = project_dir.to_string();
        editor_btn.connect_clicked(move |_| {
            let settings = AppSettings::load();
            let editor = &settings.tools.default_editor;
            if editor == "xdg-open" {
                // xdg-open on a dir opens file manager; probe for a GUI editor
                for candidate in ["code", "codium", "zed", "gnome-text-editor", "gedit", "kate"] {
                    if std::process::Command::new("which").arg(candidate).output().map(|o| o.status.success()).unwrap_or(false) {
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

        // Icon section
        let icon_group = adw::PreferencesGroup::builder()
            .title("Project Icon")
            .margin_top(12)
            .build();

        let icon_preview = gtk4::Image::builder()
            .pixel_size(48)
            .margin_start(12)
            .margin_top(8)
            .halign(gtk4::Align::Start)
            .build();
        if let Some(path) = current_icon {
            icon_preview.set_from_file(Some(path));
            icon_preview.set_visible(true);
        } else {
            icon_preview.set_visible(false);
        }

        let icon_path_label = gtk4::Label::builder()
            .label(current_icon.unwrap_or("(default initials)"))
            .halign(gtk4::Align::Start)
            .ellipsize(gtk4::pango::EllipsizeMode::Middle)
            .css_classes(["caption", "dim-label"])
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(4)
            .build();

        let icon_path_store: Rc<RefCell<Option<String>>> =
            Rc::new(RefCell::new(current_icon.map(|s| s.to_string())));

        let icon_buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        icon_buttons.set_margin_start(12);
        icon_buttons.set_margin_end(12);
        icon_buttons.set_margin_top(4);
        icon_buttons.set_margin_bottom(8);

        let auto_detect_btn = gtk4::Button::builder()
            .label("Auto-detect")
            .css_classes(["flat"])
            .build();

        let choose_file_btn = gtk4::Button::builder()
            .label("Choose File...")
            .css_classes(["flat"])
            .build();

        let clear_icon_btn = gtk4::Button::builder()
            .label("Reset")
            .css_classes(["flat"])
            .build();

        icon_buttons.append(&auto_detect_btn);
        icon_buttons.append(&choose_file_btn);
        icon_buttons.append(&clear_icon_btn);

        let icon_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        icon_box.append(&icon_preview);
        icon_box.append(&icon_path_label);
        icon_box.append(&icon_buttons);
        icon_group.add(&icon_box);
        content.append(&icon_group);

        // Auto-detect button
        let dir_owned = project_dir.to_string();
        let label_ref = icon_path_label.clone();
        let store_ref = icon_path_store.clone();
        let preview_ref = icon_preview.clone();
        auto_detect_btn.connect_clicked(move |_| {
            if let Some(icon) = detect_project_icon(Path::new(&dir_owned)) {
                label_ref.set_label(&icon);
                preview_ref.set_from_file(Some(&icon));
                preview_ref.set_visible(true);
                *store_ref.borrow_mut() = Some(icon);
            } else {
                label_ref.set_label("(no icon found)");
                preview_ref.set_visible(false);
            }
        });

        // Choose file button
        let label_ref = icon_path_label.clone();
        let store_ref = icon_path_store.clone();
        let preview_ref = icon_preview.clone();
        choose_file_btn.connect_clicked(move |btn| {
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

            let label_ref = label_ref.clone();
            let store_ref = store_ref.clone();
            let preview_ref = preview_ref.clone();
            file_dialog.open(win.as_ref(), gtk4::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        let path_str = path.to_string_lossy().to_string();
                        label_ref.set_label(&path_str);
                        preview_ref.set_from_file(Some(&*path_str));
                        preview_ref.set_visible(true);
                        *store_ref.borrow_mut() = Some(path_str);
                    }
                }
            });
        });

        // Clear icon button
        let label_ref = icon_path_label.clone();
        let store_ref = icon_path_store.clone();
        let preview_ref = icon_preview.clone();
        clear_icon_btn.connect_clicked(move |_| {
            label_ref.set_label("(default initials)");
            preview_ref.set_visible(false);
            *store_ref.borrow_mut() = None;
        });

        // Remove button
        let remove_btn = gtk4::Button::builder()
            .label("Remove Project")
            .css_classes(["destructive-action", "pill"])
            .margin_top(24)
            .halign(gtk4::Align::Center)
            .build();
        content.append(&remove_btn);

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        // Track whether remove was clicked (skip the on-close save in that case)
        let removed = Rc::new(RefCell::new(false));

        // Wire remove — fires immediately, then closes
        let dialog_ref = dialog.clone();
        let on_save = Rc::new(on_save);
        let on_save_ref = on_save.clone();
        let removed_ref = removed.clone();
        remove_btn.connect_clicked(move |_| {
            *removed_ref.borrow_mut() = true;
            on_save_ref(EditProjectResult {
                name: String::new(),
                icon_path: None,
                remove: true,
            });
            dialog_ref.close();
        });

        // Apply name/icon changes when dialog closes
        let store_ref = icon_path_store.clone();
        dialog.connect_closed(move |_| {
            if *removed.borrow() {
                return;
            }
            let name = name_row.text().to_string();
            if name.is_empty() {
                return;
            }
            on_save(EditProjectResult {
                name,
                icon_path: store_ref.borrow().clone(),
                remove: false,
            });
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
