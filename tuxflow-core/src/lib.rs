//! TuxFlow's GUI-independent heart, extracted in migration step M0.
//!
//! Everything here must stay free of GTK/iced/VTE types: this crate is
//! shared by the app (`tuxflow`, the iced shell) and the retired GTK shell (`tuxflow-gtk`),
//! and its tests run without a display server.

pub mod config;
pub mod detect;
pub mod mcp;
pub mod remote;
pub mod util;
