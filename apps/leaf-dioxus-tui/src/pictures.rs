//! Pictures, formulas and diagrams as leaf-tui reads them, for blitz-tui to
//! paint with the terminal's graphics protocol: a `file://` URL through
//! leaf-raster, and a `data:image/svg+xml` URL (a formula or a diagram,
//! which leaf-dioxus typesets to SVG) rasterised by it.

use std::path::PathBuf;

use leaf_raster::image::DynamicImage;

/// How many pixels an SVG is drawn at per CSS pixel: a terminal's cell is
/// about twice the size of the cell the editor measures in.
const SVG_SCALE: f32 = 2.0;

/// A picture's source as pixels: a `file://` URL, or an SVG `data:` URL.
pub fn decode(src: &str) -> Option<DynamicImage> {
    if let Some(path) = src.strip_prefix("file://") {
        let path = PathBuf::from(percent_decode(path)?);
        return leaf_raster::load_image(&path);
    }
    if let Some(rest) = src.strip_prefix("data:image/svg+xml") {
        let (_, body) = rest.split_once(',')?;
        let svg = percent_decode(body)?;
        return leaf_raster::rasterize_svg(svg.as_bytes(), SVG_SCALE);
    }
    None
}

/// `%XX` escapes back to bytes, read as UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_decode_and_the_rest_stays() {
        assert_eq!(percent_decode("a%20b%3Cc").as_deref(), Some("a b<c"));
        assert_eq!(percent_decode("/x/y.png").as_deref(), Some("/x/y.png"));
        assert_eq!(percent_decode("end%2F").as_deref(), Some("end/"));
    }

    #[test]
    fn an_svg_data_url_rasterises() {
        let url = "data:image/svg+xml;charset=utf-8,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2210%22%20height%3D%225%22%3E%3C%2Fsvg%3E";
        let img = decode(url).expect("an image");
        assert_eq!((img.width(), img.height()), (20, 10));
    }
}
