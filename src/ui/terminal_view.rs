use gtk4::glib;
use gtk4::prelude::*;
use vte4::prelude::*;

pub struct TerminalView {
    container: gtk4::Box,
    terminal: vte4::Terminal,
}

impl TerminalView {
    pub fn new() -> Self {
        let terminal = vte4::Terminal::new();

        // Terminal appearance
        terminal.set_scroll_on_output(true);
        terminal.set_scroll_on_keystroke(true);
        terminal.set_scrollback_lines(10000);
        terminal.set_vexpand(true);
        terminal.set_hexpand(true);

        // Set font
        let font_desc = gtk4::pango::FontDescription::from_string("Monospace 12");
        terminal.set_font(Some(&font_desc));

        // Spawn user's shell
        Self::spawn_shell(&terminal);

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.append(&terminal);

        Self {
            container,
            terminal,
        }
    }

    fn spawn_shell(terminal: &vte4::Terminal) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

        terminal.spawn_async(
            vte4::PtyFlags::DEFAULT,
            None,                         // working directory (inherit)
            &[&shell],                    // argv
            &[],                          // envv (inherit)
            glib::SpawnFlags::DEFAULT,    // spawn flags
            || {},                        // child_setup
            -1,                           // timeout (-1 = default)
            gtk4::gio::Cancellable::NONE, // cancellable
            |result| match result {
                Ok(_pid) => log::info!("Shell spawned"),
                Err(e) => log::error!("Failed to spawn shell: {e}"),
            },
        );
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }

    pub fn terminal(&self) -> &vte4::Terminal {
        &self.terminal
    }
}
