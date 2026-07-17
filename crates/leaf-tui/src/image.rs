//! Block-level images, painted with the terminal's own graphics protocol.
//!
//! leaf-core lays a block image out as a run of visual rows — a label row plus
//! blank filler rows it reserves once we tell it how tall the picture is (see
//! [`leaf_core::Doc::set_image_rows`]). This module is the terminal end of that:
//! it decodes each image, measures how many character rows the fitted picture
//! needs, and paints the raster over the reserved rows with
//! [`ratatui_image`], which speaks kitty / iTerm2 / sixel where the terminal
//! supports them and falls back to unicode half-blocks where it doesn't.
//!
//! The height has to come from here, not core: core does no I/O, so it can't
//! open the file to learn the aspect ratio. We decode once, cache the decoded
//! raster keyed by resolved path, and hand core the row counts each frame; a
//! frame that measures the same images it did last time is a no-op on both sides.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ratatui::{
    Frame,
    layout::Rect,
    widgets::Clear,
};
use ratatui_image::{
    FontSize, Resize, StatefulImage,
    picker::Picker,
    protocol::StatefulProtocol,
};

use leaf_core::ImageInfo;

/// The most rows a single image may reserve, so one tall picture can't push a
/// whole screen of text out of view. Mirrors the GUI's `IMAGE_MAX_H` pixel cap,
/// expressed in the terminal's only vertical unit.
const MAX_IMAGE_ROWS: usize = 30;

/// A decoded image plus the box it was last measured into.
struct Entry {
    /// The resizable protocol ratatui-image re-encodes to fit the paint rect. It
    /// owns the decoded pixels; it re-encodes only when the target rect changes,
    /// so a steady frame reuses the last encoding.
    protocol: StatefulProtocol,
    /// The source image's intrinsic pixel size, kept because the protocol has
    /// consumed the `DynamicImage` and box-fitting needs the original aspect.
    intrinsic: (u32, u32),
    /// The character-cell box the last [`Images::reserve`] fitted this image
    /// into — `(cols, rows)`. `rows` is what core reserved; `cols` is how wide
    /// the snug box is, so painting can hug the picture instead of the full width.
    box_cells: (u16, u16),
}

/// The terminal image subsystem: the graphics-protocol picker plus a per-path
/// cache of decoded rasters. Lives on `App` so a picture is decoded once per
/// session, not once per frame.
pub struct Images {
    picker: Picker,
    /// Resolved path → decoded entry, or `None` for a path that isn't a loadable
    /// local image (remote URL, `data:` URI, missing file, unsupported format).
    /// The `None` is cached too, so a broken reference is tried once, not every
    /// frame.
    cache: HashMap<PathBuf, Option<Entry>>,
}

impl Default for Images {
    /// A half-blocks picker with no terminal query — the safe default before
    /// [`Images::query`] has probed the real terminal (and the permanent state on
    /// a terminal that has no graphics protocol at all).
    fn default() -> Self {
        Images { picker: Picker::halfblocks(), cache: HashMap::new() }
    }
}

impl Images {
    /// Probe the terminal for its graphics protocol and font size, replacing the
    /// half-blocks default with whatever it actually supports. Must run with the
    /// terminal in raw mode (it reads escape-sequence replies), so `main` calls
    /// it right after `ratatui::init`. A terminal that doesn't answer keeps the
    /// half-blocks fallback — images still render, just coarser.
    pub fn query(&mut self) {
        if let Ok(picker) = Picker::from_query_stdio() {
            self.picker = picker;
        }
    }

    /// Decode (once) and measure every block image, returning the row count each
    /// one reserves keyed by destination — exactly the map
    /// [`leaf_core::Doc::set_image_rows`] wants. A destination that doesn't
    /// resolve to a loadable local file is left out, so core keeps its bare
    /// one-row placeholder for it. `avail_cols` is the content width the picture
    /// may fill.
    pub fn reserve(
        &mut self,
        images: &[ImageInfo],
        doc_dir: Option<&Path>,
        avail_cols: u16,
        avail_rows: u16,
    ) -> HashMap<String, usize> {
        let font = self.picker.font_size();
        // The picture sits *inside* a one-cell border box (drawn by `ui`), so it
        // fits the interior: two fewer columns, two fewer rows. Never taller than
        // the viewport interior, so the whole framed box can fit on screen and the
        // raster (which, unlike the border, can't be clipped) gets painted.
        let inner_cols = avail_cols.saturating_sub(2).max(1);
        let inner_rows = (avail_rows.saturating_sub(2) as usize).clamp(1, MAX_IMAGE_ROWS) as u16;
        let mut heights = HashMap::new();
        for info in images {
            let Some(path) = resolve_image_path(&info.destination, doc_dir) else {
                continue;
            };
            let Some(entry) = self.entry(&path) else { continue };
            let cells = box_cells(entry.intrinsic, inner_cols, inner_rows, font);
            entry.box_cells = cells;
            heights.insert(info.destination.clone(), cells.1 as usize);
        }
        heights
    }

    /// The character-cell size `(cols, rows)` of the picture inside its border —
    /// what `ui` sizes the box to and reserves the rows for. `None` for an image
    /// that isn't a loadable local file (so `ui` frames it as a bare placeholder).
    pub fn picture_cells(&self, info: &ImageInfo, doc_dir: Option<&Path>) -> Option<(u16, u16)> {
        let path = resolve_image_path(&info.destination, doc_dir)?;
        self.cache.get(&path).and_then(|e| e.as_ref()).map(|e| e.box_cells)
    }

    /// Paint an image's raster into `rect`, the interior of its border box. The
    /// caller only calls this once the whole box is on screen: a graphics-protocol
    /// image has one fixed rasterization, and drawing it into a *clipped* rect
    /// would make ratatui-image re-encode it smaller every frame as it scrolls
    /// past an edge — the picture pumps in size and the churn of protocol escapes
    /// can strand the cursor. Returns `false` (so `ui` can fall back to a labelled
    /// placeholder) when the image isn't a loadable local file.
    pub fn paint_raster(
        &mut self,
        f: &mut Frame,
        info: &ImageInfo,
        doc_dir: Option<&Path>,
        rect: Rect,
    ) -> bool {
        let Some(path) = resolve_image_path(&info.destination, doc_dir) else {
            return false;
        };
        let Some(entry) = self.cache.get_mut(&path).and_then(|e| e.as_mut()) else {
            return false;
        };
        f.render_widget(Clear, rect);
        f.render_stateful_widget(
            StatefulImage::new().resize(Resize::Fit(None)),
            rect,
            &mut entry.protocol,
        );
        true
    }

    /// The cache entry for a resolved path, decoding it on first use. `None` (and
    /// a cached `None`) when the file can't be read or decoded.
    fn entry(&mut self, path: &Path) -> Option<&mut Entry> {
        if !self.cache.contains_key(path) {
            let decoded = load_image(path).map(|img| {
                let intrinsic = (img.width(), img.height());
                Entry {
                    protocol: self.picker.new_resize_protocol(img),
                    intrinsic,
                    box_cells: (1, 1),
                }
            });
            self.cache.insert(path.to_path_buf(), decoded);
        }
        self.cache.get_mut(path).and_then(|e| e.as_mut())
    }
}

/// The character-cell box an image fits into: as wide as the content allows (but
/// never upscaled past the source's own pixels) and as tall as that width makes
/// it, capped at `max_rows`. Works in pixels so the terminal's non-square cells
/// (`font`) don't distort the aspect ratio, then rounds up to whole cells.
fn box_cells(intrinsic: (u32, u32), avail_cols: u16, max_rows: u16, font: FontSize) -> (u16, u16) {
    let (iw, ih) = (intrinsic.0.max(1) as u64, intrinsic.1.max(1) as u64);
    // `FontSize` is `(cell_width_px, cell_height_px)`.
    let (cw, ch) = (font.0.max(1) as u64, font.1.max(1) as u64);
    let avail_px = avail_cols.max(1) as u64 * cw;

    // Fit the width, never upscaling; the height follows from the aspect ratio.
    let mut w_px = iw.min(avail_px);
    let mut h_px = w_px * ih / iw;
    let max_h_px = max_rows.max(1) as u64 * ch;
    if h_px > max_h_px {
        h_px = max_h_px;
        w_px = h_px * iw / ih;
    }
    let cols = w_px.div_ceil(cw).clamp(1, avail_cols.max(1) as u64) as u16;
    let rows = h_px.div_ceil(ch).clamp(1, max_rows.max(1) as u64) as u16;
    (cols, rows)
}

/// Resolve an image destination to a readable local path, or `None` when it's
/// not one this synchronous loader handles: a remote URL, a `data:` URI, a
/// protocol-relative `//host/…`, or a relative path with no document directory
/// to anchor it. Mirrors leaf-gpui's resolver — the same policy, since both
/// frontends decode local files eagerly and leave the rest as a placeholder.
fn resolve_image_path(dest: &str, doc_dir: Option<&Path>) -> Option<PathBuf> {
    let dest = dest.trim();
    if dest.is_empty() {
        return None;
    }
    let lower = dest.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || dest.starts_with("//")
    {
        return None;
    }
    let raw = dest.strip_prefix("file://").unwrap_or(dest);
    let path = Path::new(raw);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        doc_dir.map(|d| d.join(path))
    }
}

/// Decode an image file to a `DynamicImage`, or `None` on any failure (missing,
/// unreadable, or a format the enabled decoders don't cover, e.g. SVG).
fn load_image(path: &Path) -> Option<image::DynamicImage> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> FontSize {
        (10, 20) // 10px wide, 20px tall cells
    }

    #[test]
    fn box_cells_fits_width_and_preserves_aspect() {
        // A 200×100px image (2:1) into a 40-col space with 10×20px cells: 40 cols
        // is 400px, wider than the image, so it isn't upscaled — it stays 200px =
        // 20 cols wide, 100px = 5 rows tall.
        assert_eq!(box_cells((200, 100), 40, MAX_IMAGE_ROWS as u16, font()), (20, 5));
    }

    #[test]
    fn box_cells_scales_down_to_the_available_width() {
        // An 800×400px image into the same 40-col (400px) space: scaled to 400px
        // wide (40 cols), 200px tall (10 rows).
        assert_eq!(box_cells((800, 400), 40, MAX_IMAGE_ROWS as u16, font()), (40, 10));
    }

    #[test]
    fn box_cells_caps_height_and_keeps_aspect() {
        // A skinny 100×4000px image would want 200 rows; a small row cap holds it
        // and shrinks the width to keep the aspect ratio.
        let (cols, rows) = box_cells((100, 4000), 40, 8, font());
        assert_eq!(rows, 8, "height is held to the cap");
        assert!(cols >= 1 && cols < 40, "width shrinks with the capped height: {cols}");
    }

    #[test]
    fn resolve_rejects_remote_and_anchors_relative() {
        let dir = Path::new("/docs");
        assert_eq!(resolve_image_path("pics/cat.png", Some(dir)), Some(PathBuf::from("/docs/pics/cat.png")));
        assert_eq!(resolve_image_path("/abs/cat.png", Some(dir)), Some(PathBuf::from("/abs/cat.png")));
        assert_eq!(resolve_image_path("https://x.dev/a.png", Some(dir)), None);
        assert_eq!(resolve_image_path("data:image/png;base64,AAAA", Some(dir)), None);
        // A relative path with no document directory can't be anchored.
        assert_eq!(resolve_image_path("cat.png", None), None);
    }
}
