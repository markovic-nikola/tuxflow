// Keybindings stay GTK-side (gdk key/modifier types); the rest moved to
// tuxflow-core in migration M0 — re-exported so `crate::config::` paths
// keep working.
pub mod keybindings;
pub use tuxflow_core::config::{loader, projects, schema, settings, ssh};
