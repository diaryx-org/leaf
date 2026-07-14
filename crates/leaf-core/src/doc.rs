//! The document model: a `twig::Editor` plus a byte-offset caret and selection.
//!
//! Where bough moves a selection through the *tree*, leaf moves a *caret*
//! through the *characters* — a normal text editor's model — and expresses
//! every mutation as one of twig's offset-addressed ops:
//!
//!   - typing / delete  → `edit_range(start, end, text)`   (P0)
//!   - re-anchoring      → the returned `Change`            (P1)
//!   - cursor context    → `node_at` / `ancestors_at`       (P3)
//!   - the toolbar       → `wrap_range`/`toggle_inline`/`set_block` (P5)
//!
//! twig reparses after every edit and leaves everything outside the splice
//! byte-for-byte untouched, so the document stays a live, navigable AST while
//! you type into it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use twig::{BlockKind, Editor, FlatNode, Format, InlineKind};

use crate::wysiwyg::{self, VisualMap};

/// Which view the body shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The raw document with a caret in source bytes.
    Source,
    /// Markup resolved to real styles, caret riding the rendered glyphs.
    Wysiwyg,
}

pub struct Doc {
    editor: Editor,
    pub format: Format,
    pub path: PathBuf,
    /// Current source, refreshed from the editor after every successful edit.
    pub source: String,
    /// The caret, as a byte offset into `source` (always on a char boundary).
    pub caret: usize,
    /// The selection's fixed end, if a selection is active; the moving end is
    /// the caret. `None` means no selection.
    pub anchor: Option<usize>,
    pub dirty: bool,
    pub status: Option<String>,
    pub view: View,
    /// The rendered map for the WYSIWYG view, rebuilt each frame; empty in the
    /// source view. Movement and clicks read it to stay in visible space.
    pub vmap: VisualMap,

    // View geometry the renderer stamps each frame, so mouse events can map a
    // screen cell back to a byte offset.
    pub scroll: usize,
    pub body_origin: (u16, u16),
    pub body_height: u16,
}

impl Doc {
    pub fn open(path: PathBuf) -> Result<Self> {
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let format = detect_format(&path)?;
        let editor = Editor::new(&bytes, format).map_err(|e| anyhow!("twig parse: {e}"))?;
        let source = String::from_utf8(bytes).map_err(|_| anyhow!("document is not UTF-8"))?;
        Ok(Doc {
            editor,
            format,
            path,
            source,
            caret: 0,
            anchor: None,
            dirty: false,
            status: None,
            view: View::Source,
            vmap: VisualMap::default(),
            scroll: 0,
            body_origin: (0, 0),
            body_height: 0,
        })
    }

    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            View::Source => View::Wysiwyg,
            View::Wysiwyg => View::Source,
        };
        self.scroll = 0;
        self.status = None;
    }

    pub fn view_name(&self) -> &'static str {
        match self.view {
            View::Source => "source",
            View::Wysiwyg => "wysiwyg",
        }
    }

    /// Rebuild the WYSIWYG visual map for the current tree at `width` columns
    /// (called by the renderer each frame it's in the WYSIWYG view).
    pub fn build_visual(&mut self, width: usize) {
        let nodes = self.nodes();
        self.vmap = wysiwyg::build(&nodes, &self.source, width);
    }

    fn nodes(&mut self) -> Vec<FlatNode> {
        self.editor.nodes().unwrap_or_default()
    }

    pub fn format_name(&self) -> &'static str {
        match self.format {
            Format::Djot => "djot",
            Format::Markdown => "markdown",
            Format::Xml => "xml",
            Format::Html => "html",
        }
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// The selection as an ordered `[start, end)` byte range, or `None` when the
    /// caret and anchor coincide (an empty selection is no selection).
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.anchor
            .map(|a| (a.min(self.caret), a.max(self.caret)))
            .filter(|(s, e)| s != e)
    }

    /// The selected text, or `None` when there's no selection — the source
    /// slice a copy/cut hands to the system clipboard.
    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(s, e)| &self.source[s..e])
    }

    /// The AST breadcrumb at the caret (root → deepest), e.g.
    /// `doc › para › strong`. Read live from twig via `ancestors_at`.
    pub fn breadcrumb(&mut self) -> String {
        match self.editor.ancestors_at(self.caret) {
            Ok(chain) => chain
                .iter()
                .map(|m| m.kind.as_str())
                .collect::<Vec<_>>()
                .join(" › "),
            Err(_) => String::new(),
        }
    }

    // ── editing ──────────────────────────────────────────────────────────────

    /// Replace the byte range `[start, end)` with `text`, re-anchoring the caret
    /// after it. The public form of the internal splice — a pixel frontend that
    /// hit-tests to a byte offset (or an IME that hands back an explicit range)
    /// edits through this, the same twig `edit_range` the caret ops use.
    pub fn edit(&mut self, start: usize, end: usize, text: &str) {
        self.splice(start, end, text);
    }

    /// Insert `text` at the caret, replacing the selection if there is one.
    pub fn insert(&mut self, text: &str) {
        let (s, e) = self.selection().unwrap_or((self.caret, self.caret));
        self.splice(s, e, text);
    }

    pub fn backspace(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "");
        } else if self.caret > 0 {
            let prev = prev_boundary(&self.source, self.caret);
            self.splice(prev, self.caret, "");
        }
    }

    pub fn delete_forward(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "");
        } else if self.caret < self.source.len() {
            let next = next_boundary(&self.source, self.caret);
            self.splice(self.caret, next, "");
        }
    }

    /// Delete from the caret back to the start of the previous word (⌥⌫ /
    /// Ctrl+⌫). Deletes the selection instead when one is active.
    pub fn delete_word_back(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "");
        } else {
            let start = prev_word(&self.source, self.caret);
            if start < self.caret {
                self.splice(start, self.caret, "");
            }
        }
    }

    /// Delete from the caret forward to the end of the next word (⌥⌦ /
    /// Ctrl+Del). Deletes the selection instead when one is active.
    pub fn delete_word_forward(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "");
        } else {
            let end = next_word(&self.source, self.caret);
            if end > self.caret {
                self.splice(self.caret, end, "");
            }
        }
    }

    /// One splice via twig's `edit_range`, then re-anchor the caret from the
    /// returned `Change` and refresh the cached source. A reparse-breaking edit
    /// (rare for Markdown/Djot) leaves the document untouched and reports.
    fn splice(&mut self, start: usize, end: usize, text: &str) {
        match self.editor.edit_range(start, end, text) {
            Ok(change) => {
                self.refresh();
                self.caret = change.new.end;
                self.anchor = None;
                self.dirty = true;
                self.status = None;
            }
            Err(e) => self.status = Some(format!("edit: {e}")),
        }
    }

    /// Toggle an inline mark over the selection (Bold / Italic / Code / …). Keeps
    /// the toggled region selected so a second press cleanly reverses it.
    pub fn toggle(&mut self, kind: InlineKind) {
        let Some((s, e)) = self.selection() else {
            self.status = Some("select text first".into());
            return;
        };
        match self.editor.toggle_inline(s, e, kind) {
            Ok(change) => {
                self.refresh();
                self.anchor = Some(change.new.start);
                self.caret = change.new.end;
                self.dirty = true;
                self.status = None;
            }
            Err(e) => self.status = Some(format!("{kind:?}: {e}")),
        }
    }

    /// Convert the block at the caret to a heading level or paragraph.
    pub fn set_block(&mut self, kind: BlockKind) {
        match self.editor.set_block(self.caret, kind) {
            Ok(_) => {
                self.refresh();
                self.clamp_caret();
                self.anchor = None;
                self.dirty = true;
                self.status = None;
            }
            Err(e) => self.status = Some(format!("{kind:?}: {e}")),
        }
    }

    pub fn save(&mut self) {
        match std::fs::write(&self.path, self.source.as_bytes()) {
            Ok(()) => {
                self.dirty = false;
                self.status = Some(format!("saved {}", self.file_name()));
            }
            Err(e) => self.status = Some(format!("save failed: {e}")),
        }
    }

    fn refresh(&mut self) {
        if let Ok(s) = self.editor.source_str() {
            self.source = s;
        }
        self.clamp_caret();
    }

    // ── caret movement ─────────────────────────────────────────────────────────
    // `extend` grows the selection (Shift+motion): it pins the anchor on the
    // first extended step and moves only the caret; an un-extended motion drops
    // the selection.

    /// Place the caret at byte `offset` (clamped to a char boundary), extending
    /// the selection when `extend` is set. The public form of `move_to`, for a
    /// frontend that hit-tests pixels straight to a source offset.
    pub fn place_caret(&mut self, offset: usize, extend: bool) {
        self.move_to(offset, extend);
        self.clamp_caret();
    }

    /// Select the whole document (⌘A / Ctrl+A).
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.source.len();
        self.status = None;
    }

    /// Select the word (or whitespace / punctuation run) at `offset` — the
    /// double-click gesture. Anchors on the run's start with the caret at its
    /// end so a following Shift-motion extends from the far edge.
    pub fn select_word_at(&mut self, offset: usize) {
        let (s, e) = word_range_at(&self.source, offset.min(self.source.len()));
        self.anchor = Some(s);
        self.caret = e;
        self.status = None;
        self.clamp_caret();
    }

    fn move_to(&mut self, offset: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
        self.caret = offset.min(self.source.len());
        self.status = None;
    }

    // In the source view, motion walks source bytes / source lines. In the
    // WYSIWYG view it walks the rendered glyph grid (the visual map), which is
    // what steps the caret cleanly over hidden delimiters.

    pub fn move_left(&mut self, extend: bool) {
        let target = match self.view {
            View::Source => {
                if self.caret > 0 {
                    prev_boundary(&self.source, self.caret)
                } else {
                    0
                }
            }
            View::Wysiwyg => {
                let (r, c) = self.vmap.pos_of_offset(self.caret);
                if c > 0 {
                    self.vmap.offset_of_pos(r, c - 1)
                } else if r > 0 {
                    self.vmap.offset_of_pos(r - 1, self.vmap.row_len(r - 1))
                } else {
                    self.caret
                }
            }
        };
        self.move_to(target, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        let target = match self.view {
            View::Source => {
                if self.caret < self.source.len() {
                    next_boundary(&self.source, self.caret)
                } else {
                    self.caret
                }
            }
            View::Wysiwyg => {
                let (r, c) = self.vmap.pos_of_offset(self.caret);
                if c < self.vmap.row_len(r) {
                    self.vmap.offset_of_pos(r, c + 1)
                } else if r + 1 < self.vmap.num_rows() {
                    self.vmap.offset_of_pos(r + 1, 0)
                } else {
                    self.caret
                }
            }
        };
        self.move_to(target, extend);
    }

    /// Move to the start of the previous word (⌥← / Ctrl+←). Word boundaries
    /// are computed over the source in both views, since the source is the
    /// document of record and the caret is always a source offset.
    pub fn move_word_left(&mut self, extend: bool) {
        let target = prev_word(&self.source, self.caret);
        self.move_to(target, extend);
    }

    /// Move to the end of the next word (⌥→ / Ctrl+→).
    pub fn move_word_right(&mut self, extend: bool) {
        let target = next_word(&self.source, self.caret);
        self.move_to(target, extend);
    }

    pub fn move_up(&mut self, extend: bool) {
        let (row, col) = self.caret_pos();
        if row == 0 {
            return;
        }
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row - 1, col),
            View::Wysiwyg => self.vmap.offset_of_pos(row - 1, col.min(self.vmap.row_len(row - 1))),
        };
        self.move_to(target, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        let (row, col) = self.caret_pos();
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row + 1, col),
            View::Wysiwyg => {
                if row + 1 >= self.vmap.num_rows() {
                    return;
                }
                self.vmap.offset_of_pos(row + 1, col.min(self.vmap.row_len(row + 1)))
            }
        };
        self.move_to(target, extend);
    }

    pub fn move_home(&mut self, extend: bool) {
        let (row, _) = self.caret_pos();
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row, 0),
            View::Wysiwyg => self.vmap.offset_of_pos(row, 0),
        };
        self.move_to(target, extend);
    }

    pub fn move_end(&mut self, extend: bool) {
        let (row, _) = self.caret_pos();
        let target = match self.view {
            View::Source => line_end(&self.source, row),
            View::Wysiwyg => self.vmap.offset_of_pos(row, self.vmap.row_len(row)),
        };
        self.move_to(target, extend);
    }

    /// Point the caret at the body cell `(row, col)` the mouse landed on.
    pub fn click(&mut self, row: usize, col: usize, extend: bool) {
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row, col),
            View::Wysiwyg => self.vmap.offset_of_pos(row, col),
        };
        self.move_to(target, extend);
    }

    /// The caret's screen position `(row, col)` in the active view's grid.
    pub fn caret_pos(&self) -> (usize, usize) {
        match self.view {
            View::Source => offset_to_row_col(&self.source, self.caret),
            View::Wysiwyg => self.vmap.pos_of_offset(self.caret),
        }
    }

    fn clamp_caret(&mut self) {
        if self.caret > self.source.len() {
            self.caret = self.source.len();
        }
        while self.caret > 0 && !self.source.is_char_boundary(self.caret) {
            self.caret -= 1;
        }
    }
}

// ── byte-offset ⇄ (row, col) helpers ─────────────────────────────────────────

fn prev_boundary(s: &str, i: usize) -> usize {
    let mut j = i - 1;
    while j > 0 && !s.is_char_boundary(j) {
        j -= 1;
    }
    j
}

fn next_boundary(s: &str, i: usize) -> usize {
    let mut j = i + 1;
    while j < s.len() && !s.is_char_boundary(j) {
        j += 1;
    }
    j
}

// ── word boundaries ──────────────────────────────────────────────────────────
// The shared primitive behind word-wise motion, word deletion, and
// double-click-to-select-a-word. A "word" is a maximal run of one character
// class; whitespace and punctuation are their own classes, so motion skips
// cleanly between them the way native text fields do.

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Word,
    Space,
    Other,
}

fn classify(c: char) -> Class {
    if c == '_' || c.is_alphanumeric() {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// The offset at the end of the next word to the right of `i` (⌥→ / Ctrl+→):
/// skip any leading separators, then consume the following word run.
fn next_word(s: &str, i: usize) -> usize {
    let mut off = i;
    let mut in_word = false;
    for c in s[i..].chars() {
        if classify(c) == Class::Word {
            in_word = true;
        } else if in_word {
            break;
        }
        off += c.len_utf8();
    }
    off
}

/// The offset at the start of the word to the left of `i` (⌥← / Ctrl+←):
/// skip separators walking left, then consume the preceding word run.
fn prev_word(s: &str, i: usize) -> usize {
    let mut off = i;
    let mut in_word = false;
    for c in s[..i].chars().rev() {
        if classify(c) == Class::Word {
            in_word = true;
        } else if in_word {
            break;
        }
        off -= c.len_utf8();
    }
    off
}

/// The `[start, end)` run of same-class characters surrounding `off` — the
/// word (or whitespace/punctuation run) a double-click selects. At end-of-text
/// the run ending there is used.
fn word_range_at(s: &str, off: usize) -> (usize, usize) {
    if s.is_empty() {
        return (0, 0);
    }
    let off = off.min(s.len());
    let reference = if off < s.len() {
        s[off..].chars().next()
    } else {
        s[..off].chars().next_back()
    };
    let Some(rc) = reference else {
        return (off, off);
    };
    let class = classify(rc);

    let mut start = off;
    for c in s[..start].chars().rev() {
        if classify(c) == class {
            start -= c.len_utf8();
        } else {
            break;
        }
    }
    let mut end = off;
    for c in s[end..].chars() {
        if classify(c) == class {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    (start, end)
}

/// `(row, col)` of byte offset `off`, col counted in characters from line start.
fn offset_to_row_col(s: &str, off: usize) -> (usize, usize) {
    let off = off.min(s.len());
    let mut row = 0;
    let mut line_start = 0;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if i >= off {
            break;
        }
        if b == b'\n' {
            row += 1;
            line_start = i + 1;
        }
    }
    (row, s[line_start..off].chars().count())
}

/// The byte offset of `col` chars into `row` (clamped to that line's end).
fn row_col_to_offset(s: &str, row: usize, col: usize) -> usize {
    let start = line_start(s, row);
    let end = line_end_from(s, start);
    let mut off = start;
    for _ in 0..col {
        if off >= end {
            break;
        }
        off = next_boundary(s, off);
    }
    off
}

fn line_start(s: &str, row: usize) -> usize {
    if row == 0 {
        return 0;
    }
    let mut r = 0;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if b == b'\n' {
            r += 1;
            if r == row {
                return i + 1;
            }
        }
    }
    s.len()
}

fn line_end(s: &str, row: usize) -> usize {
    line_end_from(s, line_start(s, row))
}

fn line_end_from(s: &str, start: usize) -> usize {
    s[start..].find('\n').map(|p| start + p).unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(name: &str, body: &str) -> Doc {
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_test_{name}.md"));
        std::fs::write(&p, body).unwrap();
        Doc::open(p).unwrap()
    }

    // ── golden-case harness ──────────────────────────────────────────────────
    // The pattern the whole parity suite can reuse: write a fixture with the
    // caret marked by `|`, run one action, and compare the rendered result —
    // also caret-marked — against the expected string. One readable line per
    // behavior, and it exercises the exact `Doc` ops both frontends call.

    /// Split a `|`-marked fixture into `(source, caret_offset)`.
    fn parse_caret(marked: &str) -> (String, usize) {
        let caret = marked.find('|').expect("fixture needs a `|` caret marker");
        (marked.replacen('|', "", 1), caret)
    }

    /// Render a doc's source with `|` at the caret (and `[`…`]` around any
    /// selection) so a result reads like the fixtures.
    fn render_caret(d: &Doc) -> String {
        // (offset, rank, char); rank keeps coincident markers ordered `[ | ]`
        // so the caret always renders inside its own selection.
        let mut marks: Vec<(usize, u8, char)> = vec![(d.caret, 1, '|')];
        if let Some((s, e)) = d.selection() {
            marks.push((s, 0, '['));
            marks.push((e, 2, ']'));
        }
        // Insert right-to-left: descending offset, then descending rank.
        marks.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        let mut out = d.source.clone();
        for (at, _, ch) in marks {
            out.insert(at, ch);
        }
        out
    }

    /// Load a `|`-marked fixture, run `action`, return the caret-marked result.
    fn golden(name: &str, marked: &str, action: impl FnOnce(&mut Doc)) -> String {
        let (src, caret) = parse_caret(marked);
        let mut d = doc_with(name, &src);
        d.caret = caret;
        action(&mut d);
        render_caret(&d)
    }

    #[test]
    fn word_motion_walks_word_by_word() {
        let g = |m, f: fn(&mut Doc)| golden("word_motion", m, f);
        assert_eq!(g("hello wor|ld", |d| d.move_word_left(false)), "hello |world");
        assert_eq!(g("hello| world", |d| d.move_word_left(false)), "|hello world");
        assert_eq!(g("hel|lo world", |d| d.move_word_right(false)), "hello| world");
        assert_eq!(g("hello| world", |d| d.move_word_right(false)), "hello world|");
        // Punctuation is its own class, so motion stops at the boundary.
        assert_eq!(g("|foo.bar", |d| d.move_word_right(false)), "foo|.bar");
    }

    #[test]
    fn word_motion_extends_the_selection_when_asked() {
        assert_eq!(
            golden("word_sel", "hello |world", |d| d.move_word_right(true)),
            "hello [world|]"
        );
    }

    #[test]
    fn delete_word_removes_a_whole_word() {
        let g = |m, f: fn(&mut Doc)| golden("del_word", m, f);
        assert_eq!(g("hello world|", |d| d.delete_word_back()), "hello |");
        assert_eq!(g("hello |world", |d| d.delete_word_forward()), "hello |");
        assert_eq!(g("foo |bar baz", |d| d.delete_word_back()), "|bar baz");
    }

    #[test]
    fn select_all_spans_the_document() {
        let mut d = doc_with("sel_all", "abc\n\ndef\n");
        d.select_all();
        assert_eq!(d.selection(), Some((0, d.source.len())));
    }

    #[test]
    fn select_word_at_picks_the_surrounding_word() {
        let mut d = doc_with("sel_word", "hello world\n");
        d.select_word_at(8); // inside "world"
        assert_eq!(d.selection(), Some((6, 11)));
        // Double-clicking at end-of-word still grabs the word to its left.
        d.select_word_at(5); // the space between the words
        assert_eq!(d.selection(), Some((5, 6)));
    }

    #[test]
    fn word_helpers_respect_utf8_boundaries() {
        // "café" is 5 bytes ('é' is two); motion must land on char boundaries.
        assert_eq!(golden("utf8", "|café ok", |d| d.move_word_right(false)), "café| ok");
        assert_eq!(golden("utf8b", "café |ok", |d| d.delete_word_back()), "|ok");
    }

    #[test]
    fn typing_inserts_at_the_caret_and_advances_it() {
        let mut d = doc_with("type", "hello\n");
        d.insert("Hi ");
        assert_eq!(d.source, "Hi hello\n");
        assert_eq!(d.caret, 3);
        assert!(d.dirty);
    }

    #[test]
    fn backspace_deletes_the_char_before_the_caret() {
        let mut d = doc_with("bs", "hello\n");
        d.caret = 3; // after "hel"
        d.backspace();
        assert_eq!(d.source, "helo\n");
        assert_eq!(d.caret, 2);
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut d = doc_with("replace", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6; // "word" selected
        d.insert("X");
        assert_eq!(d.source, "a X b\n");
        assert_eq!(d.caret, 3);
        assert_eq!(d.anchor, None);
    }

    #[test]
    fn toggle_bold_wraps_then_unwraps_the_selection() {
        let mut d = doc_with("bold", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a **word** b\n");
        // The toggled region stays selected, so a second toggle reverses it.
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a word b\n");
    }

    #[test]
    fn set_block_makes_a_heading_at_the_caret() {
        let mut d = doc_with("head", "Title\n\nbody\n");
        d.caret = 0;
        d.set_block(BlockKind::Heading(2));
        assert_eq!(d.source, "## Title\n\nbody\n");
        d.set_block(BlockKind::Paragraph);
        assert_eq!(d.source, "Title\n\nbody\n");
    }

    #[test]
    fn click_maps_a_row_col_to_a_byte_offset() {
        let mut d = doc_with("click", "ab\ncd\n");
        d.click(1, 1, false); // row 1 ("cd"), col 1 -> the 'd'
        assert_eq!(d.caret, 4);
    }

    fn wysiwyg_doc(name: &str, body: &str) -> Doc {
        let mut d = doc_with(name, body);
        d.view = View::Wysiwyg;
        d.build_visual(80);
        d
    }

    #[test]
    fn wysiwyg_down_crosses_a_paragraph_boundary() {
        // Regression: the blank separator row used to share the previous
        // paragraph's end offset, so Down got pinned at the boundary (while Up
        // still crossed). Both directions must now step through it symmetrically.
        let mut d = wysiwyg_doc("wys_down", "abc\n\ndef\n");
        d.caret = 3; // end of "abc" (row 0)
        d.move_down(false); // onto the blank separator line
        assert_eq!(d.caret_pos().0, 1, "Down should reach the separator row");
        d.move_down(false); // onto "def"
        assert_eq!(d.caret_pos().0, 2, "Down should reach the second paragraph");
        assert_eq!(d.caret, 5); // start of "def"
    }

    #[test]
    fn wysiwyg_up_and_down_are_inverse_across_paragraphs() {
        let mut d = wysiwyg_doc("wys_updown", "abc\n\ndef\n");
        d.caret = 5; // start of "def"
        let start = d.caret_pos();
        d.move_up(false);
        d.move_up(false);
        assert_eq!(d.caret_pos().0, 0, "two Ups reach the first paragraph");
        d.move_down(false);
        d.move_down(false);
        assert_eq!(d.caret_pos(), start, "Down retraces Up exactly");
    }

    #[test]
    fn wysiwyg_new_paragraph_shows_before_typing() {
        // Regression: two Enters at the end of a paragraph produced trailing
        // newlines with no AST node, so the caret appeared stuck on the old line
        // until a character was typed. It must ride down onto the new line now.
        let mut d = doc_with("wys_newpara", "abc\n");
        d.view = View::Wysiwyg;
        d.caret = 3;
        d.insert("\n");
        d.insert("\n"); // source is now "abc\n\n\n", caret at 5
        assert_eq!(d.source, "abc\n\n\n");
        d.build_visual(80);
        let (row, _) = d.caret_pos();
        assert!(row >= 2, "caret should have moved down to the new line, got row {row}");
        assert!(d.vmap.num_rows() >= 3, "the blank lines should render as rows");
    }

    #[test]
    fn vertical_motion_keeps_the_column() {
        let mut d = doc_with("move", "abcd\nef\n");
        d.caret = 3; // "abc|d" on row 0, col 3
        d.move_down(false); // row 1 "ef" only has cols 0..2 -> clamps to end
        assert_eq!(d.caret, 7); // just after "ef"
    }
}

fn detect_format(path: &Path) -> Result<Format> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    Ok(match ext.as_str() {
        "dj" | "djot" => Format::Djot,
        "md" | "markdown" => Format::Markdown,
        "xml" => Format::Xml,
        "html" | "htm" => Format::Html,
        other => return Err(anyhow!("unknown document extension: .{other}")),
    })
}
