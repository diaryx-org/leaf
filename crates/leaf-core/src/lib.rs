//! leaf-core — the frontend-neutral heart of leaf.
//!
//! A [`Doc`] is a `twig::Editor` plus a byte-offset caret and selection: every
//! mutation is one of twig's offset-addressed ops, and the document stays a
//! live, round-trippable AST the whole time you type into it. The [`wysiwyg`]
//! module resolves that AST into a [`wysiwyg::VisualMap`] — rendered glyphs that
//! each point back at the source byte they came from, so a caret can ride the
//! *visible* text and step over hidden markup delimiters.
//!
//! Nothing here depends on a UI toolkit. Glyphs carry a toolkit-agnostic
//! [`Style`], which a frontend crate (`leaf-tui`, and next `leaf-gui`) maps onto
//! its own styling. Both frontends share this exact caret math, edit surface,
//! and offset⇄position mapping — the split is what lets a GUI reuse the hard
//! parts instead of re-deriving them.

pub mod doc;
mod html;
pub mod style;
pub mod wysiwyg;

pub use doc::{DiskState, Doc, InlineMarks, View};
pub use style::{Role, Style};
pub use wysiwyg::{CodeBlockInfo, Glyph, TableCell, TableInfo, TableRow, VRow, VisualMap};

// Re-export the twig types a frontend needs to name when calling into a `Doc`
// (the toolbar's block/inline kinds), so frontends don't each depend on twig.
// `Alignment` comes with `TableCell`, which carries one.
// `Format` too: a filesystem-free host (wasm/FFI) picks the document's format
// itself when it calls `Doc::from_source`, since there's no file extension to
// sniff it from the way `Doc::open` does.
pub use twig::{Alignment, BlockKind, Format, InlineKind};
