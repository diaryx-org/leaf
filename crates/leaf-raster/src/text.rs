//! Shaping oversized heading text into an RGBA frame, with the editing UI —
//! caret and selection — painted into the pixels.
//!
//! The frame is drawn over a transparent background, so whatever surface it
//! lands on shows through around the glyphs. The caret and selection have to be
//! *in* the raster rather than composited over it because the terminal — the
//! first consumer — cannot draw cells over a graphics-protocol image at all:
//! carrying its own editing UI is what lets a rasterized heading stay editable
//! instead of collapsing back to plain text the moment the caret enters it.
//!
//! One layout answers everything. Rasterizing, caret placement, selection
//! rectangles, and hit-testing all read the same shaped [`Buffer`], cached per
//! [`HeadingSpec`], so a click cannot be answered by a different layout than
//! the one on screen — and so caret motion and pointer sweeps (which arrive
//! dozens of times a second) reuse the shaping instead of repeating it.

use std::collections::HashMap;

use cosmic_text::{
    Attrs, Buffer, Color, Cursor, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
};

/// Everything a heading's *geometry* depends on: the text, the level (which
/// picks the type scale), and the pixel box the raster fills. Deliberately free
/// of colors and caret state so the same spec drives rasterization,
/// hit-testing, and the fits check — a click must be answered by exactly the
/// layout that was drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadingSpec<'a> {
    pub text: &'a str,
    /// Heading level, 1 or 2 — the only levels big enough to rasterize.
    pub level: u8,
    pub width_px: u32,
    pub height_px: u32,
}

impl HeadingSpec<'_> {
    /// One layout line filling the whole box: the line height *is* the box
    /// height, and the font size is the fraction of it that optically balances
    /// each level (an H1 carries more of its block than an H2 does).
    fn metrics(&self) -> Metrics {
        let line_height = self.height_px.max(1) as f32;
        let font_size = if self.level == 1 {
            line_height * 0.72
        } else {
            line_height * 0.68
        };
        Metrics::new(font_size, line_height)
    }

    fn key(&self) -> LayoutKey {
        LayoutKey {
            text: self.text.to_owned(),
            level: self.level,
            width: self.width_px,
            height: self.height_px,
        }
    }
}

/// The editing UI to paint into a raster. Offsets are bytes into the spec's
/// `text`, on `char` boundaries; anything out of range or misaligned is dropped
/// rather than panicking, since a stale frontend offset is not worth refusing
/// to draw the heading over.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditingUi {
    /// Byte offset the caret bar is drawn at, or `None` for no caret.
    pub caret: Option<usize>,
    /// Selected byte range `(start, end)`, or `None` for no selection.
    pub selection: Option<(usize, usize)>,
    /// The fill behind selected glyphs. The terminal's selection is reverse
    /// video, so its caller passes the heading's own ink here…
    pub selection_bg: (u8, u8, u8),
    /// …and something that reads on that fill here, for the glyphs on it (and
    /// for the caret whenever it stands inside the selection, where a bar in
    /// the ink would sink into a fill of the same color).
    pub selection_fg: (u8, u8, u8),
}

/// What a shaped layout depends on — [`HeadingSpec`] by value, for the cache.
#[derive(Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: String,
    level: u8,
    width: u32,
    height: u32,
}

/// How many shaped layouts to hold before the cache is emptied. One entry per
/// distinct on-screen heading is the steady state (editing state is *not* in
/// the key), so the cap only bounds a session that scrolls through many.
const LAYOUT_CACHE_MAX: usize = 16;

/// The shaping and glyph-raster state — a font database, a glyph cache, and
/// the per-heading layout cache — shared across every raster so fonts load
/// once per session and a heading shapes once per edit, not once per frame.
pub struct Rasterizer {
    fonts: FontSystem,
    swash: SwashCache,
    layouts: HashMap<LayoutKey, Buffer>,
}

impl Default for Rasterizer {
    fn default() -> Self {
        Rasterizer {
            fonts: FontSystem::new(),
            swash: SwashCache::new(),
            layouts: HashMap::new(),
        }
    }
}

impl Rasterizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rasterize a heading to an RGBA frame: bold glyphs in `ink` over a
    /// transparent ground, the selection's fill and re-inked glyphs under and
    /// among them, and the caret bar on top.
    pub fn heading(
        &mut self,
        spec: &HeadingSpec,
        ink: (u8, u8, u8),
        ui: &EditingUi,
    ) -> image::RgbaImage {
        let width = spec.width_px.max(1);
        let height = spec.height_px.max(1);
        let selection = selection_range(ui, spec.text);
        self.shape(spec);
        // Split borrows: the draw below needs the cached buffer, the font
        // system, and the glyph cache all at once.
        let Rasterizer {
            fonts,
            swash,
            layouts,
        } = self;
        let buffer = layouts.get_mut(&spec.key()).expect("just shaped");
        let mut pixels = image::RgbaImage::new(width, height);

        // The selection's fill first, so the glyphs land on top of it. The
        // rectangles are kept: they are also what decides which glyph pixels
        // get the selection ink below.
        let mut sel_rects: Vec<(i32, i32, i32, i32)> = Vec::new();
        if let Some((s, e)) = selection {
            let (cs, ce) = (Cursor::new(0, s), Cursor::new(0, e));
            for run in buffer.layout_runs() {
                let top = run.line_top.max(0.0) as i32;
                let bottom = top + run.line_height.ceil() as i32;
                for (x, w) in run.highlight(cs, ce) {
                    sel_rects.push((x as i32, top, x as i32 + w.ceil() as i32, bottom));
                }
            }
            let (br, bg, bb) = ui.selection_bg;
            for &(x0, y0, x1, y1) in &sel_rects {
                blend_rect(
                    &mut pixels,
                    x0,
                    y0,
                    (x1 - x0) as u32,
                    (y1 - y0) as u32,
                    [br, bg, bb, 255],
                );
            }
        }

        // The glyphs, composited src-over so their antialiased edges read
        // correctly both on the transparent ground and on the selection fill.
        // A glyph pixel inside a selection rectangle swaps the ink for the
        // selection's — by clipping against the fill rather than by re-shaping
        // the text in colored spans, which would split the shaping runs at the
        // selection boundary and lay the glyphs out differently than the
        // hit-test's single run. Only pixels carrying the ink are swapped, so
        // color glyphs (an emoji in a title) keep their own pixels, as they do
        // in every other selection.
        let (ir, ig, ib) = ink;
        let (sr, sg, sb) = ui.selection_fg;
        buffer.draw(fonts, swash, Color::rgb(ir, ig, ib), |x, y, w, h, c| {
            let mut px = [c.r(), c.g(), c.b(), c.a()];
            if !sel_rects.is_empty()
                && px[..3] == [ir, ig, ib]
                && sel_rects
                    .iter()
                    .any(|&(x0, y0, x1, y1)| x >= x0 && x < x1 && y >= y0 && y < y1)
            {
                (px[0], px[1], px[2]) = (sr, sg, sb);
            }
            blend_rect(&mut pixels, x, y, w, h, px);
        });

        // The caret bar last — over the glyph it sits against, like every
        // other leaf frontend draws it. Solid rather than blinking: a
        // graphics-protocol image is retransmitted whole on every change, and
        // a blink is not worth two frames a second of that. Inside the
        // selection the bar takes the selection ink, since the fill it stands
        // on *is* the caret's usual color.
        if let Some(caret) = ui.caret {
            let caret = caret.min(spec.text.len());
            if spec.text.is_char_boundary(caret) {
                let bar = (height / 30).max(2);
                let cursor = Cursor::new(0, caret);
                // Fall back to the box's left edge when there's no run to ask —
                // an empty heading still shows where typing will land.
                let (mut x, mut top, mut h) = (0.0f32, 0i32, height);
                for run in buffer.layout_runs() {
                    if let Some(cx) = run.cursor_position(&cursor) {
                        x = cx;
                        top = run.line_top.max(0.0) as i32;
                        h = run.line_height.ceil() as u32;
                        break;
                    }
                }
                let x = (x as i32).clamp(0, width.saturating_sub(bar) as i32);
                let (r, g, b) = match selection {
                    Some((s, e)) if caret >= s && caret < e => ui.selection_fg,
                    _ => ink,
                };
                blend_rect(&mut pixels, x, top, bar, h, [r, g, b, 255]);
            }
        }
        pixels
    }

    /// The byte offset (a `char` boundary in the spec's text) a pixel position
    /// hits — the reverse of [`Rasterizer::heading`], answered by the same
    /// cached layout, so a click on the raster lands on the glyph it visually
    /// struck rather than on whatever happens to share its character cell.
    pub fn heading_hit(&mut self, spec: &HeadingSpec, x: f32, y: f32) -> usize {
        let buffer = self.shape(spec);
        match buffer.hit(x, y) {
            Some(cursor) => cursor.index.min(spec.text.len()),
            // No run to hit (an empty heading): before-or-after is all that's
            // left to say.
            None if x <= 0.0 => 0,
            None => spec.text.len(),
        }
    }

    /// Whether the heading lays out on the single line its box has room for.
    /// A longer title wraps onto a second layout line that the box height
    /// culls — its tail would be neither drawn nor clickable, and a caret in
    /// it would have nowhere truthful to stand — so the caller keeps such a
    /// heading as ordinary text instead of rasterizing it.
    pub fn heading_fits(&mut self, spec: &HeadingSpec) -> bool {
        self.shape(spec)
            .lines
            .first()
            .and_then(|line| line.layout_opt())
            .is_none_or(|lines| lines.len() <= 1)
    }

    /// The shaped layout for a spec: one bold line, wrapped only as a last
    /// resort, sized to the spec's box. Cached, since caret motion, pointer
    /// sweeps, and steady frames all ask for the same layout over and over.
    fn shape(&mut self, spec: &HeadingSpec) -> &mut Buffer {
        let key = spec.key();
        if !self.layouts.contains_key(&key) {
            if self.layouts.len() >= LAYOUT_CACHE_MAX {
                self.layouts.clear();
            }
            let mut buffer = Buffer::new(&mut self.fonts, spec.metrics());
            buffer.set_wrap(Wrap::WordOrGlyph);
            buffer.set_size(
                Some(spec.width_px.max(1) as f32),
                Some(spec.height_px.max(1) as f32),
            );
            buffer.set_text(
                spec.text,
                &Attrs::new().weight(Weight::BOLD),
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(&mut self.fonts, false);
            self.layouts.insert(key.clone(), buffer);
        }
        self.layouts.get_mut(&key).expect("just inserted")
    }
}

/// The selection clamped into the text and checked for `char` alignment —
/// `None` (draw no selection) rather than a panic on a stale offset.
fn selection_range(ui: &EditingUi, text: &str) -> Option<(usize, usize)> {
    let (s, e) = ui.selection?;
    let (s, e) = (s.min(text.len()), e.min(text.len()));
    (s < e && text.is_char_boundary(s) && text.is_char_boundary(e)).then_some((s, e))
}

/// Composite a solid rect src-over into the frame, clipped to it. Straight
/// (non-premultiplied) alpha throughout, which is what `image` speaks and what
/// the graphics protocols expect.
fn blend_rect(pixels: &mut image::RgbaImage, x: i32, y: i32, w: u32, h: u32, src: [u8; 4]) {
    if src[3] == 0 || w == 0 || h == 0 {
        return;
    }
    let (iw, ih) = (pixels.width() as i32, pixels.height() as i32);
    let (x0, y0) = (x.max(0), y.max(0));
    let x1 = x.saturating_add(w.min(i32::MAX as u32) as i32).min(iw);
    let y1 = y.saturating_add(h.min(i32::MAX as u32) as i32).min(ih);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // A fully opaque fill is a straight write, done a row at a time — a
    // selection fill can cover most of the frame, and per-pixel blending it
    // would be the slowest thing in the raster.
    if src[3] == 255 {
        let stride = pixels.width() as usize * 4;
        let buf: &mut [u8] = pixels;
        for py in y0 as usize..y1 as usize {
            let row = &mut buf[py * stride + x0 as usize * 4..py * stride + x1 as usize * 4];
            for chunk in row.chunks_exact_mut(4) {
                chunk.copy_from_slice(&src);
            }
        }
        return;
    }
    for py in y0..y1 {
        for px in x0..x1 {
            blend(pixels.get_pixel_mut(px as u32, py as u32), src);
        }
    }
}

/// One pixel of straight-alpha src-over.
fn blend(dst: &mut image::Rgba<u8>, src: [u8; 4]) {
    let sa = src[3] as u32;
    let da = dst.0[3] as u32;
    // out_a scaled by 255 so the color divide below stays in integers.
    let out_a = sa * 255 + da * (255 - sa);
    if out_a == 0 {
        return;
    }
    for (d, s) in dst.0.iter_mut().zip(src).take(3) {
        let (sc, dc) = (s as u32, *d as u32);
        *d = ((sc * sa * 255 + dc * da * (255 - sa)) / out_a) as u8;
    }
    dst.0[3] = (out_a / 255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> HeadingSpec<'_> {
        HeadingSpec {
            text,
            level: 1,
            width_px: 400,
            height_px: 60,
        }
    }

    /// Ink coverage of a column strip, to locate where something was painted
    /// without depending on which fonts this machine resolves.
    fn column_alpha(img: &image::RgbaImage, x: u32) -> u32 {
        (0..img.height())
            .map(|y| img.get_pixel(x, y).0[3] as u32)
            .sum()
    }

    #[test]
    fn the_caret_is_painted_into_the_pixels() {
        let mut r = Rasterizer::new();
        let s = spec("Hi");
        let plain = r.heading(&s, (200, 200, 200), &EditingUi::default());
        let with_caret = r.heading(
            &s,
            (200, 200, 200),
            &EditingUi {
                caret: Some(0),
                ..Default::default()
            },
        );
        // The bar at offset 0 runs the full line height at the left edge —
        // taller than any glyph column of the plain raster there.
        assert!(
            column_alpha(&with_caret, 0) > column_alpha(&plain, 0),
            "no caret ink at the left edge"
        );
    }

    #[test]
    fn an_empty_heading_still_shows_the_caret() {
        let mut r = Rasterizer::new();
        let s = spec("");
        let img = r.heading(
            &s,
            (200, 200, 200),
            &EditingUi {
                caret: Some(0),
                ..Default::default()
            },
        );
        assert!(column_alpha(&img, 0) > 0, "empty heading lost its caret");
    }

    #[test]
    fn the_selection_fills_behind_the_glyphs_and_reinks_them() {
        let mut r = Rasterizer::new();
        let s = spec("Hello");
        let img = r.heading(
            &s,
            (220, 220, 220),
            &EditingUi {
                selection: Some((0, 5)),
                selection_bg: (255, 0, 0),
                selection_fg: (10, 10, 10),
                ..Default::default()
            },
        );
        // Between glyph strokes there is a pure-fill pixel at the selection
        // color, and inside a stroke the glyph carries the selection ink.
        assert!(
            img.pixels().any(|p| p.0 == [255, 0, 0, 255]),
            "no selection fill painted"
        );
        assert!(
            img.pixels().any(|p| p.0 == [10, 10, 10, 255]),
            "selected glyphs kept the page ink"
        );
    }

    /// The empirically-found invisible caret: ink and selection fill are the
    /// same color in the terminal (reverse video), so a caret standing at the
    /// *start* of a selection must not be drawn in ink.
    #[test]
    fn a_caret_inside_the_selection_stays_visible() {
        let mut r = Rasterizer::new();
        let s = spec("Hello world");
        let ink = (100, 160, 240);
        let ui = EditingUi {
            selection: Some((0, s.text.len())),
            selection_bg: ink,
            selection_fg: (250, 250, 250),
            ..Default::default()
        };
        let without = r.heading(&s, ink, &ui);
        let with = r.heading(
            &s,
            ink,
            &EditingUi {
                caret: Some(0),
                ..ui
            },
        );
        assert_ne!(
            without.as_raw(),
            with.as_raw(),
            "the caret vanished into the selection fill"
        );
    }

    #[test]
    fn a_misaligned_selection_is_dropped_not_fatal() {
        let mut r = Rasterizer::new();
        let s = spec("héllo"); // 'é' is two bytes; offset 2 splits it
        let img = r.heading(
            &s,
            (220, 220, 220),
            &EditingUi {
                selection: Some((2, 4)),
                selection_bg: (255, 0, 0),
                ..Default::default()
            },
        );
        assert!(!img.pixels().any(|p| p.0 == [255, 0, 0, 255]));
    }

    #[test]
    fn hits_map_the_edges_to_the_ends_and_stay_monotonic() {
        let mut r = Rasterizer::new();
        let s = spec("Hello");
        assert_eq!(r.heading_hit(&s, -5.0, 30.0), 0);
        assert_eq!(r.heading_hit(&s, 399.0, 30.0), 5);
        let mut last = 0;
        for x in (0..400).step_by(20) {
            let hit = r.heading_hit(&s, x as f32, 30.0);
            assert!(hit >= last, "hit went backwards at x={x}");
            assert!(s.text.is_char_boundary(hit));
            last = hit;
        }
    }

    /// The box holds exactly one layout line, so a title that wraps would lose
    /// its tail from the raster — the fits check is what routes it back to
    /// ordinary text.
    #[test]
    fn an_overflowing_title_reports_that_it_does_not_fit() {
        let mut r = Rasterizer::new();
        assert!(r.heading_fits(&spec("Short")));
        assert!(!r.heading_fits(&spec(
            "A rather long chapter title that certainly cannot fit on one line"
        )));
        assert!(r.heading_fits(&spec("")), "empty always fits");
    }
}
