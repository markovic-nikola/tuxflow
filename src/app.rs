use gtk4::gio;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::settings::AppSettings;
use crate::ui::window::TuxFlowWindow;

const APP_ID: &str = "com.tuxflow.TuxFlow";

pub struct TuxFlowApp {
    app: adw::Application,
}

impl TuxFlowApp {
    pub fn new() -> Self {
        let app = adw::Application::builder()
            .application_id(APP_ID)
            .flags(gio::ApplicationFlags::HANDLES_OPEN)
            .build();

        app.connect_startup(|_app| {
            adw::init().expect("Failed to initialize libadwaita");

            // Register custom icons directory so GTK can find them by name
            let icons_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("data")
                .join("icons");
            gtk4::IconTheme::for_display(&gtk4::gdk::Display::default().unwrap())
                .add_search_path(&icons_dir);

            // Set the window/taskbar icon (for dev; installed apps use the .desktop file)
            gtk4::Window::set_default_icon_name("com.tuxflow.TuxFlow");
        });

        app.connect_activate(|app| {
            Self::setup_and_show(app, None);
        });

        app.connect_open(|app, files, _hint| {
            if let Some(file) = files.first() {
                let path = file.path();
                Self::setup_and_show(app, path);
            }
        });

        Self { app }
    }

    pub fn run(&self) -> gtk4::glib::ExitCode {
        self.app.run()
    }

    fn setup_and_show(app: &adw::Application, project_dir: Option<std::path::PathBuf>) {
        // Apply saved theme preference
        let settings = AppSettings::load();
        let manager = adw::StyleManager::default();
        let scheme = match settings.appearance.theme.as_str() {
            "light" => adw::ColorScheme::ForceLight,
            "system" => adw::ColorScheme::Default,
            _ => adw::ColorScheme::ForceDark,
        };
        manager.set_color_scheme(scheme);

        let window = TuxFlowWindow::new(app, project_dir.as_deref());
        window.present();
    }
}
