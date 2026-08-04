pub mod accent;
pub mod add_command_dialog;
pub mod add_remote_project_dialog;
pub mod add_ssh_dialog;
pub mod command_palette;
pub mod edit_project_dialog;
pub mod git_changes_dialog;
pub mod project_detail;
pub mod select_commands_dialog;
pub mod settings;
pub mod sidebar;
pub mod status_bar;
pub mod terminal_search;
pub mod terminal_theme;
pub mod terminal_view;
pub mod window;

use gtk4::prelude::*;

/// Stop double-clicks inside an `adw::Dialog` from toggling the MAIN
/// window's maximize state.
///
/// libadwaita (verified in 1.5) keeps `AdwHeaderBar`'s internal
/// `GtkWindowHandle` active even when the header bar sits inside a dialog,
/// and the floating-sheet dimming layer is a `GtkWindowHandle` too. A
/// dialog isn't a real window, so the handle's double-click acts on the
/// top-level `TuxFlow` window instead. Claim double-clicks headed for a
/// `WindowHandle` in the capture phase, before the handle's own gesture
/// sees them; single-click drags (move window) keep working, and
/// double-clicks on regular content (text selection etc.) are untouched.
pub fn guard_dialog_maximize(dialog: &libadwaita::Dialog) {
    let guard = gtk4::GestureClick::new();
    guard.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let dialog_ref = dialog.clone();
    guard.connect_pressed(move |gesture, n_press, x, y| {
        if n_press < 2 {
            return;
        }
        let hits_window_handle = dialog_ref
            .pick(x, y, gtk4::PickFlags::DEFAULT)
            .and_then(|target| target.ancestor(gtk4::WindowHandle::static_type()))
            .is_some();
        if hits_window_handle {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
        }
    });
    dialog.add_controller(guard);
}
