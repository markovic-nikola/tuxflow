use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::keybindings::{
    self, KeybindingMap, ShortcutAction, action_metadata, is_modifier_key, keybinding_from_event,
    keybinding_to_string,
};
use crate::config::settings::AppSettings;

pub type SettingsRef = Rc<RefCell<AppSettings>>;
pub type KeybindingMapRef = Rc<RefCell<KeybindingMap>>;

pub struct SettingsWindow;

impl SettingsWindow {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        on_single_expand_changed: Option<Rc<dyn Fn(bool)>>,
        on_auto_hide_changed: Option<Rc<dyn Fn(bool)>>,
        on_terminal_theme_changed: Option<Rc<dyn Fn(&str)>>,
        on_font_changed: Option<Rc<dyn Fn()>>,
        keybinding_map: Option<KeybindingMapRef>,
    ) {
        let settings = Rc::new(RefCell::new(AppSettings::load()));
        let kb_map = keybinding_map.unwrap_or_else(|| {
            Rc::new(RefCell::new(KeybindingMap::from_settings(
                &settings.borrow().keybindings,
            )))
        });
        Self::show_with_settings(
            parent,
            &settings,
            on_single_expand_changed,
            on_auto_hide_changed,
            on_terminal_theme_changed,
            on_font_changed,
            &kb_map,
        );
    }

    pub fn show_with_settings(
        parent: &impl IsA<gtk4::Widget>,
        settings: &SettingsRef,
        on_single_expand_changed: Option<Rc<dyn Fn(bool)>>,
        on_auto_hide_changed: Option<Rc<dyn Fn(bool)>>,
        on_terminal_theme_changed: Option<Rc<dyn Fn(&str)>>,
        on_font_changed: Option<Rc<dyn Fn()>>,
        keybinding_map: &KeybindingMapRef,
    ) {
        let dialog = adw::PreferencesDialog::new();
        dialog.set_title("Settings");

        // Appearance page
        let appearance_page =
            Self::build_appearance_page(settings, on_terminal_theme_changed, on_font_changed);
        dialog.add(&appearance_page);

        // Sidebar page
        let sidebar_page =
            Self::build_sidebar_page(settings, on_single_expand_changed, on_auto_hide_changed);
        dialog.add(&sidebar_page);

        // Notifications page
        let notifications_page = Self::build_notifications_page(settings);
        dialog.add(&notifications_page);

        // Hotkeys page
        let hotkeys_page = Self::build_hotkeys_page(settings, keybinding_map, &dialog);
        dialog.add(&hotkeys_page);

        // Tools page
        let tools_page = Self::build_tools_page(settings);
        dialog.add(&tools_page);

        // Integrations page
        let integrations_page = Self::build_integrations_page(settings);
        dialog.add(&integrations_page);

        // About page
        let about_page = Self::build_about_page();
        dialog.add(&about_page);

        dialog.present(Some(parent));
    }

    fn build_appearance_page(
        settings: &SettingsRef,
        on_terminal_theme_changed: Option<Rc<dyn Fn(&str)>>,
        on_font_changed: Option<Rc<dyn Fn()>>,
    ) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Appearance")
            .icon_name("applications-graphics-symbolic")
            .build();

        // Theme group
        let theme_group = adw::PreferencesGroup::builder().title("Theme").build();

        let theme_row = adw::ComboRow::builder()
            .title("Color Scheme")
            .subtitle("Choose the application theme")
            .model(&gtk4::StringList::new(&["System", "Dark", "Light"]))
            .build();

        // Set initial value from settings
        let theme_idx = match settings.borrow().appearance.theme.as_str() {
            "system" => 0,
            "dark" => 1,
            "light" => 2,
            _ => 1,
        };
        theme_row.set_selected(theme_idx);

        let settings_ref = settings.clone();
        theme_row.connect_selected_notify(move |row| {
            let manager = adw::StyleManager::default();
            let theme = match row.selected() {
                0 => {
                    manager.set_color_scheme(adw::ColorScheme::Default);
                    "system"
                }
                1 => {
                    manager.set_color_scheme(adw::ColorScheme::ForceDark);
                    "dark"
                }
                2 => {
                    manager.set_color_scheme(adw::ColorScheme::ForceLight);
                    "light"
                }
                _ => return,
            };
            settings_ref.borrow_mut().appearance.theme = theme.to_string();
            settings_ref.borrow().save();
        });

        theme_group.add(&theme_row);

        let choices = crate::ui::accent::color_choices();
        let choices_strs: Vec<&str> = choices.to_vec();
        let accent_row = adw::ComboRow::builder()
            .title("Accent Color")
            .subtitle("Customize the accent color throughout the UI")
            .model(&gtk4::StringList::new(&choices_strs))
            .build();
        accent_row.set_selected(crate::ui::accent::color_index(
            &settings.borrow().appearance.accent_color,
        ));

        let settings_ref = settings.clone();
        accent_row.connect_selected_notify(move |row| {
            let name = crate::ui::accent::color_name(row.selected());
            crate::ui::accent::apply(name);
            settings_ref.borrow_mut().appearance.accent_color = name.to_string();
            settings_ref.borrow().save();
        });

        theme_group.add(&accent_row);
        page.add(&theme_group);

        // Terminal font group
        let font_group = adw::PreferencesGroup::builder().title("Terminal").build();

        let s = settings.borrow();

        let theme_choices = crate::ui::terminal_theme::theme_choices();
        let theme_choices_strs: Vec<&str> = theme_choices.to_vec();
        let terminal_theme_row = adw::ComboRow::builder()
            .title("Terminal Theme")
            .subtitle("Color scheme for terminal output")
            .model(&gtk4::StringList::new(&theme_choices_strs))
            .build();
        terminal_theme_row.set_selected(crate::ui::terminal_theme::theme_index(
            &s.appearance.terminal_theme,
        ));

        let settings_ref = settings.clone();
        terminal_theme_row.connect_selected_notify(move |row| {
            let name = crate::ui::terminal_theme::theme_name(row.selected());
            settings_ref.borrow_mut().appearance.terminal_theme = name.to_string();
            settings_ref.borrow().save();
            if let Some(ref cb) = on_terminal_theme_changed {
                cb(name);
            }
        });
        font_group.add(&terminal_theme_row);

        let font_row = adw::EntryRow::builder()
            .title("Font Family")
            .text(&s.appearance.font_family)
            .build();
        let settings_ref = settings.clone();
        let font_cb = on_font_changed.clone();
        font_row.connect_changed(move |row| {
            settings_ref.borrow_mut().appearance.font_family = row.text().to_string();
            settings_ref.borrow().save();
            if let Some(ref cb) = font_cb {
                cb();
            }
        });
        font_group.add(&font_row);

        let font_size_row = adw::SpinRow::builder()
            .title("Font Size")
            .adjustment(&gtk4::Adjustment::new(
                s.appearance.font_size as f64,
                6.0,
                32.0,
                1.0,
                2.0,
                0.0,
            ))
            .build();
        let settings_ref = settings.clone();
        let font_cb = on_font_changed.clone();
        font_size_row.connect_changed(move |row| {
            settings_ref.borrow_mut().appearance.font_size = row.value() as u32;
            settings_ref.borrow().save();
            if let Some(ref cb) = font_cb {
                cb();
            }
        });
        font_group.add(&font_size_row);

        let font_weight_row = adw::SpinRow::builder()
            .title("Font Weight")
            .adjustment(&gtk4::Adjustment::new(
                s.appearance.font_weight as f64,
                100.0,
                900.0,
                100.0,
                100.0,
                0.0,
            ))
            .build();
        let settings_ref = settings.clone();
        let font_cb = on_font_changed.clone();
        font_weight_row.connect_changed(move |row| {
            settings_ref.borrow_mut().appearance.font_weight = row.value() as u32;
            settings_ref.borrow().save();
            if let Some(ref cb) = font_cb {
                cb();
            }
        });
        font_group.add(&font_weight_row);

        let bold_weight_row = adw::SpinRow::builder()
            .title("Bold Font Weight")
            .adjustment(&gtk4::Adjustment::new(
                s.appearance.bold_font_weight as f64,
                100.0,
                900.0,
                100.0,
                100.0,
                0.0,
            ))
            .build();
        let settings_ref = settings.clone();
        let font_cb = on_font_changed.clone();
        bold_weight_row.connect_changed(move |row| {
            settings_ref.borrow_mut().appearance.bold_font_weight = row.value() as u32;
            settings_ref.borrow().save();
            if let Some(ref cb) = font_cb {
                cb();
            }
        });
        font_group.add(&bold_weight_row);

        let line_height_row = adw::SpinRow::builder()
            .title("Line Height")
            .adjustment(&gtk4::Adjustment::new(
                s.appearance.line_height,
                0.8,
                2.0,
                0.1,
                0.1,
                0.0,
            ))
            .digits(1)
            .build();
        let settings_ref = settings.clone();
        let font_cb = on_font_changed.clone();
        line_height_row.connect_changed(move |row| {
            settings_ref.borrow_mut().appearance.line_height = row.value();
            settings_ref.borrow().save();
            if let Some(ref cb) = font_cb {
                cb();
            }
        });
        font_group.add(&line_height_row);

        let letter_spacing_row = adw::SpinRow::builder()
            .title("Letter Spacing")
            .adjustment(&gtk4::Adjustment::new(
                s.appearance.letter_spacing,
                -2.0,
                10.0,
                0.5,
                1.0,
                0.0,
            ))
            .digits(1)
            .build();
        let settings_ref = settings.clone();
        let font_cb = on_font_changed.clone();
        letter_spacing_row.connect_changed(move |row| {
            settings_ref.borrow_mut().appearance.letter_spacing = row.value();
            settings_ref.borrow().save();
            if let Some(ref cb) = font_cb {
                cb();
            }
        });
        font_group.add(&letter_spacing_row);

        let scrollback_row = adw::SpinRow::builder()
            .title("Scrollback Lines")
            .adjustment(&gtk4::Adjustment::new(
                s.appearance.scrollback_lines as f64,
                100.0,
                100000.0,
                100.0,
                1000.0,
                0.0,
            ))
            .build();
        let settings_ref = settings.clone();
        let font_cb = on_font_changed.clone();
        scrollback_row.connect_changed(move |row| {
            settings_ref.borrow_mut().appearance.scrollback_lines = row.value() as u32;
            settings_ref.borrow().save();
            if let Some(ref cb) = font_cb {
                cb();
            }
        });
        font_group.add(&scrollback_row);

        drop(s);
        page.add(&font_group);

        page
    }

    fn build_notifications_page(settings: &SettingsRef) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Notifications")
            .icon_name("preferences-system-notifications-symbolic")
            .build();

        let group = adw::PreferencesGroup::builder()
            .title("Desktop Notifications")
            .build();

        let s = settings.borrow();

        let notify_crash_row = adw::SwitchRow::builder()
            .title("Process Crash")
            .subtitle("Notify when a process crashes")
            .active(s.notifications.on_crash)
            .build();
        let settings_ref = settings.clone();
        notify_crash_row.connect_active_notify(move |row| {
            settings_ref.borrow_mut().notifications.on_crash = row.is_active();
            settings_ref.borrow().save();
        });
        group.add(&notify_crash_row);

        let notify_restart_row = adw::SwitchRow::builder()
            .title("Auto-Restart")
            .subtitle("Notify when a process is auto-restarted")
            .active(s.notifications.on_auto_restart)
            .build();
        let settings_ref = settings.clone();
        notify_restart_row.connect_active_notify(move |row| {
            settings_ref.borrow_mut().notifications.on_auto_restart = row.is_active();
            settings_ref.borrow().save();
        });
        group.add(&notify_restart_row);

        let notify_file_row = adw::SwitchRow::builder()
            .title("File Watch Restart")
            .subtitle("Notify when a file change triggers a restart")
            .active(s.notifications.on_file_watch_restart)
            .build();
        let settings_ref = settings.clone();
        notify_file_row.connect_active_notify(move |row| {
            settings_ref
                .borrow_mut()
                .notifications
                .on_file_watch_restart = row.is_active();
            settings_ref.borrow().save();
        });
        group.add(&notify_file_row);

        drop(s);
        page.add(&group);
        page
    }

    fn build_sidebar_page(
        settings: &SettingsRef,
        on_single_expand_changed: Option<Rc<dyn Fn(bool)>>,
        on_auto_hide_changed: Option<Rc<dyn Fn(bool)>>,
    ) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Sidebar")
            .icon_name("sidebar-show-symbolic")
            .build();

        let display_group = adw::PreferencesGroup::builder().title("Display").build();

        let s = settings.borrow();

        let single_expand_row = adw::SwitchRow::builder()
            .title("Single Project Expand")
            .subtitle("Only one project can be expanded at a time")
            .active(s.sidebar.single_project_expand)
            .build();
        let settings_ref = settings.clone();
        single_expand_row.connect_active_notify(move |row| {
            let active = row.is_active();
            settings_ref.borrow_mut().sidebar.single_project_expand = active;
            settings_ref.borrow().save();
            if let Some(ref cb) = on_single_expand_changed {
                cb(active);
            }
        });
        display_group.add(&single_expand_row);

        let auto_hide_row = adw::SwitchRow::builder()
            .title("Auto-Hide Sidebar")
            .subtitle("Hide sidebar when the terminal area gains focus")
            .active(s.sidebar.auto_hide_sidebar)
            .build();
        let settings_ref = settings.clone();
        auto_hide_row.connect_active_notify(move |row| {
            let active = row.is_active();
            settings_ref.borrow_mut().sidebar.auto_hide_sidebar = active;
            settings_ref.borrow().save();
            if let Some(ref cb) = on_auto_hide_changed {
                cb(active);
            }
        });
        display_group.add(&auto_hide_row);

        drop(s);

        let thresholds_group = adw::PreferencesGroup::builder()
            .title("Resource Thresholds")
            .description("Show resource usage when exceeding threshold")
            .build();

        let (proj_cpu, proj_mem, proc_cpu, proc_mem) = {
            let s = settings.borrow();
            (
                s.sidebar.project_cpu_threshold,
                s.sidebar.project_mem_threshold,
                s.sidebar.process_cpu_threshold,
                s.sidebar.process_mem_threshold,
            )
        };

        let project_cpu_row = adw::ComboRow::builder()
            .title("Project CPU Usage")
            .model(&gtk4::StringList::new(&[
                "Always", "25%", "50%", "100%", "200%", "Never",
            ]))
            .build();
        project_cpu_row.set_selected(proj_cpu);
        let settings_ref = settings.clone();
        project_cpu_row.connect_selected_notify(move |row| {
            settings_ref.borrow_mut().sidebar.project_cpu_threshold = row.selected();
            settings_ref.borrow().save();
        });
        thresholds_group.add(&project_cpu_row);

        let project_mem_row = adw::ComboRow::builder()
            .title("Project Memory Usage")
            .model(&gtk4::StringList::new(&[
                "Always", "500MB", "1GB", "2GB", "8GB", "Never",
            ]))
            .build();
        project_mem_row.set_selected(proj_mem);
        let settings_ref = settings.clone();
        project_mem_row.connect_selected_notify(move |row| {
            settings_ref.borrow_mut().sidebar.project_mem_threshold = row.selected();
            settings_ref.borrow().save();
        });
        thresholds_group.add(&project_mem_row);

        let process_cpu_row = adw::ComboRow::builder()
            .title("Process CPU Usage")
            .model(&gtk4::StringList::new(&[
                "Always", "10%", "30%", "60%", "90%", "Never",
            ]))
            .build();
        process_cpu_row.set_selected(proc_cpu);
        let settings_ref = settings.clone();
        process_cpu_row.connect_selected_notify(move |row| {
            settings_ref.borrow_mut().sidebar.process_cpu_threshold = row.selected();
            settings_ref.borrow().save();
        });
        thresholds_group.add(&process_cpu_row);

        let process_mem_row = adw::ComboRow::builder()
            .title("Process Memory Usage")
            .model(&gtk4::StringList::new(&[
                "Always", "100MB", "500MB", "1GB", "2GB", "Never",
            ]))
            .build();
        process_mem_row.set_selected(proc_mem);
        let settings_ref = settings.clone();
        process_mem_row.connect_selected_notify(move |row| {
            settings_ref.borrow_mut().sidebar.process_mem_threshold = row.selected();
            settings_ref.borrow().save();
        });
        thresholds_group.add(&process_mem_row);

        page.add(&display_group);
        page.add(&thresholds_group);
        page
    }

    fn build_hotkeys_page(
        settings: &SettingsRef,
        keybinding_map: &KeybindingMapRef,
        dialog: &adw::PreferencesDialog,
    ) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Hotkeys")
            .icon_name("preferences-desktop-keyboard-shortcuts-symbolic")
            .build();

        // Collect buttons so we can refresh them all on reset
        let all_buttons: Rc<RefCell<Vec<(ShortcutAction, gtk4::Button)>>> =
            Rc::new(RefCell::new(Vec::new()));

        // Group editable actions by category
        let metadata = action_metadata();
        let categories = ["General", "Navigation", "Terminal"];
        for category in &categories {
            let actions_in_category: Vec<_> = metadata
                .iter()
                .filter(|(_, _, cat)| cat == category)
                .collect();
            if actions_in_category.is_empty() {
                continue;
            }

            let group = adw::PreferencesGroup::builder().title(*category).build();

            for &&(action, display_name, _) in &actions_in_category {
                let row = adw::ActionRow::builder().title(display_name).build();

                let current_label = keybinding_map.borrow().display_string(action);
                let btn = gtk4::Button::builder()
                    .label(&current_label)
                    .css_classes(["flat", "caption", "kbd-badge"])
                    .valign(gtk4::Align::Center)
                    .build();

                let settings_ref = settings.clone();
                let kb_map_ref = keybinding_map.clone();
                let dialog_ref = dialog.clone();
                let all_btns = all_buttons.clone();

                btn.connect_clicked(move |button| {
                    Self::start_key_capture(
                        button,
                        action,
                        &settings_ref,
                        &kb_map_ref,
                        &dialog_ref,
                        &all_btns,
                    );
                });

                row.add_suffix(&btn);
                group.add(&row);
                all_buttons.borrow_mut().push((action, btn.clone()));
            }

            page.add(&group);
        }

        // Reset to Defaults button
        let reset_group = adw::PreferencesGroup::new();
        let reset_btn = gtk4::Button::builder()
            .label("Reset All to Defaults")
            .css_classes(["destructive-action", "pill"])
            .halign(gtk4::Align::Center)
            .build();

        let settings_ref = settings.clone();
        let kb_map_ref = keybinding_map.clone();
        let all_btns = all_buttons.clone();
        reset_btn.connect_clicked(move |_| {
            let defaults = keybindings::KeybindingsSettings::default();
            *kb_map_ref.borrow_mut() = KeybindingMap::from_settings(&defaults);
            settings_ref.borrow_mut().keybindings = defaults;
            settings_ref.borrow().save();
            for (action, btn) in all_btns.borrow().iter() {
                btn.set_label(&kb_map_ref.borrow().display_string(*action));
            }
        });
        reset_group.add(&reset_btn);
        page.add(&reset_group);

        // Non-editable shortcuts
        let fixed_group = adw::PreferencesGroup::builder()
            .title("Fixed Shortcuts")
            .description("These shortcuts cannot be changed")
            .build();

        let fixed_shortcuts = [
            ("Switch to Process 1-9", "Ctrl+1-9"),
            ("Switch to Project 1-9", "Alt+1-9"),
            ("Focus Terminal", "Ctrl+Return"),
            ("Close Palette", "Escape"),
            ("Search Next", "Enter"),
            ("Search Previous", "Shift+Enter"),
            ("Close Search", "Escape"),
        ];

        for (name, shortcut) in &fixed_shortcuts {
            let row = adw::ActionRow::builder().title(*name).build();
            let badge = gtk4::Label::builder()
                .label(*shortcut)
                .css_classes(["caption", "kbd-badge"])
                .valign(gtk4::Align::Center)
                .build();
            row.add_suffix(&badge);
            fixed_group.add(&row);
        }
        page.add(&fixed_group);

        page
    }

    fn start_key_capture(
        button: &gtk4::Button,
        action: ShortcutAction,
        settings: &SettingsRef,
        keybinding_map: &KeybindingMapRef,
        dialog: &adw::PreferencesDialog,
        all_buttons: &Rc<RefCell<Vec<(ShortcutAction, gtk4::Button)>>>,
    ) {
        let original_label = button.label().unwrap_or_default().to_string();
        button.set_label("Press a key combo...");
        button.add_css_class("recording");

        // Tell the window key handler to stand down while we capture
        keybinding_map.borrow().set_capturing(true);

        let key_controller = gtk4::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);

        let btn = button.clone();
        let settings_ref = settings.clone();
        let kb_map_ref = keybinding_map.clone();
        let all_btns = all_buttons.clone();
        let dialog_widget = dialog.clone();
        let original = original_label.clone();

        key_controller.connect_key_pressed(move |controller, keyval, _keycode, state| {
            // Ignore modifier-only keys
            if is_modifier_key(&keyval) {
                return gtk4::glib::Propagation::Stop;
            }

            // Escape cancels capture
            if keyval == gtk4::gdk::Key::Escape {
                btn.set_label(&original);
                btn.remove_css_class("recording");
                kb_map_ref.borrow().set_capturing(false);
                dialog_widget.remove_controller(controller);
                return gtk4::glib::Propagation::Stop;
            }

            let candidate = keybinding_from_event(keyval, state);

            // Check for conflicts
            if let Some(conflict_action) = kb_map_ref.borrow().find_conflict(action, &candidate) {
                let conflict_name = KeybindingMap::action_display_name(conflict_action);
                btn.set_label(&format!("Used by {}", conflict_name));
                btn.remove_css_class("recording");
                btn.add_css_class("conflict");
                kb_map_ref.borrow().set_capturing(false);

                // Revert after 2 seconds
                let btn_revert = btn.clone();
                let orig = original.clone();
                gtk4::glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                    btn_revert.remove_css_class("conflict");
                    btn_revert.set_label(&orig);
                });
                dialog_widget.remove_controller(controller);
                return gtk4::glib::Propagation::Stop;
            }

            // Apply the new binding
            let display = keybinding_to_string(&candidate);
            kb_map_ref.borrow_mut().update_binding(action, candidate);
            kb_map_ref.borrow().set_capturing(false);
            settings_ref
                .borrow_mut()
                .keybindings
                .set(action, display.clone());
            settings_ref.borrow().save();

            btn.set_label(&display);
            btn.remove_css_class("recording");

            // Update button in the all_buttons list (in case same action appears)
            for (a, b) in all_btns.borrow().iter() {
                if *a == action {
                    b.set_label(&display);
                }
            }

            dialog_widget.remove_controller(controller);
            gtk4::glib::Propagation::Stop
        });

        dialog.add_controller(key_controller);
    }

    fn build_tools_page(settings: &SettingsRef) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Tools")
            .icon_name("applications-utilities-symbolic")
            .build();

        let group = adw::PreferencesGroup::builder()
            .title("Default Applications")
            .build();

        let s = settings.borrow();

        let editors: &[(&str, &str)] = &[
            ("xdg-open", "System Default (xdg-open)"),
            ("code", "VS Code (code)"),
            ("cursor", "Cursor (cursor)"),
            ("codium", "VSCodium (codium)"),
            ("zed", "Zed (zed)"),
            ("nvim", "Neovim (nvim)"),
            ("vim", "Vim (vim)"),
            ("hx", "Helix (hx)"),
            ("nano", "Nano (nano)"),
            ("emacs", "Emacs (emacs)"),
            ("kate", "Kate (kate)"),
            ("gedit", "GNOME Text Editor (gedit)"),
            ("sublime_text", "Sublime Text (sublime_text)"),
            ("idea", "IntelliJ IDEA (idea)"),
        ];
        let editor_labels: Vec<&str> = editors.iter().map(|(_, label)| *label).collect();
        let editor_row = adw::ComboRow::builder()
            .title("Default Editor")
            .subtitle("Used when opening projects. Can be overridden per-project.")
            .model(&gtk4::StringList::new(&editor_labels))
            .build();
        let editor_idx = editors
            .iter()
            .position(|(cmd, _)| *cmd == s.tools.default_editor)
            .unwrap_or(0);
        editor_row.set_selected(editor_idx as u32);
        let editors_owned: Vec<String> = editors.iter().map(|(cmd, _)| cmd.to_string()).collect();
        let settings_ref = settings.clone();
        editor_row.connect_selected_notify(move |row| {
            let editor = editors_owned
                .get(row.selected() as usize)
                .map(|s| s.as_str())
                .unwrap_or("xdg-open");
            settings_ref.borrow_mut().tools.default_editor = editor.to_string();
            settings_ref.borrow().save();
        });
        group.add(&editor_row);

        let reuse_row = adw::SwitchRow::builder()
            .title("Reuse Editor Window")
            .subtitle("Open projects in the current editor window instead of a new one")
            .active(s.tools.reuse_editor_window)
            .build();
        let settings_ref = settings.clone();
        reuse_row.connect_active_notify(move |row| {
            settings_ref.borrow_mut().tools.reuse_editor_window = row.is_active();
            settings_ref.borrow().save();
        });
        group.add(&reuse_row);

        let terminals: &[(&str, &str)] = &[
            ("xdg-open", "System Default (xdg-open)"),
            ("gnome-terminal", "GNOME Terminal (gnome-terminal)"),
            ("konsole", "Konsole (konsole)"),
            ("alacritty", "Alacritty (alacritty)"),
            ("kitty", "Kitty (kitty)"),
            ("ghostty", "Ghostty (ghostty)"),
            ("wezterm", "WezTerm (wezterm)"),
            ("foot", "Foot (foot)"),
            ("tilix", "Tilix (tilix)"),
            ("xfce4-terminal", "Xfce Terminal (xfce4-terminal)"),
            ("mate-terminal", "MATE Terminal (mate-terminal)"),
            ("terminator", "Terminator (terminator)"),
            ("st", "st (st)"),
            ("urxvt", "urxvt (urxvt)"),
            ("xterm", "xterm (xterm)"),
        ];
        let terminal_labels: Vec<&str> = terminals.iter().map(|(_, label)| *label).collect();
        let terminal_row = adw::ComboRow::builder()
            .title("Default Terminal")
            .subtitle("Used when opening projects from the sidebar.")
            .model(&gtk4::StringList::new(&terminal_labels))
            .build();
        let terminal_idx = terminals
            .iter()
            .position(|(cmd, _)| *cmd == s.tools.default_terminal)
            .unwrap_or(0);
        terminal_row.set_selected(terminal_idx as u32);
        let terminals_owned: Vec<String> =
            terminals.iter().map(|(cmd, _)| cmd.to_string()).collect();
        let settings_ref = settings.clone();
        terminal_row.connect_selected_notify(move |row| {
            let terminal = terminals_owned
                .get(row.selected() as usize)
                .map(|s| s.as_str())
                .unwrap_or("xdg-open");
            settings_ref.borrow_mut().tools.default_terminal = terminal.to_string();
            settings_ref.borrow().save();
        });
        group.add(&terminal_row);

        drop(s);
        page.add(&group);
        page
    }

    fn build_integrations_page(settings: &SettingsRef) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Integrations")
            .icon_name("network-server-symbolic")
            .build();

        // MCP Server
        let mcp_group = adw::PreferencesGroup::builder()
            .title("MCP Server")
            .description("Allow AI assistants like Claude to control processes")
            .build();

        let mcp_enabled = adw::SwitchRow::builder()
            .title("Enable MCP Server")
            .subtitle("Expose process info via Unix socket")
            .active(settings.borrow().integrations.mcp_enabled)
            .build();

        let settings_ref = settings.clone();
        mcp_enabled.connect_active_notify(move |row| {
            let enabled = row.is_active();
            settings_ref.borrow_mut().integrations.mcp_enabled = enabled;
            settings_ref.borrow().save();
            crate::mcp::bridge::set_mcp_enabled(enabled);
        });

        mcp_group.add(&mcp_enabled);

        // Exposed MCP tools
        let mcp_tools = adw::ExpanderRow::builder()
            .title("Exposed MCP tools")
            .subtitle("7 tools")
            .build();
        let tools = [
            (
                "list_processes",
                "List all managed processes with their current status",
            ),
            (
                "get_project_info",
                "Get project overview with running/total counts",
            ),
            (
                "get_process_status",
                "Get detailed status of a process (PID, uptime, restarts)",
            ),
            (
                "get_process_logs",
                "Get recent terminal output from a process",
            ),
            ("restart_process", "Restart a managed process"),
            ("stop_process", "Stop a running process"),
            ("start_process", "Start a stopped process"),
        ];
        for (name, desc) in &tools {
            let row = adw::ActionRow::builder()
                .title(*name)
                .subtitle(*desc)
                .build();
            mcp_tools.add_row(&row);
        }
        mcp_group.add(&mcp_tools);

        // Setup: CLI tools
        let cli_setup = adw::ExpanderRow::builder()
            .title("Setup: CLI tools")
            .subtitle("Claude Code, Codex, OpenCode, Gemini CLI, Amp, Aider")
            .build();

        let mcp_config = r#"{
  "mcpServers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}"#;
        let claude_code_config = r#"Per-project: add to .mcp.json
Global: add to ~/.claude/settings.json

{
  "mcpServers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}

Auto-detects which project you're in.
If tuxflow-mcp is not in PATH, use the full path."#;
        Self::add_setup_row(
            &cli_setup,
            "Claude Code",
            ".mcp.json or ~/.claude/settings.json",
            claude_code_config,
        );

        let codex_config = r#"codex --mcp-config '{"tuxflow":{"command":"tuxflow-mcp"}}'
Or add to ~/.codex/config.toml under [mcp]"#;
        Self::add_setup_row(
            &cli_setup,
            "Codex",
            "CLI flag or ~/.codex/config.toml",
            codex_config,
        );
        Self::add_setup_row(&cli_setup, "OpenCode", ".opencode/mcp.json", mcp_config);

        let gemini_config = r#"gemini --mcp '{"tuxflow":{"command":"tuxflow-mcp"}}'
Or add to ~/.gemini/settings.json under mcpServers"#;
        Self::add_setup_row(
            &cli_setup,
            "Gemini CLI",
            "CLI flag or ~/.gemini/settings.json",
            gemini_config,
        );
        Self::add_setup_row(&cli_setup, "Amp", ".amp/mcp.json", mcp_config);

        let aider_config = r#"Add to .aider.conf.yml:
mcp-servers:
  - command: tuxflow-mcp"#;
        Self::add_setup_row(&cli_setup, "Aider", ".aider.conf.yml", aider_config);
        mcp_group.add(&cli_setup);

        // Setup: IDEs & apps
        let ide_setup = adw::ExpanderRow::builder()
            .title("Setup: IDEs and apps")
            .subtitle("VS Code, Cursor, Windsurf, Zed, Cline, Claude Desktop")
            .build();

        let vscode_config = r#"Add to .vscode/mcp.json:
{
  "servers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}"#;
        Self::add_setup_row(&ide_setup, "VS Code", ".vscode/mcp.json", vscode_config);

        let cursor_config = r#"Add to .cursor/mcp.json:
{
  "mcpServers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}"#;
        Self::add_setup_row(&ide_setup, "Cursor", ".cursor/mcp.json", cursor_config);
        Self::add_setup_row(&ide_setup, "Windsurf", ".windsurf/mcp.json", cursor_config);

        let zed_config = r#"Add to Zed settings.json:
{
  "context_servers": {
    "tuxflow": {
      "command": { "path": "tuxflow-mcp" }
    }
  }
}"#;
        Self::add_setup_row(&ide_setup, "Zed", "Zed settings.json", zed_config);

        let cline_config = r#"Add via Cline settings:
  Command: tuxflow-mcp"#;
        Self::add_setup_row(&ide_setup, "Cline", "Cline settings panel", cline_config);

        let desktop_config = r#"Add to claude_desktop_config.json:
{
  "mcpServers": {
    "tuxflow": {
      "command": "tuxflow-mcp"
    }
  }
}"#;
        Self::add_setup_row(
            &ide_setup,
            "Claude Desktop",
            "claude_desktop_config.json",
            desktop_config,
        );
        mcp_group.add(&ide_setup);

        page.add(&mcp_group);
        page
    }

    fn add_setup_row(parent: &adw::ExpanderRow, title: &str, subtitle: &str, config_text: &str) {
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .build();

        let copy_btn = gtk4::Button::builder()
            .icon_name("edit-copy-symbolic")
            .valign(gtk4::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("Copy configuration")
            .build();

        let text = config_text.to_string();
        copy_btn.connect_clicked(move |btn| {
            if let Some(display) = gtk4::gdk::Display::default() {
                display.clipboard().set_text(&text);
                btn.set_icon_name("emblem-ok-symbolic");
                let btn_ref = btn.clone();
                gtk4::glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                    btn_ref.set_icon_name("edit-copy-symbolic")
                });
            }
        });

        row.add_suffix(&copy_btn);
        parent.add_row(&row);
    }

    fn build_about_page() -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("About")
            .icon_name("help-about-symbolic")
            .build();

        let group = adw::PreferencesGroup::builder()
            .title("TuxFlow")
            .description("A Linux-native dev environment manager")
            .build();

        let version_row = adw::ActionRow::builder()
            .title("Version")
            .subtitle(env!("CARGO_PKG_VERSION"))
            .build();
        group.add(&version_row);

        let license_row = adw::ActionRow::builder()
            .title("License")
            .subtitle("MIT")
            .build();
        group.add(&license_row);

        let source_row = adw::ActionRow::builder()
            .title("Source Code")
            .subtitle("github.com/your-user/tuxflow")
            .build();
        group.add(&source_row);

        page.add(&group);
        page
    }
}
