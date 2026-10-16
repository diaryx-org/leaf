//! Display formulas and Mermaid diagrams, drawn as the pictures they make in
//! place of the rows core lays them out on.
//!
//! Both are SVG from a typesetter of leaf's own — `leaf-math` for TeX,
//! `leaf-mermaid` for a ```` ```mermaid ```` fence — and Blitz draws an SVG
//! as an `img`, here from a `data:` URL, so nothing is written to disk. A
//! picture is made once per source, size and ink and cached, since a
//! keystroke elsewhere repaints every block.
//!
//! A formula's rows are core's one-line placeholder, which no caret enters:
//! its picture stands for them, a click on its left half lands in front of it
//! and on its right half past it, as on a picture (`media.rs`), and its TeX is
//! edited in the source view. A Mermaid fence's rows are its code, so the
//! diagram stands for them only while the caret is elsewhere: moving into the
//! fence, or clicking the diagram, shows the code to edit, and leaving it
//! draws the diagram again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use leaf_core::{CodeBlockInfo, MathInfo, VRow};

use crate::media::GAP;
use crate::row::{PAD, Pointer, PointerSink, Target, press_mode};

/// The widest a diagram is drawn, so a wide one does not run off the page
/// at its natural size — the Swift editor's Mermaid fence.
const MAX_DIAGRAM: f32 = 600.0;

/// The ink a formula is set in: the body text's, `--leaf-text`.
const INK_LIGHT: [u8; 4] = [0x26, 0x26, 0x26, 0xff];
const INK_DARK: [u8; 4] = [0xdf, 0xdf, 0xdf, 0xff];

/// An SVG ready to draw: its `data:` URL and natural size in CSS pixels.
#[derive(Clone, PartialEq, Debug)]
pub struct Svg {
    pub url: String,
    pub size: (f32, f32),
}

/// What a picture block draws.
#[derive(Clone, PartialEq, Debug)]
pub enum Body {
    Svg(Rc<Svg>),
    /// The typesetter's complaint, shown in the picture's place.
    Error(String),
}

/// A formula or a diagram, laid out.
#[derive(Clone, PartialEq, Debug)]
pub struct Block {
    pub first: usize,
    pub rows: Range<usize>,
    pub body: Body,
    /// The box, in CSS pixels.
    pub size: (f32, f32),
    /// Where a click in front of it and past it puts the caret.
    pub start: usize,
    pub end: usize,
    /// The caret at either stop: `Some(false)` in front, `Some(true)` past.
    pub caret: Option<bool>,
    pub selected: bool,
    /// The column it is centred in, for a click's half.
    pub column: f32,
}

impl Block {
    pub fn height(&self) -> f32 {
        self.size.1 + 2.0 * GAP
    }

    /// A click `x` pixels into the column: in front of the centred box on
    /// its left half, past it on its right.
    pub fn hit(&self, x: f32) -> usize {
        if x < self.column / 2.0 {
            self.start
        } else {
            self.end
        }
    }
}

/// An inline formula's picture: its box in CSS pixels, and how far it
/// reaches below the baseline.
#[derive(Clone, PartialEq, Debug)]
pub struct Inline {
    pub url: String,
    pub size: (f32, f32),
    pub depth: f32,
}

/// An inline formula's TeX and whether it is set in display style, by offset.
type Atoms = HashMap<usize, (String, bool)>;
/// A picture's cache key: its source, its style or appearance, its size.
type MathKey = (String, bool, u32, bool);
/// A display formula's cache key: its TeX, its size and its ink.
type FormulaKey = (String, u32, [u8; 4]);

thread_local! {
    /// The inline formulas in the document as it stands, by source offset:
    /// their TeX and style, set before every layout.
    static ATOMS: RefCell<(Atoms, bool)> = RefCell::default();
    static INLINE: RefCell<HashMap<MathKey, Option<Rc<Inline>>>> = RefCell::default();
    static MATH: RefCell<HashMap<FormulaKey, Body>> = RefCell::default();
    static DIAGRAMS: RefCell<HashMap<(String, bool), Body>> = RefCell::default();
}

/// An SVG document as a `data:` URL.
fn data_url(svg: &str) -> String {
    let mut url = String::from("data:image/svg+xml;charset=utf-8,");
    for b in svg.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b' '
            | b'='
            | b'/'
            | b':'
            | b','
            | b';'
            | b'('
            | b')'
            | b'\''
            | b'!'
            | b'*'
            | b'@'
            | b'+' => url.push(b as char),
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

/// A display formula's picture at `size` pixels in `ink`, made once.
pub fn formula(tex: &str, size: f32, ink: [u8; 4]) -> Body {
    let key = (tex.to_owned(), size.to_bits(), ink);
    MATH.with(|c| {
        c.borrow_mut()
            .entry(key)
            .or_insert_with(|| match leaf_math::typeset(tex, true, size as f64, ink) {
                Ok(p) => Body::Svg(Rc::new(Svg {
                    url: data_url(&p.svg),
                    size: (
                        p.px_width(size as f64) as f32,
                        p.px_height(size as f64) as f32,
                    ),
                })),
                Err(e) => Body::Error(e.to_string()),
            })
            .clone()
    })
}

/// A Mermaid diagram's picture, made once per source and appearance.
pub fn diagram(source: &str, dark: bool) -> Body {
    DIAGRAMS.with(|c| {
        c.borrow_mut()
            .entry((source.to_owned(), dark))
            .or_insert_with(|| match leaf_mermaid::render(source, dark, None) {
                Ok(p) => Body::Svg(Rc::new(Svg {
                    url: data_url(&p.svg),
                    size: (p.width as f32, p.height as f32),
                })),
                Err(e) => Body::Error(e.message),
            })
            .clone()
    })
}

/// Note the document's inline formulas, by offset, and the appearance they
/// are inked for — before the rows are laid out, which sizes their boxes.
pub fn set_inline(doc: &leaf_core::Doc, dark: bool) {
    let atoms = doc
        .vmap
        .math
        .iter()
        .filter(|m| m.glyph.is_some())
        .map(|m| (m.src, (m.tex.clone(), m.display)))
        .collect();
    ATOMS.with(|a| *a.borrow_mut() = (atoms, dark));
}

/// The inline formula at `src`, typeset at `size` pixels.
pub fn inline_sized(src: usize, size: f32) -> Option<Rc<Inline>> {
    let (tex, display, dark) = ATOMS.with(|a| {
        let a = a.borrow();
        a.0.get(&src).map(|(t, d)| (t.clone(), *d, a.1))
    })?;
    let key = (tex, display, size.to_bits(), dark);
    INLINE.with(|c| {
        c.borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| {
                let ink = if dark { INK_DARK } else { INK_LIGHT };
                let p = leaf_math::typeset(&key.0, display, size as f64, ink).ok()?;
                let s = size as f64;
                Some(Rc::new(Inline {
                    url: data_url(&p.svg),
                    size: (p.px_width(s) as f32, p.px_height(s) as f32),
                    depth: (p.depth * s) as f32,
                }))
            })
            .clone()
    })
}

/// The inline formula at `src` as last laid out — what a row draws.
pub fn inline_at(src: usize) -> Option<Rc<Inline>> {
    LAID.with(|l| l.borrow().get(&src).cloned())
}

thread_local! {
    /// The size each atom was last laid out at, so drawing it finds the same
    /// picture layout measured.
    static LAID: RefCell<HashMap<usize, Rc<Inline>>> = RefCell::default();
}

/// Lay out the atom at `src` at `size`, remembering it for drawing.
pub fn lay_inline(src: usize, size: f32) -> Option<Rc<Inline>> {
    let p = inline_sized(src, size)?;
    LAID.with(|l| l.borrow_mut().insert(src, p.clone()));
    Some(p)
}

/// The box a body draws in a column `width` wide, its widest `max`.
fn box_of(body: &Body, width: f32, max: f32) -> (f32, f32) {
    match body {
        Body::Svg(s) => {
            let (w, h) = s.size;
            let scale = (width.min(max) / w.max(1.0)).min(1.0);
            ((w * scale).round(), (h * scale).round())
        }
        Body::Error(_) => (width.min(360.0), 44.0),
    }
}

fn caret_at(caret: usize, start: usize, end: usize) -> Option<bool> {
    match caret {
        c if c == start => Some(false),
        c if c == end => Some(true),
        _ => None,
    }
}

fn selected(selection: Option<(usize, usize)>, start: usize, end: usize) -> bool {
    selection.is_some_and(|(lo, hi)| lo <= start && hi >= end && lo < hi)
}

/// A display formula, standing for its placeholder rows.
pub fn math_block(
    m: &MathInfo,
    rows: &[VRow],
    caret: usize,
    selection: Option<(usize, usize)>,
    size: f32,
    dark: bool,
    width: f32,
) -> Option<Block> {
    if m.glyph.is_some() {
        return None;
    }
    let last = rows.get(m.rows_span.end.checked_sub(1)?)?;
    let (start, end) = (m.src, last.end_src);
    let body = formula(&m.tex, size, if dark { INK_DARK } else { INK_LIGHT });
    Some(Block {
        first: m.rows_span.start,
        rows: m.rows_span.clone(),
        size: box_of(&body, width, f32::INFINITY),
        body,
        start,
        end,
        caret: caret_at(caret, start, end),
        selected: selected(selection, start, end),
        column: width,
    })
}

/// A Mermaid fence, standing for its code rows while the caret is not in
/// them.
pub fn diagram_block(
    c: &CodeBlockInfo,
    rows: &[VRow],
    source: &str,
    caret: usize,
    selection: Option<(usize, usize)>,
    dark: bool,
    width: f32,
) -> Option<Block> {
    if !c
        .lang
        .as_deref()
        .is_some_and(|l| l.eq_ignore_ascii_case("mermaid"))
    {
        return None;
    }
    let span = &rows[c.rows_span.clone()];
    let start = span
        .iter()
        .find_map(|r| r.glyphs.first().map(|g| g.src))
        .unwrap_or(span.first()?.end_src);
    let end = span.last()?.end_src;
    // Inside the fence the code is what is edited.
    if (start..=end).contains(&caret) {
        return None;
    }
    let text = source.get(start..end)?;
    let body = diagram(text, dark);
    Some(Block {
        first: c.rows_span.start,
        rows: c.rows_span.clone(),
        size: box_of(&body, width, MAX_DIAGRAM),
        body,
        start,
        end,
        caret: None,
        selected: selected(selection, start, end),
        column: width,
    })
}

/// A picture, centred in the column, with the caret at either side of it.
#[component]
pub fn PictureBlock(block: Block) -> Element {
    let sink = use_context::<PointerSink>();
    let drag = sink.clone();
    let tap = sink.clone();
    let first = block.first;
    let (w, h) = block.size;
    let caret = |past: bool| {
        (block.caret == Some(past)).then(|| {
            rsx! { span { style: "flex: none; width: 0; height: {h}px; border-left: 2px solid var(--leaf-accent); margin-right: -2px;" } }
        })
    };
    let ring = if block.selected {
        "outline: 3px solid var(--leaf-selection);"
    } else {
        ""
    };
    let body = match &block.body {
        Body::Svg(s) => rsx! {
            img { src: "{s.url}", style: "display: block; width: {w}px; height: {h}px; {ring}" }
        },
        Body::Error(message) => rsx! {
            div { style: "display: flex; align-items: center; padding: 0 10px; box-sizing: border-box;
                          width: {w}px; height: {h}px; border-radius: 6px; border: 1px dashed var(--leaf-tertiary);
                          font-size: 13px; color: var(--leaf-secondary); white-space: pre; overflow: hidden; {ring}",
                "{message}"
            }
        },
    };
    rsx! {
        div { class: "picture", style: "display: flex; justify-content: center; align-items: flex-start;
                                        box-sizing: border-box; height: {block.height()}px; padding: {GAP}px 0;",
            onmousedown: move |e: MouseEvent| {
                let x = (e.client_coordinates().x - PAD) as f32;
                let (extend, mode) = press_mode(&e);
                (sink.0)(Pointer::Press { at: Target::Block { first }, line: 0, x, extend, mode });
            },
            onmousemove: move |e: MouseEvent| {
                if e.held_buttons().contains(MouseButton::Primary) {
                    let x = (e.client_coordinates().x - PAD) as f32;
                    (drag.0)(Pointer::Drag { at: Target::Block { first }, line: 0, x });
                }
            },
            onclick: move |e: MouseEvent| {
                let x = (e.client_coordinates().x - PAD) as f32;
                (tap.0)(Pointer::Tap { at: Target::Block { first }, line: 0, x });
            },
            {caret(false)}
            {body}
            {caret(true)}
        }
    }
}

#[cfg(test)]
mod tests {
    use leaf_core::{Doc, Format};

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.build_visual_unwrapped();
        d
    }

    #[test]
    fn a_display_formula_is_a_picture_with_stops_either_side() {
        let d = doc("Above.\n\n$$\nx^2 + 1\n$$\n\nBelow.\n");
        let m = d
            .vmap
            .math
            .iter()
            .find(|m| m.glyph.is_none())
            .expect("a display formula");
        let b = math_block(m, &d.vmap.rows, 0, None, 16.0, false, 600.0).expect("a block");
        assert!(matches!(b.body, Body::Svg(_)), "{:?}", b.body);
        assert!(b.size.0 > 0.0 && b.size.1 > 0.0);
        assert!(b.start < b.end && b.end <= d.source.len());
        assert_eq!(b.hit(10.0), b.start);
        assert_eq!(b.hit(590.0), b.end);
    }

    #[test]
    fn bad_tex_draws_the_complaint() {
        assert!(matches!(
            formula(r"\frac{", 16.0, INK_LIGHT),
            Body::Error(_)
        ));
    }

    #[test]
    fn a_mermaid_fence_is_a_diagram_until_the_caret_enters_it() {
        let src = "Top.\n\n```mermaid\ngraph TD\n  A --> B\n```\n\nEnd.\n";
        let d = doc(src);
        let c = d.vmap.code_blocks.first().expect("a fence");
        let away = diagram_block(c, &d.vmap.rows, &d.source, 0, None, false, 600.0)
            .expect("a diagram while the caret is elsewhere");
        assert!(matches!(away.body, Body::Svg(_)), "{:?}", away.body);
        assert!(away.size.0 <= MAX_DIAGRAM);
        let inside = src.find("A -->").unwrap();
        assert!(diagram_block(c, &d.vmap.rows, &d.source, inside, None, false, 600.0).is_none());
    }

    #[test]
    fn an_inline_formula_is_a_box_in_its_line_drawn_as_its_picture() {
        use crate::bridge::rows::{Marks, Opts, row_view};
        use crate::text::{Fonts, Kind, Type};
        let measure = |src: &str| {
            // The caret in a paragraph of its own: one in the formula's
            // would reveal its TeX to edit.
            let mut d = doc(&format!("{src}\nElsewhere.\n"));
            d.move_doc_end(false);
            d.set_inline_pictures(true);
            d.build_visual_unwrapped();
            set_inline(&d, false);
            let marks = Marks::of(&d.vmap, usize::MAX, None);
            let v = row_view(&d.vmap.rows[0], 0, &marks, Opts::default());
            let mut fonts = Fonts::new(Type::default());
            (fonts.layout(&v, 600.0, Kind::Prose).width, v, d)
        };
        let (plain, _, _) = measure("ab cd\n");
        let (with, v, d) = measure("ab $\\frac{1}{2} + x^2$ cd\n");
        let m = d.vmap.math.first().expect("an inline formula");
        assert!(m.glyph.is_some());
        // The atom is its own run, carrying the offset its picture is under.
        assert!(
            v.runs
                .iter()
                .any(|r| r.src == Some(m.src) && r.class.contains("math"))
        );
        let p = inline_at(m.src).expect("laid out, so drawable");
        assert!(
            with > plain + p.size.0 * 0.9,
            "{with} vs {plain} + {}",
            p.size.0
        );
        assert!(p.depth > 0.0, "a fraction reaches below the baseline");
    }

    #[test]
    fn a_svg_becomes_a_data_url_blitz_can_read() {
        let url = data_url("<svg width=\"1\" height=\"1\"><path d=\"M0 0\"/></svg>");
        assert!(url.starts_with("data:image/svg+xml"));
        assert!(!url.contains('<') && !url.contains('"'));
    }
}
