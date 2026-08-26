//! TuxFlow's GUI-independent heart, extracted in migration step M0.
//!
//! Everything here must stay free of GTK/iced/VTE types: this crate is
//! shared by the GTK app (`tuxflow`) and the iced app (`tuxflow-iced`),
//! and its tests run without a display server.

pub mod config;
pub mod detect;
pub mod mcp;
pub mod remote;
pub mod util;
