//! The GTK app's face of core's `util/update.rs`: the same checker, install
//! and relaunch, with this package's version filled in. The iced shell
//! consumes the core module directly.

pub use tuxflow_core::util::update::{
    InstallKind, UpdateInfo, binary_replaced, install_deb, install_kind, restart,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// See core's `check_for_update`.
pub fn check_for_update() -> Option<UpdateInfo> {
    tuxflow_core::util::update::check_for_update(VERSION)
}

/// See core's `download_deb`.
pub fn download_deb(url: &str) -> Result<std::path::PathBuf, String> {
    tuxflow_core::util::update::download_deb(VERSION, url)
}
