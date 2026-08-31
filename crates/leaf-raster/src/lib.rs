//! CPU rasterization shared by leaf frontends — the pieces of pixel rendering
//! that are the same whatever the pixels end up painted with.
//!
//! Each leaf frontend has its own paint path (terminal cells, a gpui scene,
//! Core Text, the DOM), but several of them also need actual pixels: the
//! terminal ships graphics-protocol images for block media and oversized
//! headings, and a headless consumer wants a frame with no toolkit at all.
//! What those uses share lives here, in backend-neutral units:
//!
//! - [`resolve_image_path`] — which image destinations a synchronous local
//!   loader handles, and where a relative one anchors. One policy, because two
//!   frontends disagreeing about what loads is a bug report.
//! - [`load_image`] / [`load_svg`] — file bytes to a decoded [`image`] raster,
//!   vector pictures included.
//! - [`fit_within`] — aspect-preserving containment in pixels, so frontends
//!   with different display units (cells, points) share the sizing policy and
//!   only round differently.
//! - [`Rasterizer`] — shape and rasterize oversized heading text to an RGBA
//!   frame, with the *editing UI* — caret and selection — painted into the
//!   pixels. That last part is the reason this is a crate and not a helper: a
//!   terminal cannot composite cells over a graphics-protocol image, so the
//!   only way a rasterized heading stays editable is for the raster itself to
//!   carry the caret. [`Rasterizer::heading_hit`] is the reverse mapping, so a
//!   click on the raster lands on the glyph it visually hit.
//!
//! Everything here is CPU-side (cosmic-text, resvg, `image`): the consumers
//! are terminals reading escape sequences and headless frames, none of which
//! are latency-bound enough to want a GPU pipeline — and the one leaf backend
//! that is (gpui) has its own. The contract is "hand me RGBA", so a GPU
//! implementation could slot in behind it without callers changing.

pub use image;

mod fit;
mod load;
mod path;
mod text;

pub use fit::fit_within;
pub use load::{load_image, load_svg};
pub use path::resolve_image_path;
pub use text::{EditingUi, HeadingSpec, Rasterizer};
