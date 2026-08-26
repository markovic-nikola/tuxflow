//! TuxFlow's look on iced: the GTK app's default terminal scheme
//! (Catppuccin Mocha, from ui/terminal_theme.rs) and the sidebar accent
//! hues — green for local projects, logo gold for remote (ui/accent.rs:
//! the color says where a project lives).

use iced::Color;

pub const LOCAL_ACCENT: Color = Color::from_rgb(0.42, 0.80, 0.46);
/// #ffce5c — the remote/logo gold.
pub const REMOTE_ACCENT: Color = Color::from_rgb(1.0, 0.81, 0.36);
pub const DIM: Color = Color::from_rgb(0.55, 0.55, 0.58);
pub const RUNNING: Color = Color::from_rgb(0.30, 0.78, 0.40);
pub const CRASHED: Color = Color::from_rgb(0.87, 0.32, 0.32);
pub const WORKING: Color = Color::from_rgb(0.92, 0.72, 0.25);
pub const SIDEBAR_BG: Color = Color::from_rgb(0.09, 0.09, 0.11);

/// Catppuccin Mocha — the GTK default, field for field.
pub fn terminal_palette() -> iced_term::ColorPalette {
    iced_term::ColorPalette {
        foreground: String::from("#CDD6F4"),
        background: String::from("#1E1E2E"),
        black: String::from("#45475A"),
        red: String::from("#F38BA8"),
        green: String::from("#A6E3A1"),
        yellow: String::from("#F9E2AF"),
        blue: String::from("#89B4FA"),
        magenta: String::from("#F5C2E7"),
        cyan: String::from("#94E2D5"),
        white: String::from("#BAC2DE"),
        bright_black: String::from("#585B70"),
        bright_red: String::from("#F38BA8"),
        bright_green: String::from("#A6E3A1"),
        bright_yellow: String::from("#F9E2AF"),
        bright_blue: String::from("#89B4FA"),
        bright_magenta: String::from("#F5C2E7"),
        bright_cyan: String::from("#94E2D5"),
        bright_white: String::from("#A6ADC8"),
        ..Default::default()
    }
}
