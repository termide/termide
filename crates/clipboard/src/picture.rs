//! Pictures for a multimodal model: a bitmap on the clipboard, or an image
//! file a pasted path or a copied file names, made ready to attach to a
//! message — scaled down to what a model reads and encoded in base64.

use std::io::Cursor;
use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine};
use image::{DynamicImage, ImageFormat, RgbaImage};

/// The longest side a picture is sent at. Models scale larger pictures down
/// themselves, so more only spends tokens and upload; this is the size
/// Anthropic recommends and well within what OpenAI-compatible models read.
pub const MAX_SIDE: u32 = 1568;

/// The largest encoded picture sent as PNG: past this, a photo goes as JPEG,
/// keeping under the 5 MB per-image limit providers set.
const MAX_PNG_BYTES: usize = 3_500_000;

/// The file extensions read as pictures, with the MIME type each is sent as.
const KINDS: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

/// A picture ready to attach to a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// `image/png`, `image/jpeg`, …
    pub media_type: String,
    /// The encoded file in base64.
    pub data: String,
    /// The size it is sent at, in pixels.
    pub width: u32,
    pub height: u32,
}

/// The MIME type of the picture file `path` names, by its extension, or
/// `None` for a file that is not one of the kinds a model reads.
#[must_use]
pub fn picture_kind(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    KINDS
        .iter()
        .find(|(known, _)| *known == ext)
        .map(|(_, mime)| *mime)
}

/// The picture file at `path`, or `None` when it is not a picture or cannot
/// be read. A file already small enough goes as it is; a larger one is
/// scaled down and encoded again.
#[must_use]
pub fn picture_from_path(path: &Path) -> Option<Picture> {
    let mime = picture_kind(path)?;
    let bytes = std::fs::read(path).ok()?;
    let format = ImageFormat::from_mime_type(mime)?;
    let decoded = image::load_from_memory_with_format(&bytes, format).ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    // GIF and WebP are sent as they are only when small: an animated GIF
    // goes as its first frame once it has to be encoded again anyway.
    if width.max(height) <= MAX_SIDE && bytes.len() <= MAX_PNG_BYTES {
        return Some(Picture {
            media_type: mime.to_string(),
            data: STANDARD.encode(&bytes),
            width,
            height,
        });
    }
    encode(decoded, mime == "image/jpeg")
}

/// A picture already encoded — `data` in base64, of `media_type` — read back,
/// as a message taken out of the queue carries it: `None` when it does not
/// decode.
#[must_use]
pub fn picture_from_data(media_type: &str, data: &str) -> Option<Picture> {
    let bytes = STANDARD.decode(data).ok()?;
    let format = ImageFormat::from_mime_type(media_type)?;
    let (width, height) = image::ImageReader::with_format(Cursor::new(&bytes), format)
        .into_dimensions()
        .ok()?;
    Some(Picture {
        media_type: media_type.to_string(),
        data: data.to_string(),
        width,
        height,
    })
}

/// The bitmap on the system clipboard — a screenshot, an image copied in a
/// browser or an editor — or `None` when it holds none.
#[must_use]
pub fn paste_picture() -> Option<Picture> {
    let mut clipboard = crate::get_clipboard().ok()?.lock().ok()?;
    let data = clipboard.get_image().ok()?;
    let width = u32::try_from(data.width).ok()?;
    let height = u32::try_from(data.height).ok()?;
    let rgba = RgbaImage::from_raw(width, height, data.bytes.into_owned())?;
    encode(DynamicImage::ImageRgba8(rgba), false)
}

/// `image` scaled to fit [`MAX_SIDE`] and encoded: as JPEG when `photo` or
/// when the PNG comes out too large, else as PNG, which keeps text in a
/// screenshot sharp.
fn encode(image: DynamicImage, photo: bool) -> Option<Picture> {
    let image = if image.width().max(image.height()) > MAX_SIDE {
        image.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Lanczos3)
    } else {
        image
    };
    let (width, height) = (image.width(), image.height());
    let png = (!photo)
        .then(|| write(&image, ImageFormat::Png))
        .flatten()
        .filter(|bytes| bytes.len() <= MAX_PNG_BYTES);
    let (media_type, bytes) = match png {
        Some(bytes) => ("image/png", bytes),
        // JPEG has no alpha channel.
        None => (
            "image/jpeg",
            write(&DynamicImage::ImageRgb8(image.to_rgb8()), ImageFormat::Jpeg)?,
        ),
    };
    Some(Picture {
        media_type: media_type.to_string(),
        data: STANDARD.encode(bytes),
        width,
        height,
    })
}

fn write(image: &DynamicImage, format: ImageFormat) -> Option<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, format).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_go_by_extension() {
        assert_eq!(picture_kind(Path::new("a/Shot.PNG")), Some("image/png"));
        assert_eq!(picture_kind(Path::new("x.jpeg")), Some("image/jpeg"));
        assert_eq!(picture_kind(Path::new("notes.txt")), None);
        assert_eq!(picture_kind(Path::new("png")), None);
    }

    #[test]
    fn a_small_file_goes_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dot.png");
        let image = DynamicImage::ImageRgba8(RgbaImage::new(4, 3));
        image.save(&path).unwrap();
        let picture = picture_from_path(&path).unwrap();
        assert_eq!(picture.media_type, "image/png");
        assert_eq!((picture.width, picture.height), (4, 3));
        assert_eq!(
            STANDARD.decode(&picture.data).unwrap(),
            std::fs::read(&path).unwrap()
        );
    }

    #[test]
    fn an_encoded_picture_reads_back() {
        let picture = encode(DynamicImage::ImageRgba8(RgbaImage::new(7, 5)), false).unwrap();
        assert_eq!(
            picture_from_data(&picture.media_type, &picture.data),
            Some(picture)
        );
        assert_eq!(picture_from_data("image/png", "bm90IGEgcG5n"), None);
    }

    #[test]
    fn a_large_picture_is_scaled_down() {
        let picture = encode(DynamicImage::ImageRgba8(RgbaImage::new(4000, 1000)), false).unwrap();
        assert_eq!((picture.width, picture.height), (MAX_SIDE, 392));
        assert_eq!(picture.media_type, "image/png");
    }

    #[test]
    fn a_photo_goes_as_jpeg() {
        let picture = encode(DynamicImage::ImageRgba8(RgbaImage::new(10, 10)), true).unwrap();
        assert_eq!(picture.media_type, "image/jpeg");
    }

    #[test]
    fn not_a_picture_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake.png");
        std::fs::write(&path, b"not a png").unwrap();
        assert_eq!(picture_from_path(&path), None);
    }
}
