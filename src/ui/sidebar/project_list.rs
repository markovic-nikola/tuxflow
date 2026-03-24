use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use vte4::prelude::*;

use libadwaita as adw;
use adw::prelude::*;

use crate::config::schema::ProcessCategory;
use crate::process::manager::{ProcessManagerRef, ProcessStatus};
use crate::ui::add_command_dialog::{AddCommandDialog, EditCommandResult};
use crate::ui::edit_project_dialog::{EditProjectDialog, EditProjectResult};
use crate::workspace::{self, WorkspaceRef};

use super::dnd;
use super::process_row::ProcessRow;
use super::project_row::ProjectRow;
use super::section_header::SectionHeader;

struct SectionInfo {
    project_name: String,
    category_title: String,
    header: SectionHeader,
    process_names: Vec<String>,
}

pub struct ProjectList {
    outer_container: gtk4::Box,
    container: gtk4::Box,
    filter_entry: gtk4::SearchEntry,
    search_btn: gtk4::ToggleButton,
    process_rows: Rc<RefCell<HashMap<String, ProcessRow>>>,
    project_rows: Rc<RefCell<HashMap<String, ProjectRow>>>,
    sections: Rc<RefCell<Vec<SectionInfo>>>,
    process_statuses: Rc<RefCell<HashMap<String, ProcessStatus>>>,
    on_process_selected: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
    on_process_deleted: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
    on_counts_changed: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    workspace: Rc<RefCell<Option<WorkspaceRef>>>,
    window: Rc<RefCell<Option<libadwaita::ApplicationWindow>>>,
    single_expand: Rc<Cell<bool>>,
    selected_qname: Rc<RefCell<Option<String>>>,
}

impl ProjectList {
    pub fn new(single_expand: Rc<Cell<bool>>) -> Self {
        let outer_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        outer_container.set_vexpand(true);
        outer_container.add_css_class("sidebar");

        // Filter entry (hidden by default, toggled from headerbar button)
        let filter_entry = gtk4::SearchEntry::builder()
            .placeholder_text("Filter processes...")
            .margin_start(8)
            .margin_end(8)
            .margin_top(8)
            .margin_bottom(4)
            .build();
        filter_entry.add_css_class("sidebar-filter");
        filter_entry.set_visible(false);
        outer_container.append(&filter_entry);

        // Stored so headerbar can wire the toggle
        let search_btn = gtk4::ToggleButton::builder()
            .icon_name("edit-find-symbolic")
            .tooltip_text("Filter Processes (Ctrl+F)")
            .build();

        // Toggle filter visibility
        let entry_ref = filter_entry.clone();
        search_btn.connect_toggled(move |btn| {
            let active = btn.is_active();
            entry_ref.set_visible(active);
            if active {
                entry_ref.grab_focus();
            } else {
                entry_ref.set_text("");
            }
        });

        // Escape hides the filter
        let key_controller = gtk4::EventControllerKey::new();
        let btn_ref = search_btn.clone();
        key_controller.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk4::gdk::Key::Escape {
                btn_ref.set_active(false);
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        });
        filter_entry.add_controller(key_controller);

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        outer_container.append(&container);

        let process_rows: Rc<RefCell<HashMap<String, ProcessRow>>> =
            Rc::new(RefCell::new(HashMap::new()));

        // Wire filter
        let rows_ref = process_rows.clone();
        filter_entry.connect_search_changed(move |entry| {
            let query = entry.text().to_string().to_lowercase();
            let rows = rows_ref.borrow();
            for (qname, row) in rows.iter() {
                let visible = query.is_empty()
                    || qname.to_lowercase().contains(&query)
                    || row.name().to_lowercase().contains(&query);
                row.widget().set_visible(visible);
            }
        });

        Self {
            outer_container,
            container,
            filter_entry,
            search_btn,
            process_rows,
            project_rows: Rc::new(RefCell::new(HashMap::new())),
            sections: Rc::new(RefCell::new(Vec::new())),
            process_statuses: Rc::new(RefCell::new(HashMap::new())),
            on_process_selected: Rc::new(RefCell::new(None)),
            on_process_deleted: Rc::new(RefCell::new(None)),
            on_counts_changed: Rc::new(RefCell::new(None)),
            workspace: Rc::new(RefCell::new(None)),
            window: Rc::new(RefCell::new(None)),
            single_expand,
            selected_qname: Rc::new(RefCell::new(None)),
        }
    }

    pub fn set_workspace(&self, ws: &WorkspaceRef) {
        *self.workspace.borrow_mut() = Some(ws.clone());
    }

    pub fn set_window(&self, window: &libadwaita::ApplicationWindow) {
        *self.window.borrow_mut() = Some(window.clone());
    }

    pub fn select_process(&self, qname: &str) {
        let rows = self.process_rows.borrow();
        if let Some(ref prev) = *self.selected_qname.borrow() {
            if let Some(prev_row) = rows.get(prev.as_str()) {
                prev_row.widget().remove_css_class("process-row-selected");
            }
        }
        if let Some(row) = rows.get(qname) {
            row.widget().add_css_class("process-row-selected");
        }
        drop(rows);
        *self.selected_qname.borrow_mut() = Some(qname.to_string());
    }

    pub fn set_on_process_selected(&self, cb: impl Fn(&str) + 'static) {
        *self.on_process_selected.borrow_mut() = Some(Box::new(cb));
    }

    pub fn set_on_process_deleted(&self, cb: impl Fn(&str) + 'static) {
        *self.on_process_deleted.borrow_mut() = Some(Box::new(cb));
    }

    pub fn set_on_counts_changed(&self, cb: impl Fn() + 'static) {
        *self.on_counts_changed.borrow_mut() = Some(Box::new(cb));
    }

    /// Add a single project to the sidebar (appends, doesn't clear)
    /// `saved_expanded` is the persisted expanded state (None = no saved preference).
    pub fn add_project(&self, manager: &ProcessManagerRef, project_name: &str, icon_path: Option<&str>, saved_expanded: Option<bool>) {
        let mgr = manager.borrow();

        let project_row = ProjectRow::new(project_name, icon_path);

        // Restore saved expanded state, or use accordion logic
        if let Some(expanded) = saved_expanded {
            project_row.set_expanded(expanded);
        } else if self.single_expand.get() {
            let has_expanded = self.project_rows.borrow().values().any(|r| r.is_expanded());
            if has_expanded {
                project_row.set_expanded(false);
            }
        }

        // Wire project row hover buttons
        Self::connect_project_actions(&project_row, manager);

        // Wire right-click context menu actions
        self.connect_project_context_actions(&project_row, manager, project_name);

        // Wire accordion toggle and persist expanded state
        {
            let project_rows_ref = self.project_rows.clone();
            let single_expand = self.single_expand.clone();
            let pname = project_name.to_string();
            let ws_ref = self.workspace.clone();
            project_row.set_on_toggled(move |_name, expanded| {
                // Persist expanded state
                if let Some(ref ws) = *ws_ref.borrow() {
                    ws.borrow_mut().set_project_expanded(&pname, expanded);
                }
                if expanded && single_expand.get() {
                    let rows = project_rows_ref.borrow();
                    for (key, row) in rows.iter() {
                        if *key != pname && row.is_expanded() {
                            row.set_expanded(false);
                            // Persist collapsed state for other projects
                            if let Some(ref ws) = *ws_ref.borrow() {
                                ws.borrow_mut().set_project_expanded(key, false);
                            }
                        }
                    }
                }
            });
        }

        // --- DragSource on project header ---
        let drag_source = gtk4::DragSource::new();
        drag_source.set_actions(gdk::DragAction::MOVE);
        let pname_drag = project_name.to_string();
        drag_source.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&glib::Value::from(&pname_drag)))
        });
        dnd::setup_drag_icon(&drag_source, project_row.header_row());
        project_row.header_row().add_controller(drag_source);

        // --- DropTarget on project header (only for project-level reordering) ---
        let drop_target = gtk4::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let widget_ref = project_row.header_row().clone();
        drop_target.connect_motion(move |_, _, y| {
            dnd::update_drop_indicator(&widget_ref, y);
            gdk::DragAction::MOVE
        });
        let widget_ref2 = project_row.header_row().clone();
        drop_target.connect_leave(move |_| {
            dnd::clear_drop_indicator(&widget_ref2);
        });
        let pname_drop = project_name.to_string();
        let container_ref = self.container.clone();
        let ws_ref = self.workspace.clone();
        let project_rows_ref = self.project_rows.clone();
        let header_ref = project_row.header_row().clone();
        drop_target.connect_drop(move |_, value, _, y| {
            let Ok(dragged_name) = value.get::<String>() else { return false };
            // Only accept project-level drags (no "::" means it's a project name)
            if dragged_name.contains("::") || dragged_name == pname_drop {
                return false;
            }
            let rows = project_rows_ref.borrow();
            let Some(dragged_row) = rows.get(&dragged_name) else { return false };
            let Some(target_row) = rows.get(&pname_drop) else { return false };

            let target_widget = target_row.widget();
            let height = target_widget.height() as f64;
            let before = y < height / 2.0;
            dnd::clear_drop_indicator(&header_ref);
            dnd::reorder_in_box(&container_ref, dragged_row.widget(), target_widget, before);

            if let Some(ref ws) = *ws_ref.borrow() {
                ws.borrow_mut().reorder_project(&dragged_name, &pname_drop, before);
            }
            true
        });
        project_row.header_row().add_controller(drop_target);

        let categories = [
            ("AGENTS", "ai-brain-symbolic", ProcessCategory::Agent),
            ("COMMANDS", "view-list-symbolic", ProcessCategory::Command),
            ("TERMINALS", "utilities-terminal-symbolic", ProcessCategory::Terminal),
            ("SSH", "network-server-symbolic", ProcessCategory::SSH),
        ];

        for (title, icon, category) in &categories {
            let procs = mgr.processes_by_category(category.clone());
            if procs.is_empty() {
                continue;
            }

            let section = SectionHeader::new(title, icon);
            let running = procs.iter().filter(|p| p.status == ProcessStatus::Running).count();
            section.set_count(running, procs.len());

            let mut section_qnames = Vec::new();

            for proc in &procs {
                let qname = workspace::qualified_name(project_name, &proc.config.name);
                let row = if *category == ProcessCategory::Terminal || *category == ProcessCategory::SSH {
                    ProcessRow::new_terminal(&proc.config.name, &proc.config.command)
                } else {
                    ProcessRow::new(&proc.config.name, &proc.config.command)
                };
                if let Some(display) = &proc.config.display_name {
                    row.set_name(display);
                }
                row.set_status(proc.status);
                self.connect_row_click(&row, &qname);
                Self::connect_row_actions(&row, manager, &qname, &self.on_process_selected, &self.process_rows, &self.sections, &self.process_statuses, &self.on_process_deleted, &self.on_counts_changed, &self.window, &self.workspace, &self.selected_qname);

                self.connect_row_dnd(&row, manager, &qname);

                section.content_box().append(row.widget());
                self.process_statuses.borrow_mut().insert(qname.clone(), proc.status);
                self.process_rows.borrow_mut().insert(qname.clone(), row);
                section_qnames.push(qname);
            }

            self.sections.borrow_mut().push(SectionInfo {
                project_name: project_name.to_string(),
                category_title: title.to_string(),
                header: section,
                process_names: section_qnames,
            });

            let last_idx = self.sections.borrow().len() - 1;
            project_row.content_box().append(self.sections.borrow()[last_idx].header.widget());
        }

        self.container.append(project_row.widget());
        self.project_rows.borrow_mut().insert(project_name.to_string(), project_row);
    }

    fn connect_project_context_actions(
        &self,
        project_row: &ProjectRow,
        manager: &ProcessManagerRef,
        project_name: &str,
    ) {
        let mgr = manager.clone();
        let pname = project_name.to_string();
        let ws_ref = self.workspace.clone();
        let win_ref = self.window.clone();
        let container_ref = self.container.clone();
        let project_rows_ref = self.project_rows.clone();

        project_row.set_on_context_action(move |action| {
            match action {
                "start_all" => {
                    mgr.borrow_mut().spawn_project_group();
                }
                "stop_all" => {
                    mgr.borrow_mut().stop_all();
                }
                "restart_all" => {
                    mgr.borrow_mut().restart_all();
                }
                "copy_path" => {
                    if let Some(ref ws) = *ws_ref.borrow() {
                        let ws_borrow = ws.borrow();
                        if let Some(dir) = ws_borrow.get_project_dir(&pname) {
                            if let Some(display) = gtk4::gdk::Display::default() {
                                display.clipboard().set_text(&dir.to_string_lossy());
                            }
                        }
                    }
                }
                "open_in_editor" => {
                    if let Some(ref ws) = *ws_ref.borrow() {
                        let ws_borrow = ws.borrow();
                        if let Some(dir) = ws_borrow.get_project_dir(&pname) {
                            let settings = crate::config::settings::AppSettings::load();
                            let editor = &settings.tools.default_editor;
                            let mut cmd = std::process::Command::new(editor);
                            if settings.tools.reuse_editor_window
                                && matches!(editor.as_str(), "code" | "cursor" | "codium" | "code-insiders")
                            {
                                cmd.arg("--reuse-window");
                            }
                            cmd.arg(&dir);
                            if let Err(e) = cmd.spawn() {
                                log::error!("Failed to open editor '{}': {}", editor, e);
                            }
                        }
                    }
                }
                "edit" => {
                    let ws = ws_ref.borrow().clone();
                    let win = win_ref.borrow().clone();
                    if let (Some(ws), Some(win)) = (ws, win) {
                        let ws_borrow = ws.borrow();
                        let dir = ws_borrow.get_project_dir(&pname)
                            .map(|d| d.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let icon = ws_borrow.get_project_icon(&pname);
                        drop(ws_borrow);

                        let ws_edit = ws.clone();
                        let pname_for_dialog = pname.clone();
                        let pname_for_closure = pname.clone();
                        let project_rows_edit = project_rows_ref.clone();
                        let container_edit = container_ref.clone();

                        EditProjectDialog::show(
                            &win,
                            &pname_for_dialog,
                            &dir,
                            icon.as_deref(),
                            move |result: EditProjectResult| {
                                if result.remove {
                                    let mut ws_mut = ws_edit.borrow_mut();
                                    ws_mut.remove_project(&pname_for_closure);
                                    drop(ws_mut);
                                    let mut rows = project_rows_edit.borrow_mut();
                                    if let Some(row) = rows.remove(&pname_for_closure) {
                                        container_edit.remove(row.widget());
                                    }
                                } else {
                                    let mut ws_mut = ws_edit.borrow_mut();
                                    ws_mut.set_project_icon(&pname_for_closure, result.icon_path.clone());
                                    if result.name != pname_for_closure {
                                        ws_mut.rename_project(&pname_for_closure, &result.name);
                                    }
                                    drop(ws_mut);

                                    let rows = project_rows_edit.borrow();
                                    if let Some(row) = rows.get(&pname_for_closure) {
                                        if result.name != pname_for_closure {
                                            row.set_name(&result.name);
                                        }
                                        row.set_icon(result.icon_path.as_deref());
                                    }
                                }
                            },
                        );
                    }
                }
                "remove" => {
                    let dialog = adw::AlertDialog::builder()
                        .heading(format!("Remove '{}'?", pname))
                        .body("This will remove the project and all its processes from the sidebar.")
                        .build();
                    dialog.add_response("cancel", "Cancel");
                    dialog.add_response("remove", "Remove");
                    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
                    dialog.set_default_response(Some("cancel"));
                    dialog.set_close_response("cancel");

                    let ws_del = ws_ref.clone();
                    let pname_del = pname.clone();
                    let rows_del = project_rows_ref.clone();
                    let container_del = container_ref.clone();

                    let win = win_ref.borrow().clone();
                    let parent_widget = win.map(|w| w.upcast::<gtk4::Widget>());

                    dialog.choose(parent_widget.as_ref(), gtk4::gio::Cancellable::NONE, move |response| {
                        if response != "remove" {
                            return;
                        }
                        if let Some(ref ws) = *ws_del.borrow() {
                            ws.borrow_mut().remove_project(&pname_del);
                        }
                        let mut rows = rows_del.borrow_mut();
                        if let Some(row) = rows.remove(&pname_del) {
                            container_del.remove(row.widget());
                        }
                    });
                }
                _ => log::warn!("Unknown project action: {action}"),
            }
        });
    }

    fn connect_project_actions(project_row: &ProjectRow, manager: &ProcessManagerRef) {
        let mgr = manager.clone();
        project_row.start_button().connect_clicked(move |_| {
            mgr.borrow_mut().spawn_project_group();
        });

        let mgr = manager.clone();
        project_row.restart_button().connect_clicked(move |_| {
            mgr.borrow_mut().restart_all();
        });

        let mgr = manager.clone();
        project_row.stop_button().connect_clicked(move |_| {
            mgr.borrow_mut().stop_all();
        });
    }

    fn connect_row_actions(
        row: &ProcessRow,
        manager: &ProcessManagerRef,
        qualified_name: &str,
        on_selected: &Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
        process_rows: &Rc<RefCell<HashMap<String, ProcessRow>>>,
        sections: &Rc<RefCell<Vec<SectionInfo>>>,
        process_statuses: &Rc<RefCell<HashMap<String, ProcessStatus>>>,
        on_process_deleted: &Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
        on_counts_changed: &Rc<RefCell<Option<Box<dyn Fn()>>>>,
        window: &Rc<RefCell<Option<adw::ApplicationWindow>>>,
        workspace: &Rc<RefCell<Option<WorkspaceRef>>>,
        selected_qname: &Rc<RefCell<Option<String>>>,
    ) {
        let mgr = manager.clone();
        let qname = qualified_name.to_string();
        let select_cb = on_selected.clone();
        let process_rows_ref = process_rows.clone();
        let sections_ref = sections.clone();
        let statuses_ref = process_statuses.clone();
        let on_deleted_ref = on_process_deleted.clone();
        let on_counts_ref = on_counts_changed.clone();
        let win_ref = window.clone();
        let ws_ref = workspace.clone();
        let selected_ref = selected_qname.clone();
        let rows_for_highlight = process_rows.clone();
        row.set_on_context_action(move |name, action| {
            let select_and_highlight = |qname: &str| {
                // Update sidebar highlight
                let rows = rows_for_highlight.borrow();
                if let Some(ref prev) = *selected_ref.borrow() {
                    if let Some(prev_row) = rows.get(prev.as_str()) {
                        prev_row.widget().remove_css_class("process-row-selected");
                    }
                }
                if let Some(row) = rows.get(qname) {
                    row.widget().add_css_class("process-row-selected");
                }
                drop(rows);
                *selected_ref.borrow_mut() = Some(qname.to_string());
                // Switch terminal
                if let Some(ref cb) = *select_cb.borrow() {
                    cb(qname);
                }
            };
            match action {
                "toggle" => {
                    let mut mgr = mgr.borrow_mut();
                    if let Some(proc) = mgr.get_process(name) {
                        if proc.status == ProcessStatus::Running {
                            mgr.kill(name);
                        } else {
                            mgr.spawn(name);
                            select_and_highlight(&qname);
                        }
                    }
                }
                "stop" => {
                    mgr.borrow_mut().kill(name);
                }
                "restart" => {
                    mgr.borrow_mut().restart(name);
                    select_and_highlight(&qname);
                }
                "clear" => {
                    if let Some(proc) = mgr.borrow().get_process(name) {
                        proc.terminal.reset(true, true);
                    }
                }
                "redraw" => {
                    if let Some(proc) = mgr.borrow().get_process(name) {
                        proc.terminal.queue_draw();
                    }
                }
                "edit" => {
                    let old_name = name.to_string();
                    let old_qname = qname.clone();
                    let mgr_edit = mgr.clone();
                    let rows_edit = process_rows_ref.clone();
                    let sections_edit = sections_ref.clone();
                    let statuses_edit = statuses_ref.clone();
                    let ws_edit = ws_ref.clone();
                    let select_edit = select_cb.clone();
                    let on_deleted_edit = on_deleted_ref.clone();
                    let on_counts_edit = on_counts_ref.clone();
                    let win_edit = win_ref.clone();

                    // Defer to idle to avoid RefCell conflicts when triggered
                    // during a crash/restart cycle that holds a manager borrow.
                    glib::idle_add_local_once(move || {
                        let (config, was_running) = {
                            let mgr_borrow = mgr_edit.borrow();
                            let Some(proc) = mgr_borrow.get_process(&old_name) else { return };
                            (proc.config.clone(), proc.status == ProcessStatus::Running)
                        };

                    let win = win_edit.borrow().clone();
                    if let Some(win) = win {
                        AddCommandDialog::show_edit(&win, &config, move |result| {
                            match result {
                                EditCommandResult::Save(new_config) => {
                                    let new_name = new_config.name.clone();
                                    let new_command = new_config.command.clone();

                                    // Stop if running
                                    if was_running {
                                        mgr_edit.borrow_mut().kill(&old_name);
                                    }

                                    // Update in-memory config
                                    let name_changed = mgr_edit.borrow_mut().update_process_config(&old_name, new_config.clone());

                                    // Persist: remove old, save new
                                    if let Some((proj_name, _)) = old_qname.split_once("::") {
                                        if let Some(ref ws) = *ws_edit.borrow() {
                                            let mut ws_mut = ws.borrow_mut();
                                            if name_changed {
                                                ws_mut.mark_process_deleted(proj_name, &old_name);
                                            }
                                            ws_mut.save_custom_command(proj_name, new_config);
                                        }
                                    }

                                    // Update sidebar row
                                    let mut rows = rows_edit.borrow_mut();
                                    if name_changed {
                                        let new_qname = if let Some((proj, _)) = old_qname.split_once("::") {
                                            workspace::qualified_name(proj, &new_name)
                                        } else {
                                            old_qname.clone()
                                        };

                                        if let Some(row) = rows.remove(&old_qname) {
                                            row.set_name(&new_name);
                                            row.set_command_tooltip(&new_command);
                                            rows.insert(new_qname.clone(), row);
                                        }

                                        // Update section tracking
                                        let mut sections = sections_edit.borrow_mut();
                                        for section in sections.iter_mut() {
                                            if let Some(pos) = section.process_names.iter().position(|n| n == &old_qname) {
                                                section.process_names[pos] = new_qname.clone();
                                            }
                                        }

                                        // Update status tracking
                                        let mut statuses = statuses_edit.borrow_mut();
                                        if let Some(status) = statuses.remove(&old_qname) {
                                            statuses.insert(new_qname.clone(), status);
                                        }
                                    } else {
                                        if let Some(row) = rows.get(&old_qname) {
                                            row.set_command_tooltip(&new_command);
                                        }
                                    }
                                    drop(rows);

                                    // Restart if was running
                                    if was_running {
                                        mgr_edit.borrow_mut().spawn(&new_name);
                                        let select_qname = if let Some((proj, _)) = old_qname.split_once("::") {
                                            workspace::qualified_name(proj, &new_name)
                                        } else {
                                            old_qname.clone()
                                        };
                                        if let Some(ref cb) = *select_edit.borrow() {
                                            cb(&select_qname);
                                        }
                                    }
                                }
                                EditCommandResult::Delete => {
                                    // Persist deletion
                                    if let Some((proj_name, proc_name)) = old_qname.split_once("::") {
                                        if let Some(ref ws) = *ws_edit.borrow() {
                                            ws.borrow_mut().mark_process_deleted(proj_name, proc_name);
                                        }
                                    }

                                    // Runtime removal
                                    mgr_edit.borrow_mut().remove_process(&old_name);

                                    // Remove the row widget from the sidebar
                                    if let Some(row) = rows_edit.borrow_mut().remove(&old_qname) {
                                        if let Some(parent) = row.widget().parent() {
                                            if let Some(parent_box) = parent.downcast_ref::<gtk4::Box>() {
                                                parent_box.remove(row.widget());
                                            }
                                        }
                                    }

                                    // Remove from section tracking and update counts
                                    statuses_edit.borrow_mut().remove(&old_qname);
                                    {
                                        let mut sections = sections_edit.borrow_mut();
                                        let mut empty_idx = None;
                                        for (idx, section) in sections.iter_mut().enumerate() {
                                            if section.process_names.contains(&old_qname) {
                                                section.process_names.retain(|n| n != &old_qname);
                                                let total = section.process_names.len();
                                                if total == 0 {
                                                    section.header.widget().set_visible(false);
                                                    empty_idx = Some(idx);
                                                } else {
                                                    let statuses = statuses_edit.borrow();
                                                    let running = section.process_names.iter()
                                                        .filter(|n| statuses.get(n.as_str()).map(|s| *s == ProcessStatus::Running).unwrap_or(false))
                                                        .count();
                                                    section.header.set_count(running, total);
                                                }
                                                break;
                                            }
                                        }
                                        if let Some(idx) = empty_idx {
                                            sections.remove(idx);
                                        }
                                    }

                                    // Notify to remove terminal from stack
                                    if let Some(ref cb) = *on_deleted_edit.borrow() {
                                        cb(&old_qname);
                                    }
                                    if let Some(ref cb) = *on_counts_edit.borrow() {
                                        cb();
                                    }
                                }
                            }
                        });
                    }
                    });
                }
                "delete" => {
                    let display_name = name.to_string();
                    let process_name = name.to_string();
                    let dialog = adw::AlertDialog::builder()
                        .heading(format!("Delete '{display_name}'?"))
                        .body("This will stop the process and remove it from the sidebar.")
                        .build();
                    dialog.add_response("cancel", "Cancel");
                    dialog.add_response("delete", "Delete");
                    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                    dialog.set_default_response(Some("cancel"));
                    dialog.set_close_response("cancel");

                    let mgr_del = mgr.clone();
                    let qname_del = qname.clone();
                    let process_rows_del = process_rows_ref.clone();
                    let sections_del = sections_ref.clone();
                    let statuses_del = statuses_ref.clone();
                    let on_deleted_del = on_deleted_ref.clone();
                    let on_counts_del = on_counts_ref.clone();
                    let ws_del = ws_ref.clone();

                    let win = win_ref.borrow().clone();
                    let parent_widget = win.map(|w| w.upcast::<gtk4::Widget>());

                    dialog.choose(parent_widget.as_ref(), gtk4::gio::Cancellable::NONE, move |response| {
                        if response != "delete" {
                            return;
                        }

                        // Persist deletion
                        if let Some((proj_name, proc_name)) = qname_del.split_once("::") {
                            if let Some(ref ws) = *ws_del.borrow() {
                                ws.borrow_mut().mark_process_deleted(proj_name, proc_name);
                            }
                        }

                        // Runtime removal
                        mgr_del.borrow_mut().remove_process(&process_name);

                        // Remove the row widget from the sidebar
                        if let Some(row) = process_rows_del.borrow_mut().remove(&qname_del) {
                            if let Some(parent) = row.widget().parent() {
                                if let Some(parent_box) = parent.downcast_ref::<gtk4::Box>() {
                                    parent_box.remove(row.widget());
                                }
                            }
                        }

                        // Remove from section tracking and update counts
                        statuses_del.borrow_mut().remove(&qname_del);
                        {
                            let mut sections = sections_del.borrow_mut();
                            let mut empty_idx = None;
                            for (idx, section) in sections.iter_mut().enumerate() {
                                if section.process_names.contains(&qname_del) {
                                    section.process_names.retain(|n| n != &qname_del);
                                    let total = section.process_names.len();
                                    if total == 0 {
                                        section.header.widget().set_visible(false);
                                        empty_idx = Some(idx);
                                    } else {
                                        let statuses = statuses_del.borrow();
                                        let running = section.process_names.iter()
                                            .filter(|n| statuses.get(n.as_str()).map(|s| *s == ProcessStatus::Running).unwrap_or(false))
                                            .count();
                                        section.header.set_count(running, total);
                                    }
                                    break;
                                }
                            }
                            if let Some(idx) = empty_idx {
                                sections.remove(idx);
                            }
                        }

                        // Notify to remove terminal from stack
                        if let Some(ref cb) = *on_deleted_del.borrow() {
                            cb(&qname_del);
                        }
                        if let Some(ref cb) = *on_counts_del.borrow() {
                            cb();
                        }
                    });
                }
                _ => log::warn!("Unknown row action: {action}"),
            }
        });
    }

    fn connect_row_dnd(
        &self,
        row: &ProcessRow,
        manager: &ProcessManagerRef,
        qualified_name: &str,
    ) {
        let proc_drag_source = gtk4::DragSource::new();
        proc_drag_source.set_actions(gdk::DragAction::MOVE);
        let qname_drag = qualified_name.to_string();
        proc_drag_source.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&glib::Value::from(&qname_drag)))
        });
        dnd::setup_drag_icon(&proc_drag_source, row.widget());
        row.widget().add_controller(proc_drag_source);

        let proc_drop_target = gtk4::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let row_widget = row.widget().clone();
        proc_drop_target.connect_motion(move |_, _, y| {
            dnd::update_drop_indicator(&row_widget, y);
            gdk::DragAction::MOVE
        });
        let row_widget2 = row.widget().clone();
        proc_drop_target.connect_leave(move |_| {
            dnd::clear_drop_indicator(&row_widget2);
        });
        let qname_drop = qualified_name.to_string();
        let sections_ref = self.sections.clone();
        let process_rows_ref = self.process_rows.clone();
        let mgr_ref = manager.clone();
        let proc_ws_ref = self.workspace.clone();
        proc_drop_target.connect_drop(move |_, value, _, y| {
            let Ok(dragged_qname) = value.get::<String>() else { return false };
            if dragged_qname == qname_drop {
                return false;
            }
            let Some((src_proj, src_proc)) = dragged_qname.split_once("::") else { return false };
            let Some((tgt_proj, tgt_proc)) = qname_drop.split_once("::") else { return false };
            if src_proj != tgt_proj {
                return false;
            }
            let mut sections = sections_ref.borrow_mut();
            let section = sections.iter_mut().find(|s| {
                s.project_name == src_proj
                    && s.process_names.contains(&dragged_qname)
                    && s.process_names.contains(&qname_drop)
            });
            let Some(section) = section else { return false };

            let rows = process_rows_ref.borrow();
            let Some(dragged_row) = rows.get(&dragged_qname) else { return false };
            let Some(target_row) = rows.get(&qname_drop) else { return false };

            let target_widget = target_row.widget();
            let height = target_widget.height() as f64;
            let before = y < height / 2.0;
            dnd::clear_drop_indicator(target_widget);
            dnd::reorder_in_box(section.header.content_box(), dragged_row.widget(), target_widget, before);

            section.process_names.retain(|n| n != &dragged_qname);
            let tgt_idx = section.process_names.iter().position(|n| n == &qname_drop).unwrap_or(0);
            let insert_idx = if before { tgt_idx } else { tgt_idx + 1 };
            section.process_names.insert(insert_idx, dragged_qname.clone());

            let mut mgr = mgr_ref.borrow_mut();
            mgr.reorder_process(src_proc, tgt_proc, before);
            let new_order = mgr.process_names().to_vec();
            drop(mgr);

            if let Some(ref ws) = *proc_ws_ref.borrow() {
                ws.borrow_mut().save_process_order(src_proj, new_order);
            }
            true
        });
        row.widget().add_controller(proc_drop_target);
    }

    fn connect_row_click(&self, row: &ProcessRow, qualified_name: &str) {
        let gesture = gtk4::GestureClick::new();
        let name = qualified_name.to_string();
        let cb_ref = self.on_process_selected.clone();
        let rows_ref = self.process_rows.clone();
        let selected_ref = self.selected_qname.clone();
        gesture.connect_released(move |_, _, _, _| {
            // Remove highlight from previous selection
            {
                let rows = rows_ref.borrow();
                if let Some(ref prev) = *selected_ref.borrow() {
                    if let Some(prev_row) = rows.get(prev.as_str()) {
                        prev_row.widget().remove_css_class("process-row-selected");
                    }
                }
                // Highlight new selection
                if let Some(row) = rows.get(name.as_str()) {
                    row.widget().add_css_class("process-row-selected");
                }
            }
            *selected_ref.borrow_mut() = Some(name.clone());
            if let Some(ref cb) = *cb_ref.borrow() {
                cb(&name);
            }
        });
        row.widget().add_controller(gesture);
    }

    /// Add a single process row to an existing project in the sidebar.
    pub fn add_process_to_project(
        &self,
        manager: &ProcessManagerRef,
        project_name: &str,
        process_name: &str,
        status: ProcessStatus,
        category: ProcessCategory,
    ) {
        let qname = workspace::qualified_name(project_name, process_name);
        let (command, display_name) = {
            let mgr = manager.borrow();
            let proc = mgr.get_process(process_name);
            let cmd = proc.map(|p| p.config.command.clone()).unwrap_or_default();
            let display = proc.and_then(|p| p.config.display_name.clone());
            (cmd, display)
        };

        let row = if category == ProcessCategory::Terminal || category == ProcessCategory::SSH {
            ProcessRow::new_terminal(process_name, &command)
        } else {
            ProcessRow::new(process_name, &command)
        };
        if let Some(display) = &display_name {
            row.set_name(display);
        }
        row.set_status(status);
        self.connect_row_click(&row, &qname);
        Self::connect_row_actions(
            &row, manager, &qname,
            &self.on_process_selected, &self.process_rows, &self.sections,
            &self.process_statuses, &self.on_process_deleted, &self.on_counts_changed,
            &self.window, &self.workspace, &self.selected_qname,
        );
        self.connect_row_dnd(&row, manager, &qname);

        let (section_title, section_icon) = match category {
            ProcessCategory::Agent => ("AGENTS", "ai-brain-symbolic"),
            ProcessCategory::Terminal => ("TERMINALS", "utilities-terminal-symbolic"),
            ProcessCategory::Command => ("COMMANDS", "view-list-symbolic"),
            ProcessCategory::SSH => ("SSH", "network-server-symbolic"),
        };

        // Find existing section for this project and category
        let mut sections = self.sections.borrow_mut();
        let existing = sections.iter_mut().find(|s| {
            s.project_name == project_name && s.category_title == section_title
        });

        if let Some(section) = existing {
            section.header.content_box().append(row.widget());
            section.process_names.push(qname.clone());
            self.process_statuses.borrow_mut().insert(qname.clone(), status);
            self.process_rows.borrow_mut().insert(qname, row);
            // Update counts
            let statuses = self.process_statuses.borrow();
            let total = section.process_names.len();
            let running = section.process_names.iter()
                .filter(|n| statuses.get(n.as_str()).map(|s| *s == ProcessStatus::Running).unwrap_or(false))
                .count();
            section.header.set_count(running, total);
        } else {
            // Create a new section
            let section = SectionHeader::new(section_title, section_icon);
            section.content_box().append(row.widget());
            self.process_statuses.borrow_mut().insert(qname.clone(), status);
            self.process_rows.borrow_mut().insert(qname.clone(), row);
            section.set_count(if status == ProcessStatus::Running { 1 } else { 0 }, 1);

            // Insert section at the correct position (AGENTS → COMMANDS → TERMINALS)
            if let Some(project_row) = self.project_rows.borrow().get(project_name) {
                let order = Self::section_order(section_title);
                // Find the first existing section for this project that should come after
                let insert_before = sections.iter().find(|s| {
                    s.project_name == project_name && Self::section_order(&s.category_title) > order
                }).map(|s| s.header.widget().clone());

                if let Some(ref before_widget) = insert_before {
                    project_row.content_box().insert_child_after(section.widget(), before_widget.prev_sibling().as_ref());
                } else {
                    project_row.content_box().append(section.widget());
                }
            }

            sections.push(SectionInfo {
                project_name: project_name.to_string(),
                category_title: section_title.to_string(),
                header: section,
                process_names: vec![qname],
            });
        }
    }

    pub fn update_process_status(&self, qualified_name: &str, status: ProcessStatus) {
        if let Some(row) = self.process_rows.borrow().get(qualified_name) {
            row.set_status(status);
        }
        self.process_statuses.borrow_mut().insert(qualified_name.to_string(), status);
        self.refresh_section_counts();
        self.notify_counts_changed();
    }

    fn section_order(category_title: &str) -> u8 {
        match category_title {
            "AGENTS" => 0,
            "COMMANDS" => 1,
            "TERMINALS" => 2,
            "SSH" => 3,
            _ => 4,
        }
    }

    fn notify_counts_changed(&self) {
        if let Some(ref cb) = *self.on_counts_changed.borrow() {
            cb();
        }
    }

    fn refresh_section_counts(&self) {
        let statuses = self.process_statuses.borrow();
        for section_info in self.sections.borrow().iter() {
            let total = section_info.process_names.len();
            let running = section_info.process_names.iter()
                .filter(|qname| {
                    statuses.get(qname.as_str())
                        .map(|s| *s == ProcessStatus::Running)
                        .unwrap_or(false)
                })
                .count();
            section_info.header.set_count(running, total);
        }
    }

    pub fn set_process_resources(&self, qualified_name: &str, cpu_percent: f64, memory_mb: f64) {
        if let Some(row) = self.process_rows.borrow().get(qualified_name) {
            let settings = crate::config::settings::AppSettings::load();
            let cpu_threshold = Self::process_cpu_threshold_value(settings.sidebar.process_cpu_threshold);
            let mem_threshold = Self::process_mem_threshold_value(settings.sidebar.process_mem_threshold);
            row.set_resources(cpu_percent, memory_mb, cpu_threshold, mem_threshold);
        }
    }

    /// Maps process CPU threshold combo index to a percentage value.
    /// Returns -1.0 for "Never" (hide always).
    fn process_cpu_threshold_value(index: u32) -> f64 {
        match index {
            0 => 0.0,     // Always
            1 => 10.0,    // 10%
            2 => 30.0,    // 30%
            3 => 60.0,    // 60%
            4 => 90.0,    // 90%
            _ => -1.0,    // Never
        }
    }

    /// Maps process memory threshold combo index to MB value.
    /// Returns -1.0 for "Never" (hide always).
    fn process_mem_threshold_value(index: u32) -> f64 {
        match index {
            0 => 0.0,       // Always
            1 => 100.0,     // 100MB
            2 => 500.0,     // 500MB
            3 => 1024.0,    // 1GB
            4 => 2048.0,    // 2GB
            _ => -1.0,      // Never
        }
    }

    pub fn is_process_running(&self, qualified_name: &str) -> bool {
        self.process_statuses
            .borrow()
            .get(qualified_name)
            .map_or(false, |s| matches!(s, ProcessStatus::Running | ProcessStatus::Restarting))
    }

    pub fn set_process_port(&self, qualified_name: &str, port: Option<u16>) {
        if let Some(row) = self.process_rows.borrow().get(qualified_name) {
            row.set_port(port);
        }
    }

    pub fn set_process_url(&self, qualified_name: &str, url: Option<&str>) {
        if let Some(row) = self.process_rows.borrow().get(qualified_name) {
            row.set_url(url);
        }
    }

    pub fn set_process_name(&self, qualified_name: &str, display_name: &str) {
        if let Some(row) = self.process_rows.borrow().get(qualified_name) {
            row.set_name(display_name);
        }
    }

    pub fn get_process_url(&self, qualified_name: &str) -> Option<String> {
        self.process_rows
            .borrow()
            .get(qualified_name)
            .and_then(|row| row.get_url())
    }

    pub fn set_single_project_expand(&self, enabled: bool) {
        self.single_expand.set(enabled);
    }

    pub fn toggle_filter(&self) {
        self.search_btn.set_active(!self.search_btn.is_active());
    }

    /// Remove a process from the sidebar and notify listeners.
    pub fn remove_process(&self, qualified_name: &str) {
        // Remove the row widget
        if let Some(row) = self.process_rows.borrow_mut().remove(qualified_name) {
            if let Some(parent) = row.widget().parent() {
                if let Some(parent_box) = parent.downcast_ref::<gtk4::Box>() {
                    parent_box.remove(row.widget());
                }
            }
        }

        // Remove from section tracking and update counts
        self.process_statuses.borrow_mut().remove(qualified_name);
        {
            let mut sections = self.sections.borrow_mut();
            let mut empty_idx = None;
            for (idx, section) in sections.iter_mut().enumerate() {
                if section.process_names.contains(&qualified_name.to_string()) {
                    section.process_names.retain(|n| n != qualified_name);
                    let total = section.process_names.len();
                    if total == 0 {
                        section.header.widget().set_visible(false);
                        empty_idx = Some(idx);
                    } else {
                        let statuses = self.process_statuses.borrow();
                        let running = section.process_names.iter()
                            .filter(|n| statuses.get(n.as_str()).is_some_and(|s| *s == ProcessStatus::Running))
                            .count();
                        section.header.set_count(running, total);
                    }
                    break;
                }
            }
            if let Some(idx) = empty_idx {
                sections.remove(idx);
            }
        }

        // Notify listeners
        if let Some(ref cb) = *self.on_process_deleted.borrow() {
            cb(qualified_name);
        }
        if let Some(ref cb) = *self.on_counts_changed.borrow() {
            cb();
        }
    }

    /// Highlight the active project row and clear others.
    pub fn set_active_project(&self, project_name: &str) {
        let rows = self.project_rows.borrow();
        for (key, row) in rows.iter() {
            row.set_active(key == project_name);
        }
    }

    /// Expand a project by name, respecting accordion mode.
    pub fn expand_project(&self, project_name: &str) {
        let rows = self.project_rows.borrow();
        if let Some(row) = rows.get(project_name) {
            if !row.is_expanded() {
                row.set_expanded(true);
                if let Some(ref ws) = *self.workspace.borrow() {
                    ws.borrow_mut().set_project_expanded(project_name, true);
                }
            }
            // Collapse others in accordion mode
            if self.single_expand.get() {
                for (key, other_row) in rows.iter() {
                    if key != project_name && other_row.is_expanded() {
                        other_row.set_expanded(false);
                        if let Some(ref ws) = *self.workspace.borrow() {
                            ws.borrow_mut().set_project_expanded(key, false);
                        }
                    }
                }
            }
        }
    }

    /// Returns the name of the last expanded project in the sidebar, if any.
    pub fn last_expanded_project(&self) -> Option<String> {
        let rows = self.project_rows.borrow();
        // Find the last expanded project (iterate in order of sections for consistency)
        let sections = self.sections.borrow();
        for section in sections.iter().rev() {
            if rows.get(&section.project_name).is_some_and(|r| r.is_expanded()) {
                return Some(section.project_name.clone());
            }
        }
        None
    }

    pub fn search_button(&self) -> &gtk4::ToggleButton {
        &self.search_btn
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.outer_container
    }
}
