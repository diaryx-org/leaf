//! Pages laid out by Blitz with no window, for tests: a page's markup is
//! built, polled and laid out as a window's would be, so what breaks Blitz's
//! layout breaks here, in a test, rather than in the app under a reader.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::Arc;
use std::task::{Context, Waker};

use blitz_dom::DocumentConfig;
use blitz_shell::BlitzShellProvider;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_core::{ScopeId, VirtualDom};
use dioxus_native::DioxusDocument;
use dioxus_native::winit::window::Window;
use unit::Unit;

use crate::offscreen::HostedWindow;

/// A page of `root`, `width` × `height` CSS pixels, built and laid out.
pub fn page(root: fn() -> Element, width: f64, height: f64) -> DioxusDocument {
    page_with(root, width, height, DocumentConfig::default())
}

/// [`page`], made with `config`: a test's own fetches, say.
pub fn page_with(
    root: fn() -> Element,
    width: f64,
    height: f64,
    config: DocumentConfig,
) -> DioxusDocument {
    let window = Arc::new(HostedWindow::new(
        NonNull::dangling(),
        width,
        height,
        1.0,
        false,
    ));
    let as_dyn: Arc<dyn Window> = window.clone();
    let proxy = crate::app::proxy();
    let vdom = VirtualDom::new(root);
    let mut document = DioxusDocument::new(vdom, config);
    {
        let w = as_dyn.clone();
        document
            .vdom
            .in_scope(ScopeId::ROOT, move || provide_context(w));
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
    settle(&mut document);
    document
}

/// Poll the page and lay it out, as a window's turn and its next frame do.
pub fn settle(document: &mut DioxusDocument) {
    for _ in 0..3 {
        blitz_dom::Document::poll(document, Some(Context::from_waker(Waker::noop())));
        document.inner.borrow_mut().resolve(0.0);
    }
}

thread_local! {
    /// The [`Unit`] the last page made, for a test to open its menus.
    pub static UNIT: RefCell<Option<Unit>> = const { RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use unit::{Dialog, Entry, Item, Keys, UnitRoot, use_unit};

    use super::*;

    #[allow(non_snake_case)]
    fn UnitPage() -> Element {
        rsx! {
            UnitRoot { keys: Keys::Host,
                Keep {}
                div { style: "height: 40px", "a toolbar" }
                p { "Some text under it." }
            }
        }
    }

    #[allow(non_snake_case)]
    fn Keep() -> Element {
        let unit = use_unit();
        use_hook(|| UNIT.with_borrow_mut(|u| *u = Some(unit)));
        rsx! {}
    }

    fn entries() -> Vec<Entry> {
        vec![
            Item::new(0, "Body").shortcut("⌃0").into(),
            Item::new(1, "Heading 1").checked(true).into(),
            Entry::Separator,
            Entry::submenu("More", vec![Item::new(2, "Verse").into()]),
        ]
    }

    fn unit() -> Unit {
        UNIT.with_borrow(|u| u.expect("the page made its unit"))
    }

    #[test]
    fn a_menu_opens_closes_and_opens_again_without_breaking_layout() {
        let mut doc = page(UnitPage, 600.0, 400.0);
        let rt = doc.vdom.runtime();
        for _ in 0..3 {
            rt.in_scope(ScopeId::ROOT, || {
                let l = unit();
                l.set_viewport(600.0, 400.0);
                l.open_menu(entries(), (20.0, 40.0), 26.0, |_| {});
            });
            settle(&mut doc);
            rt.in_scope(ScopeId::ROOT, || unit().close());
            settle(&mut doc);
        }
    }

    #[test]
    fn a_dialog_opens_and_closes_without_breaking_layout() {
        let mut doc = page(UnitPage, 600.0, 400.0);
        let rt = doc.vdom.runtime();
        rt.in_scope(ScopeId::ROOT, || {
            unit().open_dialog(Dialog::new("Link").field("x", "https://"), |_| {})
        });
        settle(&mut doc);
        rt.in_scope(ScopeId::ROOT, || unit().close());
        settle(&mut doc);
    }
}

/// Blitz's own bug, kept as a test: a box shown (`display: none` to
/// `block`) in the same frame as rows arrive in an absolutely positioned
/// child of it left blitz-dom 0.3.0-beta.4 a freed node in its layout tree,
/// and every layout after it panicked. `leaf-dioxus-ui`, before unit, kept
/// its overlay displayed for this reason. The Blitz fork's `leaf` branch fixes
/// it (a removed node is forgotten by its layout parent), so this now lays
/// out cleanly, and the overlay may hide again.
#[cfg(test)]
mod blitz {
    use super::*;

    thread_local! {
        static STEP: RefCell<Option<Signal<u32>>> = const { RefCell::new(None) };
    }

    #[allow(non_snake_case)]
    fn Overlay() -> Element {
        let n = use_signal(|| 0u32);
        use_hook(|| STEP.with_borrow_mut(|x| *x = Some(n)));
        let open = n() % 2 == 1;
        let d = if open { "block" } else { "none" };
        let rows = if open { 3 } else { 0 };
        rsx! {
            div { "page" }
            div { style: "position: fixed; left: 0; top: 0; right: 0; bottom: 0; display: {d}",
                div { style: "position: absolute; left: 10px; top: 10px",
                    for i in 0..rows { div { style: "display: flex", span { "item {i}" } } }
                }
            }
        }
    }

    #[test]
    fn showing_a_box_as_rows_arrive_in_its_absolute_child_lays_out() {
        let mut doc = page(Overlay, 600.0, 400.0);
        let rt = doc.vdom.runtime();
        for n in 1..4 {
            rt.in_scope(ScopeId::ROOT, || {
                let mut s = STEP.with_borrow(|s| s.unwrap());
                s.set(n);
            });
            settle(&mut doc);
        }
    }

    thread_local! {
        static PICTURE: RefCell<Option<Signal<&'static str>>> = const { RefCell::new(None) };
    }

    #[allow(non_snake_case)]
    fn OnePiece() -> Element {
        let svg = use_signal(|| WIDE);
        use_hook(|| PICTURE.with_borrow_mut(|p| *p = Some(svg)));
        let url = leaf_pages::data_url(svg());
        rsx! {
            style { {leaf_pages::CSS} }
            div { class: "pages",
                img { id: "piece", class: "piece", src: "{url}", alt: "",
                    style: "left: 0px; top: 0px; width: 200px" }
            }
        }
    }

    const WIDE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20" viewBox="0 0 100 20"></svg>"#;
    const TALL: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40" viewBox="0 0 100 40"></svg>"#;

    /// A piece's box is as high as the picture it draws: a node given a
    /// taller picture keeps the old one's height, not stretching it, until
    /// the new one is loaded.
    #[test]
    fn a_piece_keeps_its_pictures_shape_until_the_next_is_loaded() {
        let (net, _) = crate::app::net(Arc::new(|_| {}));
        let config = blitz_dom::DocumentConfig {
            net_provider: Some(net),
            ..Default::default()
        };
        let mut doc = page_with(OnePiece, 600.0, 400.0, config);
        let height = |doc: &mut dioxus_native::DioxusDocument| {
            let inner = doc.inner.borrow();
            let id = inner.get_element_by_id("piece").expect("the piece");
            inner.get_node(id).unwrap().final_layout().size.height
        };
        let wait = |doc: &mut dioxus_native::DioxusDocument, want: f32| {
            let start = std::time::Instant::now();
            while height(doc) != want && start.elapsed().as_secs() < 5 {
                std::thread::sleep(std::time::Duration::from_millis(5));
                settle(doc);
            }
            height(doc)
        };
        assert_eq!(wait(&mut doc, 40.0), 40.0);
        let rt = doc.vdom.runtime();
        rt.in_scope(dioxus_core::ScopeId::ROOT, || {
            PICTURE.with_borrow(|p| p.unwrap()).set(TALL);
        });
        // Rendered, its new picture not yet loaded: the old one's height.
        blitz_dom::Document::poll(
            &mut doc,
            Some(std::task::Context::from_waker(std::task::Waker::noop())),
        );
        doc.inner.borrow_mut().resolve(0.0);
        assert_eq!(height(&mut doc), 40.0);
        assert_eq!(wait(&mut doc, 80.0), 80.0);
    }
}
