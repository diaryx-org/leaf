//! Pictures, formulas and diagrams, painted with the terminal's graphics
//! protocol over the cells of the box Blitz laid out for them.
//!
//! The painter finds each `img` and records its box in cells
//! ([`crate::paint::Picture`]); it has already drawn a labelled frame there,
//! which stays when the picture cannot be read. Here each source is decoded
//! once — a `file://` URL through leaf-raster, as leaf-tui reads a picture,
//! and a `data:image/svg+xml` URL (a formula or a diagram, which leaf-dioxus
//! typesets to SVG) rasterised — and handed to ratatui-image, which speaks
//! kitty, iTerm2 or sixel where the terminal does and half-blocks where it
//! does not.
//!
//! A picture partly scrolled off the screen is cropped to its rows in view,
//! so it scrolls with the text instead of jumping whole to stay inside it.

use std::collections::HashMap;
use std::path::PathBuf;

use leaf_raster::image::DynamicImage;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Clear;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, StatefulImage};

use crate::paint::{Grid, Picture};

/// How many encoded crops to keep before starting again: each scroll step of
/// a partly visible picture is a new crop.
const PROTOCOLS_MAX: usize = 64;

/// How many pixels an SVG is drawn at per CSS pixel: a terminal's cell is
/// about twice the size of the cell the editor measures in.
const SVG_SCALE: f32 = 2.0;

pub struct Pictures {
    picker: Picker,
    /// Each source, decoded, or `None` when it would not decode.
    decoded: HashMap<String, Option<DynamicImage>>,
    /// Each crop of a source, encoded for the terminal: keyed by the source,
    /// the first row in view and how many rows are.
    encoded: HashMap<(String, u16, u16), StatefulProtocol>,
}

impl Pictures {
    /// Half-blocks until [`Pictures::query`] has asked the terminal.
    pub fn new() -> Self {
        Self {
            picker: Picker::halfblocks(),
            decoded: HashMap::new(),
            encoded: HashMap::new(),
        }
    }

    /// Ask the terminal what it can draw. In raw mode: the answers are
    /// escape sequences on stdin.
    pub fn query(&mut self) {
        if let Ok(picker) = Picker::from_query_stdio() {
            self.picker = picker;
        }
    }

    pub fn protocol(&self) -> ProtocolType {
        self.picker.protocol_type()
    }

    /// Paint every picture in `grid` that decodes over its cells.
    pub fn paint(&mut self, f: &mut Frame, grid: &Grid) {
        if self.encoded.len() > PROTOCOLS_MAX {
            self.encoded.clear();
        }
        for p in &grid.pictures {
            self.paint_one(f, grid, p);
        }
    }

    fn paint_one(&mut self, f: &mut Frame, grid: &Grid, p: &Picture) {
        let (top, bottom) = p.shown;
        let (left, right) = (p.col.max(0), (p.col + p.cols as i32).min(grid.cols as i32));
        if top >= bottom || left >= right {
            return;
        }
        let Some(img) = self
            .decoded
            .entry(p.src.clone())
            .or_insert_with(|| decode(&p.src))
            .as_ref()
        else {
            return;
        };
        let skip = (top - p.row) as u16;
        let shown = (bottom - top) as u16;
        let key = (p.src.clone(), skip, shown);
        let picker = &self.picker;
        let proto = self.encoded.entry(key).or_insert_with(|| {
            let img = if shown == p.rows {
                img.clone()
            } else {
                let h = img.height() as f32;
                let y0 = (h * skip as f32 / p.rows as f32).round() as u32;
                let y1 = (h * (skip + shown) as f32 / p.rows as f32).round() as u32;
                img.crop_imm(0, y0, img.width(), (y1 - y0).max(1))
            };
            picker.new_resize_protocol(img)
        });
        let rect = Rect::new(left as u16, top as u16, (right - left) as u16, shown);
        f.render_widget(Clear, rect);
        f.render_stateful_widget(StatefulImage::new().resize(Resize::Fit(None)), rect, proto);
    }
}

/// A picture's source as pixels: a `file://` URL, or an SVG `data:` URL.
fn decode(src: &str) -> Option<DynamicImage> {
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
