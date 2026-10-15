//! The faces the editor draws with, and the one measurement everything else
//! rides: the advance of a character in a face at a size.
//!
//! Ply measures and draws text through macroquad, and macroquad's
//! `draw_text_ex` places each glyph at the running sum of the advances before
//! it — no kerning, no shaping (macroquad/src/text.rs, `Font::measure_text`).
//! So the x of the i-th character of a run is *exactly* the sum of the first i
//! advances, measured one character at a time. That is what lets this spike
//! hit-test and place a caret without Ply exposing per-character geometry.

use std::collections::HashMap;

use ply_engine::prelude::*;
use ply_engine::renderer::{FONT_MANAGER, FontAsset, FontManager};

/// Which of the five faces a run is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
}

impl Face {
    pub const ALL: [Face; 5] = [
        Face::Regular,
        Face::Bold,
        Face::Italic,
        Face::BoldItalic,
        Face::Mono,
    ];

    pub fn styled(bold: bool, italic: bool) -> Face {
        match (bold, italic) {
            (false, false) => Face::Regular,
            (true, false) => Face::Bold,
            (false, true) => Face::Italic,
            (true, true) => Face::BoldItalic,
        }
    }
}

/// Candidate files per face, first hit wins. A spike does not ship fonts, so it
/// borrows what the system has: Liberation Sans (metric-compatible with Arial,
/// and it has all four styles) and DejaVu Sans Mono on Linux, Arial and Menlo on
/// macOS. `LEAF_PLY_FONT_DIR` overrides with a directory holding
/// `Regular.ttf`, `Bold.ttf`, `Italic.ttf`, `BoldItalic.ttf` and `Mono.ttf`.
fn candidates(face: Face) -> Vec<String> {
    let name = match face {
        Face::Regular => "Regular",
        Face::Bold => "Bold",
        Face::Italic => "Italic",
        Face::BoldItalic => "BoldItalic",
        Face::Mono => "Mono",
    };
    let mut out = Vec::new();
    if let Ok(dir) = std::env::var("LEAF_PLY_FONT_DIR") {
        out.push(format!("{dir}/{name}.ttf"));
    }
    let lib = "/usr/share/fonts/truetype/liberation";
    let dejavu = "/usr/share/fonts/truetype/dejavu";
    let mac = "/System/Library/Fonts/Supplemental";
    out.extend(match face {
        Face::Regular => vec![
            format!("{lib}/LiberationSans-Regular.ttf"),
            format!("{dejavu}/DejaVuSans.ttf"),
            format!("{mac}/Arial.ttf"),
        ],
        Face::Bold => vec![
            format!("{lib}/LiberationSans-Bold.ttf"),
            format!("{dejavu}/DejaVuSans-Bold.ttf"),
            format!("{mac}/Arial Bold.ttf"),
        ],
        Face::Italic => vec![
            format!("{lib}/LiberationSans-Italic.ttf"),
            format!("{dejavu}/DejaVuSans-Oblique.ttf"),
            format!("{mac}/Arial Italic.ttf"),
        ],
        Face::BoldItalic => vec![
            format!("{lib}/LiberationSans-BoldItalic.ttf"),
            format!("{dejavu}/DejaVuSans-BoldOblique.ttf"),
            format!("{mac}/Arial Bold Italic.ttf"),
        ],
        Face::Mono => vec![
            format!("{dejavu}/DejaVuSansMono.ttf"),
            format!("{lib}/LiberationMono-Regular.ttf"),
            "/System/Library/Fonts/Menlo.ttc".to_string(),
        ],
    });
    out
}

/// Ply wants a `&'static FontAsset` with `&'static` bytes — the shape
/// `font!("…")` produces from `include_bytes!`. A font read at run time gets
/// there by leaking it, once per face for the life of the process.
fn load(face: Face) -> &'static FontAsset {
    for path in candidates(face) {
        if let Ok(bytes) = std::fs::read(&path) {
            let file_name: &'static str = Box::leak(path.into_boxed_str());
            let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
            return Box::leak(Box::new(FontAsset::Bytes { file_name, data }));
        }
    }
    panic!("no font found for {face:?}; set LEAF_PLY_FONT_DIR (see fonts.rs)");
}

/// The metrics Ply positions a text element's glyphs with. Ply draws a text
/// element's baseline at `top + offset_y`, where `offset_y` is what macroquad
/// measures for "Xig" in that face and size (renderer.rs, `FontManager::metrics`,
/// which is private — hence measured again here, the same way).
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    /// From a text element's top to its baseline.
    pub ascent: f32,
    /// The text element's height.
    pub height: f32,
}

pub struct Fonts {
    assets: HashMap<Face, &'static FontAsset>,
    advances: HashMap<(Face, u16, char), f32>,
    metrics: HashMap<(Face, u16), Metrics>,
}

impl Fonts {
    pub async fn load() -> Fonts {
        let mut assets = HashMap::new();
        for face in Face::ALL {
            let asset = load(face);
            FontManager::ensure(asset).await;
            assets.insert(face, asset);
        }
        Fonts {
            assets,
            advances: HashMap::new(),
            metrics: HashMap::new(),
        }
    }

    pub fn asset(&self, face: Face) -> &'static FontAsset {
        self.assets[&face]
    }

    /// Ply's font cache evicts fonts it has not drawn with lately; a face the
    /// editor measures has to be resident, so every frame re-asserts all five.
    pub async fn keep_loaded(&self) {
        for asset in self.assets.values() {
            FontManager::ensure(asset).await;
        }
    }

    fn measure(&self, face: Face, size: u16, text: &str) -> TextDimensions {
        let mut fm = FONT_MANAGER.lock().unwrap();
        let font = fm.get(self.assets[&face]);
        measure_text(text, font, size, 1.0)
    }

    pub fn advance(&mut self, face: Face, size: u16, ch: char) -> f32 {
        if let Some(&a) = self.advances.get(&(face, size, ch)) {
            return a;
        }
        let mut buf = [0u8; 4];
        let a = self.measure(face, size, ch.encode_utf8(&mut buf)).width;
        self.advances.insert((face, size, ch), a);
        a
    }

    pub fn metrics(&mut self, face: Face, size: u16) -> Metrics {
        if let Some(&m) = self.metrics.get(&(face, size)) {
            return m;
        }
        let d = self.measure(face, size, "Xig");
        let m = Metrics {
            ascent: d.offset_y,
            height: d.height,
        };
        self.metrics.insert((face, size), m);
        m
    }

    /// The width Ply will give a text element of this run — what the layout
    /// pass measures, for the check that our per-character sum agrees with it.
    pub fn run_width(&self, face: Face, size: u16, text: &str) -> f32 {
        self.measure(face, size, text).width
    }
}
