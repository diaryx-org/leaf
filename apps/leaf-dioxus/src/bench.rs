//! Where a refresh of the pages goes: each stage between a keystroke and
//! the pages drawn again, timed on documents of growing size, reflowed and
//! on paper.
//!
//! ```sh
//! cargo test --release -p leaf-dioxus-app bench -- --ignored --nocapture
//! ```

use std::path::Path;
use std::time::Instant;

use leaf_core::Format;

use crate::pdf::{Paper, Sheet, Type};

/// A document of about `bytes`, of every kind of block leaf writes: what a
/// long, complex document is made of.
pub fn document(bytes: usize) -> String {
    let mut s =
        String::from("---\ntitle: A long document\nauthor: Ada\n---\n\n# A long document\n\n");
    let mut i = 0;
    while s.len() < bytes {
        s.push_str(&format!(
            "## Section {i}\n\n\
             The quick brown fox jumps over the lazy dog, with *emphasis*, **strong** \
             text, a [link](https://example.dev/{i}) and `code` besides, and a note[^n{i}]. \
             Another sentence follows to make the paragraph a realistic length, and \
             one more after it so that it wraps across several lines at any width.\n\n\
             - A first item, short\n- A second item, with **bold** in it\n  - nested under it\n\n\
             | Name | Qty | Price |\n|------|----:|------:|\n| Pears | {i} | £3 |\n| Plums | 12 | £4 |\n\n\
             ```rust\nfn section_{i}() -> usize {{\n    {i}\n}}\n```\n\n\
             > A quotation in section {i}, set apart from the text around it.\n\n\
             [^n{i}]: The note to section {i}.\n\n"
        ));
        i += 1;
    }
    s
}

struct Clock(Instant);

impl Clock {
    fn lap(&mut self, label: &str, out: &mut Vec<(String, f64)>) {
        let ms = self.0.elapsed().as_secs_f64() * 1000.0;
        out.push((label.to_owned(), ms));
        self.0 = Instant::now();
    }
}

/// One refresh of `source` set in `ty` on `sheet`, stage by stage.
fn refresh(
    source: &str,
    ty: &Type,
    sheet: Sheet,
    known: &mut std::collections::HashSet<u128>,
) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut c = Clock(Instant::now());
    let tree = leaf_core::parse_tree(source, Format::Markdown).unwrap();
    c.lap("parse the source", &mut out);
    let cfg = crate::pdf::config(ty, sheet);
    let words = twig_export::render_words(tree, source, &cfg).unwrap();
    c.lap("render to Typst", &mut out);
    let layout = crate::typeset::here_words(&words, Path::new(".")).unwrap();
    c.lap("Typst: lay out", &mut out);
    let tiles = layout.tiles();
    c.lap("cut into pieces", &mut out);
    let svg: Vec<String> = tiles
        .iter()
        .filter(|t| !known.contains(&t.key))
        .map(|t| layout.tile_svg(t))
        .collect();
    c.lap("SVG of new pieces", &mut out);
    let pages = layout.pages();
    c.lap("glyphs of every page", &mut out);
    let urls: Vec<String> = svg.iter().map(|s| crate::preview::data_url(s)).collect();
    c.lap("data: URLs", &mut out);
    let geometry = crate::pages::Geometry::new(
        &pages
            .iter()
            .map(|p| (p.height, p.glyphs.clone()))
            .collect::<Vec<_>>(),
    );
    c.lap("caret geometry", &mut out);
    std::hint::black_box((urls, geometry));
    out.push((
        format!(
            "  ({} pages, {} pieces, {} drawn anew, {} KB of SVG, {} glyphs)",
            pages.len(),
            tiles.len(),
            svg.len(),
            svg.iter().map(String::len).sum::<usize>() / 1024,
            pages.iter().map(|p| p.glyphs.len()).sum::<usize>()
        ),
        -1.0,
    ));
    *known = tiles.iter().map(|t| t.key).collect();
    out
}

/// A refresh of the reflowed view a chunk at a time (`reflow.rs`), as the
/// app does it: the blocks asked of the document, then the chunks.
fn chunked(
    source: &str,
    ty: &Type,
    known: &mut std::collections::HashSet<u128>,
) -> (f64, f64, usize, usize) {
    let t = Instant::now();
    let mut d = leaf_core::Doc::from_source(source.to_owned(), Format::Markdown).unwrap();
    let start = Instant::now();
    let blocks = d.top_level();
    let blocks_ms = start.elapsed().as_secs_f64() * 1000.0;
    let _ = t;
    let how = crate::reflow::How {
        format: Format::Markdown,
        ty: ty.clone(),
        width: 480.0,
        root: Path::new(".").into(),
    };
    let start = Instant::now();
    let out = crate::reflow::lay_out(source, &blocks, None, &how, known).unwrap();
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let drawn = out.pieces.iter().filter(|p| p.svg.is_some()).count();
    *known = out.pieces.iter().map(|p| p.key).collect();
    (blocks_ms, ms, out.pieces.len(), drawn)
}

#[test]
#[ignore = "a benchmark: run it in release, with --ignored --nocapture"]
fn bench_a_chunked_reflow() {
    let ty = Type {
        template: crate::settings::Template::default(),
        size: 12.0,
        dark: false,
    };
    for kb in [10usize, 100, 400, 1000] {
        let base = document(kb * 1024);
        let mut known = Default::default();
        let (b0, cold, pieces, _) = chunked(&base, &ty, &mut known);
        let mid = base[..base.len() / 2].rfind(". ").unwrap() + 1;
        let mut edited = base.clone();
        edited.insert(mid, 'x');
        let (b1, warm, _, drawn) = chunked(&edited, &ty, &mut known);
        edited.insert(mid + 1, 'y');
        let (_, warm2, _, _) = chunked(&edited, &ty, &mut known);
        println!(
            "=== {kb} KB, reflowed by chunks: {cold:.1} ms cold, {warm:.1} ms and {warm2:.1} ms after an edit \
             ({pieces} pieces, {drawn} drawn anew; the blocks asked {b0:.1} / {b1:.1} ms)"
        );
    }
}

#[test]
#[ignore = "a benchmark: run it in release, with --ignored --nocapture"]
fn bench_a_refresh() {
    let ty = Type {
        template: crate::settings::Template::default(),
        size: 12.0,
        dark: false,
    };
    let sheets = [
        ("reflowed", Sheet::Reflow { width: 480.0 }),
        (
            "on paper",
            Sheet::Paper(Paper {
                width: 612.0,
                height: 792.0,
                margin: 72.0,
            }),
        ),
    ];
    for kb in [10usize, 100, 400] {
        let base = document(kb * 1024);
        for (name, sheet) in sheets {
            println!("=== {kb} KB, {name} ===");
            // Cold, then a letter typed in the middle, as typing does: what
            // Typst's memory of the last setting saves.
            let mut known = Default::default();
            let cold = refresh(&base, &ty, sheet, &mut known);
            let mid = base[..base.len() / 2].rfind(". ").unwrap() + 1;
            let mut edited = base.clone();
            edited.insert(mid, 'x');
            let warm = refresh(&edited, &ty, sheet, &mut known);
            let mut total = (0.0, 0.0);
            for ((label, a), (_, b)) in cold.iter().zip(&warm) {
                if *a < 0.0 {
                    println!("{label}");
                    continue;
                }
                total.0 += a;
                total.1 += b;
                println!("  {label:<24}{a:9.1} ms cold {b:9.1} ms after an edit");
            }
            println!(
                "  {:<24}{:9.1} ms cold {:9.1} ms after an edit",
                "total", total.0, total.1
            );
        }
    }
}
