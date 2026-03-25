use std::cell::RefCell;

use gtk4::gdk;

struct AccentColor {
    name: &'static str,
    label: &'static str,
    bg: &'static str,
    fg: &'static str,
    accent: &'static str,
}

const ACCENT_COLORS: &[AccentColor] = &[
    AccentColor {
        name: "green",
        label: "Green (Default)",
        bg: "",
        fg: "",
        accent: "",
    },
    AccentColor {
        name: "blue",
        label: "Blue",
        bg: "#3584e4",
        fg: "#ffffff",
        accent: "#3584e4",
    },
    AccentColor {
        name: "purple",
        label: "Purple",
        bg: "#9141ac",
        fg: "#ffffff",
        accent: "#9141ac",
    },
    AccentColor {
        name: "teal",
        label: "Teal",
        bg: "#2190a4",
        fg: "#ffffff",
        accent: "#2190a4",
    },
    AccentColor {
        name: "orange",
        label: "Orange",
        bg: "#e66100",
        fg: "#ffffff",
        accent: "#e66100",
    },
    AccentColor {
        name: "red",
        label: "Red",
        bg: "#e01b24",
        fg: "#ffffff",
        accent: "#e01b24",
    },
    AccentColor {
        name: "pink",
        label: "Pink",
        bg: "#d56199",
        fg: "#ffffff",
        accent: "#d56199",
    },
    AccentColor {
        name: "yellow",
        label: "Yellow",
        bg: "#c88800",
        fg: "#ffffff",
        accent: "#c88800",
    },
    AccentColor {
        name: "slate",
        label: "Slate",
        bg: "#6e8898",
        fg: "#ffffff",
        accent: "#6e8898",
    },
];

thread_local! {
    static PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
}

pub fn apply(name: &str) {
    let css = match ACCENT_COLORS.iter().find(|c| c.name == name) {
        Some(c) if !c.bg.is_empty() => format!(
            "@define-color accent_bg_color {};\n\
             @define-color accent_fg_color {};\n\
             @define-color accent_color {};",
            c.bg, c.fg, c.accent,
        ),
        _ => String::new(),
    };

    PROVIDER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let provider = slot.get_or_insert_with(|| {
            let p = gtk4::CssProvider::new();
            gtk4::style_context_add_provider_for_display(
                &gdk::Display::default().expect("No display"),
                &p,
                800, // STYLE_PROVIDER_PRIORITY_USER, above APPLICATION (600)
            );
            p
        });
        provider.load_from_string(&css);
    });
}

pub fn color_choices() -> Vec<&'static str> {
    ACCENT_COLORS.iter().map(|c| c.label).collect()
}

pub fn color_index(name: &str) -> u32 {
    ACCENT_COLORS
        .iter()
        .position(|c| c.name == name)
        .unwrap_or(0) as u32
}

pub fn color_name(index: u32) -> &'static str {
    ACCENT_COLORS
        .get(index as usize)
        .map(|c| c.name)
        .unwrap_or("green")
}
