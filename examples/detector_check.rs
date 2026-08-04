use tuxflow::util::port_detector::PortDetector;
fn main() {
    let text = std::fs::read_to_string(std::env::args().nth(1).expect("path")).unwrap();
    let mut pd = PortDetector::new();
    pd.scan_output("dev", &text);
    println!("badge port: {:?}", pd.get_port("dev"));
    println!("url:        {:?}", pd.get_url("dev"));
    println!("all local:  {:?}", pd.all_local_ports("dev"));
}
