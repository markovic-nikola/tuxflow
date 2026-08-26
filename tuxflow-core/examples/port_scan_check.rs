//! Run the real port detector over captured terminal text.
//!
//! Terminal output is the detector's only input, and full-screen TUIs make it
//! surprising — this replays a `tmux capture-pane` dump through the same path
//! a remote scan takes, so what the badge *would* be is observable without a
//! GTK window.
//!
//! ```sh
//! ssh host 'tmux -L tuxflow capture-pane -pJ -t tf-dev-x -S -60' > pane.txt
//! cargo run --example port_scan_check -- pane.txt 84
//! ```

use tuxflow_core::util::port_detector::PortDetector;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: port_scan_check <file> [cols]");
    let cols: usize = args
        .next()
        .map(|c| c.parse().expect("cols must be a number"))
        .unwrap_or(usize::MAX);

    let text = std::fs::read_to_string(&path).expect("read capture");

    let mut pd = PortDetector::new();
    pd.scan_output_wrapped("dev", &text, cols);

    println!("cols          : {cols}");
    println!("rows          : {}", text.lines().count());
    println!("badge port    : {:?}", pd.get_port("dev"));
    println!("badge url     : {:?}", pd.get_url("dev"));
    println!("badge final   : {}", pd.badge_final("dev"));
    println!("tunnelled     : {:?}", pd.all_local_ports("dev"));
}
