use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::pango;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::keybindings::{
    self, KeybindingMap, ShortcutAction, action_metadata, is_modifier_key, keybinding_from_event,
    keybinding_to_string,
};
use crate::config::settings::{AppSettings, AppearanceSettings};

pub type SettingsRef = Rc<RefCell<AppSettings>>;
pub type KeybindingMapRef = Rc<RefCell<KeybindingMap>>;

pub struct SettingsWindow;

impl SettingsWindow {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        on_single_expand_changed: Option<Rc<dyn Fn(bool)>>,
        on_auto_hide_changed: Option<Rc<dyn Fn(bool)>>,
        on_keybind_hints_changed: Option<Rc<dyn Fn(bool)>>,
        on_recent_first_changed: Option<Rc<dyn Fn(bool)>>,
        on_terminal_theme_changed: Option<Rc<dyn Fn(&str)>>,
        on_font_changed: Option<Rc<dyn Fn()>>,
        on_composer_changed: Option<Rc<dyn Fn(bool)>>,
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
            on_keybind_hints_changed,
            on_recent_first_changed,
            on_terminal_theme_changed,
            on_font_changed,
            on_composer_changed,
            &kb_map,
        );
    }

    pub fn show_with_settings(
        parent: &impl IsA<gtk4::Widget>,
        settings: &SettingsRef,
        on_single_expand_changed: Option<Rc<dyn Fn(bool)>>,
        on_auto_hide_changed: Option<Rc<dyn Fn(bool)>>,
        on_keybind_hints_changed: Option<Rc<dyn Fn(bool)>>,
        on_recent_first_changed: Option<Rc<dyn Fn(bool)>>,
        on_terminal_theme_changed: Option<Rc<dyn Fn(&str)>>,
        on_font_changed: Option<Rc<dyn Fn()>>,
        on_composer_changed: Option<Rc<dyn Fn(bool)>>,
        keybinding_map: &KeybindingMapRef,
    ) {
        let dialog = adw::PreferencesDialog::new();
        dialog.set_title("Settings");

        // Appearance page
        let appearance_page =
            Self::build_appearance_page(settings, on_terminal_theme_changed, on_font_changed);
        dialog.add(&appearance_page);

        // Sidebar page
        let sidebar_page = Self::build_sidebar_page(
            settings,
            on_single_expand_changed,
            on_auto_hide_changed,
            on_keybind_hints_changed,
            on_recent_first_changed,
        );
        dialog.add(&sidebar_page);

        // Notifications page
        let notifications_page = Self::build_notifications_page(settings, &dialog);
        dialog.add(&notifications_page);

        // Hotkeys page
        let hotkeys_page = Self::build_hotkeys_page(settings, keybinding_map, &dialog);
        dialog.add(&hotkeys_page);

        // Tools page
        let tools_page = Self::build_tools_page(settings, on_composer_changed);
        dialog.add(&tools_page);

        // Integrations page
        let integrations_page = Self::build_integrations_page(settings);
        dialog.add(&integrations_page);

        // About page
        let about_page = Self::build_about_page();
        dialog.add(&about_page);

        dialog.present(Some(parent));
    }

    /// A picker over the shared accent palette. The app accent and the two
    /// sidebar accents differ only in which settings field they read and
    /// write, so `field` is the whole difference between them.
    fn accent_combo(
        title: &str,
        subtitle: &str,
        settings: &SettingsRef,
        field: fn(&mut AppearanceSettings) -> &mut String,
    ) -> adw::ComboRow {
        let choices = crate::ui::accent::color_choices();
        let row = adw::ComboRow::builder()
            .title(title)
            .subtitle(subtitle)
            .model(&gtk4::StringList::new(&choices))
            .build();
        row.set_selected(crate::ui::accent::color_index(field(
            &mut settings.borrow_mut().appearance,
        )));

        let settings_ref = settings.clone();
        row.connect_selected_notify(move |row| {
            let name = crate::ui::accent::color_name(row.selected());
            let mut s = settings_ref.borrow_mut();
            name.clone_into(field(&mut s.appearance));
            // Every choice re-renders all three: they share one provider.
            crate::ui::accent::apply(&s.appearance);
            s.save();
        });
        row
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

        theme_group.add(&Self::accent_combo(
            "Accent Color",
            "Customize the accent color throughout the UI",
            settings,
            |a| &mut a.accent_color,
        ));
        theme_group.add(&Self::accent_combo(
            "Local Project Accent",
            "Sidebar color for projects on this machine",
            settings,
            |a| &mut a.local_accent_color,
        ));
        theme_group.add(&Self::accent_combo(
            "Remote Project Accent",
            "Sidebar color for projects opened over SSH",
            settings,
            |a| &mut a.remote_accent_color,
        ));
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

        let font_row = adw::ActionRow::builder()
            .title("Font Family")
            .subtitle(&s.appearance.font_family)
            .activatable(true)
            .build();
        let font_row_suffix = gtk4::Image::from_icon_name("go-next-symbolic");
        font_row.add_suffix(&font_row_suffix);
        {
            let settings_ref = settings.clone();
            let font_cb = on_font_changed.clone();
            let font_row_ref = font_row.clone();
            font_row.connect_activated(move |row| {
                let win_ref = row.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
                let dialog = gtk4::FontDialog::builder().build();

                // Set current font as initial selection
                let current = settings_ref.borrow().appearance.font_family.clone();
                let current_size = settings_ref.borrow().appearance.font_size;
                let initial =
                    pango::FontDescription::from_string(&format!("{current} {current_size}"));

                let sr = settings_ref.clone();
                let cb = font_cb.clone();
                let row = font_row_ref.clone();
                dialog.choose_font(
                    win_ref.as_ref(),
                    Some(&initial),
                    gtk4::gio::Cancellable::NONE,
                    move |result| {
                        if let Ok(font_desc) = result {
                            let family = font_desc
                                .family()
                                .map(|f| f.to_string())
                                .unwrap_or_default();
                            if !family.is_empty() {
                                row.set_subtitle(&family);
                                let mut s = sr.borrow_mut();
                                s.appearance.font_family = family;
                                // Also update size if the user changed it in the picker
                                let picked_size = font_desc.size() / pango::SCALE;
                                if picked_size > 0 {
                                    s.appearance.font_size = picked_size as u32;
                                }
                                s.save();
                                drop(s);
                                if let Some(ref cb) = cb {
                                    cb();
                                }
                            }
                        }
                    },
                );
            });
        }
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

    fn build_notifications_page(
        settings: &SettingsRef,
        dialog: &adw::PreferencesDialog,
    ) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Notifications")
            .icon_name("preferences-system-notifications-symbolic")
            .build();

        let group = adw::PreferencesGroup::builder()
            .title("Desktop Notifications")
            .build();

        let test_btn = gtk4::Button::builder()
            .label("Send Test")
            .css_classes(["flat"])
            .valign(gtk4::Align::Center)
            .tooltip_text("Fire a sample notification right now")
            .build();
        test_btn.connect_clicked(|_| {
            crate::util::notifications::notify_finish("TuxFlow", "test", None);
        });
        group.set_header_suffix(Some(&test_btn));

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

        let notify_finish_row = adw::SwitchRow::builder()
            .title("Process Finished")
            .subtitle("Notify when a process exits on its own")
            .active(s.notifications.on_process_finish)
            .build();
        let settings_ref = settings.clone();
        notify_finish_row.connect_active_notify(move |row| {
            settings_ref.borrow_mut().notifications.on_process_finish = row.is_active();
            settings_ref.borrow().save();
        });
        group.add(&notify_finish_row);

        let notify_agent_idle_row = adw::SwitchRow::builder()
            .title("Agent Idle")
            .subtitle(
                "Notify when an AI agent finishes its turn (requires agent to emit terminal bell)",
            )
            .active(s.notifications.on_agent_idle)
            .build();
        let settings_ref = settings.clone();
        notify_agent_idle_row.connect_active_notify(move |row| {
            settings_ref.borrow_mut().notifications.on_agent_idle = row.is_active();
            settings_ref.borrow().save();
        });
        group.add(&notify_agent_idle_row);

        let notify_agent_silence_row = adw::SwitchRow::builder()
            .title("Silence-based Fallback")
            .subtitle("Also notify after N seconds of no agent output. May false-positive on long tool calls.")
            .active(s.notifications.on_agent_idle_silence_fallback)
            .build();
        let settings_ref = settings.clone();
        notify_agent_silence_row.connect_active_notify(move |row| {
            settings_ref
                .borrow_mut()
                .notifications
                .on_agent_idle_silence_fallback = row.is_active();
            settings_ref.borrow().save();
        });
        group.add(&notify_agent_silence_row);

        let agent_idle_threshold_row = adw::SpinRow::builder()
            .title("Idle Silence Threshold")
            .subtitle("Seconds of no output before firing the idle notification")
            .adjustment(&gtk4::Adjustment::new(
                s.notifications.agent_idle_silence_seconds as f64,
                5.0,
                120.0,
                1.0,
                5.0,
                0.0,
            ))
            .build();
        let settings_ref = settings.clone();
        agent_idle_threshold_row.connect_changed(move |row| {
            settings_ref
                .borrow_mut()
                .notifications
                .agent_idle_silence_seconds = row.value() as u32;
            settings_ref.borrow().save();
        });
        group.add(&agent_idle_threshold_row);

        let suppress_focused_row = adw::SwitchRow::builder()
            .title("Suppress When Focused")
            .subtitle("Skip notifications for the terminal you're currently viewing")
            .active(s.notifications.suppress_when_focused)
            .build();
        let settings_ref = settings.clone();
        suppress_focused_row.connect_active_notify(move |row| {
            settings_ref
                .borrow_mut()
                .notifications
                .suppress_when_focused = row.is_active();
            settings_ref.borrow().save();
        });
        group.add(&suppress_focused_row);

        page.add(&group);

        // --- Sound group ---
        let sound_group = adw::PreferencesGroup::builder()
            .title("Sound")
            .description("Play a sound alongside each desktop notification")
            .build();

        let sound_enabled_row = adw::SwitchRow::builder()
            .title("Play Sound")
            .subtitle("Requires paplay (pulseaudio-utils)")
            .active(s.notifications.sound_enabled)
            .build();
        let settings_ref = settings.clone();
        sound_enabled_row.connect_active_notify(move |row| {
            settings_ref.borrow_mut().notifications.sound_enabled = row.is_active();
            settings_ref.borrow().save();
        });
        sound_group.add(&sound_enabled_row);

        // Sound picker — bundled sounds ship inside the binary so this list is
        // identical on every install.
        let sound_ids: Vec<String> = crate::util::notifications::BUNDLED_SOUNDS
            .iter()
            .map(|s| s.id.to_string())
            .collect();
        let sound_labels: Vec<&str> = crate::util::notifications::BUNDLED_SOUNDS
            .iter()
            .map(|s| s.label)
            .collect();
        let string_list = gtk4::StringList::new(&sound_labels);
        let current_idx = sound_ids
            .iter()
            .position(|id| *id == s.notifications.sound_name)
            .unwrap_or(0) as u32;
        let sound_combo = adw::ComboRow::builder()
            .title("Notification Sound")
            .model(&string_list)
            .selected(current_idx)
            .build();
        let settings_ref = settings.clone();
        let sound_ids_for_select = sound_ids.clone();
        sound_combo.connect_selected_notify(move |row| {
            let idx = row.selected() as usize;
            if let Some(id) = sound_ids_for_select.get(idx) {
                settings_ref.borrow_mut().notifications.sound_name = id.clone();
                settings_ref.borrow().save();
            }
        });

        let test_btn = gtk4::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text("Preview sound")
            .css_classes(["flat"])
            .valign(gtk4::Align::Center)
            .build();
        let sound_combo_for_test = sound_combo.clone();
        let sound_ids_for_test = sound_ids.clone();
        let dialog_for_test = dialog.clone();
        test_btn.connect_clicked(move |_| {
            let idx = sound_combo_for_test.selected() as usize;
            if let Some(id) = sound_ids_for_test.get(idx)
                && let Err(msg) = crate::util::notifications::play_sound(id)
            {
                let toast = adw::Toast::new(&format!("Sound unavailable: {msg}"));
                toast.set_timeout(6);
                dialog_for_test.add_toast(toast);
            }
        });
        sound_combo.add_suffix(&test_btn);

        sound_group.add(&sound_combo);

        // Per-agent sound overrides. Each row adds a "(Use default)" entry at
        // index 0, followed by all bundled sounds. Selecting "(Use default)"
        // clears the override (falls back to the global sound). OpenCode is
        // intentionally omitted — it emits its own desktop notifications, so
        // TuxFlow stays silent for it.
        let mut per_agent_labels: Vec<&str> = vec!["(Use default)"];
        per_agent_labels.extend_from_slice(&sound_labels);

        let agents: [(
            &str,
            fn(&crate::config::settings::NotificationSettings) -> Option<String>,
        ); 3] = [
            ("Claude Sound", |n| n.claude_sound_name.clone()),
            ("Codex Sound", |n| n.codex_sound_name.clone()),
            ("Gemini Sound", |n| n.gemini_sound_name.clone()),
        ];

        for (idx, (title, getter)) in agents.iter().enumerate() {
            let list = gtk4::StringList::new(&per_agent_labels);
            let current = getter(&s.notifications);
            let selected = current
                .as_ref()
                .and_then(|id| sound_ids.iter().position(|s| s == id))
                .map(|i| (i + 1) as u32) // +1 because "(Use default)" is index 0
                .unwrap_or(0);
            let combo = adw::ComboRow::builder()
                .title(*title)
                .model(&list)
                .selected(selected)
                .build();

            let settings_ref = settings.clone();
            let sound_ids_for_select = sound_ids.clone();
            let agent_idx = idx;
            combo.connect_selected_notify(move |row| {
                let i = row.selected() as usize;
                let value = if i == 0 {
                    None
                } else {
                    sound_ids_for_select.get(i - 1).cloned()
                };
                let mut s = settings_ref.borrow_mut();
                match agent_idx {
                    0 => s.notifications.claude_sound_name = value,
                    1 => s.notifications.codex_sound_name = value,
                    2 => s.notifications.gemini_sound_name = value,
                    _ => {}
                }
                drop(s);
                settings_ref.borrow().save();
            });

            let test_btn = gtk4::Button::builder()
                .icon_name("media-playback-start-symbolic")
                .tooltip_text("Preview sound")
                .css_classes(["flat"])
                .valign(gtk4::Align::Center)
                .build();
            let combo_for_test = combo.clone();
            let sound_ids_for_test = sound_ids.clone();
            let dialog_for_test = dialog.clone();
            let settings_for_test = settings.clone();
            test_btn.connect_clicked(move |_| {
                let i = combo_for_test.selected() as usize;
                // "(Use default)" → play the global sound; otherwise play the
                // selected per-agent sound.
                let id = if i == 0 {
                    settings_for_test.borrow().notifications.sound_name.clone()
                } else {
                    match sound_ids_for_test.get(i - 1) {
                        Some(id) => id.clone(),
                        None => return,
                    }
                };
                if let Err(msg) = crate::util::notifications::play_sound(&id) {
                    let toast = adw::Toast::new(&format!("Sound unavailable: {msg}"));
                    toast.set_timeout(6);
                    dialog_for_test.add_toast(toast);
                }
            });
            combo.add_suffix(&test_btn);

            sound_group.add(&combo);
        }

        // Subtle note: OpenCode is handled specially.
        let opencode_note = adw::ActionRow::builder()
            .title("OpenCode")
            .subtitle("Uses its own desktop notifications — TuxFlow stays silent for it.")
            .sensitive(false)
            .build();
        sound_group.add(&opencode_note);

        page.add(&sound_group);

        drop(s);
        page
    }

    fn build_sidebar_page(
        settings: &SettingsRef,
        on_single_expand_changed: Option<Rc<dyn Fn(bool)>>,
        on_auto_hide_changed: Option<Rc<dyn Fn(bool)>>,
        on_keybind_hints_changed: Option<Rc<dyn Fn(bool)>>,
        on_recent_first_changed: Option<Rc<dyn Fn(bool)>>,
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

        let keybind_hints_row = adw::SwitchRow::builder()
            .title("Show Keybind Hints")
            .subtitle("Show Ctrl+1..9 shortcuts on running processes in the sidebar")
            .active(s.sidebar.show_keybind_hints)
            .build();
        let settings_ref = settings.clone();
        keybind_hints_row.connect_active_notify(move |row| {
            let active = row.is_active();
            settings_ref.borrow_mut().sidebar.show_keybind_hints = active;
            settings_ref.borrow().save();
            if let Some(ref cb) = on_keybind_hints_changed {
                cb(active);
            }
        });
        display_group.add(&keybind_hints_row);

        let recent_first_row = adw::SwitchRow::builder()
            .title("Recently Used First")
            .subtitle("Keep recently started projects at the top of the sidebar")
            .active(s.sidebar.recent_first)
            .build();
        let settings_ref = settings.clone();
        recent_first_row.connect_active_notify(move |row| {
            let active = row.is_active();
            settings_ref.borrow_mut().sidebar.recent_first = active;
            settings_ref.borrow().save();
            if let Some(ref cb) = on_recent_first_changed {
                cb(active);
            }
        });
        display_group.add(&recent_first_row);

        drop(s);

        page.add(&display_group);
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

    fn build_tools_page(
        settings: &SettingsRef,
        on_composer_changed: Option<Rc<dyn Fn(bool)>>,
    ) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Tools")
            .icon_name("applications-utilities-symbolic")
            .build();

        let agents_group = adw::PreferencesGroup::builder().title("Agents").build();
        let composer_row = adw::SwitchRow::builder()
            .title("Message Composer")
            .subtitle(
                "Compose messages locally under agent terminals and send in one go \u{2014} \
                 avoids per-keystroke lag on remote projects",
            )
            .active(settings.borrow().tools.agent_composer)
            .build();
        let settings_ref = settings.clone();
        composer_row.connect_active_notify(move |row| {
            settings_ref.borrow_mut().tools.agent_composer = row.is_active();
            settings_ref.borrow().save();
            if let Some(ref cb) = on_composer_changed {
                cb(row.is_active());
            }
        });
        agents_group.add(&composer_row);

        let mic_row = adw::SwitchRow::builder()
            .title("Remote Microphone")
            .subtitle(
                "Let agents on remote hosts record voice input through this machine's \
                 microphone \u{2014} while a remote project is open, the host can listen",
            )
            .active(settings.borrow().tools.remote_microphone)
            .build();
        let settings_ref = settings.clone();
        mic_row.connect_active_notify(move |row| {
            settings_ref.borrow_mut().tools.remote_microphone = row.is_active();
            settings_ref.borrow().save();
            // Applies live, to projects that are already open in both
            // directions — on bridges them, off tears every bridge down.
            crate::remote::mic::set_enabled(row.is_active());
            // Flipping this on is the one moment the user is actually looking
            // for a result, so report failures instead of only logging them.
            if row.is_active() {
                crate::util::worker::run(crate::remote::mic::wait_ready_all, |failures| {
                    for (host, reason) in failures {
                        crate::util::notifications::notify_mic_bridge_failed(&host, &reason);
                    }
                });
            }
        });
        agents_group.add(&mic_row);
        page.add(&agents_group);

        let group = adw::PreferencesGroup::builder()
            .title("Default Applications")
            .build();

        let s = settings.borrow();

        let editors = crate::config::settings::EDITOR_CHOICES;
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

        let terminals = crate::config::settings::TERMINAL_CHOICES;
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

        // Exposed MCP tools — copy shared with the iced shell (core mcp::setup).
        use tuxflow_core::mcp::setup;
        let mcp_tools = adw::ExpanderRow::builder()
            .title("Exposed MCP tools")
            .subtitle(&format!("{} tools", setup::EXPOSED_TOOLS.len()))
            .build();
        for (name, desc) in setup::EXPOSED_TOOLS {
            let row = adw::ActionRow::builder()
                .title(*name)
                .subtitle(*desc)
                .build();
            mcp_tools.add_row(&row);
        }
        mcp_group.add(&mcp_tools);

        let cli_setup = adw::ExpanderRow::builder()
            .title("Setup: CLI tools")
            .subtitle("Claude Code, Codex, OpenCode, Gemini CLI, Amp, Aider")
            .build();
        for (tool, location, config) in setup::CLI_SETUP {
            Self::add_setup_row(&cli_setup, tool, location, config);
        }
        mcp_group.add(&cli_setup);

        let ide_setup = adw::ExpanderRow::builder()
            .title("Setup: IDEs and apps")
            .subtitle("VS Code, Cursor, Windsurf, Zed, Cline, Claude Desktop")
            .build();
        for (tool, location, config) in setup::IDE_SETUP {
            Self::add_setup_row(&ide_setup, tool, location, config);
        }
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
            .subtitle("github.com/markovic-nikola/tuxflow")
            .activatable(true)
            .build();
        source_row.connect_activated(|row| {
            let launcher = gtk4::UriLauncher::new("https://github.com/markovic-nikola/tuxflow");
            let window = row.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            launcher.launch(window.as_ref(), gtk4::gio::Cancellable::NONE, |_| {});
        });
        group.add(&source_row);

        page.add(&group);
        page
    }
}
