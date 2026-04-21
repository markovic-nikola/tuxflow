use std::path::Path;

use gtk4::gio;
use gtk4::prelude::*;

/// Internal: send a desktop notification, optionally with a file-based icon.
fn send(title: &str, body: &str, icon_path: Option<&Path>) {
    let notification = gio::Notification::new(title);
    notification.set_body(Some(body));

    if let Some(path) = icon_path
        && path.is_file()
    {
        let file = gio::File::for_path(path);
        let icon = gio::FileIcon::new(&file);
        notification.set_icon(&icon);
    }

    if let Some(app) = gio::Application::default() {
        app.send_notification(None, &notification);
    } else {
        log::warn!("No application instance for notification: {title}");
    }
}

pub fn notify_crash(project_name: &str, process_name: &str, icon_path: Option<&Path>) {
    send(project_name, &format!("{process_name}: crashed"), icon_path);
}

pub fn notify_restart(
    project_name: &str,
    process_name: &str,
    attempt: u32,
    icon_path: Option<&Path>,
) {
    send(
        project_name,
        &format!("{process_name}: restarting (attempt {attempt})"),
        icon_path,
    );
}

pub fn notify_finish(project_name: &str, process_name: &str, icon_path: Option<&Path>) {
    send(
        project_name,
        &format!("{process_name}: finished"),
        icon_path,
    );
}
