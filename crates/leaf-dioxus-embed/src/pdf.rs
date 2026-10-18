//! The document as a PDF, laid out by the editor and paginated between its
//! rows: what Export as PDF writes and Print prints.
//!
//! The picture is the editor's own. A second editor is made offscreen on a
//! copy of the document, as wide as the paper's column and tall enough that
//! every row is rendered, in the light appearance; Blitz paints it into
//! anyrender's recording `Scene`, and each recorded command is drawn again
//! with krilla, a page at a time. So the PDF is what the window shows —
//! proportional text, tables as grids, formulas, diagrams and pictures — as
//! vectors, with text that can be selected and searched: each glyph's
//! character is read back from its font's character map, since a glyph run
//! carries ids, not text.
//!
//! A page breaks between rows, never through one, unless a row is taller
//! than a page. Type is set at the editor's size in CSS pixels and printed at
//! three quarters of it in points, as a browser prints: 16px text is 12pt.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Waker};

use anyrender::recording::RenderCommand;
use anyrender::{Paint, Scene};
use blitz_dom::DocumentConfig;
use blitz_shell::BlitzShellProvider;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus_core::{ScopeId, VirtualDom};
use dioxus_native::DioxusDocument;
use dioxus_native::winit::window::Window;
use krilla::color::rgb;
use krilla::geom::{Path, PathBuilder, Point, Rect, Size, Transform};
use krilla::num::NormalizedF32;
use krilla::paint::{Fill, FillRule, LineCap, LineJoin, Stroke, StrokeDash};
use krilla::text::{Font, GlyphId, KrillaGlyph};
use kurbo::{Affine, BezPath, PathEl, Shape};
use leaf_core::{Doc, Format};
use leaf_dioxus::{Appearance, Editor, Look};
use peniko::{ImageAlphaType, ImageFormat};
use skrifa::MetadataProvider;

use crate::callbacks::Callbacks;
use crate::runtime;
use crate::view::{Init, Root};
use crate::window::HostedWindow;

/// A sheet and its margin, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    pub width: f32,
    pub height: f32,
    pub margin: f32,
}

impl Paper {
    pub const US_LETTER: Paper = Paper {
        width: 612.0,
        height: 792.0,
        margin: 72.0,
    };
}

/// Points per CSS pixel: a browser's print scale.
const SCALE: f32 = 0.75;

/// Taller than any document, so the offscreen editor renders every row.
const TALL: f64 = 1.0e6;

/// The document `source` in `format`, at `path` for its pictures, set in
/// `look` on `paper`, as a PDF.
pub fn render(
    source: &str,
    format: Format,
    path: Option<PathBuf>,
    look: Look,
    paper: Paper,
) -> Result<Vec<u8>, String> {
    let (scene, bottoms, width) = lay_out(source, format, path, look, paper)?;
    Ok(write(&scene, &bottoms, width, paper))
}

/// The offscreen editor's paint, every row's bottom, and the page's width,
/// all in CSS pixels.
fn lay_out(
    source: &str,
    format: Format,
    path: Option<PathBuf>,
    look: Look,
    paper: Paper,
) -> Result<(Scene, Vec<f64>, f64), String> {
    let (document, ed, width) = offscreen(source, format, path, look, paper, None)?;
    let runtime = document.vdom.runtime();
    let bottoms = runtime.in_scope(ScopeId::ROOT, || ed.row_bottoms());
    let height = bottoms.last().copied().unwrap_or(0.0);
    let mut scene = Scene::new();
    {
        let mut inner = document.inner.borrow_mut();
        blitz_paint::paint_scene(
            &mut scene,
            &mut inner,
            1.0,
            width.ceil() as u32,
            height.ceil() as u32 + 1,
            0,
            0,
        );
    }
    Ok((scene, bottoms, width))
}

/// The editor laid out offscreen at the paper's column, its pictures loaded:
/// the document, the editor, and the page's width in CSS pixels. All of it
/// at once, for paper; or, given a `screen` (width, height), in a window that
/// size, at the look's own measure, caret and status line drawn, only the
/// rows it shows: as the app draws it.
fn offscreen(
    source: &str,
    format: Format,
    path: Option<PathBuf>,
    look: Look,
    paper: Paper,
    screen: Option<(f64, f64)>,
) -> Result<(DioxusDocument, Editor, f64), String> {
    // A copy ending on a blank line, with the caret there: the caret's line
    // reveals its formulas' TeX and its fences' code, and this one has none.
    let mut doc = Doc::from_source(format!("{source}\n\n"), format).map_err(|e| e.to_string())?;
    if let Some(p) = path {
        doc.path = p;
    }
    doc.move_doc_end(false);

    let column = ((paper.width - 2.0 * paper.margin) / SCALE) as f64;
    let (width, height) = screen.unwrap_or((column + 2.0 * leaf_dioxus::PAD, TALL));
    let window = Arc::new(HostedWindow::new(
        NonNull::dangling(),
        width,
        height,
        1.0,
        false,
        Callbacks::none(),
    ));
    let as_dyn: Arc<dyn Window> = window.clone();
    let proxy = runtime::proxy();
    let editor = Rc::new(RefCell::new(None));
    let look = Look {
        measure: if screen.is_some() { look.measure } else { None },
        appearance: Appearance::Light,
        ..look
    };

    let mut vdom = VirtualDom::new(Root);
    vdom.insert_any_root_context(Box::new(Init {
        doc: Rc::new(RefCell::new(Some(doc))),
        host: Rc::new(Printer),
        editor: editor.clone(),
        look,
        print: screen.is_none(),
    }));
    let net = Arc::new(blitz_net::Provider::new(Some(Arc::new(proxy.clone()) as _)));
    vdom.provide_root_context(Arc::clone(&net));
    let mut document = DioxusDocument::new(
        vdom,
        DocumentConfig {
            net_provider: Some(net as _),
            ..Default::default()
        },
    );
    {
        let w = as_dyn.clone();
        document
            .vdom
            .in_scope(ScopeId::ROOT, move || dioxus::prelude::provide_context(w));
    }
    {
        let size = window.surface_size();
        let mut inner = document.inner.borrow_mut();
        inner.set_viewport(Viewport::new(
            size.width,
            size.height,
            1.0,
            ColorScheme::Light,
        ));
        inner.set_shell_provider(Arc::new(BlitzShellProvider::new(as_dyn, proxy)));
    }
    document.initial_build();

    // Let the pictures load — `file:` and `data:` URLs, read on the network
    // provider's threads — and the layout settle on them.
    let waker = Waker::noop();
    let mut last = f64::NAN;
    let mut still = 0;
    for _ in 0..120 {
        blitz_dom::Document::poll(&mut document, Some(Context::from_waker(waker)));
        document.inner.borrow_mut().resolve(0.0);
        let h = document
            .inner
            .borrow()
            .root_element()
            .final_layout()
            .size
            .height as f64;
        still = if h == last { still + 1 } else { 0 };
        last = h;
        if still >= 6 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let ed: Editor = editor.borrow().clone().ok_or("the editor did not start")?;
    Ok((document, ed, width))
}

/// The offscreen editor's host: it asks for nothing.
struct Printer;
impl leaf_dioxus::Host for Printer {}

/// Where pages break: the bottom of the last row that fits on each, or a
/// page's height into a row taller than a page.
pub fn breaks(bottoms: &[f64], page: f64) -> Vec<(f64, f64)> {
    let end = bottoms.last().copied().unwrap_or(0.0);
    let mut pages = Vec::new();
    let mut top = 0.0;
    while top < end - 0.5 {
        let limit = top + page;
        let bottom = bottoms
            .iter()
            .copied()
            .filter(|&b| b > top + 0.5 && b <= limit)
            .fold(None, |m: Option<f64>, b| Some(m.map_or(b, |m| m.max(b))))
            .unwrap_or(limit.min(end));
        pages.push((top, bottom));
        top = bottom;
    }
    if pages.is_empty() {
        pages.push((0.0, page.min(end.max(1.0))));
    }
    pages
}

/// The recorded paint, as PDF pages.
fn write(scene: &Scene, bottoms: &[f64], width: f64, paper: Paper) -> Vec<u8> {
    let page_h = ((paper.height - 2.0 * paper.margin) / SCALE) as f64;
    let mut pdf = krilla::Document::new();
    let mut cache = Cache::default();
    for (top, bottom) in breaks(bottoms, page_h) {
        let settings = krilla::page::PageSettings::from_wh(paper.width, paper.height)
            .expect("a sheet has a size");
        let mut page = pdf.start_page_with(settings);
        let mut surface = page.surface();
        surface.push_transform(&Transform::from_row(
            SCALE,
            0.0,
            0.0,
            SCALE,
            paper.margin - leaf_dioxus::PAD as f32 * SCALE,
            paper.margin - top as f32 * SCALE,
        ));
        let band = Rect::from_ltrb(0.0, top as f32, width as f32, bottom as f32).and_then(|r| {
            let mut b = PathBuilder::new();
            b.push_rect(r);
            b.finish()
        });
        if let Some(band) = &band {
            surface.push_clip_path(band, &FillRule::NonZero);
        }
        let mut layers: Vec<usize> = Vec::new();
        for cmd in &scene.commands {
            draw(&mut surface, cmd, top, bottom, &mut layers, &mut cache);
        }
        for n in layers.into_iter().rev() {
            for _ in 0..n {
                surface.pop();
            }
        }
        if band.is_some() {
            surface.pop();
        }
        surface.pop();
        surface.finish();
        page.finish();
    }
    pdf.finish().unwrap_or_default()
}

/// A font for krilla, and the character each of its glyphs stands for.
type Face = (Font, Rc<HashMap<u32, char>>);

/// Fonts and images, each made once however many pages draw it.
#[derive(Default)]
struct Cache {
    fonts: HashMap<(u64, u32), Option<Face>>,
    images: HashMap<u64, Option<krilla::image::Image>>,
}

fn draw(
    surface: &mut krilla::surface::Surface<'_>,
    cmd: &RenderCommand,
    top: f64,
    bottom: f64,
    layers: &mut Vec<usize>,
    cache: &mut Cache,
) {
    let on_page = |r: kurbo::Rect| r.y1 >= top && r.y0 <= bottom;
    match cmd {
        RenderCommand::PushLayer(l) => {
            let mut n = 0;
            if let Some(p) = path(&l.clip).and_then(|p| p.transform(tf(l.transform))) {
                surface.push_clip_path(&p, &FillRule::NonZero);
                n += 1;
            }
            if l.alpha < 1.0
                && let Some(a) = NormalizedF32::new(l.alpha.clamp(0.0, 1.0))
            {
                surface.push_opacity(a);
                n += 1;
            }
            layers.push(n);
        }
        RenderCommand::PushClipLayer(c) => {
            let mut n = 0;
            if let Some(p) = path(&c.clip).and_then(|p| p.transform(tf(c.transform))) {
                surface.push_clip_path(&p, &FillRule::NonZero);
                n = 1;
            }
            layers.push(n);
        }
        RenderCommand::PopLayer => {
            for _ in 0..layers.pop().unwrap_or(0) {
                surface.pop();
            }
        }
        RenderCommand::Fill(f) => {
            if !on_page(f.transform.transform_rect_bbox(f.shape.bounding_box())) {
                return;
            }
            let Some(shape) = path(&f.shape) else { return };
            let rule = match f.fill {
                peniko::Fill::NonZero => FillRule::NonZero,
                peniko::Fill::EvenOdd => FillRule::EvenOdd,
            };
            surface.push_transform(&tf(f.transform));
            match &f.brush {
                Paint::Image(brush) => {
                    if let Some(img) = image(&brush.image, cache) {
                        surface.push_clip_path(&shape, &rule);
                        surface.push_transform(&tf(f.brush_transform.unwrap_or_default()));
                        if let Some(size) =
                            Size::from_wh(brush.image.width as f32, brush.image.height as f32)
                        {
                            surface.draw_image(img, size);
                        }
                        surface.pop();
                        surface.pop();
                    }
                }
                brush => {
                    if let Some((paint, opacity)) = solid(brush) {
                        surface.set_stroke(None);
                        surface.set_fill(Some(Fill {
                            paint,
                            opacity,
                            rule,
                        }));
                        surface.draw_path(&shape);
                    }
                }
            }
            surface.pop();
        }
        RenderCommand::Stroke(s) => {
            let bbox = s
                .transform
                .transform_rect_bbox(s.shape.bounding_box().inflate(s.style.width, s.style.width));
            if !on_page(bbox) {
                return;
            }
            let (Some(shape), Some((paint, opacity))) = (path(&s.shape), solid(&s.brush)) else {
                return;
            };
            surface.push_transform(&tf(s.transform));
            surface.set_fill(None);
            surface.set_stroke(Some(Stroke {
                paint,
                width: s.style.width as f32,
                miter_limit: s.style.miter_limit as f32,
                line_cap: match s.style.start_cap {
                    kurbo::Cap::Butt => LineCap::Butt,
                    kurbo::Cap::Round => LineCap::Round,
                    kurbo::Cap::Square => LineCap::Square,
                },
                line_join: match s.style.join {
                    kurbo::Join::Bevel => LineJoin::Bevel,
                    kurbo::Join::Miter => LineJoin::Miter,
                    kurbo::Join::Round => LineJoin::Round,
                },
                opacity,
                dash: (!s.style.dash_pattern.is_empty()).then(|| StrokeDash {
                    array: s.style.dash_pattern.iter().map(|d| *d as f32).collect(),
                    offset: s.style.dash_offset as f32,
                }),
            }));
            surface.draw_path(&shape);
            surface.set_stroke(None);
            surface.pop();
        }
        RenderCommand::GlyphRun(g) => {
            if g.glyphs.is_empty() {
                return;
            }
            let size = g.font_size;
            let (lo, hi) = g.glyphs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), gl| {
                (lo.min(gl.y), hi.max(gl.y))
            });
            let bbox = g.transform.transform_rect_bbox(kurbo::Rect::new(
                0.0,
                (lo - size) as f64,
                1.0,
                (hi + size) as f64,
            ));
            if !on_page(bbox) {
                return;
            }
            let Some((paint, opacity)) = solid(&g.brush) else {
                return;
            };
            let key = (g.font_data.data.id(), g.font_data.index);
            let entry = cache
                .fonts
                .entry(key)
                .or_insert_with(|| font(&g.font_data))
                .clone();
            let Some((font, chars)) = entry else { return };
            // The text the glyphs spell, as near as the font's character map
            // says, so the PDF's text can be selected and searched.
            let mut text = String::new();
            let glyphs: Vec<KrillaGlyph> = g
                .glyphs
                .iter()
                .map(|gl| {
                    let start = text.len();
                    if let Some(c) = chars.get(&gl.id) {
                        text.push(*c);
                    }
                    KrillaGlyph::new(
                        GlyphId::new(gl.id),
                        0.0,
                        gl.x / size,
                        -gl.y / size,
                        0.0,
                        start..text.len(),
                        None,
                    )
                })
                .collect();
            surface.push_transform(&tf(g.transform));
            if let Some(gt) = g.glyph_transform {
                // A synthetic italic's slant, applied to the run as a whole
                // about its baseline.
                let y = g.glyphs[0].y as f64;
                let a = Affine::translate((0.0, y)) * gt * Affine::translate((0.0, -y));
                surface.push_transform(&tf(a));
            }
            match g.style {
                peniko::Style::Fill(_) => {
                    surface.set_stroke(None);
                    surface.set_fill(Some(Fill {
                        paint,
                        opacity,
                        rule: FillRule::NonZero,
                    }));
                }
                peniko::Style::Stroke(ref st) => {
                    surface.set_fill(None);
                    surface.set_stroke(Some(Stroke {
                        paint,
                        width: st.width as f32,
                        opacity,
                        ..Default::default()
                    }));
                }
            }
            surface.draw_glyphs(Point::from_xy(0.0, 0.0), &glyphs, font, &text, size, false);
            surface.set_stroke(None);
            if g.glyph_transform.is_some() {
                surface.pop();
            }
            surface.pop();
        }
        RenderCommand::BoxShadow(_) => {}
    }
}

/// A font for krilla, and its glyphs' characters.
fn font(data: &peniko::FontData) -> Option<Face> {
    let bytes: Vec<u8> = data.data.data().to_vec();
    let mut chars = HashMap::new();
    if let Ok(f) = skrifa::FontRef::from_index(&bytes, data.index) {
        for (cp, gid) in f.charmap().mappings() {
            if let Some(c) = char::from_u32(cp) {
                chars.entry(gid.to_u32()).or_insert(c);
            }
        }
    }
    let font = Font::new(bytes.into(), data.index)?;
    Some((font, Rc::new(chars)))
}

/// A raster image for krilla, straight alpha.
fn image(data: &peniko::ImageData, cache: &mut Cache) -> Option<krilla::image::Image> {
    cache
        .images
        .entry(data.data.id())
        .or_insert_with(|| {
            let mut px = data.data.data().to_vec();
            for p in px.as_chunks_mut::<4>().0 {
                if data.format == ImageFormat::Bgra8 {
                    p.swap(0, 2);
                }
                let a = p[3] as u32;
                if data.alpha_type == ImageAlphaType::AlphaPremultiplied && a > 0 && a < 255 {
                    for c in &mut p[..3] {
                        *c = ((*c as u32 * 255) / a).min(255) as u8;
                    }
                }
            }
            Some(krilla::image::Image::from_rgba8(
                px,
                data.width,
                data.height,
            ))
        })
        .clone()
}

/// A solid paint and its opacity; a gradient as its first stop.
fn solid<I, C>(
    brush: &Paint<I, peniko::Gradient, C>,
) -> Option<(krilla::paint::Paint, NormalizedF32)> {
    let color = match brush {
        Paint::Solid(c) => *c,
        Paint::Gradient(g) => g.stops.first()?.color.to_alpha_color(),
        _ => return None,
    };
    let c = color.to_rgba8();
    let paint = rgb::Color::new(c.r, c.g, c.b).into();
    Some((paint, NormalizedF32::new(c.a as f32 / 255.0)?))
}

fn tf(a: Affine) -> Transform {
    let [a, b, c, d, e, f] = a.as_coeffs();
    Transform::from_row(a as f32, b as f32, c as f32, d as f32, e as f32, f as f32)
}

fn path(b: &BezPath) -> Option<Path> {
    let mut p = PathBuilder::new();
    for el in b.elements() {
        match *el {
            PathEl::MoveTo(a) => p.move_to(a.x as f32, a.y as f32),
            PathEl::LineTo(a) => p.line_to(a.x as f32, a.y as f32),
            PathEl::QuadTo(a, b) => p.quad_to(a.x as f32, a.y as f32, b.x as f32, b.y as f32),
            PathEl::CurveTo(a, b, c) => p.cubic_to(
                a.x as f32, a.y as f32, b.x as f32, b.y as f32, c.x as f32, c.y as f32,
            ),
            PathEl::ClosePath => p.close(),
        }
    }
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_words_rect_and_a_point_in_it_find_each_other() {
        let src = "# Title\n\nThe quick brown fox jumps over the lazy dog, and then it \
                   jumps over the lazy dog again, until the line wraps onto another.\n";
        let (document, ed, _) = offscreen(
            src,
            Format::Markdown,
            None,
            Look::default(),
            Paper::US_LETTER,
            None,
        )
        .expect("an editor");
        document.vdom.runtime().in_scope(ScopeId::ROOT, || {
            for word in ["Title", "brown", "again", "another"] {
                let from = src.find(word).unwrap();
                let to = from + word.len();
                let (x, y, w, h) = ed.rect_of(from, to).expect("a rect");
                assert!(w > 10.0 && h > 10.0, "{word}: {w} x {h}");
                // Each letter's middle finds that letter.
                for (i, _) in word.char_indices() {
                    let (cx, ..) = ed.rect_of(from + i, from + i + 1).unwrap();
                    let (nx, ..) = ed.rect_of(from + i + 1, from + i + 1).unwrap();
                    let at = ed.offset_at_point((cx + nx) / 2.0 - 0.5, y + h / 2.0);
                    assert!(
                        at == Some(from + i) || at == Some(from + i + 1),
                        "{word}[{i}] at {at:?}, rect {x},{y} {w}x{h}"
                    );
                }
                assert_eq!(
                    ed.offset_at_point(x + 1.0, y + h / 2.0),
                    Some(from),
                    "{word}"
                );
            }
            // Off the rows, nothing.
            assert_eq!(ed.offset_at_point(10.0, 1.0e6), None);
            // A heading's word is set larger than the body's, each in a face
            // with a name to find it by.
            let (head, head_px) = ed.font_at(src.find("Title").unwrap()).expect("a face");
            let (body, body_px) = ed.font_at(src.find("brown").unwrap()).expect("a face");
            assert!(head_px > body_px * 1.2, "{head_px} vs {body_px}");
            for f in [head, body] {
                use skrifa::MetadataProvider;
                let f = skrifa::FontRef::from_index(f.data.data(), f.index).unwrap();
                let name = f
                    .localized_strings(skrifa::string::StringId::POSTSCRIPT_NAME)
                    .english_or_first();
                assert!(name.is_some(), "a face without a PostScript name");
            }
        });
    }

    #[test]
    fn a_quotes_bars_meet_down_its_lines_and_rows() {
        // A heading, a paragraph that wraps, a table and a nested quote, all
        // in one quote, between two paragraphs outside it.
        let src = "Before.\n\n> # Quoted\n>\n> The quick brown fox jumps over the lazy dog, and \
                   then it jumps over the lazy dog again, until the line wraps onto another and \
                   then onto one more after that.\n>\n> | a | b |\n> |---|---|\n> | 1 | 2 |\n>\n\
                   > > Nested.\n\nAfter.\n";
        let look = Look::default();
        let (indent, line) = (look.ty.quote_indent(), look.ty.line());
        let (document, ..) = offscreen(src, Format::Markdown, None, look, Paper::US_LETTER, None)
            .expect("an editor");
        let inner = document.inner.borrow();
        let rect = |id| {
            let node = inner.get_node(id).unwrap();
            let at = node.absolute_position(0.0, 0.0);
            let size = node.final_layout().size;
            (at.x, at.y, size.width, size.height)
        };
        let bars: Vec<_> = inner
            .query_selector_all(".qbar")
            .unwrap()
            .into_iter()
            .map(rect)
            .collect();
        assert!(bars.len() > 8, "{} bars", bars.len());
        for &(_, _, w, h) in &bars {
            assert!((w - indent).abs() < 0.5, "a bar {w} wide");
            assert!(h >= line - 0.5, "a bar {h} tall");
        }
        // A line is as tall with its bars as parley measured it: no bar
        // stands past the line it is drawn in.
        for id in inner.query_selector_all(".line").unwrap() {
            let (_, y, _, h) = rect(id);
            for &(_, by, _, bh) in bars.iter().filter(|b| (b.1 - y).abs() < 0.5) {
                assert!(
                    (by + bh - (y + h)).abs() < 0.5,
                    "a bar {by}+{bh} in a line {y}+{h}"
                );
            }
        }
        // Each level's bars stand at one x and tile into one unbroken rule:
        // the outer one from the heading to the nested quote, the inner one
        // beside the nested quote alone.
        let mut xs: Vec<f32> = bars.iter().map(|b| b.0.round()).collect();
        xs.sort_by(f32::total_cmp);
        xs.dedup();
        assert_eq!(xs.len(), 2, "the levels' x: {xs:?}");
        assert!((xs[1] - xs[0] - indent).abs() < 1.0, "{xs:?}");
        for &x in &xs {
            let mut column: Vec<_> = bars.iter().filter(|b| b.0.round() == x).collect();
            column.sort_by(|a, b| a.1.total_cmp(&b.1));
            for pair in column.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                assert!(
                    (a.1 + a.3 - b.1).abs() < 0.5,
                    "a gap in the bar at x {x}: {a:?} then {b:?}"
                );
            }
        }
    }

    #[test]
    fn pages_break_between_rows() {
        let bottoms = [100.0, 250.0, 400.0, 500.0, 650.0];
        assert_eq!(
            breaks(&bottoms, 300.0),
            vec![(0.0, 250.0), (250.0, 500.0), (500.0, 650.0)]
        );
    }

    #[test]
    fn a_document_with_formulas_a_diagram_and_a_table_prints() {
        let mut src = String::from(
            "# Printed\n\nEuler: $e^{i\\pi} + 1 = 0$.\n\n$$\n\\int_0^1 x\\,dx\n$$\n\n```mermaid\ngraph TD\n  A --> B\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n",
        );
        for i in 0..80 {
            src.push_str(&format!(
                "Paragraph {i} of a long document, to run onto more pages.\n\n"
            ));
        }
        let pdf = render(
            &src,
            Format::Markdown,
            None,
            Look::default(),
            Paper::US_LETTER,
        )
        .expect("a pdf");
        assert!(pdf.starts_with(b"%PDF"), "not a PDF");
        let text = String::from_utf8_lossy(&pdf);
        let pages = text.matches("/Type /Page\n").count()
            + text.matches("/Type /Page ").count()
            + text.matches("/Type/Page/").count();
        if let Ok(out) = std::env::var("LEAF_PDF_OUT") {
            std::fs::write(out, &pdf).unwrap();
        }
        assert!(pdf.len() > 5_000, "{} bytes", pdf.len());
        let _ = pages;
    }

    #[test]
    fn a_row_taller_than_a_page_is_cut_at_the_page() {
        assert_eq!(
            breaks(&[700.0], 300.0),
            vec![(0.0, 300.0), (300.0, 600.0), (600.0, 700.0)]
        );
    }

    #[test]
    fn moving_the_caret_in_and_out_of_blocks_while_scrolled_keeps_rendering() {
        // Rows virtualised to a short window, scrolled, and the caret taken
        // into each formula and table and back out: a row turning into the
        // block that stands for it, and back. The Dioxus diff once panicked
        // on that, and never rendered the page again.
        let mut src = String::new();
        for i in 0..6 {
            src.push_str(&format!(
                "Paragraph {i}, with $x^{i}$ in it.\n\n$$\n\\int_0^{i} x\\,dx\n$$\n\n| a | b |\n|---|---|\n| {i} | 2 |\n\n"
            ));
        }
        let (mut document, ed, _) = offscreen(
            &src,
            Format::Markdown,
            None,
            Look::default(),
            Paper::US_LETTER,
            Some((600.0, 500.0)),
        )
        .expect("an editor");
        let end = ed.doc().source.len();
        let tops = [0.0, 150.0, 400.0, 700.0, 1000.0];
        let mut step = 0;
        for o in (0..end).filter(|&o| src.is_char_boundary(o)) {
            for at in [o, end] {
                step += 1;
                document.vdom.in_scope(ScopeId::ROOT, || {
                    ed.doc().select_range(at, at);
                    ed.refresh();
                    ed.scroll_to(tops[step % tops.len()]);
                });
                blitz_dom::Document::poll(&mut document, Some(Context::from_waker(Waker::noop())));
                document.inner.borrow_mut().resolve(0.0);
            }
        }
    }

    #[test]
    fn the_column_is_centred_in_a_wide_window_and_clicks_land_on_its_text() {
        let src = "The quick brown fox jumps over the lazy dog.\n";
        let (document, ed, _) = offscreen(
            src,
            Format::Markdown,
            None,
            Look::default(),
            Paper::US_LETTER,
            Some((1600.0, 500.0)),
        )
        .expect("an editor");
        let inner = document.inner.borrow();
        let column = inner
            .query_selector(".leaf")
            .ok()
            .flatten()
            .map(|id| *inner.get_node(id).unwrap().final_layout())
            .expect("the column");
        document.vdom.runtime().in_scope(ScopeId::ROOT, || {
            let left = ed.left();
            let room = 1600.0 - column.size.width as f64;
            assert!(room > 200.0, "a column narrower than the window");
            assert!(
                (column.location.x as f64 - room / 2.0).abs() < 1.0,
                "drawn at {}, of {room} to spare",
                column.location.x
            );
            assert!((left - (column.location.x as f64 + leaf_dioxus::PAD)).abs() < 1.0);
            // A press where `rect_of` says "fox" is, reported as a line
            // reports it, puts the caret before it.
            let fox = src.find("fox").unwrap();
            let (x, ..) = ed.rect_of(fox, fox + 1).unwrap();
            ed.pointer(leaf_dioxus::Pointer::Press {
                at: leaf_dioxus::Target::Row(0),
                line: 0,
                x: (x + 1.0 - leaf_dioxus::PAD) as f32,
                extend: false,
                mode: leaf_dioxus::PressMode::Place,
            });
            assert_eq!(ed.doc().caret, fox);
        });
    }
}
