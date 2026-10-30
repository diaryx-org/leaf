//! A picture as the views draw it: no larger than the screen needs, and
//! upright.
//!
//! A piece of a page is an SVG with its pictures inside it, as Typst's SVG
//! writes them: the file's own bytes. A photo from a phone's camera is
//! twelve megapixels or more, some 48 MB once decoded, which the window
//! decodes and keeps on the GPU for as long as its piece is on show, and
//! carries, base64'd twice, in the piece's `data:` URL. A few of them fill
//! the renderer's image atlas. So the views set a picture larger than
//! [`LONGEST`] from a copy scaled down to it, made once on the worker that
//! sets the pages. The copy is drawn the way up the original's EXIF
//! orientation says, as Typst sizes it, and keeps the original's size on
//! the page. A PDF is set from the originals.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use image::{DynamicImage, ImageDecoder, ImageFormat};

/// The longest side, in pixels, of a picture as the views draw it: the
/// width of the widest column a desktop's window reflows to, at twice its
/// points, with room for the page view's zoom.
pub const LONGEST: u32 = 2048;

/// `bytes` as the views draw them: a copy no longer than [`LONGEST`] on
/// either side, or the bytes themselves if they are no larger, or not a
/// JPEG, PNG or WebP. The same bytes, in the same `Arc`, are scaled once.
pub fn shown(bytes: &Arc<[u8]>) -> Arc<[u8]> {
    type Scaled = HashMap<usize, (Weak<[u8]>, Arc<[u8]>)>;
    static SCALED: OnceLock<Mutex<Scaled>> = OnceLock::new();
    let scaled = SCALED.get_or_init(Mutex::default);
    // An entry's `Weak` holds its bytes' allocation, so the address is not
    // another's while the entry is kept.
    let key = Arc::as_ptr(bytes).cast::<u8>() as usize;
    if let Some(copy) = scaled
        .lock()
        .ok()
        .and_then(|s| s.get(&key).map(|(_, c)| c.clone()))
    {
        return copy;
    }
    let copy: Arc<[u8]> = scale(bytes).map_or_else(|| Arc::clone(bytes), Arc::from);
    if let Ok(mut s) = scaled.lock() {
        s.retain(|_, (original, _)| original.strong_count() > 0);
        s.insert(key, (Arc::downgrade(bytes), Arc::clone(&copy)));
    }
    copy
}

/// A copy of the picture in `bytes` no longer than [`LONGEST`], upright, if
/// it is a JPEG, PNG or WebP longer than that: a PNG as a PNG, and any
/// other as a JPEG unless it is transparent. Its density is the original's
/// scaled with it, so Typst gives it the original's natural size.
fn scale(bytes: &[u8]) -> Option<Vec<u8>> {
    let format = image::guess_format(bytes).ok()?;
    if !matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP
    ) {
        return None;
    }
    let mut decoder = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .ok()?;
    let (w, h) = decoder.dimensions();
    if w.max(h) <= LONGEST {
        return None;
    }
    let orientation = decoder.orientation().ok();
    let mut picture = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(o) = orientation {
        picture.apply_orientation(o);
    }
    let picture = picture.thumbnail(LONGEST, LONGEST);
    // Typst's own default, for a picture that names no density.
    let dpi = density(bytes).unwrap_or(72.0) * f64::from(picture.width().max(picture.height()))
        / f64::from(w.max(h));
    let mut out = Vec::new();
    if format == ImageFormat::Png || picture.color().has_alpha() {
        let rgba = picture.to_rgba8();
        let mut png = png::Encoder::new(&mut out, rgba.width(), rgba.height());
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        let ppm = (dpi / 0.0254).round() as u32;
        png.set_pixel_dims(Some(png::PixelDimensions {
            xppu: ppm,
            yppu: ppm,
            unit: png::Unit::Meter,
        }));
        png.write_header().ok()?.write_image_data(&rgba).ok()?;
    } else {
        let mut jpeg = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88);
        jpeg.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(
            dpi.round().clamp(1.0, f64::from(u16::MAX)) as u16,
        ));
        picture.to_rgb8().write_with_encoder(jpeg).ok()?;
    }
    Some(out)
}

/// The density of the picture in `bytes`, in dots an inch, where Typst
/// finds it: its EXIF, its JPEG's JFIF header, or its PNG's `pHYs`.
fn density(bytes: &[u8]) -> Option<f64> {
    exif_density(bytes)
        .or_else(|| jfif_density(bytes))
        .or_else(|| png_density(bytes))
        .filter(|&d| d > 0.0)
}

fn exif_density(bytes: &[u8]) -> Option<f64> {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(bytes))
        .ok()?;
    let axis = |tag| match &exif.get_field(tag, exif::In::PRIMARY)?.value {
        exif::Value::Rational(r) => Some(r.first()?.to_f64()),
        _ => None,
    };
    [axis(exif::Tag::XResolution), axis(exif::Tag::YResolution)]
        .into_iter()
        .flatten()
        .reduce(f64::max)
}

fn jfif_density(bytes: &[u8]) -> Option<f64> {
    if !bytes.starts_with(b"\xFF\xD8\xFF\xE0") || bytes.get(6..12)? != b"JFIF\0\x01" {
        return None;
    }
    let u16_at = |i: usize| Some(u16::from_be_bytes(bytes.get(i..i + 2)?.try_into().ok()?));
    if u16_at(4)? < 16 {
        return None;
    }
    let dpu = f64::from(u16_at(14)?.max(u16_at(16)?));
    match bytes.get(13)? {
        1 => Some(dpu),
        2 => Some(dpu * 2.54),
        _ => None,
    }
}

fn png_density(bytes: &[u8]) -> Option<f64> {
    let decoder = png::Decoder::new(Cursor::new(bytes)).read_info().ok()?;
    let dims = decoder.info().pixel_dims?;
    match dims.unit {
        png::Unit::Meter => Some(f64::from(dims.xppu.max(dims.yppu)) * 0.0254),
        png::Unit::Unspecified => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A JPEG of `w` by `h`, red on its left half and blue on its right,
    /// with an EXIF orientation of `orientation`, as a phone's camera writes
    /// one held upright: its sensor's pixels, and the way up.
    pub(crate) fn jpeg_turned(w: u32, h: u32, orientation: u16) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, _| {
            if x < w / 2 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });
        let mut jpeg = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg)
            .unwrap();
        // APP1: "Exif", a little-endian TIFF header, and one IFD of one
        // entry, the orientation (0x0112), a SHORT.
        let mut tiff = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        tiff.extend_from_slice(&orientation.to_le_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let len = (app1.len() + 2) as u16;
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn a_large_photo_is_scaled_down_and_turned_upright() {
        // A phone held upright: a sensor's 4032 by 3024, turned a quarter.
        let original: Arc<[u8]> = jpeg_turned(4032, 3024, 6).into();
        let copy = shown(&original);
        let picture = image::load_from_memory(&copy).unwrap().into_rgb8();
        assert_eq!(picture.dimensions(), (1536, 2048));
        let (top, bottom) = (picture.get_pixel(768, 100), picture.get_pixel(768, 1948));
        assert!(
            top[0] > 200 && top[2] < 60,
            "the top is the red half: {top:?}"
        );
        assert!(
            bottom[2] > 200 && bottom[0] < 60,
            "the bottom is the blue half: {bottom:?}"
        );
        // No orientation left for anything to turn it by again.
        let mut decoder = image::ImageReader::new(Cursor::new(&*copy))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(
            decoder.orientation().unwrap(),
            image::metadata::Orientation::NoTransforms
        );
    }

    #[test]
    fn a_copy_keeps_the_originals_size_on_the_page() {
        // No density named: Typst's 72 dpi, so 4032 pixels are 56 inches;
        // the copy's 2048 at 36.6 dpi are the same.
        let original: Arc<[u8]> = jpeg_turned(4032, 3024, 1).into();
        let dpi = density(&shown(&original)).unwrap();
        assert!((dpi - 72.0 * 2048.0 / 4032.0).abs() < 0.5, "{dpi}");
    }

    #[test]
    fn a_picture_the_screen_can_draw_is_its_own() {
        let original: Arc<[u8]> = jpeg_turned(800, 600, 6).into();
        assert!(Arc::ptr_eq(&shown(&original), &original));
    }

    #[test]
    fn the_same_bytes_are_scaled_once() {
        let original: Arc<[u8]> = jpeg_turned(3000, 1000, 1).into();
        let first = shown(&original);
        assert!(!Arc::ptr_eq(&first, &original));
        assert!(Arc::ptr_eq(&shown(&original), &first));
    }

    #[test]
    fn a_large_transparent_png_stays_a_png() {
        let img = image::RgbaImage::from_pixel(3000, 100, image::Rgba([0, 0, 0, 0]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let copy = shown(&Arc::from(png));
        assert_eq!(image::guess_format(&copy).unwrap(), ImageFormat::Png);
        let picture = image::load_from_memory(&copy).unwrap();
        assert_eq!((picture.width(), picture.height()), (2048, 68));
        assert!(picture.color().has_alpha());
    }
}
