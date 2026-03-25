use gtk4::gio;
use gtk4::prelude::*;

pub fn send_notification(title: &str, body: &str) {
    let notification = gio::Notification::new(title);
    notification.set_body(Some(body));

    if let Some(app) = gio::Application::default() {
        app.send_notification(None, &notification);
    } else {
        log::warn!("No application instance for notification: {title}");
    }
}

pub fn notify_crash(process_name: &str) {
    send_notification("Process Crashed", &format!("{process_name} has crashed"));
}

pub fn notify_restart(process_name: &str, attempt: u32) {
    send_notification(
        "Process Restarting",
        &format!("{process_name} is restarting (attempt {attempt})"),
    );
}
