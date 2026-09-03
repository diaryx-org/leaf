//! Block-level images, painted with the terminal's own graphics protocol.
//!
//! leaf-core lays a block image out as a run of visual rows — a label row plus
//! blank filler rows it reserves once we tell it how tall the picture is (see
//! [`leaf_core::Doc::set_media_rows`]). This module is the terminal end of that:
//! it decodes each image, measures how many character rows the fitted picture
//! needs, and paints the raster over the reserved rows with
//! [`ratatui_image`], which speaks kitty / iTerm2 / sixel where the terminal
//! supports them and falls back to unicode half-blocks where it doesn't.
//!
//! The height has to come from here, not core: core does no I/O, so it can't
//! open the file to learn the aspect ratio. We decode once, cache the decoded
//! raster keyed by resolved path, and hand core the row counts each frame; a
//! frame that measures the same images it did last time is a no-op on both sides.
//!
//! The decoding, the fit policy, and the heading rasterization itself live in
//! [`leaf_raster`], the pixel layer shared across frontends; this module owns
//! only what is terminal-shaped — cells, the protocol picker, and the caches.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ratatui::style::Color as TerminalColor;
use ratatui::{Frame, layout::Rect, widgets::Clear};
use ratatui_image::{
    FontSize, Resize, StatefulImage,
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};

use leaf_raster::{EditingUi, HeadingSpec, Rasterizer, image, resolve_image_path};

use leaf_core::{ColorScheme, MediaInfo};

use crate::style::detect_color_scheme;

/// The most rows a single image may reserve, so one tall picture can't push a
/// whole screen of text out of view. Mirrors the GUI's `IMAGE_MAX_H` pixel cap,
/// expressed in the terminal's only vertical unit.
const MAX_IMAGE_ROWS: usize = 30;

/// How many composed heading rasters to hold before the cache is emptied and
/// rebuilt from what's on screen. One entry per distinct heading is the steady
/// state — editing a heading *replaces* its entry rather than growing the map
/// (see [`HeadingEntry`]) — so the cap only bounds a session that scrolls
/// through many headings, and a clear costs one re-raster per visible one.
const HEADING_CACHE_MAX: usize = 64;

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
    /// Composed heading raster cache: one slot per distinct heading (text,
    /// geometry, ink), each holding the raster for whatever editing UI was
    /// last baked into it. A caret move re-renders *that* heading's slot and
    /// leaves the others' encoded protocol images untouched. Bounded by
    /// [`HEADING_CACHE_MAX`].
    headings: HashMap<HeadingKey, HeadingEntry>,
    /// The shared pixel layer: font database, glyph cache, and the heading
    /// raster/hit-test pair, kept together so a click is answered by exactly
    /// the layout that was drawn.
    raster: Rasterizer,
    /// The terminal's color scheme, used to pick a `<picture>`'s
    /// `prefers-color-scheme` `<source>` (see [`MediaInfo::resolve`]). Detected
    /// once at startup and refreshable via [`Images::set_color_scheme`]; the
    /// per-path cache keys off the *resolved* file, so a scheme change naturally
    /// loads (and caches) the newly-picked image without disturbing the old one.
    scheme: ColorScheme,
    /// The terminal's own text color, when it has been asked (see
    /// [`crate::EditorState::set_foreground`]). Used to resolve a
    /// [`TerminalColor::Reset`] heading to real pixels; `None` falls back to
    /// inferring it from [`Images::scheme`].
    foreground: Option<(u8, u8, u8)>,
}

impl Default for Images {
    /// A half-blocks picker with no terminal query — the safe default before
    /// [`Images::query`] has probed the real terminal (and the permanent state on
    /// a terminal that has no graphics protocol at all). The color scheme is
    /// sniffed from the environment (see [`detect_color_scheme`]); the accurate
    /// answer arrives via [`Images::set_color_scheme`], which
    /// [`crate::EditorState::query_color_scheme`] calls once the terminal has
    /// been asked directly.
    fn default() -> Self {
        Images {
            picker: Picker::halfblocks(),
            cache: HashMap::new(),
            headings: HashMap::new(),
            raster: Rasterizer::new(),
            scheme: detect_color_scheme(),
            foreground: None,
        }
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

    /// Whether the terminal speaks a real graphics protocol — kitty, iTerm2 or
    /// sixel — rather than ratatui-image's unicode half-block fallback.
    ///
    /// The two rasterized surfaces want opposite answers to that. A photograph
    /// in half-blocks is coarse but still recognisably the photograph, so block
    /// images take the fallback and stay worth drawing. An oversized heading is
    /// *text we rasterized ourselves*: in half-blocks it comes back as a mosaic
    /// caricature of letters the terminal could have drawn properly, so there
    /// the ordinary bold coloured heading is the better rendering. [`crate::render`]
    /// asks this before expanding a heading at all.
    pub fn supports_graphics(&self) -> bool {
        self.picker.protocol_type() != ProtocolType::Halfblocks
    }

    /// Claim a graphics protocol without a terminal to ask. Everything about
    /// oversized headings — the filler rows, the raster, the caret that lives in
    /// its pixels — is gated on [`supports_graphics`](Self::supports_graphics),
    /// so a test that draws to a `TestBackend` sees none of it otherwise, and
    /// the layout bugs that live there are exactly the ones no test could reach.
    #[cfg(test)]
    pub(crate) fn assume_graphics(&mut self) {
        // Only the protocol changes: the picker keeps whatever font size it was
        // built with, which for the half-blocks default is the nominal cell the
        // box arithmetic wants and a `TestBackend` has no opinion about anyway.
        self.picker.set_protocol_type(ProtocolType::Kitty);
    }

    /// Override the detected color scheme — what a host wires to a config option
    /// or a `prefers-color-scheme`-style toggle. Re-picking is automatic: the
    /// cache keys off the resolved file, so the next frame measures and paints
    /// whichever `<source>` the new scheme selects.
    pub fn set_color_scheme(&mut self, scheme: ColorScheme) {
        self.scheme = scheme;
    }

    /// Record the terminal's own text color, for resolving a `Reset` heading to
    /// pixels. `None` goes back to inferring it from the scheme. The heading
    /// cache keys off the resolved RGB, so a change here re-rasters on the next
    /// frame without anything having to clear it.
    pub fn set_foreground(&mut self, rgb: Option<(u8, u8, u8)>) {
        self.foreground = rgb;
    }

    /// Decode (once) and measure every block image, returning the row count each
    /// one reserves keyed by destination — exactly the map
    /// [`leaf_core::Doc::set_media_rows`] wants. A destination that doesn't
    /// resolve to a loadable local file is left out, so core keeps its bare
    /// one-row placeholder for it. `avail_cols` is the content width the picture
    /// may fill.
    pub fn reserve(
        &mut self,
        images: &[MediaInfo],
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
            // No still to draw (audio, or a video with no poster): leave core's
            // labelled placeholder row and reserve nothing extra for it.
            let Some(still) = info.still(self.scheme) else {
                continue;
            };
            let Some(path) = resolve_image_path(still, doc_dir) else {
                continue;
            };
            let Some(entry) = self.entry(&path) else {
                continue;
            };
            let cells = box_cells(entry.intrinsic, inner_cols, inner_rows, font);
            entry.box_cells = cells;
            heights.insert(info.destination.clone(), cells.1 as usize);
        }
        heights
    }

    /// The character-cell size `(cols, rows)` of the picture inside its border —
    /// what `ui` sizes the box to and reserves the rows for. `None` for an image
    /// that isn't a loadable local file (so `ui` frames it as a bare placeholder).
    pub fn picture_cells(&self, info: &MediaInfo, doc_dir: Option<&Path>) -> Option<(u16, u16)> {
        let path = resolve_image_path(info.still(self.scheme)?, doc_dir)?;
        self.cache
            .get(&path)
            .and_then(|e| e.as_ref())
            .map(|e| e.box_cells)
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
        info: &MediaInfo,
        doc_dir: Option<&Path>,
        rect: Rect,
    ) -> bool {
        let Some(still) = info.still(self.scheme) else {
            return false;
        };
        let Some(path) = resolve_image_path(still, doc_dir) else {
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

    /// Shape and paint an H1/H2 as a terminal graphics image, returning whether
    /// anything was actually painted. The source remains ordinary Leaf text;
    /// this is only its composed projection — but it is an *editable*
    /// projection: the caret and selection, given as byte offsets into `text`,
    /// are painted into the pixels, because nothing drawn from cells can land
    /// on top of a graphics-protocol image.
    ///
    /// A `false` no-op without a graphics protocol (see
    /// [`Images::supports_graphics`]): callers already skip the expansion in
    /// that case, and repeating the rule here means a caller that forgets
    /// leaves the terminal's own heading text standing rather than painting
    /// half-blocks over it. The caller uses the return to know whether the
    /// heading's editing UI now lives in the raster — a skipped paint keeps
    /// the ordinary text and the real terminal caret.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_heading(
        &mut self,
        f: &mut Frame,
        text: &str,
        level: u8,
        color: TerminalColor,
        rect: Rect,
        caret: Option<usize>,
        selection: Option<(usize, usize)>,
    ) -> bool {
        if !self.supports_graphics() || rect.width == 0 || rect.height == 0 || text.is_empty() {
            return false;
        }
        let font = self.picker.font_size();
        let rgb = terminal_rgb(color, self.scheme, self.foreground);
        let key = HeadingKey {
            text: text.to_owned(),
            level,
            cells: (rect.width, rect.height),
            font: (font.0, font.1),
            rgb,
        };
        let ui = (caret, selection);
        if self.headings.get(&key).is_none_or(|h| h.ui != ui) {
            // Only a heading never seen before grows the map — an edited one
            // replaces its own slot — so the cap trips only when many distinct
            // headings have scrolled by, and everything still on screen
            // re-rasters on the very next frame.
            if !self.headings.contains_key(&key) && self.headings.len() >= HEADING_CACHE_MAX {
                self.headings.clear();
            }
            let spec = heading_spec(&key.text, level, key.cells, font);
            let editing = EditingUi {
                caret,
                selection,
                // The terminal's selection is reverse video: the heading's own
                // ink becomes the fill, and the glyphs on it take whichever
                // ink reads on that fill — decided by the fill itself, not the
                // scheme, so an amber heading on a light palette still gets
                // dark selected glyphs (and so the choice can never go stale
                // in the cache: it is a pure function of `rgb`, which is in
                // the key).
                selection_bg: rgb,
                selection_fg: selection_ink(rgb),
            };
            let image = self.raster.heading(&spec, rgb, &editing);
            let intrinsic = (image.width(), image.height());
            self.headings.insert(
                key.clone(),
                HeadingEntry {
                    ui,
                    entry: Entry {
                        protocol: self
                            .picker
                            .new_resize_protocol(image::DynamicImage::ImageRgba8(image)),
                        intrinsic,
                        box_cells: (rect.width, rect.height),
                    },
                },
            );
        }
        let Some(heading) = self.headings.get_mut(&key) else {
            return false;
        };
        f.render_widget(Clear, rect);
        f.render_stateful_widget(
            StatefulImage::new().resize(Resize::Fit(None)),
            rect,
            &mut heading.entry.protocol,
        );
        true
    }

    /// The character index (into `text`) a click on a painted heading raster
    /// lands on. `cells` is the box the raster was painted into and `pos` the
    /// clicked cell within it; the answer comes from the same layout the raster
    /// was drawn from, so the caret goes to the glyph the pointer visually hit
    /// — the rasterized glyphs are far wider than character cells, and mapping
    /// the click through the cell grid instead would land it half a title away.
    pub fn heading_hit(
        &mut self,
        text: &str,
        level: u8,
        cells: (u16, u16),
        pos: (u16, u16),
    ) -> usize {
        let font = self.picker.font_size();
        let spec = heading_spec(text, level, cells, font);
        // The middle of the clicked cell, in raster pixels.
        let x = (pos.0 as f32 + 0.5) * font.0.max(1) as f32;
        let y = (pos.1 as f32 + 0.5) * font.1.max(1) as f32;
        let byte = self.raster.heading_hit(&spec, x, y).min(text.len());
        text[..byte].chars().count()
    }

    /// Whether `text` lays out on the one line a rasterized heading of `level`
    /// gets in a box of `cells`. `false` routes the heading back to ordinary
    /// terminal text: a wrapped tail would be culled from the raster —
    /// invisible, unclickable, and with no truthful place for its caret.
    pub fn heading_fits(&mut self, text: &str, level: u8, cells: (u16, u16)) -> bool {
        let font = self.picker.font_size();
        let spec = heading_spec(text, level, cells, font);
        self.raster.heading_fits(&spec)
    }

    /// The cache entry for a resolved path, decoding it on first use. `None` (and
    /// a cached `None`) when the file can't be read or decoded.
    fn entry(&mut self, path: &Path) -> Option<&mut Entry> {
        if !self.cache.contains_key(path) {
            let decoded = leaf_raster::load_image(path).map(|img| {
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

/// What identifies a composed heading: its text, geometry, and ink. The
/// editing UI is deliberately *not* here — a caret stop is a different picture
/// but the same heading, so it lives on the entry and replaces in place
/// ([`HeadingEntry`]) instead of minting a cache entry per keystroke.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct HeadingKey {
    text: String,
    level: u8,
    /// The character-cell box the heading occupies, `(cols, rows)`.
    cells: (u16, u16),
    /// The terminal's cell size in pixels, `(width, height)`.
    font: (u16, u16),
    rgb: (u8, u8, u8),
}

/// A heading's cached raster plus the editing UI baked into its pixels — the
/// caret byte offset and selected byte range the raster was drawn with. When
/// the frame wants a different pair the slot is re-rendered and replaced.
struct HeadingEntry {
    ui: (Option<usize>, Option<(usize, usize)>),
    entry: Entry,
}

/// The ink for glyphs standing on a selection fill. The fill is the heading's
/// own color (reverse video), so the ink is picked against *it* by relative
/// luminance — the same split [`crate::style`]'s `contrast_ink` makes for
/// host-colored highlights, and for the same reason: a scheme-based choice
/// misreads whenever a color's luminance disagrees with its scheme (the light
/// palette's amber H2 is a light fill wanting dark glyphs).
fn selection_ink((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    let luminance = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
    if luminance > 0.5 * 255.0 {
        (20, 20, 20)
    } else {
        (250, 250, 250)
    }
}

/// The [`HeadingSpec`] for a heading painted into a cell box — the one place
/// cells become pixels, used by both the raster and the hit-test so the two
/// cannot disagree about the layout.
fn heading_spec<'a>(
    text: &'a str,
    level: u8,
    cells: (u16, u16),
    font: FontSize,
) -> HeadingSpec<'a> {
    HeadingSpec {
        text,
        level,
        width_px: u32::from(cells.0) * u32::from(font.0.max(1)),
        height_px: u32::from(cells.1) * u32::from(font.1.max(1)),
    }
}

fn terminal_rgb(
    color: TerminalColor,
    scheme: ColorScheme,
    foreground: Option<(u8, u8, u8)>,
) -> (u8, u8, u8) {
    match color {
        TerminalColor::Rgb(r, g, b) => (r, g, b),
        TerminalColor::Black => (0, 0, 0),
        TerminalColor::White => (255, 255, 255),
        TerminalColor::Red | TerminalColor::LightRed => (235, 95, 95),
        TerminalColor::Green | TerminalColor::LightGreen => (100, 210, 140),
        TerminalColor::Blue | TerminalColor::LightBlue => (100, 160, 240),
        TerminalColor::Cyan | TerminalColor::LightCyan => (80, 205, 215),
        TerminalColor::Magenta | TerminalColor::LightMagenta => (205, 130, 225),
        TerminalColor::Yellow | TerminalColor::LightYellow => (220, 165, 55),
        TerminalColor::Gray | TerminalColor::DarkGray => (145, 145, 145),
        // `Reset` means "whatever ink this terminal draws text in" — the color a
        // plain heading has to match exactly, since it sits in a raster among
        // cells the terminal painted itself. So the measured answer is used when
        // there is one, and only an unasked or unanswering terminal falls back to
        // guessing from the scheme.
        TerminalColor::Reset => foreground.unwrap_or(match scheme {
            ColorScheme::Dark => (235, 235, 235),
            ColorScheme::Light => (35, 35, 35),
        }),
        // Indexed terminal colors cannot be queried portably, and unlike `Reset`
        // there is nothing measured to fall back on: slot 94 is whatever this
        // terminal decided it is. The curated Leaf palettes use RGB; this is a
        // legible fallback for a custom indexed one.
        TerminalColor::Indexed(_) => match scheme {
            ColorScheme::Dark => (235, 235, 235),
            ColorScheme::Light => (35, 35, 35),
        },
    }
}

/// The character-cell box an image fits into: as wide as the content allows (but
/// never upscaled past the source's own pixels) and as tall as that width makes
/// it, capped at `max_rows`. The policy is [`leaf_raster::fit_within`], run in
/// pixels so the terminal's non-square cells (`font`) don't distort the aspect
/// ratio; only the round-up to whole cells is the terminal's own.
fn box_cells(intrinsic: (u32, u32), avail_cols: u16, max_rows: u16, font: FontSize) -> (u16, u16) {
    // `FontSize` is `(cell_width_px, cell_height_px)`.
    let (cw, ch) = (u32::from(font.0.max(1)), u32::from(font.1.max(1)));
    let (w_px, h_px) = leaf_raster::fit_within(
        intrinsic,
        u32::from(avail_cols.max(1)) * cw,
        u32::from(max_rows.max(1)) * ch,
    );
    let cols = w_px.div_ceil(cw).clamp(1, u32::from(avail_cols.max(1))) as u16;
    let rows = h_px.div_ceil(ch).clamp(1, u32::from(max_rows.max(1))) as u16;
    (cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> FontSize {
        (10, 20) // 10px wide, 20px tall cells
    }

    /// The point of asking the terminal for its foreground: a plain heading is
    /// drawn in pixels next to cells the terminal painted, so `Reset` has to
    /// resolve to the terminal's *real* ink, not to a plausible near-white.
    #[test]
    fn a_measured_foreground_beats_the_guess_for_reset() {
        let ink = Some((198, 160, 246));
        assert_eq!(
            terminal_rgb(TerminalColor::Reset, ColorScheme::Dark, ink),
            (198, 160, 246)
        );
        // …and it is the scheme's guess that it replaces, in either scheme.
        assert_eq!(
            terminal_rgb(TerminalColor::Reset, ColorScheme::Light, ink),
            (198, 160, 246)
        );
    }

    /// An unasked or unanswering terminal keeps the old scheme-derived ink,
    /// which is why `None` is a supported state rather than a bug.
    #[test]
    fn without_a_measurement_reset_falls_back_to_the_scheme() {
        assert_eq!(
            terminal_rgb(TerminalColor::Reset, ColorScheme::Dark, None),
            (235, 235, 235)
        );
        assert_eq!(
            terminal_rgb(TerminalColor::Reset, ColorScheme::Light, None),
            (35, 35, 35)
        );
    }

    /// Only `Reset` means "the terminal's text color". An indexed slot is a
    /// different question with no measured answer, so it must not pick up the
    /// foreground.
    #[test]
    fn an_indexed_color_is_not_the_foreground() {
        let ink = Some((198, 160, 246));
        assert_eq!(
            terminal_rgb(TerminalColor::Indexed(94), ColorScheme::Dark, ink),
            (235, 235, 235)
        );
        // A named color keeps its curated legible value too.
        assert_eq!(
            terminal_rgb(TerminalColor::Red, ColorScheme::Dark, ink),
            (235, 95, 95)
        );
    }

    #[test]
    fn box_cells_fits_width_and_preserves_aspect() {
        // A 200×100px image (2:1) into a 40-col space with 10×20px cells: 40 cols
        // is 400px, wider than the image, so it isn't upscaled — it stays 200px =
        // 20 cols wide, 100px = 5 rows tall.
        assert_eq!(
            box_cells((200, 100), 40, MAX_IMAGE_ROWS as u16, font()),
            (20, 5)
        );
    }

    #[test]
    fn box_cells_scales_down_to_the_available_width() {
        // An 800×400px image into the same 40-col (400px) space: scaled to 400px
        // wide (40 cols), 200px tall (10 rows).
        assert_eq!(
            box_cells((800, 400), 40, MAX_IMAGE_ROWS as u16, font()),
            (40, 10)
        );
    }

    #[test]
    fn box_cells_caps_height_and_keeps_aspect() {
        // A skinny 100×4000px image would want 200 rows; a small row cap holds it
        // and shrinks the width to keep the aspect ratio.
        let (cols, rows) = box_cells((100, 4000), 40, 8, font());
        assert_eq!(rows, 8, "height is held to the cap");
        assert!(
            (1..40).contains(&cols),
            "width shrinks with the capped height: {cols}"
        );
    }
}
