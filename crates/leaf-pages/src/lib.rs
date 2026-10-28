//! The document as Typst sets it, edited there: the Reflow and Pages views
//! over the leaf-dioxus editor ([`leaf_dioxus`]), for any Dioxus-native host.
//!
//! The editor keeps the document, the keys and the commands; it is hidden
//! under the pages and draws nothing of the document there. What is drawn is
//! twig-export's: the document set by Typst on a worker thread, in one of its
//! built-in templates or one the host installed, each block a picture (a
//! [`Piece`]), with the caret and the selection drawn on Typst's own glyphs
//! and a press on a page put into the text as the editor's own pointer event.
//!
//! - [`Pages`]: a window's view of its document — which [`Mode`] it is in,
//!   setting the pages again as the document changes ([`Pages::update`]),
//!   the caret, zoom, a throw. A host makes one, and draws it with
//!   [`PagesPane`].
//! - [`Host`]: what the views need of the host — its editor, its window's
//!   size, a way to wake its UI when a setting is done, what to set in.
//! - [`Setup`]: what the process sets in, once: installed templates, fonts,
//!   and where a template not built in is set ([`Elsewhere`]).
//! - [`geometry`]: the glyphs as lines, the caret's box, a press's byte;
//!   [`reflow`]: the reflowed view set a chunk at a time; [`pdf`] and
//!   [`typeset`]: the document set on paper or reflowed, as a PDF or as
//!   pieces; [`templates`]: what it is set in; [`net`]: the pieces' pictures
//!   loaded in order.
//!
//! A host mounts it so:
//!
//! ```ignore
//! let pages = Rc::new(leaf_pages::Pages::new(Rc::new(MyHost { .. })));
//! // In the page, beside the editor's render:
//! rsx! { {ed.render()} style { {leaf_pages::CSS} } leaf_pages::PagesPane { pages: pages.clone() } }
//! // After every event that may have changed the document or the window:
//! let pane = pages.place(width); // the editor loses `pane` on the right
//! pages.update();
//! ```

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

pub mod geometry;
pub mod net;
mod pages;
pub mod pdf;
pub mod reflow;
pub mod templates;
pub mod typeset;
mod view;

#[cfg(test)]
mod bench;

pub use geometry::{Geometry, PageBox, PageLines, Piece, Sheet, Spread};
pub use pages::{Mode, Pages, STAND_IN, data_url};
pub use templates::{Entry, Template};
pub use view::{CSS, PagesPane, PagesView, ZOOM_STEP, ZOOMS};

use leaf_dioxus::{Editor, Pointer};

/// The part of the window the editor and the pages share, in CSS pixels:
/// below the host's own bars, as wide as the editor's box may be.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Area {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// What the pages are set in, as the host's display choices say.
#[derive(Clone, Debug, PartialEq)]
pub struct Choices {
    pub template: Template,
    /// The body text's size, in CSS pixels: a point is three quarters of
    /// one, as a browser prints.
    pub size: f32,
    /// The window's text zoom over it: the reflowed view's text grows by
    /// it, as the editor's does; sheets are paper, and do not.
    pub zoom: f32,
    /// The paper the page view lays the document on, in points, at an
    /// inch's margin.
    pub paper: (f32, f32),
}

/// What a window's pages need of their host. The views are the host's
/// window's, on its UI thread: every call here is made there.
pub trait Host {
    /// The editor whose document the pages show, and whose caret they
    /// follow; `None` until it is made.
    fn editor(&self) -> Option<Editor>;

    /// Run `f` inside the Dioxus runtime the pages are drawn in, where a
    /// signal can be written and a task spawned, and draw the window again
    /// after.
    fn in_scope(&self, f: &mut dyn FnMut()) {
        f()
    }

    /// The part of the window the pages and the editor share.
    fn area(&self) -> Area;

    /// Whether the window draws dark: the reflowed view's ink is light then.
    fn dark(&self) -> bool;

    /// What the pages are set in now.
    fn choices(&self) -> Choices;

    /// Keep `then` to run on the UI thread, on its next turn, when the
    /// answer this returns is called from another (the worker that set the
    /// pages): the host's way of waking its UI.
    fn park(&self, then: Box<dyn FnOnce()>) -> Box<dyn FnOnce() + Send>;

    /// Draw the window again: a throw's frame.
    fn redraw(&self);

    /// The view changed mode ([`Pages::show`]): place the editor and the
    /// pages again ([`Pages::place`]) and bring the host's toolbar and menus
    /// up to date.
    fn switched(&self) {}

    /// The number of pages on show changed: `Some` in the page view, `None`
    /// in any other. For a host to show beside the word count.
    fn counted(&self, _pages: Option<usize>) {}

    /// A press, drag or tap on the pages, for the editor, at the byte it
    /// landed on. The editor's now, unless the host has it wait for a turn
    /// of its own.
    fn point(&self, p: Pointer) {
        if let Some(ed) = self.editor() {
            ed.pointer(p);
        }
    }
}

/// Where a template not built into twig-export is set: somewhere it cannot
/// take the host down with it. Leaf's desktop app runs a typesetter of its
/// own, in another process, its memory and time limited. Without one, such
/// a template is set in this process.
pub trait Elsewhere: Send + Sync {
    /// `job` laid out, a picture made for each piece whose key is not
    /// `known`.
    fn layout(&self, job: &typeset::Job, known: &HashSet<u128>) -> Result<typeset::Laid, String>;
    /// `job` as a PDF.
    fn pdf(&self, job: &typeset::Job) -> Result<Vec<u8>, String>;
}

/// What the process sets documents with, given once, before the first
/// setting ([`set_up`]); without it, the built-in templates, Typst's own
/// fonts and the system's, and nothing else.
#[derive(Default)]
pub struct Setup {
    /// The folder installed templates are found in, laid out as Typst's
    /// packages are (`namespace/name/version`).
    pub packages: Option<PathBuf>,
    /// The templates installed there, as a menu lists them: asked each time
    /// a template is looked up, so the host may cache its reading of the
    /// folder, and read it again after an install.
    pub installed: Option<Arc<dyn Fn() -> Vec<Entry> + Send + Sync>>,
    /// Where a template not built in is set.
    pub elsewhere: Option<Arc<dyn Elsewhere>>,
    /// Font files (TrueType or OpenType, a collection or a face), found
    /// before Typst's own and the system's: a host's own faces, which the
    /// system has not got.
    ///
    /// Not taken by Typst yet: twig-export's `Typesetter` has no way to be
    /// handed fonts (docs/tasks/typst-sets-in-a-hosts-own-fonts.md). Until
    /// it does, these are kept, and only the system's are found.
    pub fonts: Vec<Vec<u8>>,
}

static SETUP: OnceLock<Setup> = OnceLock::new();

/// Set the process's [`Setup`]: once, before the first setting. A second,
/// or one after the first setting, is refused and handed back.
pub fn set_up(setup: Setup) -> Result<(), Setup> {
    SETUP.set(setup)
}

/// The process's [`Setup`], or the default.
pub(crate) fn setup() -> &'static Setup {
    SETUP.get_or_init(Setup::default)
}
