use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::prelude::*;

use crate::config::schema::ProcessCategory;
use crate::process::manager::{ProcessManagerRef, ProcessStatus};

use super::process_row::ProcessRow;
use super::project_row::ProjectRow;
use super::section_header::SectionHeader;

pub struct ProjectList {
    container: gtk4::Box,
    process_rows: Rc<RefCell<HashMap<String, ProcessRow>>>,
    on_process_selected: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
}

impl ProjectList {
    pub fn new() -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.set_vexpand(true);
        container.add_css_class("sidebar");

        Self {
            container,
            process_rows: Rc::new(RefCell::new(HashMap::new())),
            on_process_selected: Rc::new(RefCell::new(None)),
        }
    }

    pub fn set_on_process_selected(&self, cb: impl Fn(&str) + 'static) {
        *self.on_process_selected.borrow_mut() = Some(Box::new(cb));
    }

    pub fn populate(&self, manager: &ProcessManagerRef, project_name: &str) {
        // Clear existing content
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }

        let mgr = manager.borrow();

        // Create project row
        let project_row = ProjectRow::new(project_name);

        // Create sections
        let agents = mgr.processes_by_category(ProcessCategory::Agent);
        let terminals = mgr.processes_by_category(ProcessCategory::Terminal);
        let commands = mgr.processes_by_category(ProcessCategory::Command);

        // Agents section
        if !agents.is_empty() {
            let section = SectionHeader::new("AGENTS", "system-users-symbolic");
            let running = agents.iter().filter(|p| p.status == ProcessStatus::Running).count();
            section.set_count(running, agents.len());

            for proc in &agents {
                let row = ProcessRow::new(&proc.config.name);
                row.set_status(proc.status);
                self.connect_row_click(&row);
                section.content_box().append(row.widget());
                self.process_rows.borrow_mut().insert(proc.config.name.clone(), row);
            }

            project_row.content_box().append(section.widget());
        }

        // Terminals section
        if !terminals.is_empty() {
            let section = SectionHeader::new("TERMINALS", "utilities-terminal-symbolic");
            let running = terminals.iter().filter(|p| p.status == ProcessStatus::Running).count();
            section.set_count(running, terminals.len());

            for proc in &terminals {
                let row = ProcessRow::new(&proc.config.name);
                row.set_status(proc.status);
                self.connect_row_click(&row);
                section.content_box().append(row.widget());
                self.process_rows.borrow_mut().insert(proc.config.name.clone(), row);
            }

            project_row.content_box().append(section.widget());
        }

        // Commands section
        if !commands.is_empty() {
            let section = SectionHeader::new("COMMANDS", "view-list-symbolic");
            let running = commands.iter().filter(|p| p.status == ProcessStatus::Running).count();
            section.set_count(running, commands.len());

            for proc in &commands {
                let row = ProcessRow::new(&proc.config.name);
                row.set_status(proc.status);
                self.connect_row_click(&row);
                section.content_box().append(row.widget());
                self.process_rows.borrow_mut().insert(proc.config.name.clone(), row);
            }

            project_row.content_box().append(section.widget());
        }

        self.container.append(project_row.widget());
    }

    fn connect_row_click(&self, row: &ProcessRow) {
        let gesture = gtk4::GestureClick::new();
        let name = row.name();
        let cb_ref = self.on_process_selected.clone();
        gesture.connect_released(move |_, _, _, _| {
            if let Some(ref cb) = *cb_ref.borrow() {
                cb(&name);
            }
        });
        row.widget().add_controller(gesture);
    }

    pub fn update_process_status(&self, name: &str, status: ProcessStatus) {
        if let Some(row) = self.process_rows.borrow().get(name) {
            row.set_status(status);
        }
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }
}
