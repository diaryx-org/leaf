//! The editor laid out offscreen, for tests of its geometry: where a word
//! is drawn and where a press lands, a quote's bars, the column in a wide
//! window, rows that change as the caret moves while scrolled.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Waker};

use blitz_dom::DocumentConfig;
use blitz_shell::BlitzShellProvider;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus_core::{ScopeId, VirtualDom};
use dioxus_native::DioxusDocument;
use dioxus_native::winit::window::Window;
use leaf_core::{Doc, Format};
use leaf_dioxus::{Appearance, Editor, Look};

use crate::offscreen::HostedWindow;

/// A US Letter page's column at a 72-point margin, in CSS pixels.
const COLUMN: f64 = (612.0 - 2.0 * 72.0) / 0.75;

/// Taller than any document, so the offscreen editor renders every row.
const TALL: f64 = 1.0e6;

/// The editor laid out offscreen, its pictures loaded: the document, the
/// editor, and the page's width in CSS pixels. All of it at once, at a US
/// Letter page's column; or, given a `screen` (width, height), in a window
/// that size, at the look's own measure, caret and status line drawn, only
/// the rows it shows: as the app draws it.
pub fn offscreen(
    source: &str,
    format: Format,
    look: Look,
    screen: Option<(f64, f64)>,
) -> Result<(DioxusDocument, Editor, f64), String> {
    // A copy ending on a blank line, with the caret there: the caret's line
    // reveals its formulas' TeX and its fences' code, and this one has none.
    let mut doc = Doc::from_source(format!("{source}\n\n"), format).map_err(|e| e.to_string())?;
    doc.move_doc_end(false);

    let (width, height) = screen.unwrap_or((COLUMN + 2.0 * leaf_dioxus::PAD, TALL));
    let window = Arc::new(HostedWindow::new(
        NonNull::dangling(),
        width,
        height,
        1.0,
        false,
    ));
    let as_dyn: Arc<dyn Window> = window.clone();
    let proxy = crate::app::proxy();
    let editor = Rc::new(RefCell::new(None));
    let look = Look {
        measure: if screen.is_some() { look.measure } else { None },
        appearance: Appearance::Light,
        ..look
    };

    let mut vdom = VirtualDom::new(Sheet);
    vdom.insert_any_root_context(Box::new(SheetInit {
        doc: Rc::new(RefCell::new(Some(doc))),
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

/// What the offscreen page is made with, and where it leaves its editor.
#[derive(Clone)]
struct SheetInit {
    doc: Rc<RefCell<Option<Doc>>>,
    editor: Rc<RefCell<Option<Editor>>>,
    look: Look,
    /// Laid out whole: no caret, no status line.
    print: bool,
}

/// The offscreen page: the editor, and nothing else.
#[allow(non_snake_case)]
fn Sheet() -> dioxus::prelude::Element {
    use dioxus::prelude::*;
    let init = use_context::<SheetInit>();
    let ed = leaf_dioxus::use_editor(
        || {
            init.doc
                .borrow_mut()
                .take()
                .unwrap_or_else(|| Doc::blank().expect("a blank document"))
        },
        || init.look.clone(),
        || Rc::new(Printer) as Rc<dyn leaf_dioxus::Host>,
    );
    use_hook(|| *init.editor.borrow_mut() = Some(ed.clone()));
    rsx! {
        {ed.render()}
        if init.print {
            style { ".caret-in, .status {{ display: none; }}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_words_rect_and_a_point_in_it_find_each_other() {
        let src = "# Title\n\nThe quick brown fox jumps over the lazy dog, and then it \
                   jumps over the lazy dog again, until the line wraps onto another.\n";
        let (document, ed, _) =
            offscreen(src, Format::Markdown, Look::default(), None).expect("an editor");
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

    // Off the Mac, in the face the default falls back to, a quoted line
    // fits only with its trailing space hanging, and parley breaks it after
    // the bar: docs/tasks/a-quotes-bar-breaks-a-line-blitz-was-told-not-to-
    // wrap.md.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_quotes_bars_meet_down_its_lines_and_rows() {
        // A heading, a paragraph that wraps, a table and a nested quote, all
        // in one quote, between two paragraphs outside it.
        let src = "Before.\n\n> # Quoted\n>\n> The quick brown fox jumps over the lazy dog, and \
                   then it jumps over the lazy dog again, until the line wraps onto another and \
                   then onto one more after that.\n>\n> | a | b |\n> |---|---|\n> | 1 | 2 |\n>\n\
                   > > Nested.\n\nAfter.\n";
        let look = Look::default();
        let (indent, line) = (look.ty.quote_indent(), look.ty.line());
        let (document, ..) = offscreen(src, Format::Markdown, look, None).expect("an editor");
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
            Look::default(),
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
            Look::default(),
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
