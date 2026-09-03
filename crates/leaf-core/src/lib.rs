//! leaf-core — the frontend-neutral heart of leaf.
//!
//! A [`Doc`] is a `twig::Editor` plus a byte-offset caret and selection: every
//! mutation is one of twig's offset-addressed ops, and the document stays a
//! live, round-trippable AST the whole time you type into it. The [`wysiwyg`]
//! module resolves that AST into a [`wysiwyg::VisualMap`] — rendered glyphs that
//! each point back at the source byte they came from, so a caret can ride the
//! *visible* text and step over hidden markup delimiters.
//!
//! The [`source`] module is that map's opposite number: where [`wysiwyg`]
//! resolves the markup away, it styles the markup *itself*, so the source view
//! can paint a heading's `# ` and a link's destination as the scaffolding they
//! are. Both read the same twig AST, so the two views cannot disagree about what
//! the document is.
//!
//! Nothing here depends on a UI toolkit. Glyphs carry a toolkit-agnostic
//! [`Style`], which a frontend crate (`leaf-tui`, and next `leaf-gui`) maps onto
//! its own styling. Both frontends share this exact caret math, edit surface,
//! and offset⇄position mapping — the split is what lets a GUI reuse the hard
//! parts instead of re-deriving them.

pub mod doc;
mod html;
pub mod source;
pub mod style;
pub mod wysiwyg;

pub use doc::{
    Capabilities, DiskState, Doc, FootnoteDef, FootnoteRef, Highlight, HighlightCursor,
    InlineMarks, Landing, LineFlow, MarkupMode, Quote, View, VisualKey,
};
pub use source::{SourceMap, StyledRun};
pub use style::{Baseline, Role, Style};
pub use wysiwyg::{
    BlockClass, Boundary, CodeBlockInfo, ColorScheme, Glyph, MediaInfo, MediaKind, MediaSource,
    TableCell, TableInfo, TableRow, VRow, VisualMap,
};

// Re-export the twig types a frontend needs to name when calling into a `Doc`
// (the toolbar's block/inline kinds), so frontends don't each depend on twig.
// `Alignment` comes with `TableCell`, which carries one.
// `Format` too: a filesystem-free host (wasm/FFI) picks the document's format
// itself when it calls `Doc::from_source`, since there's no file extension to
// sniff it from the way `Doc::open` does.
// `Gesture` names one authoring capability for `Doc::supports`, the finer-grained
// half of `Capabilities` — a frontend wanting a mark leaf's own toolbar doesn't
// offer asks with one of these.
pub use twig::{Alignment, BlockContainerKind, BlockKind, Format, Gesture, InlineKind};
