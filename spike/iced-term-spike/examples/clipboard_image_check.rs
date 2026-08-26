//! Sanity check for the arboard image path behind the demo's Ctrl+Shift+V
//! fallback: set an image on the clipboard, read it back through a second
//! connection (what the demo opens on paste), byte-compare. Needs a display;
//! `xvfb-run -a cargo run --example clipboard_image_check` works headlessly.

fn main() {
    #[rustfmt::skip]
    let rgba: Vec<u8> = vec![
        255, 0, 0, 255,   0, 255, 0, 255,
        0, 0, 255, 255,   255, 255, 255, 128,
    ];
    let mut setter = arboard::Clipboard::new().expect("open clipboard (set)");
    setter
        .set_image(arboard::ImageData {
            width: 2,
            height: 2,
            bytes: rgba.clone().into(),
        })
        .expect("set image");
    // X11: the clipboard lives in the owning connection — `setter` must
    // stay alive while the reader asks for the data.
    let img = arboard::Clipboard::new()
        .expect("open clipboard (get)")
        .get_image()
        .expect("get image");
    assert_eq!((img.width, img.height), (2, 2));
    assert_eq!(img.bytes.as_ref(), rgba.as_slice());
    println!("arboard image roundtrip OK ({}x{})", img.width, img.height);
}
