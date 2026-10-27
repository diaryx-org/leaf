//! Look Up: no system dictionary to show, so the context menu offers none.

use crate::page::Page;

/// Whether the system has a dictionary to look a word up in.
pub const AVAILABLE: bool = false;

pub fn show(_page: &Page, _loc: usize, _len: usize) {}

/// A force click: no trackpad gesture gives Look Up here.
pub fn force_click(_page: &Page) {}

/// No word: nothing is looked up.
pub fn word_at(_text: &str, _index: usize) -> Option<(usize, usize)> {
    None
}
