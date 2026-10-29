//! A document window's pages, as the window hosts them: the Reflow and
//! Pages views, and the preview beside the source, are leaf-pages's
//! ([`leaf_pages::Pages`]); this is what they ask of the window.
//!
//! The window's area is below the toolbar ([`crate::page::chrome_height`]);
//! the pages are set in the display choices at the window's own zoom; a
//! setting finished on the worker is taken on the handler's turn
//! ([`app::park`]); and a press on the pages is the editor's on the
//! handler's turn too, not inside Blitz's event, where the editor's
//! document may be held.

use std::cell::Cell;

use dioxus_native::winit::window::WindowId;
use leaf_dioxus::{Editor, Pointer};
use leaf_pages::{Area, Choices};

use crate::app::{self, AppEvent};

/// What a window's pages ask of it, by the window's id: given once the
/// window is made.
#[derive(Default)]
pub struct Desk {
    pub id: Cell<Option<WindowId>>,
}

impl Desk {
    fn page(&self) -> Option<std::rc::Rc<crate::page::Page>> {
        app::page(self.id.get()?)
    }
}

impl leaf_pages::Host for Desk {
    fn editor(&self) -> Option<Editor> {
        self.page()?.editor()
    }

    fn in_scope(&self, f: &mut dyn FnMut()) {
        if let Some(page) = self.page() {
            page.in_scope(f);
        }
    }

    fn area(&self) -> Area {
        let Some(page) = self.page() else {
            return Area::default();
        };
        let (width, height) = page.size();
        let top = crate::page::chrome_height();
        Area {
            left: 0.0,
            top,
            width,
            height: height - top,
        }
    }

    /// The display choices', or the system's.
    fn dark(&self) -> bool {
        use dioxus_native::winit::window::Theme;
        match app::settings().appearance {
            leaf_dioxus::Appearance::Dark => true,
            leaf_dioxus::Appearance::Light => false,
            leaf_dioxus::Appearance::System => self
                .page()
                .is_some_and(|p| p.window.theme() == Some(Theme::Dark)),
        }
    }

    fn choices(&self) -> Choices {
        let settings = app::settings();
        Choices {
            template: settings.template.clone(),
            size: settings.size.px(),
            zoom: self
                .page()
                .and_then(|p| p.shell().map(|s| s.zoom.get()))
                .unwrap_or(1.0),
            paper: settings.paper.size(),
        }
    }

    fn park(&self, then: Box<dyn FnOnce()>) -> Box<dyn FnOnce() + Send> {
        let token = app::park(move |()| then());
        Box::new(move || app::unpark(token, ()))
    }

    fn redraw(&self) {
        if let Some(page) = self.page() {
            page.window.request_redraw();
        }
    }

    /// Placed again, the toolbar's tools and the menus' ticks follow.
    fn switched(&self) {
        let Some(page) = self.page() else { return };
        page.place();
        if let Some(s) = page.shell() {
            s.stale.set(true);
        }
        page.sync();
    }

    /// In the status line, before the word count.
    fn counted(&self, pages: Option<usize>) {
        let aside = pages.map(|n| match n {
            1 => "1 page".to_string(),
            n => format!("{n} pages"),
        });
        if let Some(page) = self.page() {
            page.with_editor(|ed, _| ed.set_aside(aside));
        }
    }

    fn point(&self, p: Pointer) {
        let Some(id) = self.id.get() else { return };
        app::post(AppEvent::Run(Box::new(move || {
            if let Some(page) = app::page(id) {
                page.with_editor(|ed, _| ed.pointer(p));
            }
        })));
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use leaf_dioxus::{Pointer, Target};
    use leaf_pages::{Geometry, PageLines};

    #[test]
    fn the_pages_are_drawn_in_colours_set_for_both_appearances() {
        let all = format!(
            "{}{}{}",
            leaf_dioxus::stylesheet(&leaf_dioxus::Type::default()),
            crate::toolbar::CSS,
            leaf_pages::CSS
        );
        leaf_dioxus::Palette::of(&all)
            .check()
            .expect("a whole palette");
    }

    /// While the page view gives the editor its lines, ↓, End and ↑ go by
    /// the lines twig-export set, not the editor's rows.
    #[test]
    fn the_editors_keys_move_by_the_pages_lines() {
        use dioxus::html::{Code, Key, Modifiers, NamedKey};
        use dioxus_core::ScopeId;

        let src = "First line.\n\nSecond line, longer.\n";
        let layout = leaf_pages::pdf::layout(
            src,
            leaf_core::Format::Markdown,
            None,
            &leaf_pages::pdf::Type {
                template: crate::settings::Template::default(),
                size: 12.0,
                dark: false,
            },
            leaf_pages::pdf::Sheet::Paper(leaf_pages::pdf::Paper {
                width: 612.0,
                height: 792.0,
                margin: 72.0,
            }),
            &Default::default(),
        )
        .expect("a layout");
        let pages: Vec<_> = layout
            .pages
            .into_iter()
            .map(|p| (p.height, p.glyphs))
            .collect();
        let lines = Rc::new(PageLines::default());
        *lines.geometry.borrow_mut() = Rc::new(Geometry::new(&pages, &[]));
        let (document, ed, _) = crate::sheet::offscreen(
            src,
            leaf_core::Format::Markdown,
            leaf_dioxus::Look::default(),
            Some((800.0, 600.0)),
        )
        .expect("an editor");
        document.vdom.runtime().in_scope(ScopeId::ROOT, || {
            ed.set_lines(Some(lines.clone()));
            ed.doc().select_range(0, 0);
            let press = |key: NamedKey, code: Code| {
                ed.key(Key::Named(key), code, Modifiers::empty());
                ed.doc().caret
            };
            let second = src.find("Second").unwrap();
            assert_eq!(press(NamedKey::ArrowDown, Code::ArrowDown), second);
            let end = second + "Second line, longer.".len();
            assert_eq!(press(NamedKey::End, Code::End), end);
            assert_eq!(press(NamedKey::ArrowUp, Code::ArrowUp), "First line.".len());
        });
    }

    /// A press on the pages is the editor's own: placed at the byte the
    /// page found, and a second on the same line selects the word, by the
    /// editor's counting.
    #[test]
    fn a_press_on_the_pages_is_the_editors() {
        use dioxus_core::ScopeId;
        use leaf_dioxus::PressMode;

        let src = "Some words here.\n";
        let (document, ed, _) = crate::sheet::offscreen(
            src,
            leaf_core::Format::Markdown,
            leaf_dioxus::Look::default(),
            Some((800.0, 600.0)),
        )
        .expect("an editor");
        let words = src.find("words").unwrap();
        let press = Pointer::Press {
            at: Target::Offset(words + 2),
            line: 0,
            x: 0.0,
            extend: false,
            mode: PressMode::Place,
        };
        document.vdom.runtime().in_scope(ScopeId::ROOT, || {
            ed.pointer(press);
            assert_eq!(ed.doc().caret, words + 2);
            ed.pointer(press);
            assert_eq!(ed.doc().selection(), Some((words, words + "words".len())));
        });
    }
}
