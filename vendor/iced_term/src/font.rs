use crate::settings::FontSettings;
use iced::{Font, Size};
use iced_core::{
    alignment::Vertical,
    text::{Alignment, LineHeight, Paragraph, Shaping as TextShaping},
    Text,
};
use iced_graphics::text::paragraph;

#[derive(Debug, Clone)]
pub struct TermFont {
    pub(crate) size: f32,
    pub(crate) font_type: Font,
    pub(crate) scale_factor: f32,
    pub(crate) bold_weight: iced::font::Weight,
    pub(crate) letter_spacing: f32,
    /// Cell size: the glyph advance plus `letter_spacing`, by line height.
    pub(crate) measure: Size<f32>,
}

impl TermFont {
    pub fn new(settings: FontSettings) -> Self {
        Self {
            size: settings.size,
            font_type: settings.font_type,
            scale_factor: settings.scale_factor,
            bold_weight: settings.bold_weight,
            letter_spacing: settings.letter_spacing,
            measure: font_measure(
                settings.size,
                settings.scale_factor,
                settings.font_type,
                settings.letter_spacing,
            ),
        }
    }

    pub fn sync(&mut self) {
        self.measure = font_measure(
            self.size,
            self.scale_factor,
            self.font_type,
            self.letter_spacing,
        )
    }
}

fn font_measure(
    font_size: f32,
    scale_factor: f32,
    font_type: Font,
    letter_spacing: f32,
) -> Size<f32> {
    let paragraph = paragraph::Paragraph::with_text(Text {
        content: "m",
        font: font_type,
        size: iced_core::Pixels(font_size),
        align_y: Vertical::Center,
        align_x: Alignment::Center,
        shaping: TextShaping::Advanced,
        line_height: LineHeight::Relative(scale_factor),
        bounds: Size::INFINITE,
        wrapping: iced_core::text::Wrapping::Glyph,
    });

    let glyph = paragraph.min_bounds();
    // Never let spacing collapse a cell below the glyph itself.
    Size::new(
        glyph.width + letter_spacing.max(-glyph.width * 0.5),
        glyph.height,
    )
}
