//! TuxFlow's GUI-independent heart, extracted in migration step M0.
//!
//! Everything here must stay free of GTK/iced/VTE types: this crate is
//! the app's (`tuxflow`, the iced shell) GUI-free half, and its tests run
//! without a display server.

pub mod config;
pub mod detect;
pub mod mcp;
pub mod remote;
pub mod util;
