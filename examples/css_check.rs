fn main() {
    gtk4::init().expect("gtk init");
    let provider = gtk4::CssProvider::new();
    provider.connect_parsing_error(|_, section, error| {
        eprintln!("CSS PARSE ERROR at {section}: {error}");
    });
    provider.load_from_string(include_str!("../data/style.css"));
    eprintln!("css load done");
}
