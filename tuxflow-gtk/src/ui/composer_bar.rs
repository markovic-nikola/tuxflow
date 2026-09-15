use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;

type SendCallback = Rc<RefCell<Option<Box<dyn Fn(String, Vec<Attachment>)>>>>;
type ImagePasteCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;
/// (pending attachment, its chip widget in the chips row)
type Attachments = Rc<RefCell<Vec<(Attachment, gtk4::Box)>>>;

/// A pending image attachment: where the file lives on the machine the
/// agent runs on, plus the pasted texture (thumbnail + local clipboard
/// staging at send time).
#[derive(Clone)]
pub struct Attachment {
    pub path: String,
    pub texture: Option<gtk4::gdk::Texture>,
}

/// Local message composer shown under agent terminals. Typing happens in a
/// local widget — no per-keystroke ssh round trip on remote projects — and
/// the whole message is delivered to the PTY in one write on send.
/// Enter sends, Shift+Enter inserts a newline.
///
/// Pasting an image fires `on_image_paste`; the window-side bridge saves or
/// uploads it and calls `add_attachment`. Chips show pending attachments;
/// on send they're handed to the send callback, which delivers each one
/// natively (clipboard staging + Ctrl+V) before the text.
pub struct ComposerBar {
    container: gtk4::Box,
    chips_row: gtk4::Box,
    text_view: gtk4::TextView,
    attachments: Attachments,
    /// Selection key (project::process) the attachments belong to — paths
    /// are machine-specific, so switching terminals clears them.
    context: RefCell<String>,
    on_send: SendCallback,
    on_image_paste: ImagePasteCallback,
    /// Display-level provider carrying the input font size, kept in sync
    /// with the terminal font-size setting (points, like VTE).
    font_provider: gtk4::CssProvider,
    /// Wraps the text view; its max height caps the auto-grow.
    scroll: gtk4::ScrolledWindow,
}

impl ComposerBar {
    pub fn new() -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.add_css_class("composer-bar");
        container.set_visible(false);

        // Pending image attachments (hidden until one is added)
        let chips_row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .visible(false)
            .css_classes(["composer-chips"])
            .build();
        container.append(&chips_row);

        let input_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        container.append(&input_row);

        let text_view = gtk4::TextView::builder()
            .wrap_mode(gtk4::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(10)
            .right_margin(10)
            .css_classes(["composer-input"])
            .build();

        // Grows with content, then scrolls. The max height is set from the
        // font size in apply_terminal_style (MAX_GROW_LINES).
        let scroll = gtk4::ScrolledWindow::builder()
            .child(&text_view)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(110)
            .hexpand(true)
            .css_classes(["composer-field"])
            .build();
        input_row.append(&scroll);

        // Default fill valign: the button stretches to the field's height
        // (growing with it on multi-line input) with the icon centered.
        let send_btn = gtk4::Button::builder()
            .icon_name("mail-send-symbolic")
            .tooltip_text("Send to agent (Enter — Shift+Enter for newline)")
            .css_classes(["flat", "status-chip"])
            .build();
        input_row.append(&send_btn);

        let on_send: SendCallback = Rc::new(RefCell::new(None));
        let on_image_paste: ImagePasteCallback = Rc::new(RefCell::new(None));
        let attachments: Attachments = Rc::new(RefCell::new(Vec::new()));

        let do_send = {
            let buffer = text_view.buffer();
            let on_send = on_send.clone();
            let attachments = attachments.clone();
            let chips_row = chips_row.clone();
            Rc::new(move || {
                let (start, end) = buffer.bounds();
                let text = buffer.text(&start, &end, false);
                let text = text.trim_end();
                let pending: Vec<Attachment> = attachments
                    .borrow()
                    .iter()
                    .map(|(att, _)| att.clone())
                    .collect();
                if text.is_empty() && pending.is_empty() {
                    return;
                }
                if let Some(ref cb) = *on_send.borrow() {
                    cb(text.to_string(), pending);
                }
                buffer.set_text("");
                for (_, chip) in attachments.borrow_mut().drain(..) {
                    chips_row.remove(&chip);
                }
                chips_row.set_visible(false);
            })
        };

        let send_ref = do_send.clone();
        send_btn.connect_clicked(move |_| send_ref());

        // Enter sends; Shift+Enter falls through to the TextView's default
        // handler and inserts a newline. Ctrl(+Shift)+V with an image on
        // the clipboard routes to the image-paste bridge; text pastes fall
        // through to the TextView.
        let key = gtk4::EventControllerKey::new();
        let send_ref = do_send.clone();
        let image_cb = on_image_paste.clone();
        let tv_for_key = text_view.clone();
        key.connect_key_pressed(move |_, keyval, _, state| {
            let is_enter = matches!(
                keyval,
                gtk4::gdk::Key::Return | gtk4::gdk::Key::KP_Enter | gtk4::gdk::Key::ISO_Enter
            );
            if is_enter && !state.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
                send_ref();
                return gtk4::glib::Propagation::Stop;
            }
            let is_paste = matches!(keyval, gtk4::gdk::Key::v | gtk4::gdk::Key::V)
                && state.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
            if is_paste
                && tv_for_key
                    .clipboard()
                    .formats()
                    .contains_type(gtk4::gdk::Texture::static_type())
            {
                if let Some(ref cb) = *image_cb.borrow() {
                    cb();
                }
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        });
        text_view.add_controller(key);

        let font_provider = gtk4::CssProvider::new();
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("No display"),
            &font_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );

        Self {
            container,
            chips_row,
            text_view,
            attachments,
            context: RefCell::new(String::new()),
            on_send,
            on_image_paste,
            font_provider,
            scroll,
        }
    }

    /// Match the input's font size to the terminal font-size setting
    /// (points, as VTE interprets it) and the bar background to the
    /// terminal theme, so the composer blends into the terminal area.
    /// The auto-grow cap scales with the font so the line budget stays
    /// constant across font sizes.
    pub fn apply_terminal_style(&self, font_pt: u32, background: &str) {
        /// Lines the input grows to before it starts scrolling.
        const MAX_GROW_LINES: f64 = 8.0;
        self.font_provider.load_from_string(&format!(
            ".composer-input, .composer-input text {{ font-size: {font_pt}pt; }} \
             .composer-bar {{ background: {background}; }}"
        ));
        // pt → px at CSS 96dpi, ~1.5× for line height, plus the view's
        // vertical margins.
        let line_px = font_pt as f64 * (96.0 / 72.0) * 1.5;
        self.scroll
            .set_max_content_height((line_px * MAX_GROW_LINES) as i32 + 12);
    }

    /// Add a pending attachment chip: thumbnail and a remove button.
    /// `path` is where the file lives on the machine the agent runs on.
    pub fn add_attachment(&self, path: &str, texture: Option<&gtk4::gdk::Texture>) {
        let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        chip.add_css_class("composer-chip");
        chip.set_tooltip_text(Some(path));

        if let Some(texture) = texture {
            // A bare GtkPicture requests the image's NATURAL size (a wide
            // screenshot would stretch the whole chip — size_request only
            // raises the minimum). Cap the allocation with an Overlay: the
            // fixed-size base child decides the size, the picture is an
            // unmeasured overlay clipped to it (same trick as the keybind
            // hint overlay in process_row.rs).
            let frame = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            frame.set_size_request(36, 26);
            let thumb = gtk4::Picture::for_paintable(texture);
            thumb.set_content_fit(gtk4::ContentFit::Cover);
            let overlay = gtk4::Overlay::new();
            overlay.set_child(Some(&frame));
            overlay.add_overlay(&thumb);
            overlay.set_measure_overlay(&thumb, false);
            overlay.set_overflow(gtk4::Overflow::Hidden);
            overlay.add_css_class("composer-chip-thumb");
            chip.append(&overlay);
        } else {
            let icon = gtk4::Image::from_icon_name("image-x-generic-symbolic");
            icon.add_css_class("dim-label");
            chip.append(&icon);
        }

        let close = gtk4::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove attachment")
            .css_classes(["flat", "composer-chip-close"])
            .build();
        chip.append(&close);

        self.chips_row.append(&chip);
        self.chips_row.set_visible(true);
        self.attachments.borrow_mut().push((
            Attachment {
                path: path.to_string(),
                texture: texture.cloned(),
            },
            chip.clone(),
        ));

        let attachments = self.attachments.clone();
        let chips_row = self.chips_row.clone();
        close.connect_clicked(move |_| {
            attachments.borrow_mut().retain(|(_, c)| c != &chip);
            chips_row.remove(&chip);
            if attachments.borrow().is_empty() {
                chips_row.set_visible(false);
            }
        });
    }

    /// Attachments are bound to one terminal's machine — switching to a
    /// different process clears them (the draft text survives).
    pub fn set_context(&self, key: &str) {
        if *self.context.borrow() == key {
            return;
        }
        *self.context.borrow_mut() = key.to_string();
        for (_, chip) in self.attachments.borrow_mut().drain(..) {
            self.chips_row.remove(&chip);
        }
        self.chips_row.set_visible(false);
    }

    /// Route a paste to this composer: images to the attachment bridge,
    /// text to the TextView's normal paste. Called by the global paste
    /// shortcut when the composer has focus (the window-level capture
    /// controller would otherwise route the paste to the terminal).
    pub fn paste(&self) {
        if self
            .text_view
            .clipboard()
            .formats()
            .contains_type(gtk4::gdk::Texture::static_type())
        {
            if let Some(ref cb) = *self.on_image_paste.borrow() {
                cb();
            }
        } else {
            self.text_view.emit_by_name::<()>("paste-clipboard", &[]);
        }
    }

    pub fn input_has_focus(&self) -> bool {
        self.text_view.has_focus()
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }

    pub fn set_on_send(&self, cb: impl Fn(String, Vec<Attachment>) + 'static) {
        *self.on_send.borrow_mut() = Some(Box::new(cb));
    }

    pub fn set_on_image_paste(&self, cb: impl Fn() + 'static) {
        *self.on_image_paste.borrow_mut() = Some(Box::new(cb));
    }

    pub fn set_visible(&self, visible: bool) {
        self.container.set_visible(visible);
    }

    pub fn focus(&self) {
        self.text_view.grab_focus();
    }
}
