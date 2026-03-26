use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::schema::ProcessConfig;
use crate::detect::detector::DetectedStack;

pub struct SelectCommandsDialog;

impl SelectCommandsDialog {
    pub fn show(
        parent: &impl IsA<gtk4::Widget>,
        project_name: &str,
        stacks: &[DetectedStack],
        on_confirm: impl FnOnce(Vec<ProcessConfig>) + 'static,
    ) {
        let total: usize = stacks.iter().map(|s| s.suggested_processes.len()).sum();

        let dialog = adw::Dialog::builder()
            .title("Select Commands")
            .content_width(500)
            .content_height(550)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let headerbar = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&headerbar);

        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .build();

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        // Description
        let desc = gtk4::Label::new(Some(&format!(
            "{total} commands detected for \"{project_name}\". Select which to add:"
        )));
        desc.add_css_class("dim-label");
        desc.set_margin_bottom(12);
        desc.set_wrap(true);
        content.append(&desc);

        // Collect all switch rows paired with their ProcessConfig
        let switches: Rc<RefCell<Vec<(adw::SwitchRow, ProcessConfig)>>> =
            Rc::new(RefCell::new(Vec::with_capacity(total)));

        // Per-stack sections
        for stack in stacks {
            let group = adw::PreferencesGroup::builder()
                .title(&stack.name)
                .margin_top(8)
                .build();

            for proc_config in &stack.suggested_processes {
                let row = adw::SwitchRow::builder()
                    .title(&proc_config.name)
                    .subtitle(&proc_config.command)
                    .active(true)
                    .build();
                group.add(&row);
                switches.borrow_mut().push((row, proc_config.clone()));
            }

            content.append(&group);
        }

        // Select All / Deselect All
        let btn_row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .halign(gtk4::Align::Center)
            .spacing(8)
            .margin_top(16)
            .build();

        let select_all_btn = gtk4::Button::builder()
            .label("Select All")
            .css_classes(["flat"])
            .build();

        let deselect_all_btn = gtk4::Button::builder()
            .label("Deselect All")
            .css_classes(["flat"])
            .build();

        {
            let sw = switches.clone();
            select_all_btn.connect_clicked(move |_| {
                for (row, _) in sw.borrow().iter() {
                    row.set_active(true);
                }
            });
        }
        {
            let sw = switches.clone();
            deselect_all_btn.connect_clicked(move |_| {
                for (row, _) in sw.borrow().iter() {
                    row.set_active(false);
                }
            });
        }

        btn_row.append(&select_all_btn);
        btn_row.append(&deselect_all_btn);
        content.append(&btn_row);

        // Confirm button with dynamic count
        let confirm_btn = gtk4::Button::builder()
            .label(&format!("Add {total} Commands"))
            .css_classes(["suggested-action", "pill"])
            .margin_top(24)
            .halign(gtk4::Align::Center)
            .build();
        content.append(&confirm_btn);

        // Update button label when switches toggle
        {
            let sw = switches.clone();
            let btn = confirm_btn.clone();
            let update_label = move || {
                let count = sw.borrow().iter().filter(|(r, _)| r.is_active()).count();
                btn.set_label(&format!("Add {count} Commands"));
            };

            let sw2 = switches.clone();
            for (row, _) in sw2.borrow().iter() {
                let update = update_label.clone();
                row.connect_active_notify(move |_| update());
            }
        }

        scrolled.set_child(Some(&content));
        toolbar_view.set_content(Some(&scrolled));
        dialog.set_child(Some(&toolbar_view));

        let dialog_ref = dialog.clone();
        // Wrap FnOnce in Cell<Option<>> so it can be called from an Fn closure
        let on_confirm = Cell::new(Some(on_confirm));

        confirm_btn.connect_clicked(move |_| {
            let selected: Vec<ProcessConfig> = switches
                .borrow()
                .iter()
                .filter(|(row, _)| row.is_active())
                .map(|(_, config)| config.clone())
                .collect();
            if let Some(cb) = on_confirm.take() {
                cb(selected);
            }
            dialog_ref.close();
        });

        dialog.present(Some(parent));
    }
}
