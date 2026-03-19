use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;

#[derive(Clone)]
pub struct PaletteItem {
    pub category: String,
    pub label: String,
    pub icon: String,
    pub action: String,
}

pub struct CommandPalette {
    overlay: gtk4::Overlay,
    revealer: gtk4::Revealer,
    entry: gtk4::SearchEntry,
    results_box: gtk4::ListBox,
    items: Rc<RefCell<Vec<PaletteItem>>>,
    on_action: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
}

impl CommandPalette {
    pub fn new() -> Self {
        let overlay = gtk4::Overlay::new();

        // Semi-transparent backdrop
        let backdrop = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        backdrop.add_css_class("command-palette-backdrop");
        backdrop.set_vexpand(true);
        backdrop.set_hexpand(true);

        // Center container
        let center = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        center.set_halign(gtk4::Align::Center);
        center.set_valign(gtk4::Align::Start);
        center.set_margin_top(80);
        center.set_width_request(500);
        center.add_css_class("command-palette");

        // Search entry
        let entry = gtk4::SearchEntry::builder()
            .placeholder_text("New command, terminal, or agent...")
            .hexpand(true)
            .build();
        entry.add_css_class("command-palette-entry");
        center.append(&entry);

        // Results list
        let results_scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .max_content_height(400)
            .propagate_natural_height(true)
            .build();

        let results_box = gtk4::ListBox::new();
        results_box.add_css_class("command-palette-results");
        results_box.set_selection_mode(gtk4::SelectionMode::Single);
        results_scroll.set_child(Some(&results_box));
        center.append(&results_scroll);

        // Footer hint
        let footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        footer.set_margin_start(12);
        footer.set_margin_end(12);
        footer.set_margin_top(8);
        footer.set_margin_bottom(8);

        let hints = [
            ("\u{2191}\u{2193}", "navigate"),
            ("\u{21B5}", "select"),
            ("esc", "close"),
        ];
        for (key, label) in hints {
            let hint_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            let key_label = gtk4::Label::builder()
                .label(key)
                .css_classes(["caption-heading"])
                .build();
            let desc_label = gtk4::Label::builder()
                .label(label)
                .css_classes(["caption", "dim-label"])
                .build();
            hint_box.append(&key_label);
            hint_box.append(&desc_label);
            footer.append(&hint_box);
        }
        center.append(&footer);

        // Revealer wrapping the whole thing
        let revealer = gtk4::Revealer::builder()
            .reveal_child(false)
            .transition_type(gtk4::RevealerTransitionType::Crossfade)
            .transition_duration(150)
            .halign(gtk4::Align::Fill)
            .valign(gtk4::Align::Fill)
            .build();

        let revealer_content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        revealer_content.append(&backdrop);

        // Use an overlay to center the palette on top of the backdrop
        let inner_overlay = gtk4::Overlay::new();
        inner_overlay.set_child(Some(&revealer_content));
        inner_overlay.add_overlay(&center);

        revealer.set_child(Some(&inner_overlay));

        let items = Rc::new(RefCell::new(Vec::new()));
        let on_action: Rc<RefCell<Option<Box<dyn Fn(&str)>>>> = Rc::new(RefCell::new(None));

        // Filter on search
        let items_ref = items.clone();
        let results_ref = results_box.clone();
        let on_action_ref = on_action.clone();
        entry.connect_search_changed(move |entry| {
            let query = entry.text().to_string().to_lowercase();
            Self::populate_results(&results_ref, &items_ref.borrow(), &query, &on_action_ref);
        });

        // Close on backdrop click
        let gesture = gtk4::GestureClick::new();
        let revealer_ref = revealer.clone();
        gesture.connect_released(move |_, _, _, _| {
            revealer_ref.set_reveal_child(false);
        });
        backdrop.add_controller(gesture);

        // Enter to select first result
        let revealer_ref2 = revealer.clone();
        let on_action_ref2 = on_action.clone();
        let items_ref2 = items.clone();
        entry.connect_activate(move |entry| {
            let query = entry.text().to_string().to_lowercase();
            let items = items_ref2.borrow();
            let matched: Vec<&PaletteItem> = if query.is_empty() {
                items.iter().collect()
            } else {
                items
                    .iter()
                    .filter(|i| i.label.to_lowercase().contains(&query) || i.category.to_lowercase().contains(&query))
                    .collect()
            };

            if let Some(item) = matched.first() {
                if let Some(ref cb) = *on_action_ref2.borrow() {
                    cb(&item.action);
                }
                revealer_ref2.set_reveal_child(false);
            }
        });

        let palette = Self {
            overlay,
            revealer,
            entry,
            results_box,
            items,
            on_action,
        };

        // Populate with default items
        palette.set_items(Self::default_items());

        palette
    }

    fn default_items() -> Vec<PaletteItem> {
        vec![
            PaletteItem {
                category: "PROJECT".to_string(),
                label: "Create new terminal tab".to_string(),
                icon: "utilities-terminal-symbolic".to_string(),
                action: "new_terminal".to_string(),
            },
            PaletteItem {
                category: "PROJECT".to_string(),
                label: "Add new process".to_string(),
                icon: "list-add-symbolic".to_string(),
                action: "add_process".to_string(),
            },
            PaletteItem {
                category: "ACTIONS".to_string(),
                label: "Start all processes".to_string(),
                icon: "media-playback-start-symbolic".to_string(),
                action: "start_all".to_string(),
            },
            PaletteItem {
                category: "ACTIONS".to_string(),
                label: "Stop all processes".to_string(),
                icon: "media-playback-stop-symbolic".to_string(),
                action: "stop_all".to_string(),
            },
            PaletteItem {
                category: "ACTIONS".to_string(),
                label: "Restart all processes".to_string(),
                icon: "view-refresh-symbolic".to_string(),
                action: "restart_all".to_string(),
            },
        ]
    }

    pub fn add_navigation_items(&self, process_names: &[String]) {
        let mut items = self.items.borrow_mut();
        for name in process_names {
            items.push(PaletteItem {
                category: "NAVIGATION".to_string(),
                label: format!("Switch to {name}"),
                icon: "go-jump-symbolic".to_string(),
                action: format!("switch:{name}"),
            });
        }
    }

    pub fn set_items(&self, new_items: Vec<PaletteItem>) {
        *self.items.borrow_mut() = new_items;
        Self::populate_results(
            &self.results_box,
            &self.items.borrow(),
            "",
            &self.on_action,
        );
    }

    pub fn set_on_action(&self, cb: impl Fn(&str) + 'static) {
        *self.on_action.borrow_mut() = Some(Box::new(cb));
    }

    fn populate_results(
        results_box: &gtk4::ListBox,
        items: &[PaletteItem],
        query: &str,
        _on_action: &Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
    ) {
        // Clear
        while let Some(child) = results_box.first_child() {
            results_box.remove(&child);
        }

        let filtered: Vec<&PaletteItem> = if query.is_empty() {
            items.iter().collect()
        } else {
            items
                .iter()
                .filter(|i| {
                    i.label.to_lowercase().contains(query)
                        || i.category.to_lowercase().contains(query)
                        || i.action.to_lowercase().contains(query)
                })
                .collect()
        };

        for item in filtered {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
            row.set_margin_start(12);
            row.set_margin_end(12);
            row.set_margin_top(6);
            row.set_margin_bottom(6);

            let cat_label = gtk4::Label::builder()
                .label(&item.category)
                .width_request(90)
                .css_classes(["caption", "dim-label"])
                .halign(gtk4::Align::Start)
                .build();
            row.append(&cat_label);

            let icon = gtk4::Image::from_icon_name(&item.icon);
            row.append(&icon);

            let label = gtk4::Label::builder()
                .label(&item.label)
                .halign(gtk4::Align::Start)
                .hexpand(true)
                .build();
            row.append(&label);

            let list_row = gtk4::ListBoxRow::new();
            list_row.set_child(Some(&row));
            list_row.add_css_class("command-palette-row");

            results_box.append(&list_row);
        }

        // Select first row
        if let Some(first) = results_box.row_at_index(0) {
            results_box.select_row(Some(&first));
        }
    }

    pub fn toggle(&self) {
        let visible = self.revealer.reveals_child();
        self.revealer.set_reveal_child(!visible);
        if !visible {
            self.entry.set_text("");
            self.entry.grab_focus();
            Self::populate_results(
                &self.results_box,
                &self.items.borrow(),
                "",
                &self.on_action,
            );
        }
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
    }

    pub fn is_visible(&self) -> bool {
        self.revealer.reveals_child()
    }

    pub fn widget(&self) -> &gtk4::Revealer {
        &self.revealer
    }
}
