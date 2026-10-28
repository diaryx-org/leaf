//! The pages Export as PDF writes, set again as the document changes:
//! beside the editor (the preview), or in its place, where they are edited
//! (the page view, `pages.rs`).
//!
//! The pages are the PDF's (`pdf.rs`): twig-export sets the document with
//! Typst on a worker thread, and each page comes back as an SVG, drawn as a
//! picture on the paper's white whatever the appearance, and as its glyphs,
//! each with the bytes of the source it was set from. One setting is under
//! way at a time, and what is typed while it is set is set together after
//! it, so the pages keep up with typing as closely as Typst does and never
//! fall behind. A page whose picture is the one on show already is left as
//! it is: an edit draws again only the pages it moved.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use base64::Engine;
use dioxus::prelude::*;
use leaf_core::Format;
use twig_export::pdf::Glyph;

use crate::app;
use crate::page::Page;
use crate::pages::{Geometry, PageLines, Sheet, Spread};
use crate::pdf::{Paper, Type};

/// The share of the window's width the preview takes.
const SHARE: f64 = 0.5;

/// Where the pages are, if anywhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    /// The preview: beside the editor, its right half.
    Beside,
    /// The page view: in the editor's place, edited there.
    Pages,
}

/// A document window's pages, kept by its [`Page`].
#[derive(Default)]
pub struct Preview {
    mode: Cell<Mode>,
    /// A setting is under way on the worker.
    busy: Cell<bool>,
    /// What the pages were last set from, or are being set from.
    from: RefCell<Option<Setting>>,
    /// The page view's lines, which the editor moves by while it is shown.
    lines: Rc<PageLines>,
    /// The caret and the selection the view last drew, and the revision of
    /// the pages it drew them on.
    drawn: Cell<Option<(usize, Option<usize>, u64)>>,
    /// Which setting of the pages is on show, counting up.
    set: Cell<u64>,
}

impl Preview {
    pub fn mode(&self) -> Mode {
        self.mode.get()
    }
}

/// Everything the pages depend on but the source: what decides whether
/// they need setting again.
#[derive(Clone, Debug, PartialEq)]
struct Setting {
    revision: u64,
    format: Format,
    path: PathBuf,
    ty: Type,
    paper: Paper,
}

/// One page as the worker sets it, for the view to take on its turn.
struct Set {
    url: String,
    width: f64,
    height: f64,
    glyphs: Vec<Glyph>,
}

/// The pages' own part of the window's width, their view told where it is:
/// the editor has the rest. In the page view the editor keeps the window,
/// under the pages, so that its status line shows below them.
pub fn place(page: &Page, width: f64) -> f64 {
    let mode = page.preview.mode();
    let (left, pane, bottom) = match mode {
        Mode::Off => (0.0, 0.0, 0.0),
        Mode::Beside => {
            let pane = (width * SHARE).round();
            (width - pane, pane, 0.0)
        }
        Mode::Pages => (0.0, width, leaf_dioxus::STATUS),
    };
    let screen = page.size().1;
    page.with_view(|mut v| {
        let now = v.peek().clone();
        if (now.left, now.width, now.bottom, now.editing)
            != (left, pane, bottom, mode == Mode::Pages)
        {
            let mut v = v.write();
            v.left = left;
            v.width = pane;
            v.bottom = bottom;
            v.editing = mode == Mode::Pages;
            v.scroll = v.clamped(v.scroll, v.height(screen));
        }
    });
    measure(page);
    if mode == Mode::Beside { pane } else { 0.0 }
}

/// Tell the page view's lines how many points of the pages a screen shows,
/// for a move by a screen: at the scale the pages are drawn at now.
fn measure(page: &Page) {
    let screen = page.size().1;
    let points = page.with_view_peek(|v| {
        let scale = Spread::of(&v.sheets, v.width).scale;
        v.height(screen) / scale
    });
    page.preview.lines.screen.set(points);
}

/// Show the pages in `mode`, or, if they are shown so already, hide them.
pub fn toggle(page: &Page, mode: Mode) {
    let now = page.preview.mode();
    let next = if now == mode { Mode::Off } else { mode };
    page.preview.mode.set(next);
    let lines =
        (next == Mode::Pages).then(|| page.preview.lines.clone() as Rc<dyn leaf_dioxus::Lines>);
    page.with_editor(|ed, _| ed.set_lines(lines));
    page.preview.drawn.set(None);
    page.place();
    // The toolbar's tools and the menus' ticks follow; so do the pages.
    if let Some(s) = page.shell() {
        s.stale.set(true);
    }
    page.sync();
}

/// Set the pages again, if they are shown and the document or the display
/// choices have changed since they were last set; after the setting under
/// way, if there is one. And draw the caret on them where it is now.
pub fn update(page: &Page) {
    let p = &page.preview;
    if p.mode() == Mode::Off {
        return;
    }
    caret(page);
    if p.busy.get() {
        return;
    }
    let settings = app::settings();
    let Some((source, setting)) = page
        .with_doc(|d| {
            let setting = Setting {
                revision: d.revision(),
                format: d.format,
                path: d.path.clone(),
                ty: Type::of(&settings),
                paper: Paper::of(&settings),
            };
            (p.from.borrow().as_ref() != Some(&setting)).then(|| (d.source.clone(), setting))
        })
        .flatten()
    else {
        return;
    };
    *p.from.borrow_mut() = Some(setting.clone());
    p.busy.set(true);
    let id = page.window.id();
    let token = app::park(move |set: Result<Vec<Set>, String>| {
        let Some(page) = app::page(id) else { return };
        page.preview.busy.set(false);
        show(&page, set);
        update(&page);
    });
    tokio::task::spawn_blocking(move || {
        let s = setting;
        let layout = crate::pdf::layout(&source, s.format, Some(&s.path), &s.ty, s.paper);
        let set: Result<Vec<Set>, String> = layout.map(|l| {
            l.svg()
                .iter()
                .zip(l.pages())
                .map(|(svg, page)| Set {
                    url: data_url(svg),
                    width: page.width,
                    height: page.height,
                    glyphs: page.glyphs,
                })
                .collect()
        });
        app::unpark(token, set);
    });
}

/// The pages as set, or why they could not be: a page that has not changed
/// keeps the picture on show.
fn show(page: &Page, set: Result<Vec<Set>, String>) {
    let p = &page.preview;
    let Ok(set) = set else {
        let e = set.err();
        page.with_view(|mut v| v.write().error = e);
        return;
    };
    let geometry = Rc::new(Geometry::new(
        &set.iter()
            .map(|s| (s.height, s.glyphs.clone()))
            .collect::<Vec<_>>(),
    ));
    *p.lines.geometry.borrow_mut() = geometry.clone();
    p.set.set(p.set.get() + 1);
    page.with_view(|mut v| {
        let mut v = v.write();
        let sheets = set
            .into_iter()
            .enumerate()
            .map(|(i, s)| Sheet {
                url: match v.sheets.get(i) {
                    Some(old) if *old.url == *s.url => old.url.clone(),
                    _ => Rc::from(s.url),
                },
                width: s.width,
                height: s.height,
            })
            .collect();
        v.sheets = sheets;
        v.geometry = geometry;
        v.error = None;
    });
    measure(page);
    caret(page);
}

/// Draw the editor's caret and selection on the pages, if either, or the
/// pages under them, has changed since they were last drawn; and scroll
/// the caret into view if it moved.
fn caret(page: &Page) {
    let p = &page.preview;
    let Some((caret, anchor, source)) = page.with_doc(|d| (d.caret, d.anchor, d.source.clone()))
    else {
        return;
    };
    let now = (caret, anchor, p.set.get());
    let before = p.drawn.replace(Some(now));
    if before == Some(now) {
        return;
    }
    let moved = before.is_none_or(|(c, ..)| c != caret);
    let screen = page.size().1;
    page.with_view(|mut v| {
        let mut v = v.write();
        let g = v.geometry.clone();
        v.caret = g.caret_box(caret, &source);
        v.selection = match anchor.filter(|&a| a != caret) {
            Some(a) => g.selection(a.min(caret), a.max(caret)),
            None => Vec::new(),
        };
        if moved && let Some(b) = v.caret {
            let height = v.height(screen);
            v.scroll = v.clamped(v.reveal(b, height), height);
        }
    });
}

/// An SVG as a URL Blitz draws a picture from.
fn data_url(svg: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg);
    format!("data:image/svg+xml;base64,{b64}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pages_are_drawn_in_colours_set_for_both_appearances() {
        let all = format!(
            "{}{}{}",
            leaf_dioxus::stylesheet(&leaf_dioxus::Type::default()),
            crate::toolbar::CSS,
            crate::pages::CSS
        );
        leaf_dioxus::Palette::of(&all)
            .check()
            .expect("a whole palette");
    }

    /// A page is a picture of its SVG, and its glyphs lead back to the
    /// source: a caret after "One" is drawn after the heading's last glyph,
    /// before the paragraph's first.
    #[test]
    fn a_page_is_a_picture_and_its_glyphs() {
        let source = "# One\n\nA page.\n";
        let layout = crate::pdf::layout(
            source,
            Format::Markdown,
            None,
            &Type {
                face: "Libertinus Serif".into(),
                size: 12.0,
            },
            Paper {
                width: 612.0,
                height: 792.0,
                margin: 72.0,
            },
        )
        .expect("a layout");
        let svg = layout.svg();
        assert_eq!(svg.len(), 1);
        let url = data_url(&svg[0]);
        let b64 = url.strip_prefix("data:image/svg+xml;base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert!(bytes.starts_with(b"<svg"));
        assert!(svg[0].contains(r#"viewBox="0 0 612 792""#));

        let pages = layout.pages();
        let g = Geometry::new(&[(pages[0].height, pages[0].glyphs.clone())]);
        let one = g.caret_box(5, source).expect("after the heading");
        let a = g.caret_box(7, source).expect("before the paragraph");
        assert!(a.top > one.top, "{one:?} {a:?}");
        assert_eq!(g.line_edge(9, false), Some(7));
        assert_eq!(g.line_edge(9, true), Some(14));
    }
}
