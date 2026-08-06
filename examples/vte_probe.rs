// Probe VTE's text_range_format behavior for hard-wrapped rows.
// Run headless: xvfb-run -a cargo run --example vte_probe
use vte4::prelude::*;

fn main() {
    gtk4::init().expect("gtk init");
    let term = vte4::Terminal::new();
    term.set_size(84, 20);

    // Simulate tmux/Ink output: an 83-char row (width-1) with a hard
    // newline, then its continuation, then a shorter line, then blanks.
    let line1 = format!(
        " Preview URL: https://admin.shopify.com/store/nik-chris/apps/{}",
        "4a74c956c6cc86a667104e"
    );
    assert_eq!(line1.chars().count(), 83);
    // Continuation and following panel rows carry Ink's 1-space box padding.
    let feed = format!(
        "{line1}\r\n 5f76c8afda?dev-console=show\r\n GraphiQL URL: http://localhost:3457/g\r\n"
    );
    term.feed(feed.as_bytes());

    // Let VTE process the feed.
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..200 {
        while ctx.iteration(false) {}
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let cols = term.column_count();
    let (row, col) = {
        let p = term.cursor_position();
        (p.1, p.0)
    };
    println!("cols={cols} cursor_row={row} cursor_col={col}");

    let (text, _) = term.text_range_format(vte4::Format::Text, 0, 0, 10, cols);
    match text {
        Some(t) => {
            for (i, l) in t.lines().enumerate() {
                println!("line {i}: {} chars: {l:?}", l.chars().count());
            }
            let mut pd = tuxflow::util::port_detector::PortDetector::new();
            pd.scan_output_wrapped("probe", &t, cols as usize);
            println!("detected url: {:?}", pd.get_url("probe"));
        }
        None => println!("NO TEXT"),
    }
}
