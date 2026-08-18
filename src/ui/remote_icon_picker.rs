//! Pick a project icon from an image file on an ssh host.
//!
//! This used to be an expanding section inside the Edit Project dialog, and
//! that placement is what made it unusable: the Icon row sits partway down a
//! scrolling preferences page with the Commands section under it, so the
//! list only ever got the sliver between the entry and the sheet's bottom
//! edge — growing the dialog bought a row or two and no more. A picker of
//! its own gets the whole sheet, which is what browsing a filesystem wants.

use std::time::Duration;

use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::ui::path_completion;

/// Long enough to skip probing on every keystroke, short enough to feel
/// live over a warm ControlMaster connection.
const SUGGEST_DEBOUNCE_MS: u64 = 200;

pub struct RemoteIconPicker;

impl RemoteIconPicker {
    /// Browse `host` starting at `start_dir`, handing the absolute path of
    /// the chosen image to `on_pick` and closing. Directories descend; the
    /// entry stays editable, so a path can also be typed or pasted.
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        host: &str,
        start_dir: &str,
        on_pick: impl Fn(String) + 'static,
    ) {
        let dialog = adw::Dialog::builder()
            .title(format!("Choose Icon on {host}"))
            .content_width(560)
            .content_height(620)
            .build();
        crate::ui::guard_dialog_maximize(&dialog);

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        headerbar.set_show_start_title_buttons(false);
        headerbar.set_show_end_title_buttons(false);
        let cancel_btn = gtk4::Button::builder().label("Cancel").build();
        headerbar.pack_start(&cancel_btn);
        toolbar_view.add_top_bar(&headerbar);

        let content = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(12)
            .build();

        let entry = gtk4::Entry::builder()
            .placeholder_text("Path to an image on the host…")
            .build();
        content.append(&entry);

        let list = gtk4::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk4::SelectionMode::None);
        // Hug the rows: `boxed-list` paints a card, and a stretched one
        // trails a slab of empty background under a short listing.
        list.set_valign(gtk4::Align::Start);
        // The list is the point of this dialog: it takes every pixel the
        // sheet has left rather than a fixed max-content-height.
        let scroll = gtk4::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .build();
        content.append(&scroll);

        // Shown in the list's place once a probe comes back empty — an
        // empty box reads as "still loading".
        let empty_label = gtk4::Label::builder()
            .label("No folders or images match")
            .css_classes(["dim-label"])
            .vexpand(true)
            .visible(false)
            .build();
        content.append(&empty_label);

        {
            let host = host.to_string();
            let scroll = scroll.clone();
            let empty_label = empty_label.clone();
            path_completion::attach(
                &entry,
                &list,
                Duration::from_millis(SUGGEST_DEBOUNCE_MS),
                move |prefix| {
                    if !prefix.starts_with('/') {
                        return None;
                    }
                    let host = host.clone();
                    Some(move || crate::remote::fs::list_remote_icon_paths(&host, &prefix))
                },
                move |paths| {
                    scroll.set_visible(!paths.is_empty());
                    empty_label.set_visible(paths.is_empty());
                },
            );
        }

        {
            let entry = entry.clone();
            let dialog = dialog.clone();
            list.connect_row_activated(move |_, row| {
                let Some(path) = path_completion::row_path(row) else {
                    return;
                };
                if path.ends_with('/') {
                    // Descend: retyping the entry re-runs the completion
                    // one level deeper.
                    entry.set_text(&path);
                    entry.set_position(-1);
                    entry.grab_focus();
                    return;
                }
                on_pick(path);
                dialog.close();
            });
        }

        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });

        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        // Prefilling the project's own directory both seeds the listing and
        // fires the first completion.
        entry.set_text(&format!("{}/", start_dir.trim_end_matches('/')));
        entry.set_position(-1);

        dialog.present(Some(parent));
        entry.grab_focus();
    }
}
