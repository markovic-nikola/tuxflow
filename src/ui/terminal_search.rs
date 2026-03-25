use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use vte4::prelude::*;

pub struct TerminalSearch {
    container: gtk4::Revealer,
    entry: gtk4::SearchEntry,
    match_label: gtk4::Label,
    terminal: Rc<RefCell<Option<vte4::Terminal>>>,
}

impl TerminalSearch {
    pub fn new() -> Self {
        let container = gtk4::Revealer::builder()
            .reveal_child(false)
            .transition_type(gtk4::RevealerTransitionType::SlideDown)
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Start)
            .can_target(false)
            .build();

        let search_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        search_box.add_css_class("terminal-search-bar");
        search_box.set_margin_top(4);
        search_box.set_margin_end(12);

        let entry = gtk4::SearchEntry::builder()
            .placeholder_text("Search terminal...")
            .width_request(250)
            .build();
        entry.add_css_class("terminal-search-entry");
        search_box.append(&entry);

        let match_label = gtk4::Label::builder()
            .label("")
            .css_classes(["caption", "dim-label"])
            .build();
        search_box.append(&match_label);

        let prev_btn = gtk4::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("Previous Match (Shift+Enter)")
            .css_classes(["flat", "circular"])
            .build();
        search_box.append(&prev_btn);

        let next_btn = gtk4::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text("Next Match (Enter)")
            .css_classes(["flat", "circular"])
            .build();
        search_box.append(&next_btn);

        let close_btn = gtk4::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close (Escape)")
            .css_classes(["flat", "circular"])
            .build();
        search_box.append(&close_btn);

        container.set_child(Some(&search_box));

        let terminal: Rc<RefCell<Option<vte4::Terminal>>> = Rc::new(RefCell::new(None));

        // Search on text change
        let term_ref = terminal.clone();
        let match_label_ref = match_label.clone();
        entry.connect_search_changed(move |entry| {
            let query = entry.text().to_string();
            if let Some(ref term) = *term_ref.borrow() {
                if query.is_empty() {
                    term.search_set_regex(None::<&vte4::Regex>, 0);
                    match_label_ref.set_label("");
                } else {
                    // Escape regex special chars for literal search
                    let escaped = regex_escape(&query);
                    match vte4::Regex::for_search(&escaped, 0) {
                        Ok(regex) => {
                            term.search_set_regex(Some(&regex), 0);
                            if term.search_find_previous() {
                                match_label_ref.set_label("Found");
                            } else {
                                match_label_ref.set_label("No matches");
                            }
                        }
                        Err(_) => {
                            match_label_ref.set_label("Invalid");
                        }
                    }
                }
            }
        });

        // Enter → next match
        let term_ref = terminal.clone();
        entry.connect_activate(move |_| {
            if let Some(ref term) = *term_ref.borrow() {
                term.search_find_next();
            }
        });

        // Next button
        let term_ref = terminal.clone();
        next_btn.connect_clicked(move |_| {
            if let Some(ref term) = *term_ref.borrow() {
                term.search_find_next();
            }
        });

        // Previous button
        let term_ref = terminal.clone();
        prev_btn.connect_clicked(move |_| {
            if let Some(ref term) = *term_ref.borrow() {
                term.search_find_previous();
            }
        });

        // Close button
        let container_ref = container.clone();
        close_btn.connect_clicked(move |_| {
            container_ref.set_reveal_child(false);
            container_ref.set_can_target(false);
        });

        // Escape key
        let key_controller = gtk4::EventControllerKey::new();
        let container_ref = container.clone();
        let term_ref = terminal.clone();
        key_controller.connect_key_pressed(move |_, keyval, _, state| {
            use gtk4::gdk::Key;
            match keyval {
                Key::Escape => {
                    container_ref.set_reveal_child(false);
                    container_ref.set_can_target(false);
                    // Clear search
                    if let Some(ref term) = *term_ref.borrow() {
                        term.search_set_regex(None::<&vte4::Regex>, 0);
                    }
                    gtk4::glib::Propagation::Stop
                }
                Key::Return if state.contains(gtk4::gdk::ModifierType::SHIFT_MASK) => {
                    if let Some(ref term) = *term_ref.borrow() {
                        term.search_find_previous();
                    }
                    gtk4::glib::Propagation::Stop
                }
                _ => gtk4::glib::Propagation::Proceed,
            }
        });
        entry.add_controller(key_controller);

        Self {
            container,
            entry,
            match_label,
            terminal,
        }
    }

    pub fn set_terminal(&self, terminal: &vte4::Terminal) {
        *self.terminal.borrow_mut() = Some(terminal.clone());
    }

    pub fn toggle(&self) {
        let visible = self.container.reveals_child();
        self.container.set_reveal_child(!visible);
        self.container.set_can_target(!visible);
        if !visible {
            self.entry.set_text("");
            self.entry.grab_focus();
        } else {
            // Clear search on hide
            if let Some(ref term) = *self.terminal.borrow() {
                term.search_set_regex(None::<&vte4::Regex>, 0);
            }
        }
    }

    pub fn show(&self) {
        self.container.set_reveal_child(true);
        self.container.set_can_target(true);
        self.entry.set_text("");
        self.entry.grab_focus();
    }

    pub fn is_visible(&self) -> bool {
        self.container.reveals_child()
    }

    pub fn widget(&self) -> &gtk4::Revealer {
        &self.container
    }
}

fn regex_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}
