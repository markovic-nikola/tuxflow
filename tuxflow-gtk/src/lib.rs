// The portable modules live in tuxflow-core since M0 of the iced
// migration; re-exports keep every existing `tuxflow::`/`crate::` path
// working. `config` and `util` stay as local shim modules because each
// keeps a GTK-side member (keybindings; worker/notifications/...).
pub use tuxflow_core::{detect, mcp, remote};

pub mod config;
pub mod util;
