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
use crate::pages::{Geometry, PageLines, Sheet};
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
    /// The reflowed view: in the editor's place, edited there, as one page
    /// as wide as the window and as tall as the text.
    Reflow,
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
        Mode::Off => (0.0, 0.0, 0.0),
        Mode::Beside => {
            let pane = (width * SHARE).round();
            (width - pane, pane, 0.0)
        }
        Mode::Pages | Mode::Reflow => (0.0, width, leaf_dioxus::STATUS),
    };
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
    if mode == Mode::Beside { pane } else { 0.0 }
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

/// Show the pages in `mode`, or, if they are shown so already, hide them.
pub fn toggle(page: &Page, mode: Mode) {
    let now = page.preview.mode();
    let next = if now == mode { Mode::Off } else { mode };
    page.preview.mode.set(next);
    let lines = next
        .edits()
        .then(|| page.preview.lines.clone() as Rc<dyn leaf_dioxus::Lines>);
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
    let token = app::park(move |set: Result<Vec<Set>, String>| {
        let Some(page) = app::page(id) else { return };
        page.preview.busy.set(false);
        show(&page, set);
        update(&page);
    });
    tokio::task::spawn_blocking(move || {
        let s = setting;
        let layout = crate::pdf::layout(&source, s.format, Some(&s.path), &s.ty, s.sheet);
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
    let screen = page.size().1;
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
        v.fit(screen);
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
    if matches!(mode, Mode::Off | Mode::Reflow) || !delta.is_finite() {
        return false;
    }
    let screen = page.size().1;
    let top = crate::page::chrome_height();
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
    page.preview.mode() != Mode::Off
        && y >= crate::page::chrome_height()
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
