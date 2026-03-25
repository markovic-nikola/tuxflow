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
    on_refresh: Rc<RefCell<Option<Box<dyn Fn(&CommandPalette)>>>>,
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
        let footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
        footer.set_margin_start(12);
        footer.set_margin_end(12);
        footer.set_margin_top(6);
        footer.set_margin_bottom(6);
        footer.set_halign(gtk4::Align::Start);

        let hints = [
            ("\u{2191} \u{2193}", "navigate"),
            ("\u{21B5}", "select"),
            ("esc", "close"),
        ];
        for (key, label) in hints {
            let hint_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            let key_label = gtk4::Label::builder()
                .label(key)
                .css_classes(["caption", "kbd-badge"])
                .build();
            let desc_label = gtk4::Label::builder()
                .label(label)
                .css_classes(["dim-label", "palette-hint"])
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
            .can_target(false)
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
        let on_refresh: Rc<RefCell<Option<Box<dyn Fn(&CommandPalette)>>>> =
            Rc::new(RefCell::new(None));

        // Filter on search
        let items_ref = items.clone();
        let results_ref = results_box.clone();
        let on_action_ref = on_action.clone();
        entry.connect_search_changed(move |entry| {
            let query = entry.text().to_string().to_lowercase();
            Self::populate_results(&results_ref, &items_ref.borrow(), &query, &on_action_ref);
        });

        // Arrow keys + Escape on the entry
        let key_controller = gtk4::EventControllerKey::new();
        let results_ref_key = results_box.clone();
        let revealer_ref_key = revealer.clone();
        let scroll_ref = results_scroll.clone();
        key_controller.connect_key_pressed(move |_, keyval, _, _| {
            use gtk4::gdk::Key;
            match keyval {
                Key::Escape => {
                    revealer_ref_key.set_reveal_child(false);
                    revealer_ref_key.set_can_target(false);
                    gtk4::glib::Propagation::Stop
                }
                Key::Down => {
                    let rb = &results_ref_key;
                    let current = rb.selected_row().map(|r| r.index()).unwrap_or(-1);
                    if let Some(next) = rb.row_at_index(current + 1) {
                        rb.select_row(Some(&next));
                        Self::scroll_to_row(&scroll_ref, &next);
                    }
                    gtk4::glib::Propagation::Stop
                }
                Key::Up => {
                    let rb = &results_ref_key;
                    let current = rb.selected_row().map(|r| r.index()).unwrap_or(0);
                    if current > 0 {
                        if let Some(prev) = rb.row_at_index(current - 1) {
                            rb.select_row(Some(&prev));
                            Self::scroll_to_row(&scroll_ref, &prev);
                        }
                    }
                    gtk4::glib::Propagation::Stop
                }
                _ => gtk4::glib::Propagation::Proceed,
            }
        });
        entry.add_controller(key_controller);

        // Close on backdrop click
        let gesture = gtk4::GestureClick::new();
        let revealer_ref = revealer.clone();
        gesture.connect_released(move |_, _, _, _| {
            revealer_ref.set_reveal_child(false);
            revealer_ref.set_can_target(false);
        });
        backdrop.add_controller(gesture);

        // Enter to activate selected row
        let revealer_ref2 = revealer.clone();
        let on_action_ref2 = on_action.clone();
        let items_ref2 = items.clone();
        let results_ref2 = results_box.clone();
        entry.connect_activate(move |entry| {
            let query = entry.text().to_string().to_lowercase();
            let items = items_ref2.borrow();
            let matched: Vec<&PaletteItem> = if query.is_empty() {
                items.iter().collect()
            } else {
                items
                    .iter()
                    .filter(|i| {
                        i.label.to_lowercase().contains(&query)
                            || i.category.to_lowercase().contains(&query)
                            || i.action.to_lowercase().contains(&query)
                    })
                    .collect()
            };

            // Use selected row index, fall back to first
            let idx = results_ref2
                .selected_row()
                .map(|r| r.index() as usize)
                .unwrap_or(0);

            if let Some(item) = matched.get(idx) {
                if let Some(ref cb) = *on_action_ref2.borrow() {
                    cb(&item.action);
                }
                revealer_ref2.set_reveal_child(false);
                revealer_ref2.set_can_target(false);
            }
        });

        // Click on row to activate
        let on_action_ref3 = on_action.clone();
        let items_ref3 = items.clone();
        let revealer_ref3 = revealer.clone();
        let entry_ref = entry.clone();
        results_box.connect_row_activated(move |_, row| {
            let idx = row.index() as usize;
            let query = entry_ref.text().to_string().to_lowercase();
            let items = items_ref3.borrow();
            let matched: Vec<&PaletteItem> = if query.is_empty() {
                items.iter().collect()
            } else {
                items
                    .iter()
                    .filter(|i| {
                        i.label.to_lowercase().contains(&query)
                            || i.category.to_lowercase().contains(&query)
                            || i.action.to_lowercase().contains(&query)
                    })
                    .collect()
            };

            if let Some(item) = matched.get(idx) {
                if let Some(ref cb) = *on_action_ref3.borrow() {
                    cb(&item.action);
                }
                revealer_ref3.set_reveal_child(false);
                revealer_ref3.set_can_target(false);
            }
        });

        let palette = Self {
            overlay,
            revealer,
            entry,
            results_box,
            items,
            on_action,
            on_refresh,
        };

        // Populate with default items
        palette.set_items(Self::default_items());

        palette
    }

    fn default_items() -> Vec<PaletteItem> {
        vec![
            // Matches sidebar order: AGENTS → COMMANDS → TERMINALS → SSH
            PaletteItem {
                category: "AGENT".to_string(),
                label: "New Claude agent".to_string(),
                icon: "ai-brain-symbolic".to_string(),
                action: "new_agent:claude".to_string(),
            },
            PaletteItem {
                category: "AGENT".to_string(),
                label: "New Codex agent".to_string(),
                icon: "ai-brain-symbolic".to_string(),
                action: "new_agent:codex".to_string(),
            },
            PaletteItem {
                category: "AGENT".to_string(),
                label: "New Gemini agent".to_string(),
                icon: "ai-brain-symbolic".to_string(),
                action: "new_agent:gemini".to_string(),
            },
            PaletteItem {
                category: "AGENT".to_string(),
                label: "New OpenCode agent".to_string(),
                icon: "ai-brain-symbolic".to_string(),
                action: "new_agent:opencode".to_string(),
            },
            PaletteItem {
                category: "AGENT".to_string(),
                label: "New custom agent".to_string(),
                icon: "ai-brain-symbolic".to_string(),
                action: "new_custom_agent".to_string(),
            },
            PaletteItem {
                category: "COMMAND".to_string(),
                label: "New command".to_string(),
                icon: "list-add-symbolic".to_string(),
                action: "add_process".to_string(),
            },
            PaletteItem {
                category: "TERMINAL".to_string(),
                label: "New terminal tab".to_string(),
                icon: "utilities-terminal-symbolic".to_string(),
                action: "new_terminal".to_string(),
            },
            PaletteItem {
                category: "SSH".to_string(),
                label: "New SSH connection".to_string(),
                icon: "network-server-symbolic".to_string(),
                action: "new_ssh".to_string(),
            },
            PaletteItem {
                category: "PROJECT".to_string(),
                label: "New project (open directory)".to_string(),
                icon: "folder-open-symbolic".to_string(),
                action: "add_project".to_string(),
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
        Self::populate_results(&self.results_box, &self.items.borrow(), "", &self.on_action);
    }

    pub fn set_on_action(&self, cb: impl Fn(&str) + 'static) {
        *self.on_action.borrow_mut() = Some(Box::new(cb));
    }

    pub fn set_on_refresh(&self, cb: impl Fn(&CommandPalette) + 'static) {
        *self.on_refresh.borrow_mut() = Some(Box::new(cb));
    }

    fn refresh(&self) {
        // Remove existing navigation items
        self.items
            .borrow_mut()
            .retain(|item| item.category != "NAVIGATION");
        // Let the callback re-add current ones
        if let Some(ref cb) = *self.on_refresh.borrow() {
            cb(self);
        }
    }

    fn scroll_to_row(scroll: &gtk4::ScrolledWindow, row: &gtk4::ListBoxRow) {
        let adj = scroll.vadjustment();
        let alloc = row.allocation();
        let y = alloc.y() as f64;
        let height = alloc.height() as f64;
        let visible_top = adj.value();
        let visible_height = adj.page_size();

        if y + height > visible_top + visible_height {
            adj.set_value(y + height - visible_height);
        } else if y < visible_top {
            adj.set_value(y);
        }
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
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
            row.set_margin_start(8);
            row.set_margin_end(8);
            row.set_margin_top(2);
            row.set_margin_bottom(2);

            // Category badge
            let cat_label = gtk4::Label::builder()
                .label(&item.category)
                .width_request(72)
                .css_classes(["caption", "palette-category"])
                .halign(gtk4::Align::End)
                .build();
            row.append(&cat_label);

            // Icon
            let icon = gtk4::Image::from_icon_name(&item.icon);
            icon.set_pixel_size(16);
            icon.add_css_class("dim-label");
            row.append(&icon);

            // Label
            let label = gtk4::Label::builder()
                .label(&item.label)
                .halign(gtk4::Align::Start)
                .hexpand(true)
                .css_classes(["palette-label"])
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
        self.revealer.set_can_target(!visible);
        if !visible {
            self.refresh();
            self.entry.set_text("");
            self.entry.grab_focus();
            Self::populate_results(&self.results_box, &self.items.borrow(), "", &self.on_action);
        }
    }

    pub fn show_with_text(&self, text: &str) {
        self.refresh();
        self.revealer.set_reveal_child(true);
        self.revealer.set_can_target(true);
        self.entry.set_text(text);
        self.entry.set_position(-1); // cursor at end
        self.entry.grab_focus();
        let query = text.to_lowercase();
        Self::populate_results(
            &self.results_box,
            &self.items.borrow(),
            &query,
            &self.on_action,
        );
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
        self.revealer.set_can_target(false);
    }

    pub fn is_visible(&self) -> bool {
        self.revealer.reveals_child()
    }

    pub fn widget(&self) -> &gtk4::Revealer {
        &self.revealer
    }
}
