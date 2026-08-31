//! Decoding image files — raster formats through the `image` codecs, SVG
//! through resvg — to the `DynamicImage` every consumer's pipeline speaks.

use std::path::Path;

/// Decode an image file to a `DynamicImage`, or `None` on any failure (missing,
/// unreadable, or a format no decoder covers). SVG is rasterized with resvg
/// ([`load_svg`]); every raster format goes through the `image` codec crate.
pub fn load_image(path: &Path) -> Option<image::DynamicImage> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        return load_svg(&std::fs::read(path).ok()?);
    }
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}

/// Rasterize an SVG's bytes to a `DynamicImage`, or `None` if it won't parse.
///
/// The `image` crate has no SVG support, so this is the vector path: usvg parses
/// the document, resvg paints it onto a tiny-skia pixmap, and we hand the pixels
/// back as an `RgbaImage` the rest of the pipeline treats like any decoded
/// raster. We render at a fixed target resolution (scaling the SVG's own size so
/// its longer side is ~[`SVG_TARGET_PX`]) rather than its intrinsic size: an SVG
/// may declare a tiny viewport, and rasterizing that small would leave the
/// consumer upscaling a blurry thumbnail. System fonts are loaded so an SVG that
/// draws real `<text>` (not outlined paths) still renders its glyphs.
pub fn load_svg(data: &[u8]) -> Option<image::DynamicImage> {
    use resvg::{tiny_skia, usvg};

    /// The longer side, in pixels, we rasterize an SVG to before the consumer
    /// downscales it — big enough to stay crisp, capped so a huge viewport can't
    /// blow up the allocation.
    const SVG_TARGET_PX: f32 = 640.0;

    // The system font set, enumerated once per process rather than once per
    // SVG — loading it is tens of milliseconds of directory walking, and it
    // runs on the render path. usvg shares the database by `Arc`, so every
    // decode after the first borrows the same one.
    fn svg_fontdb() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
        use std::sync::{Arc, OnceLock};
        static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
        FONTS
            .get_or_init(|| {
                let mut db = resvg::usvg::fontdb::Database::new();
                db.load_system_fonts();
                Arc::new(db)
            })
            .clone()
    }

    let opt = usvg::Options {
        fontdb: svg_fontdb(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(data, &opt).ok()?;

    let size = tree.size();
    let longest = size.width().max(size.height()).max(1.0);
    // Scale so the longer side hits the target; clamp so a big SVG scales down
    // and a small one up, but neither runs away. Never below 1px per side.
    let scale = (SVG_TARGET_PX / longest).clamp(0.05, 16.0);
    let w = (size.width() * scale).ceil().max(1.0) as u32;
    let h = (size.height() * scale).ceil().max(1.0) as u32;

    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    // tiny-skia stores premultiplied alpha; `image` expects straight alpha, so
    // demultiply each pixel on the way into the RGBA buffer.
    let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(
        w, h, rgba,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_svg_rasterizes_to_straight_alpha_rgba() {
        // A 20×10 solid-red rect. `image` can't decode SVG at all, so this only
        // works via the resvg path.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#ff0000"/></svg>"##;
        let img = load_svg(svg).expect("valid SVG should rasterize");
        // Rendered at the target resolution, so upscaled from its 20×10 viewport
        // while keeping the 2:1 aspect.
        assert!(
            img.width() >= 20 && img.height() >= 10,
            "got {}×{}",
            img.width(),
            img.height()
        );
        assert_eq!(img.width(), img.height() * 2, "aspect ratio preserved");
        // The fill lands as opaque, straight-alpha red — not premultiplied mush.
        let rgba = img.to_rgba8();
        let center = rgba.get_pixel(rgba.width() / 2, rgba.height() / 2).0;
        assert_eq!(center, [255, 0, 0, 255], "center pixel is opaque red");
    }

    #[test]
    fn load_svg_rejects_garbage() {
        assert!(load_svg(b"not an svg at all").is_none());
    }
}
