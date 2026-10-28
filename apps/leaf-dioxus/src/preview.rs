//! The pages Export as PDF writes, set again as the document changes: in
//! the editor's place, where they are edited, reflowed to the window's width
//! (the default) or on sheets (the page view, `pages.rs`); or beside the
//! source view, where the source is edited and the pages show what it sets.
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
use crate::pages::{Geometry, PageLines, Sheet};
use crate::pdf::{Paper, Type};

/// The share of the window's width the preview takes.
const SHARE: f64 = 0.5;

/// How a window shows its document. There is no view of the editor's own
/// rendering: what reads as the document is what Typst sets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The reflowed view: in the editor's place, edited there, as one page
    /// as wide as the window and as tall as the text.
    #[default]
    Reflow,
    /// The page view: in the editor's place, edited there, on sheets.
    Pages,
    /// The editor's source view on the left, and the pages it sets on the
    /// right.
    Source,
}

impl Mode {
    /// Whether it is in the editor's place, and edited there.
    pub fn edits(self) -> bool {
        matches!(self, Mode::Pages | Mode::Reflow)
    }
}

/// A document window's pages, kept by its [`Page`].
#[derive(Default)]
pub struct Preview {
    mode: Cell<Mode>,
    /// The view the document was edited in before the source: what leaving
    /// the source goes back to.
    last: Cell<Mode>,
    /// Whether the editor moves by the pages' lines.
    lines_on: Cell<bool>,
    /// Whether the caret the document opened with has been put on a stop.
    snapped: Cell<bool>,
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
    /// The source the pages on show were set from: until the pages of an
    /// edit are set, the caret is drawn through the edit onto these.
    laid: RefCell<Rc<str>>,
    /// The pages' throw: odd while one is under way. Starting another, or
    /// stopping it, moves it on, and the loop that moves the pages stops.
    glide: Cell<u64>,
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
    /// Where a stand-in is set for the caret's empty line ([`hole`]).
    hole: Option<usize>,
    format: Format,
    path: PathBuf,
    ty: Type,
    sheet: crate::pdf::Sheet,
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
        Mode::Source => {
            let pane = (width * SHARE).round();
            (width - pane, pane, 0.0)
        }
        Mode::Pages | Mode::Reflow => (0.0, width, leaf_dioxus::STATUS),
    };
    // Edited on the pages, the editor moves by their lines.
    if page.preview.lines_on.get() != mode.edits() {
        page.preview.lines_on.set(mode.edits());
        let lines = mode
            .edits()
            .then(|| page.preview.lines.clone() as Rc<dyn leaf_dioxus::Lines>);
        page.with_editor(|ed, _| ed.set_lines(lines));
    }
    let screen = page.size().1;
    let reflow = mode == Mode::Reflow;
    page.with_view(|mut v| {
        let now = v.peek().clone();
        if (now.left, now.width, now.bottom, now.editing, now.reflow)
            != (left, pane, bottom, mode.edits(), reflow)
        {
            let mut v = v.write();
            v.left = left;
            v.width = pane;
            v.bottom = bottom;
            v.editing = mode.edits();
            v.reflow = reflow;
            if reflow {
                v.scroll.zoom = 1.0;
            }
        }
        if v.peek().scroll.view != (pane, v.peek().height(screen)) {
            v.write().fit(screen);
        }
    });
    measure(page);
    // Reflowed, a new width is a new page: set it again.
    if reflow {
        update(page);
    }
    if mode == Mode::Source { pane } else { 0.0 }
}

/// Tell the page view's lines how many points of the pages a screen shows,
/// for a move by a screen: at the scale the pages are drawn at now.
fn measure(page: &Page) {
    let screen = page.size().1;
    let points = page.with_view_peek(|v| {
        let scale = v.spread().scale * v.scroll.zoom;
        v.height(screen) / scale
    });
    page.preview.lines.screen.set(points);
}

/// Show the document in `mode`: the editor in its source view for the
/// source, in its rich one otherwise, which is what its commands act as.
pub fn show(page: &Page, mode: Mode) {
    let source = mode == Mode::Source;
    page.with_editor(|ed, _| {
        if (ed.doc().view == leaf_core::View::Source) != source {
            ed.doc().toggle_view();
            ed.refresh();
        }
    });
    switch(page, mode);
}

/// The editor's view changed under the window (⌘E, the editor's own
/// command): follow it, into the source or back to the view before.
pub fn follow(page: &Page, source_view: bool) {
    let mode = page.preview.mode();
    if source_view && mode != Mode::Source {
        switch(page, Mode::Source);
    } else if !source_view && mode == Mode::Source {
        switch(page, page.preview.last.get());
    }
}

fn switch(page: &Page, mode: Mode) {
    let p = &page.preview;
    let now = p.mode();
    if now == mode {
        return;
    }
    if now.edits() {
        p.last.set(now);
    }
    p.mode.set(mode);
    p.drawn.set(None);
    // Another view is another scale: start from the top, and the caret's
    // reveal scrolls it into view from there.
    page.with_view(|mut v| {
        let mut v = v.write();
        v.scroll.x = 0.0;
        v.scroll.y = 0.0;
    });
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
    // The find bar is the editor's, at the top of its box: edited on the
    // pages, they start below it while it is open.
    let inset = if p.mode().edits() {
        page.with_editor(|ed, _| ed.bar_height()).unwrap_or(0.0)
    } else {
        0.0
    };
    if page.with_view_peek(|v| v.inset) != inset {
        let screen = page.size().1;
        page.with_view(|mut v| {
            let mut v = v.write();
            v.inset = inset;
            v.fit(screen);
        });
        measure(page);
    }
    caret(page);
    if p.busy.get() {
        return;
    }
    let settings = app::settings();
    let (ty, sheet) = if p.mode() == Mode::Reflow {
        // The view's width, in points; the reader's size at the window's
        // zoom, as the editor's; light ink in a dark window.
        let width = page.with_view_peek(|v| v.width);
        if width <= 0.0 {
            return;
        }
        let zoom = page.shell().map_or(1.0, |s| s.zoom.get());
        let ty = Type {
            size: settings.size.px() * 0.75 * zoom,
            dark: dark(page),
            ..Type::of(&settings)
        };
        (
            ty,
            crate::pdf::Sheet::Reflow {
                width: (width * 0.75) as f32,
            },
        )
    } else {
        (
            Type::of(&settings),
            crate::pdf::Sheet::Paper(Paper::of(&settings)),
        )
    };
    let Some((source, setting)) = page
        .with_doc(|d| {
            let setting = Setting {
                revision: d.revision(),
                hole: hole(&d.source, d.caret),
                format: d.format,
                path: d.path.clone(),
                ty,
                sheet,
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
    let token = app::park(move |(source, set): (String, Result<Vec<Set>, String>)| {
        let Some(page) = app::page(id) else { return };
        page.preview.busy.set(false);
        if set.is_ok() {
            *page.preview.laid.borrow_mut() = Rc::from(source);
        }
        take(&page, set);
        update(&page);
        // The pages are a signal's: the window's page runs again to draw
        // them, now, not at the next event that happens to poll it.
        app::poll(id);
    });
    tokio::task::spawn_blocking(move || {
        let s = setting;
        // The caret's empty line, set with a stand-in where the caret is.
        let set_from = match s.hole {
            Some(at) => format!("{}{STAND_IN}{}", &source[..at], &source[at..]),
            None => source.clone(),
        };
        let layout = crate::pdf::layout(&set_from, s.format, Some(&s.path), &s.ty, s.sheet);
        let set: Result<Vec<Set>, String> = layout.map(|l| {
            l.svg()
                .iter()
                .zip(l.pages())
                .map(|(svg, page)| Set {
                    url: data_url(svg),
                    width: page.width,
                    height: page.height,
                    glyphs: match s.hole {
                        Some(at) => unhole(page.glyphs, at),
                        None => page.glyphs,
                    },
                })
                .collect()
        });
        app::unpark(token, (source, set));
    });
}

/// What stands in for the text of a line that has none, where the caret
/// is: a no-break space, which Typst sets as a glyph of a space's width and
/// draws as nothing.
const STAND_IN: &str = "\u{a0}";

/// Where to set a stand-in for the caret, if it is on a line with no text
/// for Typst to set: a new paragraph, an empty heading, list item or quote.
/// Without one, Typst sets the line as nothing, and there is no glyph to
/// draw the caret by until a letter is typed.
fn hole(source: &str, caret: usize) -> Option<usize> {
    if !source.is_char_boundary(caret) {
        return None;
    }
    let start = source[..caret].rfind('\n').map_or(0, |i| i + 1);
    let end = source[caret..]
        .find('\n')
        .map_or(source.len(), |i| caret + i);
    if !source[caret..end].trim().is_empty() {
        return None;
    }
    // What is before the caret on the line is the block's markup alone.
    let mut rest = source[start..caret].trim_start();
    loop {
        let before = rest;
        for marker in ["#", ">", "- ", "* ", "+ ", "[ ] ", "[x] ", "[X] "] {
            rest = rest.strip_prefix(marker).unwrap_or(rest);
        }
        // A footnote's definition, `[^1]: `.
        if rest.starts_with("[^")
            && let Some(end) = rest.find("]:")
        {
            rest = &rest[end + 2..];
        }
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 && (rest[digits..].starts_with(". ") || rest[digits..].starts_with(") ")) {
            rest = &rest[digits + 2..];
        }
        rest = rest.trim_start();
        if rest == before {
            break;
        }
    }
    // `-`, `*` or a number with nothing after it yet is a list's too.
    let rest = rest.trim_end_matches(['-', '*', '+', '.', ')']);
    rest.trim_start_matches(|c: char| c.is_ascii_digit())
        .is_empty()
        .then_some(caret)
}

/// Glyphs set from a source with a stand-in at `at`, as glyphs of the
/// source without it: the stand-in's at `at`, empty, which a caret there
/// is drawn before.
fn unhole(glyphs: Vec<Glyph>, at: usize) -> Vec<Glyph> {
    let len = STAND_IN.len();
    glyphs
        .into_iter()
        .map(|mut g| {
            g.source = g.source.map(|r| {
                let map = |b: usize| {
                    if b <= at {
                        b
                    } else {
                        b.saturating_sub(len).max(at)
                    }
                };
                map(r.start)..map(r.end)
            });
            g
        })
        .collect()
}

/// The pages as set, or why they could not be: a page that has not changed
/// keeps the picture on show.
fn take(page: &Page, set: Result<Vec<Set>, String>) {
    let p = &page.preview;
    let Ok(set) = set else {
        let e = set.err();
        page.with_view(|mut v| v.write().error = e);
        return;
    };
    let screen = page.size().1;
    let geometry = Rc::new(Geometry::new(
        &set.iter()
            .map(|s| (s.height, s.glyphs.clone()))
            .collect::<Vec<_>>(),
    ));
    *p.lines.geometry.borrow_mut() = geometry.clone();
    p.set.set(p.set.get() + 1);
    // Set at a new width, the text moves under the reader: where the caret
    // is on the screen, if it is on it, to keep it there.
    let held = page.with_view_peek(|v| {
        let widened = v.sheets.first().map(|s| s.width) != set.first().map(|s| s.width);
        let b = v.caret.filter(|_| widened)?;
        let y = v.content_box(&b).1 * v.scroll.zoom - v.scroll.y;
        (0.0..=v.height(screen)).contains(&y).then_some(y)
    });
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
        v.fit(screen);
    });
    measure(page);
    caret(page);
    if let Some(y) = held {
        page.with_view(|mut v| {
            let mut v = v.write();
            if let Some(b) = v.caret {
                v.scroll.y = v.content_box(&b).1 * v.scroll.zoom - y;
                v.fit(screen);
            }
        });
    }
}

/// Draw the editor's caret and selection on the pages, if either, or the
/// pages under them, has changed since they were last drawn; and scroll
/// the caret into view if it moved.
fn caret(page: &Page) {
    let p = &page.preview;
    // A document opens with its caret at its first byte, which in the rich
    // view may be hidden markup (a heading's `# `), where typing would type
    // before the marker: put it on the first letter, as a press there would.
    if !p.snapped.get() {
        let snapped = page.with_doc(|d| {
            if d.view == leaf_core::View::Wysiwyg && d.anchor.is_none() {
                d.build_visual_unwrapped();
                d.place_caret(d.caret, false);
            }
        });
        p.snapped.set(snapped.is_some());
    }
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
    let laid = p.laid.borrow().clone();
    page.with_view(|mut v| {
        let mut v = v.write();
        let g = v.geometry.clone();
        v.caret = g.caret_through(caret, &source, &laid);
        v.selection = match anchor.filter(|&a| a != caret) {
            Some(a) => {
                let (a, c) = (
                    crate::pages::through(a, &source, &laid),
                    crate::pages::through(caret, &source, &laid),
                );
                g.selection(a.min(c), a.max(c))
            }
            None => Vec::new(),
        };
        if moved && let Some(b) = v.caret {
            v.fit(screen);
            v.reveal(b);
        }
    });
}

/// Zoom the pages in, out, or to their width, about the view's middle, if
/// the page view is shown: whether it was, and so took the command.
pub fn zoom(page: &Page, z: leaf_dioxus::ZoomCommand) -> bool {
    use leaf_dioxus::ZoomCommand;
    if page.preview.mode() != Mode::Pages {
        return false;
    }
    let screen = page.size().1;
    page.with_view(|mut v| {
        let mut v = v.write();
        v.fit(screen);
        let (px, py) = (v.scroll.view.0 / 2.0, v.scroll.view.1 / 2.0);
        let to = match z {
            ZoomCommand::In => v.scroll.zoom * crate::pages::ZOOM_STEP,
            ZoomCommand::Out => v.scroll.zoom / crate::pages::ZOOM_STEP,
            ZoomCommand::Reset => 1.0,
        };
        v.scroll.zoom_to(to, px, py);
    });
    measure(page);
    true
}

/// A trackpad's pinch, by `delta` (positive spreads the fingers), at the
/// pointer `at` in the window: the pages zoom about it, if they are shown
/// and it is over them. Whether they took it.
pub fn pinch(page: &Page, delta: f64, at: Option<(f64, f64)>) -> bool {
    // Reflowed, the text grows by its size, not by a zoom of the page.
    let mode = page.preview.mode();
    if mode == Mode::Reflow || !delta.is_finite() {
        return false;
    }
    let screen = page.size().1;
    let top = page.with_view_peek(|v| v.top());
    let mut took = false;
    page.with_view(|mut v| {
        let (x, y) = at.unwrap_or((0.0, 0.0));
        let (left, width) = (v.peek().left, v.peek().width);
        if at.is_none() || x < left || x > left + width || y < top {
            return;
        }
        let mut v = v.write();
        v.fit(screen);
        v.scroll.zoom_at(1.0 + delta, x - left, y - top);
        took = true;
    });
    if took {
        measure(page);
    }
    took
}

/// Carry the pages on at `vel` CSS pixels a millisecond down, slowing as
/// the editor's throw slows (`unit::scroll::Glide`): frame by frame, each
/// moved as far as the time since the last says.
pub fn fling(page: &Page, vel: f64) {
    let p = &page.preview;
    let id = p.glide.get() / 2 * 2 + 3;
    p.glide.set(id);
    let Some(mut g) = page.with_view_peek(|v| {
        let (_, (min, max)) = v.scroll.range();
        Some(unit::scroll::Glide {
            pos: v.scroll.y,
            vel,
            min,
            max,
        })
    }) else {
        return;
    };
    let window = page.window.id();
    page.in_scope(|| {
        spawn(async move {
            let mut last = std::time::Instant::now();
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(8)).await;
                let Some(page) = app::page(window) else {
                    return;
                };
                if page.preview.glide.get() != id {
                    return;
                }
                let now = std::time::Instant::now();
                let dt = now.duration_since(last).as_secs_f64() * 1000.0;
                last = now;
                g.max = page.with_view_peek(|v| v.scroll.range().1.1);
                let moving = g.step(dt);
                page.with_view(|mut v| v.write().scroll.y = g.pos);
                page.window.request_redraw();
                if !moving {
                    page.preview.glide.set(id + 1);
                    return;
                }
            }
        });
    });
}

/// Stop the pages' throw where they are.
pub fn stop_fling(page: &Page) {
    let g = page.preview.glide.get();
    if g % 2 == 1 {
        page.preview.glide.set(g + 1);
    }
}

/// Whether the pages are shown under window point (`x`, `y`).
pub fn under(page: &Page, (x, y): (f64, f64)) -> bool {
    y >= page.with_view_peek(|v| v.top())
        && page.with_view_peek(|v| v.width > 0.0 && x >= v.left && x <= v.left + v.width)
}

/// Whether the window draws dark: the display choices', or the system's.
fn dark(page: &Page) -> bool {
    use dioxus_native::winit::window::Theme;
    match app::settings().appearance {
        leaf_dioxus::Appearance::Dark => true,
        leaf_dioxus::Appearance::Light => false,
        leaf_dioxus::Appearance::System => page.window.theme() == Some(Theme::Dark),
    }
}

/// An SVG as a URL Blitz draws a picture from.
fn data_url(svg: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg);
    format!("data:image/svg+xml;base64,{b64}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new paragraph, an empty heading or list item has a stand-in; a
    /// line with text, or the caret before its text, has none.
    #[test]
    fn a_line_with_nothing_to_set_has_a_stand_in() {
        for (src, caret) in [
            ("Hello\n\n", 7),
            ("# \n", 2),
            ("- one\n- ", 8),
            ("1. ", 3),
            ("> ", 2),
            ("- [ ] ", 6),
            ("Text[^1].\n\n[^1]: ", 17),
            ("", 0),
        ] {
            assert_eq!(hole(src, caret), Some(caret), "{src:?}");
        }
        for (src, caret) in [
            ("Hello", 5),
            ("# Title", 2),
            ("- one", 2),
            ("Hello\n\nWorld", 7),
        ] {
            assert_eq!(hole(src, caret), None, "{src:?} at {caret}");
        }
    }

    /// The stand-in's glyph is the caret's place; the glyphs after it are
    /// the source's again.
    #[test]
    fn a_stand_ins_glyph_is_where_the_caret_is() {
        let g = |r: std::ops::Range<usize>| Glyph {
            x: 0.0,
            baseline: 0.0,
            width: 1.0,
            ascent: 1.0,
            descent: 0.0,
            text: String::new(),
            source: Some(r),
        };
        let out = unhole(vec![g(0..1), g(3..5), g(5..6)], 3);
        let ranges: Vec<_> = out.into_iter().map(|g| g.source.unwrap()).collect();
        assert_eq!(ranges, [0..1, 3..3, 3..4]);
    }

    /// A Markdown paragraph of a stand-in alone is set, as a glyph.
    #[test]
    fn a_stand_in_is_set() {
        let src = format!("Hello\n\n{STAND_IN}");
        let layout = crate::pdf::layout(
            &src,
            Format::Markdown,
            None,
            &Type {
                template: crate::settings::Template::default(),
                size: 12.0,
                dark: false,
            },
            crate::pdf::Sheet::Reflow { width: 400.0 },
        )
        .expect("a layout");
        let glyphs = unhole(layout.pages()[0].glyphs.clone(), 7);
        let hello = glyphs.iter().find(|g| g.source == Some(0..1)).unwrap();
        let caret = glyphs
            .iter()
            .find(|g| g.source == Some(7..7))
            .expect("the stand-in");
        assert!(caret.baseline > hello.baseline, "on a line of its own");
    }

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
                template: crate::settings::Template::default(),
                size: 12.0,
                dark: false,
            },
            crate::pdf::Sheet::Paper(crate::pdf::Paper {
                width: 612.0,
                height: 792.0,
                margin: 72.0,
            }),
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
