//! The source view (⌘E): the document's markup, one row per source line.
//!
//! Core has the model — `View::Source`, in which `click`, every motion and
//! `caret_pos` work in source lines and display columns — and the styling,
//! `SourceMap`: styled byte ranges over the source, read off the same parse the
//! caret rides, so a heading's `# ` is a delimiter and a fence's body keeps its
//! syntax colours. What a frontend owns is turning those into rows, as
//! leaf-gpui's `gather_logical` does; this is that, as the bridge's
//! `RowView`s, so the rest of the editor — layout, rendering, hit-testing,
//! the caret — does not know which view it is drawing.
//!
//! There is no `VisualMap` here for the bridge's `Frame` to diff, so rows are
//! reused by *content*: a line that draws the same as some line last frame
//! keeps that line's handle, so a Return relays out the line it split, not
//! every line below it.

use std::collections::HashMap;
use std::ops::Range;

use crate::bridge::rows::{RowHandle, RowView, Run};
use crate::bridge::style::class_of;
use leaf_core::{Doc, Style};

use crate::text::cols;

/// The source view's rows, and last frame's for reuse.
#[derive(Default)]
pub struct SourceRows {
    handles: Vec<RowHandle>,
    /// How many rows the last update built anew, for the trace.
    pub rebuilt: usize,
}

impl SourceRows {
    /// The rows for `doc`'s source as it stands, with the caret at `caret`,
    /// the selection `selection` and a composition `composing` (source
    /// offsets). Answers the rows and the caret's (row, column).
    pub fn update(
        &mut self,
        doc: &Doc,
        caret: usize,
        selection: Option<(usize, usize)>,
        composing: Option<(usize, usize)>,
    ) -> (&[RowHandle], (usize, usize)) {
        let mut previous: HashMap<&RowView, &RowHandle> = HashMap::with_capacity(self.handles.len());
        for h in &self.handles {
            previous.insert(&h.0, h);
        }
        let mut rebuilt = 0;
        let mut handles = Vec::with_capacity(self.handles.len().max(1));
        let mut caret_pos = (0, 0);
        let runs = doc.smap.runs();
        let mut next_run = 0;
        let mut start = 0;
        for (row, line) in doc.source.split('\n').enumerate() {
            let end = start + line.len();
            // The styled runs are ascending; skip those wholly before this line.
            while next_run < runs.len() && runs[next_run].span.end <= start {
                next_run += 1;
            }
            let caret_col = (start..=end).contains(&caret).then(|| cols(&line[..caret - start]));
            if let Some(c) = caret_col {
                caret_pos = (row, c);
            }
            let view = RowView {
                runs: line_runs(line, start, &runs[next_run..], selection, composing),
                caret_col,
                code: false,
                decoration: false,
            };
            let handle = match previous.get(&view) {
                Some(h) => (*h).clone(),
                None => {
                    rebuilt += 1;
                    RowHandle::new(view)
                }
            };
            handles.push(handle);
            start = end + 1;
        }
        self.handles = handles;
        self.rebuilt = rebuilt;
        (&self.handles, caret_pos)
    }

    /// Forget last frame's rows — on a switch of view, when none of them can
    /// be the same.
    pub fn clear(&mut self) {
        self.handles.clear();
    }
}

/// One source line as runs: its bytes grouped by the style core gives them,
/// and by whether they are selected or being composed — the same classes the
/// rendered view's rows carry, so one stylesheet colours both.
fn line_runs(
    line: &str,
    start: usize,
    styled: &[leaf_core::StyledRun],
    selection: Option<(usize, usize)>,
    composing: Option<(usize, usize)>,
) -> Vec<Run> {
    let within = |r: Option<(usize, usize)>, at: usize| r.is_some_and(|(lo, hi)| at >= lo && at < hi);
    let mut out: Vec<Run> = Vec::new();
    let mut open: Option<(Style, bool, bool)> = None;
    let mut class = String::new();
    let mut s = 0;
    for (i, ch) in line.char_indices() {
        let at = start + i;
        while s < styled.len() && styled[s].span.end <= at {
            s += 1;
        }
        let style = styled
            .get(s)
            .filter(|r| contains(&r.span, at))
            .map_or(Style::default(), |r| r.style);
        let key = (style, within(selection, at), within(composing, at));
        if open != Some(key) {
            class.clear();
            class_of(&style, &mut class);
            if key.1 {
                class.push_str(" sel");
            }
            if key.2 {
                class.push_str(" ime");
            }
            out.push(Run { text: String::new(), class: class.clone(), src: None });
            open = Some(key);
        }
        out.last_mut().expect("a run was just opened").text.push(ch);
    }
    out
}

fn contains(r: &Range<usize>, at: usize) -> bool {
    r.start <= at && at < r.end
}

/// The source offset at (row, column) of the source view — where a click
/// there lands, before core snaps it.
pub fn offset_at(doc: &Doc, row: usize, col: usize) -> usize {
    let mut start = 0;
    for (r, line) in doc.source.split('\n').enumerate() {
        if r == row {
            return start + crate::text::byte_of_col(line, col);
        }
        start += line.len() + 1;
    }
    doc.source.len()
}

/// The (row, column) of a source offset in the source view.
pub fn pos_of(doc: &Doc, offset: usize) -> (usize, usize) {
    let offset = offset.min(doc.source.len());
    let before = &doc.source[..offset];
    let row = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (row, cols(&before[line_start..]))
}

#[cfg(test)]
mod tests {
    use leaf_core::Format;

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.toggle_view();
        d.build_source();
        d
    }

    fn text(v: &RowView) -> String {
        v.runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn a_row_per_line_with_the_markup_shown() {
        let d = doc("# Title\n\nSome **bold** text.\n");
        let mut s = SourceRows::default();
        let (rows, _) = s.update(&d, 0, None, None);
        let rows: Vec<String> = rows.iter().map(|h| text(&h.0)).collect();
        assert_eq!(rows, ["# Title", "", "Some **bold** text.", ""]);
    }

    #[test]
    fn the_markup_is_classed_as_a_delimiter() {
        let d = doc("Some **bold** text.\n");
        let mut s = SourceRows::default();
        let (rows, _) = s.update(&d, 0, None, None);
        let stars = rows[0].0.runs.iter().find(|r| r.text == "**").expect("a delimiter run");
        assert!(stars.class.contains("delim"), "{}", stars.class);
    }

    #[test]
    fn the_caret_has_its_row_and_column() {
        let d = doc("one\ntwo\n");
        let mut s = SourceRows::default();
        let (rows, pos) = s.update(&d, 6, None, None);
        assert_eq!(pos, (1, 2));
        assert_eq!(rows[1].0.caret_col, Some(2));
        assert_eq!(pos_of(&d, 6), (1, 2));
        assert_eq!(offset_at(&d, 1, 2), 6);
    }

    #[test]
    fn an_unchanged_line_keeps_its_handle_even_when_shifted() {
        let mut d = doc("alpha\nbeta\n");
        let mut s = SourceRows::default();
        let beta = s.update(&d, 0, None, None).0[1].clone();
        d.place_caret(0, false);
        d.newline();
        d.build_source();
        let caret = d.caret;
        let (rows, _) = s.update(&d, caret, None, None);
        assert_eq!(rows[2], beta, "beta moved down a row and kept its Rc");
    }
}

