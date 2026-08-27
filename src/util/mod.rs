pub mod icon_detector;
pub mod notifications;
pub mod update_checker;
pub mod worker;
// Moved to tuxflow-core (migration M0; editor followed for the iced context menus).
pub use tuxflow_core::util::{editor, port_detector};
