//! Pictures drawn among other content, as the agent's transcript shows them:
//! the terminal's graphics detected once, the cells a picture takes, and the
//! encoded pictures kept across frames.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::OnceLock;

use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

/// Encoded pictures kept at once; the one drawn longest ago goes first.
const KEPT: usize = 8;

/// The terminal's graphics: its protocol and the size of a cell in pixels.
#[derive(Clone)]
pub struct Graphics {
    picker: Picker,
}

impl Graphics {
    /// Ask the terminal, once per process. The query writes to the terminal
    /// and reads its answer, so call it between frames, never while one is
    /// drawn. `None` when the terminal cannot be asked at all; a terminal
    /// with no graphics protocol gets half-block pictures.
    ///
    /// Once is a must, not a saving: `ratatui-image` reads the answer on a
    /// thread of its own, and when the answer is late the call gives up but
    /// the thread reads stdin on, taking keys and halves of escape sequences
    /// from termide for the rest of its life. Every image path goes through
    /// here rather than querying itself.
    #[must_use]
    pub fn detect() -> Option<Self> {
        static DETECTED: OnceLock<Option<Picker>> = OnceLock::new();
        DETECTED
            .get_or_init(|| Picker::from_query_stdio().ok())
            .clone()
            .map(|picker| Self { picker })
    }

    /// The picker, for a widget that draws through `ratatui-image` itself.
    #[must_use]
    pub fn into_picker(self) -> Picker {
        self.picker
    }

    /// Half-block pictures, without asking the terminal.
    #[must_use]
    pub fn halfblocks() -> Self {
        Self {
            picker: Picker::halfblocks(),
        }
    }

    /// The size of a cell in pixels.
    #[must_use]
    pub fn cell_px(&self) -> (u16, u16) {
        self.picker.font_size()
    }

    /// Whether a picture moves on screen as cheaply as text does. Kitty
    /// sends a picture once and places it with characters in its cells; half
    /// blocks are characters. Sixel and iTerm2 send the whole picture again
    /// wherever it moves.
    #[must_use]
    pub fn moves_cheaply(&self) -> bool {
        matches!(
            self.picker.protocol_type(),
            ProtocolType::Kitty | ProtocolType::Halfblocks
        )
    }

    /// Sixel graphics, without asking the terminal, for tests.
    #[must_use]
    pub fn sixel() -> Self {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Sixel);
        Self { picker }
    }
}

fn decode_bytes(data: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

fn decode(data: &str) -> Option<DynamicImage> {
    let bytes = decode_bytes(data)?;
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}

/// The size in pixels of the picture in base64 `data`, read from its header.
#[must_use]
pub fn picture_size(data: &str) -> Option<(u32, u32)> {
    let bytes = decode_bytes(data)?;
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// A key for the picture in base64 `data`, the same for the same picture.
#[must_use]
pub fn picture_key(data: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

/// The `(columns, rows)` a picture of `px` pixels takes, drawn no larger
/// than itself and within `max` cells, its proportions kept, on cells of
/// `cell` pixels.
#[must_use]
pub fn fit_cells(px: (u32, u32), cell: (u16, u16), max: (u16, u16)) -> (u16, u16) {
    let (cw, ch) = (f64::from(cell.0.max(1)), f64::from(cell.1.max(1)));
    let (w, h) = (f64::from(px.0.max(1)), f64::from(px.1.max(1)));
    let (max_cols, max_rows) = (max.0.max(1), max.1.max(1));
    let scale = 1f64
        .min(f64::from(max_cols) * cw / w)
        .min(f64::from(max_rows) * ch / h);
    // Whole cells: the rounding never adds more than a cell, and the bound
    // holds.
    let cols = ((w * scale / cw).ceil() as u16).clamp(1, max_cols);
    let rows = ((h * scale / ch).ceil() as u16).clamp(1, max_rows);
    (cols, rows)
}

struct Entry {
    /// `None` when the data does not decode: nothing is drawn for it.
    protocol: Option<StatefulProtocol>,
    /// The frame it was last drawn in.
    drawn: u64,
}

/// Pictures encoded for the terminal, by [`picture_key`], so a picture that
/// stays on screen is decoded and encoded once rather than every frame.
#[derive(Default)]
pub struct InlinePictures {
    entries: HashMap<u64, Entry>,
    frame: u64,
}

impl InlinePictures {
    /// Start a frame: what is drawn from now on counts as drawn in it.
    pub fn next_frame(&mut self) {
        self.frame += 1;
    }

    /// Draw the picture in base64 `data`, keyed `key`, fitted into `area`.
    pub fn draw(
        &mut self,
        graphics: &Graphics,
        key: u64,
        data: &str,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if !self.entries.contains_key(&key) && self.entries.len() >= KEPT {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.drawn)
                .map(|(key, _)| *key);
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        let entry = self.entries.entry(key).or_insert_with(|| Entry {
            protocol: decode(data).map(|image| graphics.picker.new_resize_protocol(image)),
            drawn: 0,
        });
        entry.drawn = self.frame;
        if let Some(protocol) = entry.protocol.as_mut() {
            let widget = StatefulImage::default().resize(Resize::Fit(Some(FilterType::Triangle)));
            widget.render(area, buf, protocol);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`×`h` PNG as base64.
    fn png(w: u32, h: u32) -> String {
        let image =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 30])));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn the_size_comes_from_the_header() {
        assert_eq!(picture_size(&png(30, 20)), Some((30, 20)));
        assert_eq!(picture_size("not base64!"), None);
    }

    #[test]
    fn a_picture_fits_its_bound_and_keeps_its_proportions() {
        // 10×20 px cells: 1000×1000 px into 40×10 cells is height-bound.
        assert_eq!(fit_cells((1000, 1000), (10, 20), (40, 10)), (20, 10));
        // Width-bound.
        assert_eq!(fit_cells((1000, 100), (10, 20), (40, 10)), (40, 2));
        // Never larger than itself.
        assert_eq!(fit_cells((50, 40), (10, 20), (40, 10)), (5, 2));
    }

    #[test]
    fn drawing_fills_the_area_and_keeps_few_pictures() {
        let graphics = Graphics::halfblocks();
        let mut pictures = InlinePictures::default();
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        for n in 0..=KEPT as u32 {
            pictures.next_frame();
            let data = png(8 + n, 8);
            pictures.draw(&graphics, picture_key(&data), &data, area, &mut buf);
        }
        assert_eq!(pictures.entries.len(), KEPT);
        // Half blocks paint the cells in the picture's colour.
        assert!(buf
            .content
            .iter()
            .any(|cell| cell.fg != ratatui::style::Color::Reset
                || cell.bg != ratatui::style::Color::Reset));
    }
}
