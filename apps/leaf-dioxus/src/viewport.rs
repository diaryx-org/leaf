//! Which rows are on screen — README.md's "What it would take", item 1.
//!
//! Rendering every row costs the document on a reflow (27 416 mutations at
//! 5 199 rows) and makes Blitz restyle and repaint all of it on every scroll
//! frame. So the editor renders only the rows in view, plus [`OVERSCAN`] on
//! either side, between two spacers that stand in for the rest.
//!
//! A spacer needs the height of the rows it replaces, and a row's height is
//! not known until Blitz lays it out — which an unrendered row never is. So
//! heights come from the editor's own layout of every row (`text.rs`), the
//! same parley Blitz draws with. A disagreement would show as the scroll
//! drifting slightly, never as a row drawn in the wrong place, because the
//! rendered rows are laid out for real.

/// A nominal body line, for a height wanted before any text is set (the
/// input method's first caret, a floor on the viewport) and for the tests.
/// The real one is the settings' (`text::Type::line`).
pub const LINE: f64 = 15.0 * 1.5;

/// Rows rendered beyond each edge of the viewport, so a fast scroll shows
/// rows rather than spacer before the next frame catches up.
pub const OVERSCAN: usize = 20;

/// Every row's top, and the document's height after the last.
#[derive(Default, Clone, PartialEq)]
pub struct Layout {
    /// `tops[i]` is row `i`'s y; `tops[rows]` is the total height.
    tops: Vec<f64>,
}

impl Layout {
    /// From each row's height, in order.
    pub fn of(heights: impl Iterator<Item = f64>) -> Self {
        let mut tops = vec![0.0];
        let mut y = 0.0;
        for h in heights {
            y += h;
            tops.push(y);
        }
        Self { tops }
    }

    pub fn rows(&self) -> usize {
        self.tops.len() - 1
    }

    pub fn top(&self, row: usize) -> f64 {
        self.tops[row.min(self.rows())]
    }

    pub fn bottom(&self, row: usize) -> f64 {
        self.tops[(row + 1).min(self.rows())]
    }

    pub fn height(&self) -> f64 {
        self.tops[self.rows()]
    }

    /// The rows to render for a viewport `height` tall scrolled to `top`:
    /// `first..last`, overscan included.
    pub fn window(&self, top: f64, height: f64) -> (usize, usize) {
        // The first row whose bottom is below `top`, and the first whose top
        // is below the viewport's bottom.
        let first = self.tops[1..].partition_point(|&b| b <= top);
        let last = self.tops.partition_point(|&t| t < top + height);
        (
            first.saturating_sub(OVERSCAN),
            (last + OVERSCAN).min(self.rows()),
        )
    }

    /// Where to scroll so `row` is fully in view, or `None` if it already is.
    pub fn reveal(&self, row: usize, top: f64, height: f64) -> Option<f64> {
        let (y, b) = (self.top(row), self.bottom(row));
        if y < top {
            Some(y)
        } else if b > top + height {
            Some(b - height)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(n: usize) -> Layout {
        Layout::of((0..n).map(|_| LINE))
    }

    #[test]
    fn a_taller_row_pushes_the_rows_below_it_down() {
        let l = Layout::of([LINE * 2.0, LINE].into_iter());
        assert_eq!(l.top(1), LINE * 2.0);
        assert_eq!(l.height(), LINE * 3.0);
    }

    #[test]
    fn the_window_is_the_visible_rows_plus_overscan() {
        let l = layout(1000);
        // Rows 100..110 are in view.
        let (first, last) = l.window(LINE * 100.0, LINE * 10.0);
        assert_eq!((first, last), (100 - OVERSCAN, 110 + OVERSCAN));
    }

    #[test]
    fn the_window_is_clamped_to_the_document() {
        let l = layout(5);
        assert_eq!(l.window(0.0, 1000.0), (0, 5));
    }

    #[test]
    fn reveal_scrolls_only_when_the_row_is_out_of_view() {
        let l = layout(100);
        let (top, h) = (LINE * 10.0, LINE * 10.0);
        assert_eq!(l.reveal(15, top, h), None);
        assert_eq!(l.reveal(5, top, h), Some(LINE * 5.0));
        assert_eq!(l.reveal(25, top, h), Some(LINE * 26.0 - h));
    }
}
