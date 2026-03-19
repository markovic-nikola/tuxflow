use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;

pub struct SettingsWindow;

impl SettingsWindow {
    pub fn show(parent: &impl IsA<gtk4::Widget>) {
        let dialog = adw::PreferencesDialog::new();
        dialog.set_title("Settings");

        // Appearance page
        let appearance_page = Self::build_appearance_page();
        dialog.add(&appearance_page);

        // Notifications page
        let notifications_page = Self::build_notifications_page();
        dialog.add(&notifications_page);

        // About page
        let about_page = Self::build_about_page();
        dialog.add(&about_page);

        dialog.present(Some(parent));
    }

    fn build_appearance_page() -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Appearance")
            .icon_name("applications-graphics-symbolic")
            .build();

        // Theme group
        let theme_group = adw::PreferencesGroup::builder()
            .title("Theme")
            .build();

        let theme_row = adw::ComboRow::builder()
            .title("Color Scheme")
            .subtitle("Choose the application theme")
            .model(&gtk4::StringList::new(&["System", "Dark", "Light"]))
            .build();

        // Default to Dark (index 1)
        theme_row.set_selected(1);

        theme_row.connect_selected_notify(|row| {
            let manager = adw::StyleManager::default();
            match row.selected() {
                0 => manager.set_color_scheme(adw::ColorScheme::Default),
                1 => manager.set_color_scheme(adw::ColorScheme::ForceDark),
                2 => manager.set_color_scheme(adw::ColorScheme::ForceLight),
                _ => {}
            }
        });

        theme_group.add(&theme_row);
        page.add(&theme_group);

        // Terminal font group
        let font_group = adw::PreferencesGroup::builder()
            .title("Terminal")
            .build();

        let font_row = adw::EntryRow::builder()
            .title("Font Family")
            .text("Monospace")
            .build();
        font_group.add(&font_row);

        let font_size_row = adw::SpinRow::builder()
            .title("Font Size")
            .adjustment(&gtk4::Adjustment::new(12.0, 6.0, 32.0, 1.0, 2.0, 0.0))
            .build();
        font_group.add(&font_size_row);

        let scrollback_row = adw::SpinRow::builder()
            .title("Scrollback Lines")
            .adjustment(&gtk4::Adjustment::new(10000.0, 100.0, 100000.0, 100.0, 1000.0, 0.0))
            .build();
        font_group.add(&scrollback_row);

        page.add(&font_group);

        page
    }

    fn build_notifications_page() -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title("Notifications")
            .icon_name("preferences-system-notifications-symbolic")
            .build();

        let group = adw::PreferencesGroup::builder()
            .title("Desktop Notifications")
            .build();

        let notify_crash_row = adw::SwitchRow::builder()
            .title("Process Crash")
            .subtitle("Notify when a process crashes")
            .active(true)
            .build();
        group.add(&notify_crash_row);

        let notify_restart_row = adw::SwitchRow::builder()
            .title("Auto-Restart")
            .subtitle("Notify when a process is auto-restarted")
            .active(true)
            .build();
        group.add(&notify_restart_row);

        let notify_file_row = adw::SwitchRow::builder()
            .title("File Watch Restart")
            .subtitle("Notify when a file change triggers a restart")
            .active(false)
            .build();
        group.add(&notify_file_row);

        page.add(&group);
        page
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
