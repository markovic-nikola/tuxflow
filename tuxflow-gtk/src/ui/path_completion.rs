//! Debounced path completion against an ssh host, shared by every dialog
//! that lets you type a path that lives on the other end of a connection.
//!
//! The listing is a blocking ssh exec, so it runs on a worker thread; the
//! generation counter drops results whose keystroke has already been
//! superseded, which is what keeps a slow probe from overwriting the rows
//! for what the user is typing now.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk4::glib;
use gtk4::prelude::*;

/// Wire `entry` to fill `list` with remote path suggestions.
///
/// `make_job` runs on the main thread with the entry's current text and
/// returns the blocking listing to run on a worker — or `None` when the
/// input can't be completed yet (relative path, host field still empty),
/// which clears the list. `on_results` runs after the rows are rebuilt,
/// for whatever each dialog does around them (visibility, sizing).
pub fn attach<J>(
    entry: &impl IsA<gtk4::Editable>,
    list: &gtk4::ListBox,
    debounce: Duration,
    make_job: impl Fn(String) -> Option<J> + 'static,
    on_results: impl Fn(&[String]) + 'static,
) where
    J: FnOnce() -> Vec<String> + Send + 'static,
{
    let generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let make_job = Rc::new(make_job);
    let on_results = Rc::new(on_results);
    let list = list.clone();

    entry.connect_changed(move |entry| {
        let my_gen = generation.get() + 1;
        generation.set(my_gen);

        let Some(job) = make_job(entry.text().to_string()) else {
            clear(&list);
            on_results(&[]);
            return;
        };

        let gen_at_fire = generation.clone();
        let list = list.clone();
        let on_results = on_results.clone();
        glib::timeout_add_local_once(debounce, move || {
            if gen_at_fire.get() != my_gen {
                return; // superseded by a newer keystroke
            }
            crate::util::worker::run(job, move |paths| {
                if gen_at_fire.get() != my_gen {
                    return; // superseded while probing
                }
                clear(&list);
                for path in &paths {
                    list.append(&row_for(path));
                }
                on_results(&paths);
            });
        });
    });
}

/// Read the path back off a row built by [`attach`].
pub fn row_path(row: &gtk4::ListBoxRow) -> Option<String> {
    row.child()
        .and_downcast::<gtk4::Label>()
        .map(|label| label.text().to_string())
}

fn row_for(path: &str) -> gtk4::ListBoxRow {
    // Ellipsize at the start: these are absolute paths, so the tail (the
    // name being completed) is the part worth reading.
    let label = gtk4::Label::builder()
        .label(path)
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::Start)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&label));
    row
}

fn clear(list: &gtk4::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}
