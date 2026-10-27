//! The PDF preview: beside the editor, the pages Export as PDF writes, set
//! again as the document changes.
//!
//! The pages are the PDF's (`pdf.rs`): twig-export sets the document with
//! Typst on a worker thread, each page comes back as an SVG, and the pane
//! draws them top to bottom as pictures, on the paper's white whatever the
//! appearance. One setting is under way at a time, and what is typed while
//! it is set is set together after it, so the pane keeps up with typing as
//! closely as Typst does and never falls behind. A page whose picture is the
//! one on show already is left as it is: an edit draws again only the pages
//! it moved.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use base64::Engine;
use dioxus::prelude::*;
use leaf_core::Format;

use crate::app;
use crate::page::{Page, chrome_height};
use crate::pdf::{Paper, Type};

/// The share of the window's width the pane takes.
const SHARE: f64 = 0.5;

/// The pane's look: a desk the pages lie on, in both appearances, under
/// the toolbar and over whatever of the editor reaches past its box.
pub const CSS: &str = "
:root {
  --desk-bg: #e4e4e7;
  --desk-text: #3f3f46;
}
@media (prefers-color-scheme: dark) {
  :root {
    --desk-bg: #232326;
    --desk-text: #d4d4d8;
  }
}
.preview { position: fixed; right: 0; bottom: 0; z-index: 2; box-sizing: border-box;
           overflow-y: auto; padding: 16px 16px 0; background: var(--desk-bg);
           color: var(--desk-text); border-left: 1px solid var(--field-border); }
.preview .sheet { display: block; width: 100%; height: auto; margin-bottom: 16px;
                  background: white; box-shadow: 0 1px 4px rgba(0, 0, 0, 0.3); }
.preview .note { font: 12px system-ui, -apple-system, sans-serif; white-space: pre-wrap;
                 margin-bottom: 12px; }";

/// What the pane draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheets {
    /// The pane's width in CSS pixels; none while it is hidden.
    pub width: f64,
    /// Each page, as a `data:` URL of its SVG.
    pub pages: Vec<Rc<str>>,
    /// Why the document as it stands would not set, over the pages it last
    /// set as.
    pub error: Option<String>,
}

/// A document window's preview, kept by its [`Page`].
#[derive(Default)]
pub struct Preview {
    shown: Cell<bool>,
    /// A setting is under way on the worker.
    busy: Cell<bool>,
    /// What the pages were last set from, or are being set from.
    from: RefCell<Option<Setting>>,
}

impl Preview {
    pub fn shown(&self) -> bool {
        self.shown.get()
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

/// The preview's own part of the window's width, its signal told: the
/// editor has the rest.
pub fn place(page: &Page, width: f64) -> f64 {
    let pane = if page.preview.shown() {
        (width * SHARE).round()
    } else {
        0.0
    };
    page.with_sheets(|mut s| {
        if s.peek().width != pane {
            s.write().width = pane;
        }
    });
    pane
}

/// Show the pane, or hide it.
pub fn toggle(page: &Page) {
    page.preview.shown.set(!page.preview.shown());
    page.place();
    // The toolbar's tool and the menu's tick follow; so do the pages.
    if let Some(s) = page.shell() {
        s.stale.set(true);
    }
    page.sync();
}

/// Set the pages again, if the pane is shown and the document or the
/// display choices have changed since they were last set; after the
/// setting under way, if there is one.
pub fn update(page: &Page) {
    let p = &page.preview;
    if !p.shown() || p.busy.get() {
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
    let token = app::park(move |set: Result<Vec<String>, String>| {
        let Some(page) = app::page(id) else { return };
        page.preview.busy.set(false);
        show(&page, set);
        update(&page);
    });
    tokio::task::spawn_blocking(move || {
        let s = setting;
        let set: Result<Vec<String>, String> =
            crate::pdf::pages(&source, s.format, Some(&s.path), &s.ty, s.paper)
                .map(|pages| pages.iter().map(|svg| data_url(svg)).collect());
        app::unpark(token, set);
    });
}

/// The pages as set, or why they could not be: a page that has not changed
/// keeps the picture on show.
fn show(page: &Page, set: Result<Vec<String>, String>) {
    page.with_sheets(|mut s| {
        let mut s = s.write();
        match set {
            Ok(urls) => {
                let pages = urls
                    .into_iter()
                    .enumerate()
                    .map(|(i, url)| match s.pages.get(i) {
                        Some(old) if **old == *url => old.clone(),
                        _ => Rc::from(url),
                    })
                    .collect();
                s.pages = pages;
                s.error = None;
            }
            Err(e) => s.error = Some(e),
        }
    });
}

/// An SVG as a URL Blitz draws a picture from.
fn data_url(svg: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg);
    format!("data:image/svg+xml;base64,{b64}")
}

#[derive(Props, Clone, PartialEq)]
pub struct PaneProps {
    sheets: Signal<Sheets>,
}

/// The pane, while it is shown: the pages, top to bottom, at its width.
#[allow(non_snake_case)]
pub fn Pane(props: PaneProps) -> Element {
    let s = props.sheets.read();
    if s.width <= 0.0 {
        return rsx! {};
    }
    let top = chrome_height();
    let width = s.width;
    let note = match (&s.error, s.pages.is_empty()) {
        (Some(e), _) => Some(format!("This version will not set as a PDF:\n{e}")),
        (None, true) => Some("Setting the pages…".to_string()),
        (None, false) => None,
    };
    rsx! {
        div { class: "preview", role: "region", aria_label: "PDF preview",
            style: "top: {top}px; width: {width}px",
            // The keys stay with the editor.
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            if let Some(n) = note {
                div { class: "note", "{n}" }
            }
            for (i, url) in s.pages.iter().enumerate() {
                img { key: "{i}", class: "sheet", src: "{url}", alt: "Page {i + 1}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pane_is_drawn_in_colours_set_for_both_appearances() {
        let all = format!(
            "{}{}{CSS}",
            leaf_dioxus::stylesheet(&leaf_dioxus::Type::default()),
            crate::toolbar::CSS
        );
        leaf_dioxus::Palette::of(&all)
            .check()
            .expect("a whole palette");
    }

    #[test]
    fn a_page_is_a_picture_of_its_svg() {
        let pages = crate::pdf::pages(
            "# One\n\nA page.\n",
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
        .expect("pages");
        assert_eq!(pages.len(), 1);
        let url = data_url(&pages[0]);
        let b64 = url.strip_prefix("data:image/svg+xml;base64,").unwrap();
        let svg = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert!(svg.starts_with(b"<svg"));
        assert!(pages[0].contains(r#"viewBox="0 0 612 792""#));
    }
}
