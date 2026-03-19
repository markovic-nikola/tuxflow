use gtk4::prelude::*;
use gtk4::gio;
use libadwaita as adw;

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
        });

        app.connect_activate(|app| {
            Self::setup_and_show(app, std::env::current_dir().ok());
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
        // Set dark preference
        let manager = adw::StyleManager::default();
        manager.set_color_scheme(adw::ColorScheme::PreferDark);

        let window = TuxFlowWindow::new(app, project_dir.as_deref());
        window.present();
    }
}
