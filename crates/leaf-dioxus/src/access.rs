//! The document as assistive tech reads and edits it: a text area, as
//! AccessKit describes one, for VoiceOver and for agents that drive the
//! app through the accessibility API.
//!
//! The page draws the document as rows of runs, which Blitz's tree would
//! give as static text with no value, selection or caret. So the host hands
//! the editor's node to [`Editor::describe`], which makes it a
//! `MultilineTextInput`: its value the whole text, one `TextRun` child per
//! visual line with each character's length, position and width and where
//! its words start, and the selection. What assistive tech then asks of it
//! (set the value, replace the selection, select a range) the host hands to
//! [`Editor::accessibility_action`], which edits through the document as
//! typing does.
//!
//! **The text is the text as shown.** In the rendered view that is each
//! row's drawn glyphs, a line per row, the rows that hold no text (a table's
//! rules, the gap between blocks) left out; in the source view it is the
//! Markdown source. A position in it maps to the source through the view's
//! own (row, column) map ([`crate::motion::pos_of`],
//! [`crate::motion::offset_at`]), the map a click goes through. A character
//! is a grapheme cluster, the unit the caret moves by.

use std::ops::Range;
use std::rc::Rc;

use accesskit::{
    Action, ActionData, ActionRequest, Node, NodeId, Rect, Role, TextDirection, TextPosition,
    TextSelection,
};
use dioxus::prelude::*;
use leaf_core::{Doc, View};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::editor::{Editor, shown_caret};
use crate::motion::{offset_at, pos_of};

/// The element [`Editor::render`] draws the document's rows in, by its DOM
/// id: the node a host finds in Blitz's tree and hands to
/// [`Editor::describe`].
pub const TEXT_ID: &str = "leaf-text";

/// The first of the ids [`Editor::describe`] gives the text runs, clear of
/// the ids Blitz gives a document's nodes (their slab indices).
pub const RUN_IDS: u64 = 0x4c45_4146 << 32;

/// The most characters a run holds: AccessKit counts a run's word starts in
/// a byte each. A longer line is split into runs on the same line.
const RUN_MAX: usize = u8::MAX as usize;

/// One line of the text: a row of the view.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    /// The view's row it is.
    row: usize,
    text: String,
    /// Each character's first byte in `text`, and its display column.
    chars: Vec<(usize, usize)>,
    /// The display column past the last character.
    end_col: usize,
    /// The source offset just past each character: where a range that ends
    /// with it ends. Not always the next character's offset, which in the
    /// rendered view may be past hidden markup (`**` after a bold word).
    ends: Vec<usize>,
}

impl Line {
    /// Line `row`, its text `text`, and the source offset just past each
    /// of its `char`s, in order.
    fn new(row: usize, text: String, char_ends: &[usize]) -> Self {
        let mut col = 0;
        let mut seen = 0;
        let mut ends = Vec::new();
        let chars = text
            .grapheme_indices(true)
            .map(|(b, g)| {
                let at = (b, col);
                col += UnicodeWidthStr::width(g);
                seen += g.chars().count();
                ends.push(char_ends.get(seen - 1).copied().unwrap_or(0));
                at
            })
            .collect();
        Self {
            row,
            text,
            chars,
            end_col: col,
            ends,
        }
    }

    fn len(&self) -> usize {
        self.chars.len()
    }

    /// The bytes of character `ch`; at the end, the empty range there.
    fn bytes(&self, ch: usize) -> Range<usize> {
        let start = self.chars.get(ch).map_or(self.text.len(), |c| c.0);
        let end = self.chars.get(ch + 1).map_or(self.text.len(), |c| c.0);
        start..end
    }

    /// The display column of character `ch`, or past the last.
    fn col(&self, ch: usize) -> usize {
        self.chars.get(ch).map_or(self.end_col, |c| c.1)
    }

    /// The character at display column `col`: the first that starts at or
    /// past it, or the end.
    fn char_at_col(&self, col: usize) -> usize {
        self.chars.partition_point(|c| c.1 < col)
    }
}

/// The document's text as shown: its lines, a line break between each two.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Text {
    lines: Vec<Line>,
}

impl Text {
    /// The text of `doc` in the view it shows, as of its last build (the
    /// rendered view's map, the source view's rows).
    pub(crate) fn of(doc: &Doc) -> Self {
        let mut lines: Vec<Line> = match doc.view {
            View::Source => {
                let mut start = 0;
                doc.source
                    .split('\n')
                    .enumerate()
                    .map(|(r, l)| {
                        let ends: Vec<usize> = l
                            .char_indices()
                            .map(|(b, c)| start + b + c.len_utf8())
                            .collect();
                        start += l.len() + 1;
                        Line::new(r, l.to_string(), &ends)
                    })
                    .collect()
            }
            // A glyph a character, so each one's end is its own.
            View::Wysiwyg => doc
                .vmap
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| !r.decoration)
                .map(|(r, row)| {
                    let ends: Vec<usize> =
                        row.glyphs.iter().map(|g| g.src + g.ch.len_utf8()).collect();
                    Line::new(r, row.glyphs.iter().map(|g| g.drawn()).collect(), &ends)
                })
                .collect(),
        };
        if lines.is_empty() {
            lines.push(Line::new(0, String::new(), &[]));
        }
        Self { lines }
    }

    pub(crate) fn value(&self) -> String {
        let mut v = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                v.push('\n');
            }
            v.push_str(&l.text);
        }
        v
    }

    /// The (line, character) a source offset is shown at.
    pub(crate) fn position(&self, doc: &Doc, offset: usize) -> (usize, usize) {
        let (row, col) = pos_of(doc, offset);
        let line = self
            .lines
            .partition_point(|l| l.row < row)
            .min(self.lines.len() - 1);
        let l = &self.lines[line];
        if l.row != row {
            // A row the text leaves out: the start of the next line.
            return (line, 0);
        }
        (line, l.char_at_col(col))
    }

    /// The source offset of (line, character).
    pub(crate) fn offset(&self, doc: &Doc, line: usize, ch: usize) -> usize {
        let l = &self.lines[line.min(self.lines.len() - 1)];
        offset_at(doc, l.row, l.col(ch.min(l.len())))
    }

    /// The source offset where a range ending before (line, character)
    /// ends: just past the character before it, or the line's start.
    pub(crate) fn end(&self, doc: &Doc, line: usize, ch: usize) -> usize {
        let l = &self.lines[line.min(self.lines.len() - 1)];
        match ch.min(l.len()).checked_sub(1) {
            Some(before) => l.ends[before],
            None => self.offset(doc, line, 0),
        }
    }

    /// The byte of [`value`](Self::value) that (line, character) is at.
    pub(crate) fn byte_of(&self, line: usize, ch: usize) -> usize {
        let line = line.min(self.lines.len() - 1);
        let start: usize = self.lines[..line].iter().map(|l| l.text.len() + 1).sum();
        let l = &self.lines[line];
        start + l.chars.get(ch).map_or(l.text.len(), |c| c.0)
    }

    /// The (line, character) at byte `at` of [`value`](Self::value): the
    /// character that starts at or after it.
    pub(crate) fn locate(&self, at: usize) -> (usize, usize) {
        let mut start = 0;
        for (i, l) in self.lines.iter().enumerate() {
            let end = start + l.text.len();
            if at <= end || i + 1 == self.lines.len() {
                let within = at.saturating_sub(start);
                return (i, l.chars.partition_point(|c| c.0 < within));
            }
            start = end + 1;
        }
        (0, 0)
    }
}

/// One text run: a stretch of a line, on one visual line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Run {
    line: usize,
    chars: Range<usize>,
    /// It is its line's last, and the line break is its last character.
    breaks: bool,
    /// The run after it is on the same visual line: a line split at
    /// [`RUN_MAX`].
    continues: bool,
    /// Which of its row's visual lines it is on.
    visual: usize,
}

/// The text as last described, with its runs: what assistive tech's next
/// request names positions in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Described {
    text: Text,
    runs: Vec<Run>,
}

impl Described {
    /// `text` in runs, each line split where `wraps` says its visual lines
    /// start (as characters of it, after the first), and at [`RUN_MAX`].
    pub(crate) fn new(text: Text, wraps: impl Fn(&Line) -> Vec<usize>) -> Self {
        let mut runs = Vec::new();
        let last = text.lines.len() - 1;
        for (i, l) in text.lines.iter().enumerate() {
            // Visual line `k` runs up to where line `k + 1` starts.
            let mut cuts = wraps(l);
            cuts.push(l.len());
            let mut from = 0;
            let first = runs.len();
            for (visual, cut) in cuts.into_iter().enumerate() {
                let cut = cut.clamp(from, l.len());
                while cut - from > RUN_MAX {
                    runs.push(Run {
                        line: i,
                        chars: from..from + RUN_MAX,
                        breaks: false,
                        continues: true,
                        visual,
                    });
                    from += RUN_MAX;
                }
                if cut > from || runs.len() == first && cut == l.len() {
                    runs.push(Run {
                        line: i,
                        chars: from..cut,
                        breaks: false,
                        continues: false,
                        visual,
                    });
                }
                from = cut;
            }
            runs.last_mut().expect("a run per line").breaks = i < last;
        }
        Self { text, runs }
    }

    /// The run and the index within it of (line, character): at a soft
    /// wrap, the start of the next visual line; at a line's end, its break.
    fn text_position(&self, line: usize, ch: usize) -> TextPosition {
        let first = self.runs.partition_point(|r| r.line < line);
        let mut i = first;
        while i + 1 < self.runs.len()
            && self.runs[i + 1].line == line
            && self.runs[i].chars.end <= ch
        {
            i += 1;
        }
        let r = &self.runs[i];
        TextPosition {
            node: run_id(i),
            character_index: ch.clamp(r.chars.start, r.chars.end) - r.chars.start,
        }
    }

    /// The (line, character) of a position in a run; `None` for a node not
    /// a run of this text.
    fn line_char(&self, p: TextPosition) -> Option<(usize, usize)> {
        let i = p.node.0.checked_sub(RUN_IDS)? as usize;
        let r = self.runs.get(i)?;
        Some((r.line, (r.chars.start + p.character_index).min(r.chars.end)))
    }

    /// Run `i` as AccessKit's node, without its geometry.
    fn run_node(&self, i: usize) -> Node {
        let r = &self.runs[i];
        let l = &self.text.lines[r.line];
        let mut node = Node::new(Role::TextRun);
        let bytes = l.bytes(r.chars.start).start..l.bytes(r.chars.end).start;
        let mut value = l.text[bytes.clone()].to_string();
        let mut lengths: Vec<u8> = r
            .chars
            .clone()
            .map(|c| l.bytes(c).len().min(u8::MAX as usize) as u8)
            .collect();
        if r.breaks {
            value.push('\n');
            lengths.push(1);
        }
        // Where a word starts: a word-bound segment that opens on a letter
        // or a digit, as the caret's word motion stops.
        let starts: Vec<u8> = l
            .text
            .split_word_bound_indices()
            .filter(|(b, w)| {
                bytes.contains(b) && w.chars().next().is_some_and(char::is_alphanumeric)
            })
            .map(|(b, _)| (l.chars.partition_point(|c| c.0 < b) - r.chars.start) as u8)
            .collect();
        node.set_value(value);
        node.set_character_lengths(lengths);
        node.set_word_starts(starts);
        node.set_text_direction(TextDirection::LeftToRight);
        if r.continues {
            node.set_next_on_line(run_id(i + 1));
        }
        if i > 0 && self.runs[i - 1].continues {
            node.set_previous_on_line(run_id(i - 1));
        }
        node
    }

    /// The selection of `doc`, from its anchor to its caret.
    fn selection(&self, doc: &Doc) -> TextSelection {
        let caret = match doc.view {
            View::Wysiwyg => shown_caret(doc, doc.caret),
            View::Source => doc.caret,
        };
        let at = |offset| {
            let (line, ch) = self.text.position(doc, offset);
            self.text_position(line, ch)
        };
        let focus = at(caret);
        TextSelection {
            anchor: doc.anchor.map_or(focus, at),
            focus,
        }
    }

    /// The editor's node, made the text area: its value, its runs as its
    /// children, and the selection.
    fn text_area(&self, doc: &Doc, node: &mut Node) {
        node.set_role(Role::MultilineTextInput);
        node.set_value(self.text.value());
        node.clear_children();
        node.clear_labelled_by();
        for i in 0..self.runs.len() {
            node.push_child(run_id(i));
        }
        node.set_text_selection(self.selection(doc));
        node.add_action(Action::Focus);
        if doc.read_only() {
            node.set_read_only();
        } else {
            node.add_action(Action::SetValue);
            node.add_action(Action::ReplaceSelectedText);
        }
        node.add_action(Action::SetTextSelection);
    }

    /// What `request` asks of `doc`, done: whether it was this text's to do.
    pub(crate) fn act(&self, doc: &mut Doc, request: &ActionRequest) -> bool {
        match (request.action, &request.data) {
            (Action::SetTextSelection, Some(ActionData::SetTextSelection(s))) => {
                let (Some(a), Some(f)) = (self.line_char(s.anchor), self.line_char(s.focus)) else {
                    return false;
                };
                // The range's start is where its first character is, and
                // its end just past its last.
                let (anchor, focus) = match a.cmp(&f) {
                    std::cmp::Ordering::Less => (
                        self.text.offset(doc, a.0, a.1),
                        self.text.end(doc, f.0, f.1),
                    ),
                    std::cmp::Ordering::Greater => (
                        self.text.end(doc, a.0, a.1),
                        self.text.offset(doc, f.0, f.1),
                    ),
                    std::cmp::Ordering::Equal => {
                        let at = self.text.offset(doc, a.0, a.1);
                        (at, at)
                    }
                };
                doc.select_range(anchor, focus);
                true
            }
            (Action::ReplaceSelectedText, Some(ActionData::Value(text))) => {
                doc.insert(text);
                true
            }
            (Action::SetValue, Some(ActionData::Value(text))) => {
                self.set_value(doc, text);
                true
            }
            (Action::Focus, _) => true,
            _ => false,
        }
    }

    /// Bring the text to `new`. In the source view the source is the text,
    /// written exactly. In the rendered view the stretch that differs is
    /// selected and `new`'s part of it typed over it, as a hand would.
    fn set_value(&self, doc: &mut Doc, new: &str) {
        if doc.view == View::Source {
            doc.replace_source(new);
            return;
        }
        let old = self.text.value();
        let mut prefix = old
            .bytes()
            .zip(new.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let room = old.len().min(new.len()) - prefix;
        let mut suffix = old
            .bytes()
            .rev()
            .zip(new.bytes().rev())
            .take(room)
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix)
        {
            suffix -= 1;
        }
        let (from, to) = (prefix, old.len() - suffix);
        let with = &new[prefix..new.len() - suffix];
        if from == to && with.is_empty() {
            return;
        }
        let (l0, c0) = self.text.locate(from);
        let (l1, c1) = self.text.locate(to);
        let start = self.text.offset(doc, l0, c0);
        let end = if to > from {
            self.text.end(doc, l1, c1)
        } else {
            start
        };
        doc.select_range(start, end);
        doc.insert(with);
    }
}

/// What a described text was built from; see [`Editor::describe`].
#[derive(PartialEq)]
struct Key {
    revision: u64,
    source: bool,
    caret: usize,
    anchor: Option<usize>,
    /// Each row's layout, by its address.
    layouts: Vec<usize>,
}

/// A text as last described, its runs' nodes made but for their geometry.
pub(crate) struct Cached {
    key: Key,
    described: Described,
    nodes: Vec<Node>,
}

fn run_id(i: usize) -> NodeId {
    NodeId(RUN_IDS + i as u64)
}

/// Whether node `id` is one [`Editor::describe`] made: a text run, whose
/// requests are the editor's ([`Editor::accessibility_action`]).
pub fn is_text_run(id: NodeId) -> bool {
    id.0 >= RUN_IDS
}

/// How many lines either side of the caret's [`Editor::shown_around`]
/// gives: enough to read on from the caret, few enough to hand over at
/// every key.
const AROUND: usize = 40;

impl Editor {
    /// The text as shown around the caret, for a host whose text field
    /// stands in for the document (a browser's, `InputArea`): the caret's
    /// line and [`AROUND`] either side, with the caret and the selection's
    /// anchor in it (the anchor kept inside it), and where it starts in the
    /// whole text as shown — the offsets [`select_shown`](Self::select_shown)
    /// takes. `None` while a composition is open, whose marked text the field
    /// holds itself.
    pub(crate) fn shown_around(&self) -> Option<crate::host::Around> {
        if self.st.composer.borrow().marked().is_some() {
            return None;
        }
        let doc = self.st.doc.borrow();
        let text = Text::of(&doc);
        let (cl, cc) = text.position(&doc, doc.caret);
        let (al, ac) = text.position(&doc, doc.anchor.unwrap_or(doc.caret));
        let first = cl.saturating_sub(AROUND);
        let last = (cl + AROUND).min(text.lines.len() - 1);
        let (al, ac) = match al {
            l if l < first => (first, 0),
            l if l > last => (last, text.lines[last].len()),
            _ => (al, ac),
        };
        let start = text.byte_of(first, 0);
        let end = text.byte_of(last, text.lines[last].len());
        let value = text.value();
        Some(crate::host::Around {
            text: value[start..end].to_owned(),
            start,
            caret: text.byte_of(cl, cc) - start,
            anchor: text.byte_of(al, ac) - start,
        })
    }

    /// Select from byte `from` to byte `to` of the text as shown — the
    /// offsets [`InputArea`](crate::InputArea)'s text is at, from its start —
    /// as the source it shows: what a browser's text field selected before
    /// an input, such as the word an autocorrection replaces.
    pub fn select_shown(&self, from: usize, to: usize) {
        let mut doc = self.st.doc.borrow_mut();
        let text = Text::of(&doc);
        let (l0, c0) = text.locate(from.min(to));
        let (l1, c1) = text.locate(from.max(to));
        let start = text.offset(&doc, l0, c0);
        let end = if to != from {
            text.end(&doc, l1, c1)
        } else {
            start
        };
        doc.select_range(start, end);
    }

    /// Make `node` — the editor's in the host's accessibility tree, the
    /// element [`TEXT_ID`] names — the document's text area, and answer its
    /// text runs, which the host puts in the tree in place of the nodes it
    /// had under `node`. Bounds are in physical pixels from the window's
    /// top-left, as Blitz's are. `None` while the editor has its document
    /// borrowed: `node` is then left as it was.
    pub fn describe(&self, node: &mut Node) -> Option<Vec<(NodeId, Node)>> {
        let t0 = crate::clock::Instant::now();
        let doc = self.st.doc.try_borrow().ok()?;
        let lays = self.sig.lays.peek();
        // The text and its runs hold while the document, the caret (which
        // reveals markup in the rendered view) and the rows' layouts do:
        // across the frames a blinking caret or a scroll draws.
        let key = Key {
            revision: doc.revision(),
            source: doc.view == View::Source,
            caret: doc.caret,
            anchor: doc.anchor,
            layouts: lays.iter().map(|l| Rc::as_ptr(&l.0) as usize).collect(),
        };
        let cached = self.st.described.borrow().clone().filter(|c| c.key == key);
        let cached = cached.unwrap_or_else(|| {
            let text = Text::of(&doc);
            let described = Described::new(text, |l| match lays.get(l.row) {
                // Each visual line after the first starts a run.
                Some(lay) if !self.in_block(l.row) => lay
                    .0
                    .line_cols()
                    .into_iter()
                    .skip(1)
                    .map(|col| l.char_at_col(col))
                    .collect(),
                _ => Vec::new(),
            });
            let nodes = (0..described.runs.len())
                .map(|i| described.run_node(i))
                .collect();
            let c = Rc::new(Cached {
                key,
                described,
                nodes,
            });
            *self.st.described.borrow_mut() = Some(c.clone());
            c
        });
        let described = &cached.described;
        described.text_area(&doc, node);

        let scale = self.st.window.scale_factor();
        let layout = self.sig.layout.peek();
        let page_top = self.sig.origin.peek().1 + self.sig.bar.peek().height();
        let y0 = page_top - *self.sig.top.peek();
        let page_bottom = page_top + self.view_height();
        let (left, width) = (self.left(), *self.sig.width.peek() as f64);
        let px = |x: f64, y: f64, w: f64, h: f64| {
            Rect::new(x * scale, y * scale, (x + w) * scale, (y + h) * scale)
        };
        let runs = (0..described.runs.len())
            .map(|i| {
                let mut n = cached.nodes[i].clone();
                let r = &described.runs[i];
                let l = &described.text.lines[r.line];
                let row = l.row;
                let lay = lays.get(row).filter(|_| !self.in_block(row)).map(|l| &*l.0);
                if row >= layout.rows() {
                    return (run_id(i), n);
                }
                let Some(lay) = lay else {
                    // A block (a table, a picture) draws its rows its own
                    // way: the run is its row's share of it.
                    let top = layout.top(row);
                    n.set_bounds(px(left, y0 + top, width, layout.bottom(row) - top));
                    return (run_id(i), n);
                };
                let vl = r.visual;
                let Some(line) = lay.lines.get(vl) else {
                    return (run_id(i), n);
                };
                let y = y0 + layout.top(row) + line.top as f64;
                let h = line.height as f64;
                n.set_bounds(px(left, y, width, h));
                // Where each character is, for the runs on screen; measuring
                // the rest would cost a layout walk each for nothing seen.
                if y + h > page_top && y < page_bottom {
                    let xs: Vec<f32> = r
                        .chars
                        .clone()
                        .chain(std::iter::once(r.chars.end))
                        .map(|c| {
                            let (at, x) = lay.caret(l.col(c));
                            if at == vl { x } else { f32::NAN }
                        })
                        .collect();
                    let mut positions = Vec::with_capacity(xs.len());
                    let mut widths = Vec::with_capacity(xs.len());
                    for k in 0..r.chars.len() {
                        let x = xs[k];
                        // The last character before a soft wrap ends where
                        // the next visual line starts: as wide as the one
                        // before it.
                        let w = if xs[k + 1].is_nan() {
                            widths.last().copied().unwrap_or(0.0) / scale as f32
                        } else {
                            xs[k + 1] - x
                        };
                        positions.push(x * scale as f32);
                        widths.push(w.max(0.0) * scale as f32);
                    }
                    if r.breaks {
                        let end = xs.last().copied().filter(|x| !x.is_nan());
                        positions.push(end.unwrap_or(0.0) * scale as f32);
                        widths.push(0.0);
                    }
                    n.set_character_positions(positions);
                    n.set_character_widths(widths);
                }
                (run_id(i), n)
            })
            .collect();
        drop(doc);
        crate::trace::trace(|| format!("described the text: {:.1?}", t0.elapsed()));
        Some(runs)
    }

    /// What assistive tech asks of the text area or one of its runs: set its
    /// value, replace its selection, select a range. Done through the
    /// document as typing is, then repainted. Whether it was the editor's to
    /// do: `false` for any other action, and before the first
    /// [`describe`](Self::describe).
    pub fn accessibility_action(&self, request: &ActionRequest) -> bool {
        let st = &*self.st;
        let Some(cached) = st.described.borrow().clone() else {
            return false;
        };
        let done = {
            let mut doc = st.doc.borrow_mut();
            st.composer.borrow_mut().settle(&mut doc);
            cached.described.act(&mut doc, request)
        };
        if done {
            st.goal.set(None);
            self.refresh();
            self.ime_area();
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use accesskit::TreeId;
    use leaf_core::Format;

    use super::*;

    fn rendered(src: &str) -> Doc {
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.set_inline_pictures(true);
        d.build_visual_unwrapped();
        d
    }

    fn source(src: &str) -> Doc {
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.toggle_view();
        d.build_source();
        d
    }

    /// The runs, a line each.
    fn lines(d: &Doc) -> Described {
        Described::new(Text::of(d), |_| Vec::new())
    }

    fn request(action: Action, data: ActionData) -> ActionRequest {
        ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: NodeId(1),
            data: Some(data),
        }
    }

    fn at(node: usize, character_index: usize) -> TextPosition {
        TextPosition {
            node: run_id(node),
            character_index,
        }
    }

    #[test]
    fn a_place_in_the_text_is_a_byte_of_its_value_and_back() {
        let d = rendered("# Title\n\nSome **bold** é text.\n");
        let t = Text::of(&d);
        let value = t.value();
        let at = value.find("bold").unwrap();
        assert_eq!(t.locate(at), (1, 5));
        assert_eq!(t.byte_of(1, 5), at);
        // Past the é, two bytes for one character.
        let after = value.find("text").unwrap();
        assert_eq!(t.byte_of(t.locate(after).0, t.locate(after).1), after);
        // The end of a line is its length.
        assert_eq!(t.byte_of(0, 5), "Title".len());
        // Back to the source: the word shown, past its hidden markup.
        let (l, c) = t.locate(at);
        assert_eq!(&d.source[t.offset(&d, l, c)..][..4], "bold");
    }

    #[test]
    fn the_rendered_text_is_the_text_shown_a_line_a_row() {
        let d = rendered("# Title\n\nSome **bold** text.\n");
        let t = Text::of(&d);
        assert_eq!(t.value(), "Title\nSome bold text.");
        let r = lines(&d);
        let first = r.run_node(0);
        assert_eq!(first.role(), Role::TextRun);
        // The line break ends the line's run, a character of its own.
        assert_eq!(first.value(), Some("Title\n"));
        assert_eq!(first.character_lengths(), &[1, 1, 1, 1, 1, 1]);
        assert_eq!(r.run_node(1).value(), Some("Some bold text."));
        assert_eq!(r.run_node(1).word_starts(), &[0, 5, 10]);
    }

    #[test]
    fn the_source_view_text_is_the_markdown() {
        let d = source("# Title\n\nSome **bold** text.");
        assert_eq!(Text::of(&d).value(), "# Title\n\nSome **bold** text.");
    }

    #[test]
    fn a_character_is_a_grapheme_cluster() {
        let d = source("e\u{301}🙂x");
        let r = lines(&d);
        assert_eq!(r.run_node(0).character_lengths(), &[3, 4, 1]);
    }

    #[test]
    fn the_selection_is_shown_where_the_caret_is() {
        let mut d = rendered("# Title\n\nSome **bold** text.\n");
        let src = d.source.clone();
        let b = src.find("bold").unwrap();
        d.select_range(b, b + 4);
        d.build_visual_unwrapped();
        let r = lines(&d);
        let s = r.selection(&d);
        assert_eq!(s.anchor, at(1, 5));
        assert_eq!(s.focus, at(1, 9));
    }

    #[test]
    fn a_caret_at_a_lines_end_is_on_its_break() {
        let mut d = source("ab\ncd");
        d.select_range(2, 2);
        let s = lines(&d).selection(&d);
        assert_eq!(s.focus, at(0, 2));
        assert_eq!(s.anchor, s.focus);
    }

    #[test]
    fn selecting_a_range_selects_its_source() {
        let mut d = rendered("Some **bold** text.\n");
        let r = lines(&d);
        assert!(r.act(
            &mut d,
            &request(
                Action::SetTextSelection,
                ActionData::SetTextSelection(TextSelection {
                    anchor: at(0, 5),
                    focus: at(0, 9),
                }),
            ),
        ));
        assert_eq!(d.selected_text(), Some("bold"));
    }

    #[test]
    fn replacing_the_selection_types_over_it() {
        let mut d = rendered("Some **bold** text.\n");
        let b = d.source.find("bold").unwrap();
        d.select_range(b, b + 4);
        let r = lines(&d);
        assert!(r.act(
            &mut d,
            &request(Action::ReplaceSelectedText, ActionData::Value("big".into()))
        ));
        assert_eq!(d.source, "Some **big** text.\n");
    }

    #[test]
    fn setting_the_value_edits_only_what_differs() {
        let mut d = rendered("# Title\n\nSome **bold** text.\n");
        let r = lines(&d);
        assert!(r.act(
            &mut d,
            &request(
                Action::SetValue,
                ActionData::Value("Title\nSome bold text, and more.".into())
            ),
        ));
        assert_eq!(d.source, "# Title\n\nSome **bold** text, and more.\n");
    }

    #[test]
    fn setting_the_source_views_value_writes_it_exactly() {
        let mut d = source("# Title\n");
        let r = lines(&d);
        assert!(r.act(
            &mut d,
            &request(Action::SetValue, ActionData::Value("## Other *x*\n".into()))
        ));
        assert_eq!(d.source, "## Other *x*\n");
    }

    #[test]
    fn a_long_line_is_runs_on_one_line() {
        let long = "word ".repeat(120);
        let d = source(long.trim_end());
        let r = lines(&d);
        assert_eq!(r.runs.len(), 3);
        assert_eq!(r.run_node(0).next_on_line(), Some(run_id(1)));
        assert_eq!(r.run_node(1).previous_on_line(), Some(run_id(0)));
        assert_eq!(r.run_node(2).next_on_line(), None);
        // A position past the first run's end is in the second.
        assert_eq!(r.text_position(0, 300), at(1, 45));
    }

    #[test]
    fn a_soft_wrap_starts_a_run() {
        let d = source("one two three");
        let r = Described::new(Text::of(&d), |_| vec![4, 8]);
        let values: Vec<_> = (0..r.runs.len())
            .map(|i| r.run_node(i).value().unwrap().to_string())
            .collect();
        assert_eq!(values, ["one ", "two ", "three"]);
        // At the wrap, the caret is at the next visual line's start.
        assert_eq!(r.text_position(0, 4), at(1, 0));
        assert_eq!(r.text_position(0, 13), at(2, 5));
    }

    #[test]
    fn an_empty_document_is_one_empty_line() {
        let d = rendered("");
        let r = lines(&d);
        assert_eq!(r.runs.len(), 1);
        assert_eq!(r.run_node(0).value(), Some(""));
        let mut node = Node::new(Role::GenericContainer);
        r.text_area(&d, &mut node);
        assert_eq!(node.role(), Role::MultilineTextInput);
        assert_eq!(node.children(), &[run_id(0)]);
    }
}
