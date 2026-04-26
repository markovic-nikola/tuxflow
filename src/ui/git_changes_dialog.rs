use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;
use std::sync::mpsc;

use adw::prelude::*;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_nonewlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

#[derive(Clone)]
enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Clone)]
struct ChangedFile {
    path: String,
    status: FileStatus,
    staged: bool,
}

struct DiffResult {
    text: String,
    /// (line_index, byte_offset_in_line, length, hex_color)
    highlights: Vec<(usize, usize, usize, String)>,
}

impl FileStatus {
    fn label(&self) -> &str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
        }
    }

    fn css_class(&self) -> &str {
        match self {
            Self::Modified => "git-status-modified",
            Self::Added => "git-status-added",
            Self::Deleted => "git-status-deleted",
            Self::Renamed => "git-status-modified",
            Self::Untracked => "git-status-untracked",
        }
    }
}

fn parse_status_line(line: &str) -> Option<ChangedFile> {
    if line.len() < 4 {
        return None;
    }
    let bytes = line.as_bytes();
    let index = bytes[0];
    let worktree = bytes[1];
    let path = line[3..].to_string();

    let (status, staged) = match (index, worktree) {
        (b'?', b'?') => (FileStatus::Untracked, false),
        (b'A', _) => (FileStatus::Added, true),
        (b'D', _) => (FileStatus::Deleted, true),
        (b'R', _) => (FileStatus::Renamed, true),
        (b'M', _) => (FileStatus::Modified, true),
        (_, b'M') => (FileStatus::Modified, false),
        (_, b'D') => (FileStatus::Deleted, false),
        _ => return None,
    };

    Some(ChangedFile {
        path,
        status,
        staged,
    })
}

fn load_changed_files(project_dir: &Path) -> Vec<ChangedFile> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(project_dir)
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().filter_map(parse_status_line).collect()
}

fn git_status_hash(project_dir: &Path) -> u64 {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(project_dir)
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let mut hasher = DefaultHasher::new();
            o.stdout.hash(&mut hasher);
            hasher.finish()
        }
        _ => 0,
    }
}

fn commits_ahead(project_dir: &Path) -> usize {
    let output = std::process::Command::new("git")
        .args(["rev-list", "--count", "@{u}..HEAD"])
        .current_dir(project_dir)
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(0),
        _ => 0,
    }
}

pub fn commits_behind(project_dir: &Path) -> usize {
    let output = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD..@{u}"])
        .current_dir(project_dir)
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(0),
        _ => 0,
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

pub fn git_fetch(project_dir: &Path) {
    let _ = std::process::Command::new("git")
        .args(["fetch"])
        .current_dir(project_dir)
        .output();
}

fn run_git_command(project_dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(project_dir)
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn current_branch(project_dir: &Path) -> Option<String> {
    let name = run_git_command(project_dir, &["branch", "--show-current"])
        .ok()?
        .trim()
        .to_string();
    if name.is_empty() { None } else { Some(name) }
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

fn load_diff_for_file(project_dir: &Path, file: &ChangedFile) -> DiffResult {
    let output = if matches!(file.status, FileStatus::Untracked) {
        std::process::Command::new("git")
            .args(["diff", "--no-index", "--", "/dev/null", &file.path])
            .current_dir(project_dir)
            .output()
    } else if file.staged {
        std::process::Command::new("git")
            .args(["diff", "--cached", "--", &file.path])
            .current_dir(project_dir)
            .output()
    } else {
        std::process::Command::new("git")
            .args(["diff", "--", &file.path])
            .current_dir(project_dir)
            .output()
    };

    let text = match output {
        Ok(o) => {
            let raw = String::from_utf8_lossy(&o.stdout);
            // Strip diff header lines and hunk headers (@@)
            let text: String = raw
                .lines()
                .skip_while(|l| !l.starts_with("@@"))
                .filter(|l| !l.starts_with("@@"))
                .collect::<Vec<_>>()
                .join("\n");
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() > 5000 {
                let mut truncated: String = lines[..5000].join("\n");
                truncated.push_str("\n\n... (truncated — diff exceeds 5000 lines)");
                truncated
            } else {
                text
            }
        }
        Err(_) => String::from("Failed to load diff"),
    };

    let highlights = highlight_diff(&text, &file.path);
    DiffResult { text, highlights }
}

fn highlight_diff(text: &str, file_path: &str) -> Vec<(usize, usize, usize, String)> {
    use syntect::easy::HighlightLines;

    let ss = &*SYNTAX_SET;
    let ts = &*THEME_SET;
    let theme = &ts.themes["base16-eighties.dark"];

    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let syntax = ss
        .find_syntax_by_extension(ext)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, theme);

    let mut highlights = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        let (prefix_len, code) = match line.as_bytes().first() {
            Some(b'+') if !line.starts_with("+++") => (1, &line[1..]),
            Some(b'-') if !line.starts_with("---") => (1, &line[1..]),
            Some(b' ') => (1, &line[1..]),
            _ => continue,
        };

        if let Ok(ranges) = h.highlight_line(code, ss) {
            let mut byte_offset = prefix_len;
            for (style, token) in ranges {
                if !token.is_empty() {
                    let color = format!(
                        "#{:02x}{:02x}{:02x}",
                        style.foreground.r, style.foreground.g, style.foreground.b
                    );
                    highlights.push((line_idx, byte_offset, token.len(), color));
                }
                byte_offset += token.len();
            }
        }
    }
    highlights
}

fn build_file_row(file: &ChangedFile) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.add_css_class("git-file-row");
    row.set_margin_start(4);
    row.set_margin_end(4);

    let badge = gtk4::Label::builder()
        .label(file.status.label())
        .css_classes([file.status.css_class()])
        .build();
    row.append(&badge);

    let path_label = gtk4::Label::builder()
        .label(&file.path)
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

    // Build line start offsets for syntax highlight positioning
    let mut line_starts = Vec::new();
    let mut offset = 0;
    for line in result.text.lines() {
        line_starts.push(offset);
        offset += line.len() + 1; // +1 for newline
    }

    // Apply diff background tags
    for (line_idx, line) in result.text.lines().enumerate() {
        let line_start = line_starts[line_idx];
        let line_end = line_start + line.len();
        let start_iter = buffer.iter_at_offset(line_start as i32);
        let end_iter = buffer.iter_at_offset(line_end as i32);

        if line.starts_with('+') && !line.starts_with("+++") {
            buffer.apply_tag_by_name("addition", &start_iter, &end_iter);
        } else if line.starts_with('-') && !line.starts_with("---") {
            buffer.apply_tag_by_name("deletion", &start_iter, &end_iter);
        }
    }

    // Apply syntax foreground tags
    for (line_idx, byte_off, len, color) in &result.highlights {
        if *line_idx >= line_starts.len() {
            continue;
        }
        let tag_name = format!("fg_{color}");
        if tag_table.lookup(&tag_name).is_none() {
            tag_table.add(
                &gtk4::TextTag::builder()
                    .name(&tag_name)
                    .foreground(color.as_str())
                    .build(),
            );
        }
        let abs_start = line_starts[*line_idx] + byte_off;
        let abs_end = abs_start + len;
        buffer.apply_tag_by_name(
            &tag_name,
            &buffer.iter_at_offset(abs_start as i32),
            &buffer.iter_at_offset(abs_end as i32),
        );
    }
}

pub struct GitChangesDialog;

impl GitChangesDialog {
    pub fn show(parent: &impl IsA<gtk4::Widget>, project_dir: &Path) {
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
        let dir = project_dir.to_path_buf();

        // Initial branch label (synchronous — single fast git call)
        if let Some(branch) = current_branch(&dir) {
            branch_label.set_label(&format!("⎇ {branch}"));
        }

        // Initial push/pull status
        {
            let dir_init = dir.clone();
            let push_btn_init = push_btn.clone();
            let pull_btn_init = pull_btn.clone();
            let (tx, rx) = mpsc::channel::<(usize, usize)>();
            std::thread::spawn(move || {
                git_fetch(&dir_init);
                let ahead = commits_ahead(&dir_init);
                let behind = commits_behind(&dir_init);
                let _ = tx.send((ahead, behind));
            });
            glib::idle_add_local(move || {
                if let Ok((ahead, behind)) = rx.try_recv() {
                    update_push_button(&push_btn_init, ahead);
                    update_pull_button(&pull_btn_init, behind);
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
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

                    let (tx, rx) = mpsc::channel::<DiffResult>();
                    std::thread::spawn(move || {
                        let result = load_diff_for_file(&dir, &file);
                        let _ = tx.send(result);
                    });

                    let buffer_ref = buffer.clone();
                    glib::idle_add_local(move || {
                        if let Ok(result) = rx.try_recv() {
                            if result.text.is_empty() {
                                buffer_ref.set_text("(no diff available)");
                            } else {
                                apply_styling(&buffer_ref, &result);
                            }
                            return glib::ControlFlow::Break;
                        }
                        glib::ControlFlow::Continue
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
            let (tx, rx) = mpsc::channel::<Result<usize, String>>();
            std::thread::spawn(move || {
                if let Err(e) = run_git_command(&dir, &["add", "-A"]) {
                    let _ = tx.send(Err(e));
                    return;
                }
                if let Err(e) = run_git_command(&dir, &["commit", "-m", &msg]) {
                    let _ = tx.send(Err(e));
                    return;
                }
                let _ = tx.send(Ok(commits_ahead(&dir)));
            });
            glib::idle_add_local(move || {
                if let Ok(result) = rx.try_recv() {
                    cb.set_label("Commit");
                    match result {
                        Ok(ahead) => {
                            buf.set_text("");
                            update_push_button(&pb, ahead);
                        }
                        Err(err) => {
                            cb.set_sensitive(true);
                            show_error_dialog(&dlg, "Commit Failed", &err);
                        }
                    }
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
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
        push_btn.connect_clicked(move |btn| {
            pushing_click.set(true);
            btn.set_label("Pushing...");
            btn.set_sensitive(false);
            let dir = dir_push.clone();
            let dlg = dialog_push.clone();
            let pb = push_btn_push.clone();
            let (tx, rx) = mpsc::channel::<Result<usize, String>>();
            std::thread::spawn(move || {
                // Try normal push first; if no upstream, push with -u
                let result = match run_git_command(&dir, &["push"]) {
                    Ok(out) => Ok(out),
                    Err(e) if e.contains("no upstream") || e.contains("set-upstream") => {
                        // Get current branch name
                        let branch = run_git_command(&dir, &["branch", "--show-current"])
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        if branch.is_empty() {
                            Err(e)
                        } else {
                            run_git_command(&dir, &["push", "-u", "origin", &branch])
                        }
                    }
                    Err(e) => Err(e),
                };
                match result {
                    Ok(_) => {
                        let _ = tx.send(Ok(commits_ahead(&dir)));
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            });
            let pushing_done = pushing_click.clone();
            glib::idle_add_local(move || {
                if let Ok(result) = rx.try_recv() {
                    pushing_done.set(false);
                    match result {
                        Ok(_) => {
                            dlg.close();
                        }
                        Err(err) => {
                            update_push_button(&pb, 1); // re-enable on error
                            show_error_dialog(&dlg, "Push Failed", &err);
                        }
                    }
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
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
            let (tx, rx) = mpsc::channel::<Result<(usize, usize), String>>();
            std::thread::spawn(move || {
                let result = run_git_command(&dir, &["pull", "--ff-only"]).or_else(|e| {
                    // Transient: initial pull occasionally fails with "no such ref
                    // was fetched" or "Cannot fast-forward to multiple branches"
                    // when the prior fetch left the upstream config in an
                    // intermediate state. An explicit fetch + retry clears it up.
                    if e.contains("no such ref was fetched")
                        || e.contains("Cannot fast-forward to multiple branches")
                    {
                        run_git_command(&dir, &["fetch"])
                            .and_then(|_| run_git_command(&dir, &["pull", "--ff-only"]))
                    } else {
                        Err(e)
                    }
                });
                match result {
                    Ok(_) => {
                        let ahead = commits_ahead(&dir);
                        let behind = commits_behind(&dir);
                        let _ = tx.send(Ok((ahead, behind)));
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            });
            let pulling_done = pulling_click.clone();
            let dir_reload = dir_pull.clone();
            glib::idle_add_local(move || {
                if let Ok(result) = rx.try_recv() {
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
                        }
                        Err(err) => {
                            pb_pull.set_sensitive(true);
                            show_error_dialog(&dlg, "Pull Failed", &err);
                        }
                    }
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
        });

        dialog.present(Some(parent));

        // Auto-refresh: poll git status every 2 seconds
        let alive = Rc::new(Cell::new(true));
        let alive_close = alive.clone();
        dialog.connect_closed(move |_| {
            alive_close.set(false);
        });

        let poll_dir = project_dir.to_path_buf();
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

            let (tx, rx) = mpsc::channel::<(u64, usize, usize, Option<String>)>();
            std::thread::spawn(move || {
                // Fetch every ~30 seconds (15 ticks * 2 seconds)
                if fetch_tick % 15 == 0 {
                    git_fetch(&dir);
                }
                let hash = git_status_hash(&dir);
                let ahead = commits_ahead(&dir);
                let behind = commits_behind(&dir);
                let branch = current_branch(&dir);
                let _ = tx.send((hash, ahead, behind, branch));
            });

            let dir2 = poll_dir.clone();
            glib::idle_add_local(move || {
                if !alive_ref.get() {
                    return glib::ControlFlow::Break;
                }
                if let Ok((hash, ahead, behind, branch)) = rx.try_recv() {
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
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
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
        project_dir: PathBuf,
        listbox: gtk4::ListBox,
        content_stack: gtk4::Stack,
        diff_view: gtk4::TextView,
        files_store: std::rc::Rc<std::cell::RefCell<Vec<ChangedFile>>>,
    ) {
        let (tx, rx) = mpsc::channel::<Vec<ChangedFile>>();

        std::thread::spawn(move || {
            let files = load_changed_files(&project_dir);
            let _ = tx.send(files);
        });

        glib::idle_add_local(move || {
            if let Ok(files) = rx.try_recv() {
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

                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }
}
