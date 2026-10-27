//! Leaf in a browser tab (`web.rs`). A binary rather than a library because
//! the splitter (`cargo xtask web`) hangs everything the page's glue reaches
//! off `main`, which only a binary has; the page itself starts the editor by
//! calling `start` with the faces it fetched, so `main` has nothing to do.
//!
//! There is nothing to build off the web.

#[cfg(target_arch = "wasm32")]
mod field;
#[cfg(target_arch = "wasm32")]
mod files;
#[cfg(target_arch = "wasm32")]
mod pictures;
#[cfg(target_arch = "wasm32")]
mod web;

fn main() {}
