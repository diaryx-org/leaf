//! Spike: leaf's editing surface through a virtual DOM.
//!
//! Could one [Dioxus](https://dioxuslabs.com) widget be leaf's web, desktop
//! and mobile frontend at once — the place `leaf-web` (a framework-agnostic JS
//! editor over `leaf-wasm`), `leaf-gpui`, `leaf-swift` and `leaf-android` each
//! stand today? The question the code here answers is the one that decides it:
//! **what does a keystroke cost through a virtual DOM**, when leaf already
//! knows exactly which row changed.
//!
//! The answer, and what it would take, are in README.md. The short of it is
//! that a virtual DOM rediscovering leaf's own frame delta costs the document
//! per keystroke, and that handing it the delta instead — as a reused `Rc` per
//! unchanged row — costs the row. See [`frame`] for the mechanism and
//! [`editor`] for the two renderers it is measured against.
//!
//! This is a spike. It renders, it takes keys, and it is not an editor: no
//! IME, no clipboard, no mouse, no tables, no images, no formulas.

// `style`, `rows` and `frame` are the bridge from leaf's rows to a virtual
// DOM's nodes, and they name no Dioxus type: see the `widget` feature.
pub mod frame;
pub mod rows;
pub mod style;

#[cfg(feature = "widget")]
pub mod editor;
#[cfg(feature = "widget")]
pub mod input;

#[cfg(feature = "widget")]
pub use editor::{Document, FlatDocument, LeafEditor, SharedRows};
pub use frame::{Frame, Stats};
pub use rows::{Marks, Opts, RowHandle, RowView, row_view};
