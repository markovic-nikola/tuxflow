mod app;
mod config;
mod process;
mod ui;
mod util;
mod watcher;
mod workspace;

// Extracted to tuxflow-core (migration M0); the re-export keeps all
// `crate::detect`/`crate::mcp`/`crate::remote` paths working.
pub use tuxflow_core::{detect, mcp, remote};

fn main() {
    env_logger::init();

    let app = app::TuxFlowApp::new();
    app.run();
}
