//! The page view and the PDF, drawn side by side and compared, page by page.
//!
//! The page view draws a document as twig-export's pieces (`tiles`), each an
//! SVG that Blitz reads with usvg and paints where [`crate::view`] puts it on
//! a white sheet; Export as PDF writes the same layout as a PDF. What the
//! reader sees on the screen is to be what prints. So each document here is
//! set both ways, as a host sets it ([`crate::print`]), the pieces
//! painted where the view paints them (resvg, from the usvg tree Blitz
//! reads) and the PDF's pages rasterised (hayro), both at [`SCALE`] pixels
//! a point, and the two are held to the same pages, the same size, and the
//! same pixels but for the edges of glyphs, which two rasterisers smooth a
//! little differently.
//!
//!     LEAF_PARITY_OUT=/tmp/parity cargo test -p leaf-pages parity
//!
//! writes each page both ways, and where they differ, as PNGs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use leaf_core::Format;

use crate::typeset::{Job, Supply};

/// Pixels a point: a phone's screen, about.
pub const SCALE: f32 = 2.0;

/// A page drawn, as straight RGBA on white.
pub struct Drawn {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// `job` as the page view draws it: each page a white sheet, each piece's
/// SVG over it at the piece's place and the page's width, as
/// `view.rs` places an `img` (its height the picture's own).
pub fn view(job: &Job) -> Result<Vec<Drawn>, String> {
    let laid = crate::typeset::layout_here(job, &HashSet::new())?;
    // A picture is made once for a key; a second piece of it has none.
    let svgs: HashMap<u128, String> = laid
        .tiles
        .iter()
        .filter_map(|t| Some((t.key, t.svg.clone()?)))
        .collect();
    let opts = resvg::usvg::Options::default();
    let mut out = Vec::new();
    for (i, page) in laid.pages.iter().enumerate() {
        let (w, h) = px(page.width, page.height);
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("an empty page")?;
        pixmap.fill(resvg::tiny_skia::Color::WHITE);
        for t in laid.tiles.iter().filter(|t| t.page == i) {
            let svg = svgs.get(&t.key).ok_or("a piece with no picture")?;
            let tree = resvg::usvg::Tree::from_str(svg, &opts).map_err(|e| e.to_string())?;
            let s = (t.width as f32 * SCALE) / tree.size().width();
            let at =
                resvg::tiny_skia::Transform::from_row(s, 0.0, 0.0, s, 0.0, t.top as f32 * SCALE);
            resvg::render(&tree, at, &mut pixmap.as_mut());
        }
        out.push(Drawn {
            width: w,
            height: h,
            rgba: unpremultiply(pixmap.data()),
        });
    }
    Ok(out)
}

/// The PDF `bytes`, each page rasterised on white.
pub fn pdf(bytes: Vec<u8>) -> Result<Vec<Drawn>, String> {
    let data: hayro::hayro_syntax::PdfData = Arc::new(bytes).into();
    let doc = hayro::hayro_syntax::Pdf::new(data).map_err(|e| format!("{e:?}"))?;
    let pages = hayro::render_pdf(&doc, SCALE, Default::default(), None).ok_or("no pages")?;
    Ok(pages
        .into_iter()
        .map(|p| Drawn {
            width: u32::from(p.width()),
            height: u32::from(p.height()),
            rgba: unpremultiply(p.data_as_u8_slice()),
        })
        .collect())
}

fn px(width: f64, height: f64) -> (u32, u32) {
    let s = f64::from(SCALE);
    ((width * s).floor() as u32, (height * s).floor() as u32)
}

/// Premultiplied RGBA, over white, as straight RGBA.
fn unpremultiply(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for p in data.as_chunks::<4>().0 {
        let a = u32::from(p[3]);
        for &c in &p[..3] {
            // Premultiplied over white: c + (255 - a).
            out.push((u32::from(c) + 255 - a).min(255) as u8);
        }
        out.push(255);
    }
    out
}

/// How a page drawn both ways differs.
#[derive(Debug, Default)]
pub struct Differs {
    /// Pixels that differ by more than the smoothing of an edge.
    pub pixels: usize,
    /// The box they lie in, in points: left, top, right, bottom.
    pub within: Option<(f32, f32, f32, f32)>,
}

/// A pixel differs when a channel differs by more than this: past what two
/// rasterisers' smoothing of the same edge comes to.
const EDGE: u8 = 96;

/// How far a channel of a blended edge pixel may stray past the colours it
/// lies between: rounding, and a JPEG's own noise.
const BLEND: u8 = 8;

/// Where `a` and `b`, the same size, differ: a pixel of one that is far
/// from every pixel of the other at and around it. Two rasterisers may put
/// the edge of a picture or a rule on neighbouring pixels; a line set a
/// little lower, or a word in another place, is far from all of them.
pub fn differ(a: &Drawn, b: &Drawn) -> Differs {
    let (w, h) = (a.width as i64, a.height as i64);
    let at = |d: &Drawn, x: i64, y: i64| {
        let i = ((y * w + x) * 4) as usize;
        [d.rgba[i], d.rgba[i + 1], d.rgba[i + 2]]
    };
    let near = |p: [u8; 3], q: [u8; 3]| p.iter().zip(&q).all(|(x, y)| x.abs_diff(*y) <= EDGE);
    // A pixel is alone when no pixel of the other at and around it is near
    // it, and it is no blend of them either: an edge one rasteriser covers
    // a part of a pixel and the other the whole of it (a picture set at a
    // fraction of a pixel) is a mix of the colours on either side.
    let alone = |one: &Drawn, other: &Drawn, x: i64, y: i64| {
        let p = at(one, x, y);
        let (mut lo, mut hi) = ([u8::MAX; 3], [u8::MIN; 3]);
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if !((0..w).contains(&nx) && (0..h).contains(&ny)) {
                    continue;
                }
                let q = at(other, nx, ny);
                if near(p, q) {
                    return false;
                }
                for c in 0..3 {
                    lo[c] = lo[c].min(q[c]);
                    hi[c] = hi[c].max(q[c]);
                }
            }
        }
        let between = (0..3)
            .all(|c| lo[c].saturating_sub(BLEND) <= p[c] && p[c] <= hi[c].saturating_add(BLEND));
        !between
    };
    let mut d = Differs::default();
    let mut bx = (i64::MAX, i64::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if near(at(a, x, y), at(b, x, y)) {
                continue;
            }
            if alone(a, b, x, y) || alone(b, a, x, y) {
                d.pixels += 1;
                bx = (bx.0.min(x), bx.1.min(y), bx.2.max(x), bx.3.max(y));
            }
        }
    }
    if d.pixels > 0 {
        let p = |v: i64| v as f32 / SCALE;
        d.within = Some((p(bx.0), p(bx.1), p(bx.2), p(bx.3)));
    }
    d
}

/// Both ways, compared: `Err` naming every page that differs.
pub fn compare(name: &str, job: &Job, pdf_bytes: Vec<u8>) -> Result<(), String> {
    let seen = view(job)?;
    let max = std::env::var("LEAF_PARITY_TRACE").is_ok();
    let printed = pdf(pdf_bytes)?;
    let out = std::env::var_os("LEAF_PARITY_OUT").map(PathBuf::from);
    let mut wrong = Vec::new();
    if seen.len() != printed.len() {
        wrong.push(format!(
            "{name}: {} pages on screen, {} printed",
            seen.len(),
            printed.len()
        ));
    }
    for (i, (a, b)) in seen.iter().zip(&printed).enumerate() {
        if (a.width, a.height) != (b.width, b.height) {
            wrong.push(format!(
                "{name} p{}: {}×{} on screen, {}×{} printed",
                i + 1,
                a.width,
                a.height,
                b.width,
                b.height
            ));
            continue;
        }
        let d = differ(a, b);
        if let Some(dir) = &out {
            save(dir, &format!("{name}-p{}-view", i + 1), a);
            save(dir, &format!("{name}-p{}-pdf", i + 1), b);
        }
        if max {
            eprintln!(
                "{name} p{}: {} pixels differ, within {:?}",
                i + 1,
                d.pixels,
                d.within
            );
        }
        // A few stray pixels at most: a glyph's edge, a hairline's end.
        if d.pixels > 40 {
            wrong.push(format!(
                "{name} p{}: {} pixels differ, within {:?} (points)",
                i + 1,
                d.pixels,
                d.within
            ));
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(wrong.join("\n"))
    }
}

fn save(dir: &Path, name: &str, d: &Drawn) {
    let _ = std::fs::create_dir_all(dir);
    let _ = image::save_buffer(
        dir.join(format!("{name}.png")),
        &d.rgba,
        d.width,
        d.height,
        image::ColorType::Rgba8,
    );
}

/// The documents, by name: everything the page view draws.
pub fn documents() -> Vec<(&'static str, String)> {
    let mut long = String::from("# A long one\n\n");
    for i in 0..40 {
        long.push_str(&format!(
            "Paragraph {i}, long enough to wrap over a line or two and so to \
             cross a page's foot somewhere along the way, as a journal's do.\n\n"
        ));
    }
    vec![
        (
            "headings",
            "# One\n\nText.\n\n## Two\n\nText.\n\n### Three\n\nText.\n\n#### Four\n\n\
             ##### Five\n\n###### Six\n\nText under them all.\n"
                .into(),
        ),
        (
            "inline",
            "Some **bold**, some *italic*, some ***both***, `code`, a [link](https://example.com), \
             ~~struck~~, <u>underlined</u>, ==marked== and H~2~O and x^2^.\n"
                .into(),
        ),
        (
            "lists",
            "- one\n- two\n  - two a\n  - two b\n- three\n\n1. first\n2. second\n   1. nested\n3. third\n\n\
             - [ ] to do\n- [x] done\n"
                .into(),
        ),
        (
            "quote",
            "> A quote, long enough to wrap onto a second line of the page, or so it is hoped.\n>\n\
             > A second paragraph in it.\n\nAfter it.\n"
                .into(),
        ),
        (
            "blocks",
            "A rule:\n\n---\n\nA fence:\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n\
             | a | b |\n|---|---|\n| 1 | 2 |\n| three | four |\n"
                .into(),
        ),
        (
            "aligned",
            "<div style=\"text-align: center\">\n\nCentred.\n\n</div>\n\n\
             <div style=\"text-align: right\">\n\nRight.\n\n</div>\n\n\
             <div style=\"text-align: justify\">\n\nJustified text that runs long enough to wrap and \
             so to be spread from margin to margin on every line but its last.\n\n</div>\n"
                .into(),
        ),
        (
            "emoji",
            "A party 🎉, a heart ❤️ and a flag 🇵🇹 among the words.\n".into(),
        ),
        ("long", long),
        (
            "front",
            "---\ntitle: A titled page\nauthor: Someone\n---\n\nThe body, under its title.\n"
                .into(),
        ),
        (
            "pictures",
            "A small picture:\n\n![small](small.png)\n\nA large one, scaled for the screen:\n\n\
             ![large](large.jpg)\n\nAnd after.\n"
                .into(),
        ),
    ]
}

/// A folder holding the documents' pictures: a small PNG, and a JPEG
/// larger than the views draw ([`crate::shown::LONGEST`]).
pub fn pictures() -> PathBuf {
    // A folder of its own for each call: tests run side by side, and one
    // writing the pictures while another reads them reads half a picture.
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("leaf-parity-{}-{call}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let small = image::RgbImage::from_fn(120, 80, |x, y| {
        image::Rgb([(x * 2) as u8, (y * 3) as u8, 160])
    });
    small.save(dir.join("small.png")).unwrap();
    let large = image::RgbImage::from_fn(3000, 2000, |x, y| {
        image::Rgb([(x / 12) as u8, (y / 8) as u8, ((x + y) / 20) as u8])
    });
    image::DynamicImage::ImageRgb8(large)
        .save_with_format(dir.join("large.jpg"), image::ImageFormat::Jpeg)
        .unwrap();
    dir
}

/// The page view's choices on a phone's Letter page: 16 pixels, 12 points.
pub fn choices() -> crate::Choices {
    crate::Choices {
        template: crate::Template::default(),
        size: 16.0,
        zoom: 1.0,
        paper: (612.0, 792.0),
        paper_size: None,
    }
}

/// `source` as a document in `dir`, as the editor holds it.
pub fn doc(source: &str, dir: &Path) -> leaf_core::Doc {
    let mut d = leaf_core::Doc::from_source(source.to_owned(), Format::Markdown).unwrap();
    d.path = dir.join("page.md");
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every document, drawn as the page view draws it and as Export as PDF
    /// prints it (`print.rs`), is the same pages.
    #[test]
    fn parity_the_page_view_is_what_prints() {
        let dir = pictures();
        let mut wrong = Vec::new();
        for (name, src) in documents() {
            let mut d = doc(&src, &dir);
            let job = crate::print::job(&mut d, None, &choices());
            let printed = crate::print::print(&mut d, None, &choices()).expect("a PDF");
            if let Err(e) = compare(name, &job, printed) {
                wrong.push(e);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    /// The comparison sees what a reader would: a PDF set a size larger
    /// than the screen's, or with room the screen has not left, is not what
    /// the page view showed.
    #[test]
    fn parity_a_difference_a_reader_would_see_is_caught() {
        let dir = pictures();
        let src = documents()
            .into_iter()
            .find(|(n, _)| *n == "long")
            .unwrap()
            .1;
        let mut d = doc(&src, &dir);
        let job = crate::print::job(&mut d, None, &choices());
        let larger = crate::Choices {
            size: 18.0,
            ..choices()
        };
        let printed = crate::print::print(&mut d, None, &larger).unwrap();
        assert!(
            compare("larger", &job, printed).is_err(),
            "a larger size passed"
        );
        let mut roomy = job.clone();
        let mut supply = Supply::default();
        supply.rooms.insert(src.find("Paragraph 3,").unwrap(), 60.0);
        roomy.supply = Arc::new(supply);
        let printed = crate::typeset::pdf_here(&roomy).unwrap();
        assert!(compare("roomy", &job, printed).is_err(), "a room passed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
