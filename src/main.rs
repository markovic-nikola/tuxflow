mod app;
mod config;
mod detect;
mod mcp;
mod process;
mod remote;
mod ui;
mod util;
mod watcher;
mod workspace;

fn main() {
    env_logger::init();

    let app = app::TuxFlowApp::new();
    app.run();
}
