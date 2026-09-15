//! The Git Changes dialog: file list, per-file diff, commit box and
//! push/pull. Every git call it makes now lives in
//! `tuxflow_core::remote::git` — both shells run on that plumbing, so
//! only the widgets are left here.

use std::cell::Cell;
use std::rc::Rc;

use tokio::sync::oneshot;

use adw::prelude::*;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::remote::ProjectLocation;
use tuxflow_core::remote::git::{
    ChangedFile, DiffResult, FileStatus, changed_files, commit_all, load_diff, pull, push,
    status_hash,
};

// window.rs imports its git helpers from here; re-exporting keeps those
// call sites unchanged now that the implementations live in core.
pub use tuxflow_core::remote::git::{
    commits_ahead, commits_behind, current_branch, diff_shortstat, fetch as git_fetch,
    has_git_repo, sync_with_remote, untracked_count,
};

/// GTK-only half of `FileStatus`: which stylesheet class paints the badge.
fn status_css_class(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Modified | FileStatus::Renamed => "git-status-modified",
        FileStatus::Added => "git-status-added",
        FileStatus::Deleted => "git-status-deleted",
        FileStatus::Untracked => "git-status-untracked",
    }
}

fn update_push_button(btn: &gtk4::Button, ahead: usize) {
    if ahead > 0 {
        btn.set_label(&format!("Push ({ahead})"));
        btn.set_sensitive(true);
    } else {
        btn.set_label("Push");
        btn.set_sensitive(false);
    }
}

fn update_pull_button(btn: &gtk4::Button, behind: usize) {
    if behind > 0 {
        btn.set_label(&format!("Pull ({behind})"));
        btn.set_visible(true);
    } else {
        btn.set_label("Pull");
        btn.set_visible(false);
    }
}

fn show_error_dialog(parent: &impl IsA<gtk4::Widget>, heading: &str, message: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(message)
        .build();
    dialog.add_response("ok", "OK");
    dialog.set_default_response(Some("ok"));
    dialog.present(Some(parent));
}

fn build_file_row(file: &ChangedFile) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.add_css_class("git-file-row");
    row.set_margin_start(4);
    row.set_margin_end(4);

    let badge = gtk4::Label::builder()
        .label(file.status.label())
        .css_classes([status_css_class(file.status)])
        .build();
    row.append(&badge);

    let path_label = gtk4::Label::builder()
        .label(&file.path)
        // Ellipsized — hover shows the full path.
        .tooltip_text(&file.path)
        .ellipsize(gtk4::pango::EllipsizeMode::Start)
        .hexpand(true)
        .xalign(0.0)
        .build();
    row.append(&path_label);

    row
}

fn apply_styling(buffer: &gtk4::TextBuffer, result: &DiffResult) {
    let tag_table = buffer.tag_table();

    // Diff background tags
    tag_table.add(
        &gtk4::TextTag::builder()
            .name("addition")
            .background("rgba(115,201,145,0.15)")
            .build(),
    );
    tag_table.add(
        &gtk4::TextTag::builder()
            .name("deletion")
            .background("rgba(241,76,76,0.15)")
            .build(),
    );
    buffer.set_text(&result.text);

    // Cache an iter at the start of each line by walking the buffer once.
    // The previous version called `buffer.iter_at_offset(abs)` per tag span,
    // which is O(buffer_size) each — a few thousand spans against a 100 KB+
    // buffer froze the main loop. Walking once and cloning/advancing is
    // O(buffer + spans).
    let line_count = result.text.lines().count();
    let mut line_start_iters: Vec<gtk4::TextIter> = Vec::with_capacity(line_count);
    let mut walker = buffer.start_iter();
    for _ in 0..line_count {
        line_start_iters.push(walker.clone());
        if !walker.forward_line() {
            break;
        }
    }

    // Apply diff background tags by cloning each cached line-start iter and
    // advancing to line end (cheap — O(line length)).
    for (line_idx, line) in result.text.lines().enumerate() {
        let Some(start) = line_start_iters.get(line_idx) else {
            break;
        };
        let mut end = start.clone();
        end.forward_to_line_end();

        if line.starts_with('+') && !line.starts_with("+++") {
            buffer.apply_tag_by_name("addition", start, &end);
        } else if line.starts_with('-') && !line.starts_with("---") {
            buffer.apply_tag_by_name("deletion", start, &end);
        }
    }

    // Apply syntax foreground tags via short forward_chars hops from the
    // cached line-start iter. (Note: this treats syntect's byte offsets as
    // char offsets — matches prior behavior, correct for ASCII source.)
    for (line_idx, byte_off, len, color) in &result.highlights {
        let Some(line_start) = line_start_iters.get(*line_idx) else {
            continue;
        };
        let tag_name = format!("fg_{color}");
        if tag_table.lookup(&tag_name).is_none() {
            tag_table.add(
                &gtk4::TextTag::builder()
                    .name(&tag_name)
                    .foreground(color.as_str())
                    .build(),
            );
        }
        let mut span_start = line_start.clone();
        span_start.forward_chars(*byte_off as i32);
        let mut span_end = span_start.clone();
        span_end.forward_chars(*len as i32);
        buffer.apply_tag_by_name(&tag_name, &span_start, &span_end);
    }
}

pub struct GitChangesDialog;

/// Last-known sync state, carried over from the status-bar poller so the
/// dialog can render the Push/Pull buttons and branch label immediately
/// instead of waiting for its own fetch + recount round trip.
pub struct GitSeed {
    pub ahead: usize,
    pub behind: usize,
    pub branch: Option<String>,
}

impl GitChangesDialog {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        location: &ProjectLocation,
        seed: GitSeed,
        on_git_state_changed: impl Fn() + 'static,
    ) {
        let on_changed: Rc<dyn Fn()> = Rc::new(on_git_state_changed);

        // Match the parent window size
        let (w, h) = parent
            .root()
            .and_then(|r| r.downcast::<gtk4::Window>().ok())
            .map(|win| (win.width(), win.height()))
            .unwrap_or((900, 700));

        let dialog = adw::Dialog::builder()
            .title("Git Changes")
            .content_width(w)
            .content_height(h)
            .build();
        crate::ui::guard_dialog_maximize(&dialog);

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        headerbar.set_show_start_title_buttons(false);
        headerbar.set_show_end_title_buttons(false);

        let close_btn = gtk4::Button::builder().label("Close").build();
        headerbar.pack_start(&close_btn);

        let refresh_btn = gtk4::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Refresh")
            .css_classes(["flat"])
            .build();
        headerbar.pack_end(&refresh_btn);
        toolbar_view.add_top_bar(&headerbar);

        let dialog_close = dialog.clone();
        close_btn.connect_clicked(move |_| {
            dialog_close.close();
        });

        // Content area — starts with a spinner
        let content_stack = gtk4::Stack::new();
        content_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        content_stack.set_transition_duration(150);

        let spinner = gtk4::Spinner::new();
        spinner.start();
        spinner.set_width_request(32);
        spinner.set_height_request(32);
        spinner.set_halign(gtk4::Align::Center);
        spinner.set_valign(gtk4::Align::Center);
        content_stack.add_named(&spinner, Some("loading"));

        // Empty state
        let empty_label = gtk4::Label::builder()
            .label("No changes")
            .css_classes(["dim-label", "title-3"])
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .vexpand(true)
            .build();
        content_stack.add_named(&empty_label, Some("empty"));

        // Paned view: file list + diff
        let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
        paned.set_position(260);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);

        let listbox = gtk4::ListBox::new();
        listbox.set_selection_mode(gtk4::SelectionMode::Single);
        listbox.add_css_class("navigation-sidebar");

        let list_scroll = gtk4::ScrolledWindow::builder()
            .child(&listbox)
            .min_content_width(200)
            .vexpand(true)
            .build();
        paned.set_start_child(Some(&list_scroll));

        let diff_view = gtk4::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk4::WrapMode::None)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(12)
            .right_margin(12)
            .build();

        let diff_scroll = gtk4::ScrolledWindow::builder()
            .child(&diff_view)
            .vexpand(true)
            .hexpand(true)
            .build();
        paned.set_end_child(Some(&diff_scroll));

        content_stack.add_named(&paned, Some("content"));
        content_stack.set_visible_child_name("loading");

        toolbar_view.set_content(Some(&content_stack));

        // Bottom bar: commit message + action buttons
        let bottom_bar = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        bottom_bar.set_margin_start(8);
        bottom_bar.set_margin_end(8);
        bottom_bar.set_margin_top(8);
        bottom_bar.set_margin_bottom(8);

        // Multi-line commit message input
        let commit_textview = gtk4::TextView::builder()
            .wrap_mode(gtk4::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        commit_textview.add_css_class("commit-textview");

        let commit_scroll = gtk4::ScrolledWindow::builder()
            .child(&commit_textview)
            .hexpand(true)
            .min_content_height(72)
            .max_content_height(72)
            .has_frame(true)
            .build();

        // Placeholder label via overlay
        let placeholder_label = gtk4::Label::builder()
            .label("Commit message...")
            .halign(gtk4::Align::Start)
            .valign(gtk4::Align::Start)
            .margin_start(12)
            .margin_top(10)
            .css_classes(["dim-label"])
            .can_target(false)
            .build();

        let commit_overlay = gtk4::Overlay::new();
        commit_overlay.set_child(Some(&commit_scroll));
        commit_overlay.add_overlay(&placeholder_label);

        bottom_bar.append(&commit_overlay);

        // Button row
        let buttons_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);

        let commit_btn = gtk4::Button::builder()
            .label("Commit")
            .css_classes(["suggested-action"])
            .sensitive(false)
            .build();

        let branch_label = gtk4::Label::builder()
            .label("")
            .css_classes(["dim-label", "caption"])
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .max_width_chars(40)
            .build();

        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);

        let pull_btn = gtk4::Button::builder()
            .label("Pull")
            .css_classes(["git-pull-btn"])
            .visible(false)
            .build();

        let push_btn = gtk4::Button::builder()
            .label("Push")
            .css_classes(["git-push-btn"])
            .build();

        buttons_row.append(&commit_btn);
        buttons_row.append(&branch_label);
        buttons_row.append(&spacer);
        buttons_row.append(&pull_btn);
        buttons_row.append(&push_btn);

        bottom_bar.append(&buttons_row);
        toolbar_view.add_bottom_bar(&bottom_bar);

        dialog.set_child(Some(&toolbar_view));

        // Store files for selection callback
        let files_store = std::rc::Rc::new(std::cell::RefCell::new(Vec::<ChangedFile>::new()));
        let dir = location.clone();

        // Seed branch label and Push/Pull buttons from the status-bar
        // poller's last-known state — instant, no git calls on open.
        if let Some(ref branch) = seed.branch {
            branch_label.set_label(&format!("⎇ {branch}"));
        }
        update_push_button(&push_btn, seed.ahead);
        update_pull_button(&pull_btn, seed.behind);

        // Then fetch + recount in the background to correct the seed
        // (the ↓ count is only as fresh as the last fetch).
        {
            let dir_init = dir.clone();
            let push_btn_init = push_btn.clone();
            let pull_btn_init = pull_btn.clone();
            let branch_label_init = branch_label.clone();
            let (tx, rx) = oneshot::channel::<(usize, usize, Option<String>)>();
            std::thread::spawn(move || {
                git_fetch(&dir_init);
                let ahead = commits_ahead(&dir_init);
                let behind = commits_behind(&dir_init);
                let _ = tx.send((ahead, behind, current_branch(&dir_init)));
            });
            glib::spawn_future_local(async move {
                if let Ok((ahead, behind, branch)) = rx.await {
                    update_push_button(&push_btn_init, ahead);
                    update_pull_button(&pull_btn_init, behind);
                    if let Some(branch) = branch {
                        branch_label_init.set_label(&format!("⎇ {branch}"));
                    }
                }
            });
        }

        // Load file list
        Self::load_files(
            dir.clone(),
            listbox.clone(),
            content_stack.clone(),
            diff_view.clone(),
            files_store.clone(),
        );

        // File selection → load diff
        let dir_sel = dir.clone();
        let files_sel = files_store.clone();
        let diff_view_sel = diff_view.clone();
        listbox.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                let files = files_sel.borrow();
                if let Some(file) = files.get(idx) {
                    let file = file.clone();
                    let dir = dir_sel.clone();
                    let buffer = diff_view_sel.buffer();

                    // Clear all tags before loading new diff
                    Self::clear_tags(&buffer);
                    buffer.set_text("Loading...");

                    let (tx, rx) = oneshot::channel::<DiffResult>();
                    std::thread::spawn(move || {
                        let result = load_diff(&dir, &file);
                        let _ = tx.send(result);
                    });

                    let buffer_ref = buffer.clone();
                    glib::spawn_future_local(async move {
                        if let Ok(result) = rx.await {
                            if result.text.is_empty() {
                                buffer_ref.set_text("(no diff available)");
                            } else {
                                apply_styling(&buffer_ref, &result);
                            }
                        }
                    });
                }
            }
        });

        // Refresh button
        let dir_refresh = dir.clone();
        let listbox_ref = listbox.clone();
        let stack_ref = content_stack.clone();
        let files_ref = files_store.clone();
        let diff_view_ref = diff_view.clone();
        refresh_btn.connect_clicked(move |_| {
            stack_ref.set_visible_child_name("loading");
            Self::load_files(
                dir_refresh.clone(),
                listbox_ref.clone(),
                stack_ref.clone(),
                diff_view_ref.clone(),
                files_ref.clone(),
            );
        });

        // Commit textview enables/disables commit button + placeholder
        let commit_btn_ref = commit_btn.clone();
        let placeholder_ref = placeholder_label.clone();
        let buffer = commit_textview.buffer();
        buffer.connect_changed(move |buf| {
            let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
            let is_empty = text.trim().is_empty();
            commit_btn_ref.set_sensitive(!is_empty);
            placeholder_ref.set_visible(is_empty);
        });

        // Commit button: stages all + commits
        let dir_commit = dir.clone();
        let dialog_commit = dialog.clone();
        let buffer_commit = commit_textview.buffer();
        let push_btn_commit = push_btn.clone();
        let on_changed_commit = on_changed.clone();
        commit_btn.connect_clicked(move |btn| {
            let msg = {
                let buf = &buffer_commit;
                buf.text(&buf.start_iter(), &buf.end_iter(), false)
                    .trim()
                    .to_string()
            };
            if msg.is_empty() {
                return;
            }
            btn.set_label("Committing...");
            btn.set_sensitive(false);
            let dir = dir_commit.clone();
            let dlg = dialog_commit.clone();
            let buf = buffer_commit.clone();
            let pb = push_btn_commit.clone();
            let cb = btn.clone();
            let (tx, rx) = oneshot::channel::<Result<usize, String>>();
            std::thread::spawn(move || {
                let _ = tx.send(match commit_all(&dir, &msg) {
                    Ok(()) => Ok(commits_ahead(&dir)),
                    Err(e) => Err(e),
                });
            });
            let on_changed = on_changed_commit.clone();
            glib::spawn_future_local(async move {
                let Ok(result) = rx.await else { return };
                cb.set_label("Commit");
                match result {
                    Ok(ahead) => {
                        buf.set_text("");
                        update_push_button(&pb, ahead);
                        on_changed();
                    }
                    Err(err) => {
                        cb.set_sensitive(true);
                        show_error_dialog(&dlg, "Commit Failed", &err);
                    }
                }
            });
        });

        // Ctrl+Enter in the commit TextView fires the Commit button. Plain
        // Enter still inserts a newline (default TextView behavior). We
        // check is_sensitive() because emit_clicked bypasses the button's
        // sensitivity gate — this respects the existing "buffer empty" and
        // "commit in flight" guards without re-implementing them.
        let commit_btn_shortcut = commit_btn.clone();
        let key_controller = gtk4::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        key_controller.connect_key_pressed(move |_, keyval, _, state| {
            if keyval == gtk4::gdk::Key::Return
                && state.contains(gtk4::gdk::ModifierType::CONTROL_MASK)
            {
                if commit_btn_shortcut.is_sensitive() {
                    commit_btn_shortcut.emit_clicked();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        commit_textview.add_controller(key_controller);

        // Push button
        let pushing = Rc::new(Cell::new(false));
        let dir_push = dir.clone();
        let dialog_push = dialog.clone();
        let push_btn_push = push_btn.clone();
        let pushing_click = pushing.clone();
        let on_changed_push = on_changed.clone();
        push_btn.connect_clicked(move |btn| {
            pushing_click.set(true);
            btn.set_label("Pushing...");
            btn.set_sensitive(false);
            let dir = dir_push.clone();
            let dlg = dialog_push.clone();
            let pb = push_btn_push.clone();
            let (tx, rx) = oneshot::channel::<Result<usize, String>>();
            std::thread::spawn(move || {
                let _ = tx.send(match push(&dir) {
                    Ok(()) => Ok(commits_ahead(&dir)),
                    Err(e) => Err(e),
                });
            });
            let pushing_done = pushing_click.clone();
            let on_changed = on_changed_push.clone();
            glib::spawn_future_local(async move {
                let Ok(result) = rx.await else { return };
                pushing_done.set(false);
                match result {
                    Ok(_) => {
                        on_changed();
                        dlg.close();
                    }
                    Err(err) => {
                        update_push_button(&pb, 1); // re-enable on error
                        show_error_dialog(&dlg, "Push Failed", &err);
                    }
                }
            });
        });

        // Pull button
        let pulling = Rc::new(Cell::new(false));
        let dir_pull = dir.clone();
        let dialog_pull = dialog.clone();
        let pull_btn_pull = pull_btn.clone();
        let push_btn_pull = push_btn.clone();
        let pulling_click = pulling.clone();
        let listbox_pull = listbox.clone();
        let stack_pull = content_stack.clone();
        let diff_view_pull = diff_view.clone();
        let files_store_pull = files_store.clone();
        let on_changed_pull = on_changed.clone();
        pull_btn.connect_clicked(move |btn| {
            pulling_click.set(true);
            btn.set_label("Pulling...");
            btn.set_sensitive(false);
            let dir = dir_pull.clone();
            let dlg = dialog_pull.clone();
            let pb_push = push_btn_pull.clone();
            let pb_pull = pull_btn_pull.clone();
            let lb = listbox_pull.clone();
            let cs = stack_pull.clone();
            let dv = diff_view_pull.clone();
            let fs = files_store_pull.clone();
            let (tx, rx) = oneshot::channel::<Result<(usize, usize), String>>();
            std::thread::spawn(move || {
                let _ = tx.send(match pull(&dir) {
                    Ok(()) => Ok((commits_ahead(&dir), commits_behind(&dir))),
                    Err(e) => Err(e),
                });
            });
            let pulling_done = pulling_click.clone();
            let dir_reload = dir_pull.clone();
            let on_changed = on_changed_pull.clone();
            glib::spawn_future_local(async move {
                let Ok(result) = rx.await else { return };
                pulling_done.set(false);
                match result {
                    Ok((ahead, behind)) => {
                        update_push_button(&pb_push, ahead);
                        update_pull_button(&pb_pull, behind);
                        GitChangesDialog::load_files(
                            dir_reload.clone(),
                            lb.clone(),
                            cs.clone(),
                            dv.clone(),
                            fs.clone(),
                        );
                        on_changed();
                    }
                    Err(err) => {
                        pb_pull.set_sensitive(true);
                        show_error_dialog(&dlg, "Pull Failed", &err);
                    }
                }
            });
        });

        dialog.present(Some(parent));

        // Auto-refresh: poll git status every 2 seconds
        let alive = Rc::new(Cell::new(true));
        let alive_close = alive.clone();
        dialog.connect_closed(move |_| {
            alive_close.set(false);
        });

        let poll_dir = location.clone();
        let last_hash = Rc::new(Cell::new(0u64));
        let fetch_counter = Rc::new(Cell::new(0u32));
        let poll_listbox = listbox.clone();
        let poll_stack = content_stack.clone();
        let poll_diff = diff_view.clone();
        let poll_files = files_store.clone();
        let poll_push = push_btn.clone();
        let poll_pull = pull_btn.clone();
        let poll_pushing = pushing.clone();
        let poll_pulling = pulling.clone();
        let poll_branch = branch_label.clone();
        glib::timeout_add_seconds_local(2, move || {
            if !alive.get() {
                return glib::ControlFlow::Break;
            }

            let dir = poll_dir.clone();
            let hash_ref = last_hash.clone();
            let alive_ref = alive.clone();
            let lb = poll_listbox.clone();
            let cs = poll_stack.clone();
            let dv = poll_diff.clone();
            let fs = poll_files.clone();
            let pb = poll_push.clone();
            let pl = poll_pull.clone();
            let bl = poll_branch.clone();
            let is_pushing = poll_pushing.clone();
            let is_pulling = poll_pulling.clone();

            let fetch_tick = fetch_counter.get();
            fetch_counter.set(fetch_tick + 1);

            let (tx, rx) = oneshot::channel::<(u64, usize, usize, Option<String>)>();
            std::thread::spawn(move || {
                // Fetch every ~30 seconds (15 ticks * 2 seconds)
                if fetch_tick % 15 == 0 {
                    git_fetch(&dir);
                }
                let hash = status_hash(&dir);
                let ahead = commits_ahead(&dir);
                let behind = commits_behind(&dir);
                let branch = current_branch(&dir);
                let _ = tx.send((hash, ahead, behind, branch));
            });

            let dir2 = poll_dir.clone();
            glib::spawn_future_local(async move {
                let Ok((hash, ahead, behind, branch)) = rx.await else {
                    return;
                };
                if !alive_ref.get() {
                    return;
                }
                if !is_pushing.get() {
                    update_push_button(&pb, ahead);
                }
                if !is_pulling.get() {
                    update_pull_button(&pl, behind);
                }
                if let Some(name) = branch {
                    let new_text = format!("⎇ {name}");
                    if bl.label().as_str() != new_text {
                        bl.set_label(&new_text);
                    }
                }
                let prev = hash_ref.get();
                hash_ref.set(hash);
                if prev != 0 && hash != prev {
                    GitChangesDialog::load_files(
                        dir2.clone(),
                        lb.clone(),
                        cs.clone(),
                        dv.clone(),
                        fs.clone(),
                    );
                }
            });

            glib::ControlFlow::Continue
        });
    }

    fn clear_tags(buffer: &gtk4::TextBuffer) {
        let tag_table = buffer.tag_table();
        let mut tags_to_remove = Vec::new();
        // Collect tag names to remove (can't modify table while iterating)
        tag_table.foreach(|tag| {
            if let Some(name) = tag.name() {
                tags_to_remove.push(name.to_string());
            }
        });
        for name in &tags_to_remove {
            if let Some(tag) = tag_table.lookup(name) {
                tag_table.remove(&tag);
            }
        }
    }

    fn load_files(
        location: ProjectLocation,
        listbox: gtk4::ListBox,
        content_stack: gtk4::Stack,
        diff_view: gtk4::TextView,
        files_store: std::rc::Rc<std::cell::RefCell<Vec<ChangedFile>>>,
    ) {
        let (tx, rx) = oneshot::channel::<Vec<ChangedFile>>();

        std::thread::spawn(move || {
            let files = changed_files(&location);
            let _ = tx.send(files);
        });

        glib::spawn_future_local(async move {
            let Ok(files) = rx.await else { return };

            // Clear existing rows
            while let Some(child) = listbox.first_child() {
                listbox.remove(&child);
            }

            diff_view.buffer().set_text("");

            if files.is_empty() {
                content_stack.set_visible_child_name("empty");
            } else {
                for file in &files {
                    let row_content = build_file_row(file);
                    listbox.append(&row_content);
                }
                *files_store.borrow_mut() = files;
                content_stack.set_visible_child_name("content");

                // Auto-select first file
                if let Some(first) = listbox.row_at_index(0) {
                    listbox.select_row(Some(&first));
                }
            }
        });
    }
}
