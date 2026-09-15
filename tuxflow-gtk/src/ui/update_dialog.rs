//! "Update available" dialog: release notes plus a one-click install.
//!
//! Replaces the old badge behaviour, which opened the GitHub release page and
//! left the user to download a .deb, run it, authenticate, and then work out
//! that the running window was still the old build.

use adw::prelude::*;
use gtk4::glib;
use libadwaita as adw;

use crate::util::update_checker::{self, InstallKind, UpdateInfo};

pub fn present(parent: Option<&gtk4::Window>, info: &UpdateInfo) {
    let dialog = adw::MessageDialog::builder()
        .heading(format!("TuxFlow v{} is available", info.latest_version))
        .body_use_markup(false)
        .modal(true)
        .build();
    if let Some(p) = parent {
        dialog.set_transient_for(Some(p));
    }

    if !info.notes.trim().is_empty() {
        dialog.set_extra_child(Some(&notes_view(&info.notes)));
    }

    // Installing in place only works for a dpkg-owned binary; a tarball or
    // `cargo run` build has nothing for apt to upgrade.
    let can_install =
        matches!(update_checker::install_kind(), InstallKind::Deb) && info.deb_url.is_some();

    dialog.add_response("later", "Later");
    dialog.add_response("page", "View release");
    if can_install {
        dialog.add_response("install", "Install and restart");
        dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("install"));
    } else {
        dialog.set_default_response(Some("page"));
    }
    dialog.set_close_response("later");

    let info = info.clone();
    dialog.connect_response(None, move |dlg, response| {
        match response {
            "install" => {
                let Some(url) = info.deb_url.clone() else {
                    return;
                };
                install_flow(dlg.transient_for().as_ref(), url);
            }
            "page" => {
                let launcher = gtk4::UriLauncher::new(&info.release_url);
                launcher.launch(
                    dlg.transient_for().as_ref(),
                    gtk4::gio::Cancellable::NONE,
                    |_| {},
                );
            }
            _ => {}
        }
        dlg.close();
    });

    dialog.present();
}

/// Release notes are markdown; render them as plain selectable text in a
/// bounded scroller rather than pulling in a markdown widget.
fn notes_view(notes: &str) -> gtk4::ScrolledWindow {
    let label = gtk4::Label::builder()
        .label(notes.trim())
        .wrap(true)
        .selectable(true)
        .xalign(0.0)
        .build();
    label.add_css_class("dim-label");

    gtk4::ScrolledWindow::builder()
        .child(&label)
        .min_content_height(140)
        .max_content_height(320)
        .propagate_natural_height(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build()
}

/// Download, install via one polkit prompt, then offer to relaunch. Both the
/// download and `pkexec` block, so they run on a worker thread.
fn install_flow(parent: Option<&gtk4::Window>, deb_url: String) {
    let progress = adw::MessageDialog::builder()
        .heading("Installing update")
        .body("Downloading…")
        .modal(true)
        .build();
    if let Some(p) = parent {
        progress.set_transient_for(Some(p));
    }
    progress.present();

    let progress_ref = progress.clone();
    crate::util::worker::run(
        move || {
            let path = update_checker::download_deb(&deb_url)?;
            let result = update_checker::install_deb(&path);
            let _ = std::fs::remove_file(&path);
            result
        },
        move |result: Result<(), String>| {
            progress_ref.close();
            let parent = progress_ref.transient_for();
            match result {
                Ok(()) => offer_restart(parent.as_ref()),
                Err(msg) => {
                    let err = adw::MessageDialog::builder()
                        .heading("Update failed")
                        .body(msg)
                        .modal(true)
                        .build();
                    if let Some(p) = parent.as_ref() {
                        err.set_transient_for(Some(p));
                    }
                    err.add_response("ok", "OK");
                    err.connect_response(None, |d, _| d.close());
                    err.present();
                }
            }
        },
    );
}

/// Also the whole flow when the update arrived from outside the app (the
/// system's software manager): the new version is already on disk, so there
/// is nothing to download and the only thing left to ask for is the restart.
pub fn offer_restart(parent: Option<&gtk4::Window>) {
    let dialog = adw::MessageDialog::builder()
        .heading("Update installed")
        .body("Restart TuxFlow to run the new version. Remote processes keep running while it restarts.")
        .modal(true)
        .build();
    if let Some(p) = parent {
        dialog.set_transient_for(Some(p));
    }
    dialog.add_response("later", "Later");
    dialog.add_response("restart", "Restart now");
    dialog.set_response_appearance("restart", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("restart"));

    dialog.connect_response(None, |dlg, response| {
        if response == "restart" {
            let parent = dlg.transient_for();
            match update_checker::restart() {
                // The relauncher waits for this process to exit before
                // starting the new one, so quitting is what triggers it.
                Ok(()) => {
                    glib::idle_add_local_once(|| {
                        if let Some(app) = gtk4::gio::Application::default() {
                            app.quit();
                        }
                    });
                }
                // Surfaced, not just logged: a silent failure here reads as
                // the button doing nothing at all.
                Err(msg) => {
                    log::error!("Restart failed: {msg}");
                    let err = adw::MessageDialog::builder()
                        .heading("Could not restart")
                        .body(format!(
                            "{msg}\n\nThe update is installed — quit and start \
                             TuxFlow again to use it."
                        ))
                        .modal(true)
                        .build();
                    if let Some(p) = parent.as_ref() {
                        err.set_transient_for(Some(p));
                    }
                    err.add_response("ok", "OK");
                    err.connect_response(None, |d, _| d.close());
                    err.present();
                }
            }
        }
        dlg.close();
    });
    dialog.present();
}
