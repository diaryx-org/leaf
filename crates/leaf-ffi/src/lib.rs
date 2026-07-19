//! leaf-ffi — the Swift / C-ABI frontend binding for leaf.
//!
//! This is the native-Apple analogue of `leaf-wasm`: it takes `leaf-core`'s
//! frontend-neutral [`Doc`] — the byte-offset caret model and the AST→glyph
//! [`VisualMap`] — and exposes it across a C ABI (via UniFFI) in the shape an
//! AppKit/SwiftUI renderer wants. Core stays the single source of truth for the
//! text, the caret math, and the offset⇄position mapping; the Swift side only
//! paints glyphs and forwards key/mouse events back in, exactly as the TUI, gpui,
//! and wasm frontends do.
//!
//! ## The boundary is style *runs*, not glyphs
//!
//! [`Doc::build_visual`] resolves the document to rows of per-character glyphs,
//! each tagged with a semantic [`Role`] and the author's emphasis. Sending one
//! object per character would make every keystroke O(document) in boundary
//! crossings. Instead [`LeafDoc::view`] coalesces each row's glyphs into maximal
//! **runs** of identical style and ships those — a handful of records per line.
//! The Swift renderer maps each run's `role` to a font/size/weight and its
//! emphasis flags to traits, the native counterpart of the TUI's `to_ratatui`
//! and the web's CSS class.
//!
//! ## Core owns the grid; Swift owns the pixels
//!
//! Core lays a row out in whole character *columns* (a terminal-cell measure),
//! and every offset⇄position method speaks that grid. It deliberately does *not*
//! dictate presentation. So a native renderer is *proportional* — body text in a
//! real family, headings by **size** and weight, code in a monospace panel — and
//! never multiplies `col × cell_width`. It lets `NSLayoutManager` / Core Text
//! shape each row, places the caret at [`DocView::caret_ch`] (a UTF-16 offset,
//! which is exactly what `NSAttributedString` and `NSTextView` count in), and
//! hit-tests a click through `characterIndex(for:)`, feeding the resulting
//! row + UTF-16 offset back through [`LeafDoc::click_ch`]. Core measures nothing
//! in pixels; Swift positions nothing in the model.
//!
//! ## Threading
//!
//! A UniFFI object is handed to Swift as a reference-counted handle whose methods
//! take `&self`, so the [`Doc`] lives behind a [`Mutex`]. Every call locks, edits
//! or reads, and returns a fresh [`DocView`] — one boundary crossing both mutates
//! and repaints, same as the wasm frontend. Drive it from the main thread.

use std::sync::{Arc, Mutex};

use leaf_core::style::{Role, Style as LStyle};
use leaf_core::wysiwyg::text_width;
use leaf_core::{BlockKind, Doc, Format, InlineKind, View, VisualMap};
use unicode_segmentation::UnicodeSegmentation;

uniffi::setup_scaffolding!();

/// A parse failure constructing a document — the only fallible entry point. Every
/// other method is infallible (it operates on an already-parsed model), so they
/// return a [`DocView`] directly.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum LeafError {
    /// The `format` string handed to [`LeafDoc::new`] wasn't one leaf understands.
    #[error("unknown format: {name}")]
    UnknownFormat { name: String },
    /// `leaf-core` failed to parse `source` as the requested format.
    #[error("parse error: {message}")]
    Parse { message: String },
}

/// One maximal span of same-styled glyphs on a visual row — the unit the Swift
/// renderer turns into a single styled attributed-string run.
#[derive(uniffi::Record)]
pub struct Run {
    /// The run's text, glyphs concatenated in column order.
    pub text: String,
    /// The glyph's semantic role as a renderer class id: `body`, `h1`…`h6`,
    /// `code`, `link`, `mark`, `list`, `quote`, `rule`.
    pub role: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Whether this run lies inside the active selection — so the renderer can
    /// paint a selection background without re-deriving it from offsets.
    pub sel: bool,
}

/// One visual line: its styled runs plus the row-level flags a frontend draws
/// chrome from.
#[derive(uniffi::Record)]
pub struct Row {
    pub runs: Vec<Run>,
    /// Drawn but holds no caret (a table rule, a block-gap blank line): the
    /// renderer skips it for click/caret math. See [`leaf_core::VRow`].
    pub decoration: bool,
    /// A fenced/indented code-block line — the renderer draws a tinted, bordered
    /// panel around each maximal run of these.
    pub code: bool,
    /// A fenced block's language, carried on the block's first code row only.
    pub code_lang: Option<String>,
    /// The heading level (1–6) if this row belongs to a heading block, else
    /// `None`. A proportional renderer sizes the *whole* row from this so an
    /// inline `` `code` `` run inside a heading still reads at the heading's size.
    pub heading: Option<u8>,
}

/// A whole rendered frame: the rows to paint, where the caret sits, and the
/// toolbar state — everything the Swift side needs for one repaint, in one value.
/// Returned by every view-producing method.
#[derive(uniffi::Record)]
pub struct DocView {
    pub rows: Vec<Row>,
    /// The caret's row: an index into [`Self::rows`].
    pub caret_row: u32,
    /// The caret's display *column* within its row — core's grid position. Kept
    /// for callers reasoning in columns; a proportional renderer wants
    /// [`Self::caret_ch`] instead.
    pub caret_col: u32,
    /// The caret's offset within its row's text in **UTF-16 code units** — what
    /// `NSAttributedString`/`NSTextView` count to. This is `caret_col` mapped
    /// through the row's grapheme widths, so it lands the caret correctly past
    /// wide glyphs (CJK, emoji) where a column and a character index diverge.
    pub caret_ch: u32,
    /// Whether a (non-empty) selection is active.
    pub has_selection: bool,
    /// The selection's *fixed* end (the caret is the moving end), as a row and a
    /// UTF-16 offset — so the renderer can restore a native selection with the
    /// same direction the model has. Equal to the caret when `has_selection` is
    /// false.
    pub anchor_row: u32,
    pub anchor_ch: u32,
    /// Whether the buffer differs from the last saved bytes — for a "● modified"
    /// affordance.
    pub dirty: bool,
    /// `"wysiwyg"` or `"source"`, for a view-toggle affordance.
    pub view: String,
    /// The heading level at the caret, if any — a toolbar lights H1…H6 from it.
    pub heading: Option<u32>,
    /// The inline marks active at the caret (`bold`, `italic`, `code`, …) — the
    /// toolbar lights the matching buttons.
    pub active: Vec<String>,
}

/// A visual position: a row index plus a UTF-16 offset within that row's text —
/// the coordinate the geometry side (Core Text) draws from. Returned by
/// [`LeafDoc::pos_for_offset`], the bridge from a source offset (what a
/// `UITextPosition` wraps) to where it sits on screen.
#[derive(uniffi::Record)]
pub struct RowCol {
    pub row: u32,
    pub ch: u32,
}

/// A live leaf document bound for a native Apple frontend: `leaf_core::Doc` plus
/// the wrap width the current viewport implies, behind a mutex. Constructed from
/// an in-memory string and driven entirely through method calls — there is no
/// filesystem behind it.
#[derive(uniffi::Object)]
pub struct LeafDoc {
    inner: Mutex<Inner>,
}

/// The guarded state. Its methods assume the lock is held (they take `&mut
/// self`); the [`LeafDoc`] exported wrappers acquire it, delegate, and return the
/// resulting frame.
struct Inner {
    doc: Doc,
    /// The wrap mode. `Some(cols)` wraps the map at that column budget (a terminal,
    /// or a fixed-cell frontend); `None` builds it **unwrapped** — one row per block —
    /// for a proportional GUI that wraps at its own pixel width. `build_visual`
    /// caches on `(revision, width)`, so re-syncing when neither moved is free.
    width: Option<usize>,
}

// SAFETY: `Doc` embeds a `twig::Editor`, which holds a `NonNull<TwigEditor>` and
// is therefore `!Send`. UniFFI hands `LeafDoc` to Swift as a reference-counted
// handle that must be `Send + Sync`, so `Inner` must be `Send`. This is sound
// because:
//   1. Every access goes through `LeafDoc::lock()` — the `Mutex` serializes all
//      reads and mutations, so there is never concurrent access to the handle.
//   2. twig's editor handle owns a plain heap allocation with no thread-affinity
//      (no thread-locals, no per-thread state) — moving the pointer between
//      threads is fine as long as use is serialized, which (1) guarantees.
// The intended usage is still main-thread-driven; this impl only permits the
// handle to cross threads safely, it does not invite concurrent use.
unsafe impl Send for Inner {}

impl Inner {
    /// Rebuild the visual map at the current width. Cheap (cached) when nothing
    /// changed; the guard that lets every movement/click method assume a fresh
    /// grid regardless of call order.
    fn sync(&mut self) {
        match self.width {
            Some(w) => self.doc.build_visual(w),
            None => self.doc.build_visual_unwrapped(),
        }
    }

    /// The plain text of visual row `row` in the active view — the string the
    /// renderer concatenates its runs into. Backs the column⇄UTF-16 mapping.
    fn row_text(&self, row: usize) -> String {
        match self.doc.view {
            View::Wysiwyg => self
                .doc
                .vmap
                .rows
                .get(row)
                .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
                .unwrap_or_default(),
            View::Source => self.doc.source.split('\n').nth(row).unwrap_or("").to_string(),
        }
    }

    /// The `(row, display-column)` a source offset sits at in the active view.
    fn pos_of_offset(&self, off: usize) -> (usize, usize) {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.pos_of_offset(off),
            View::Source => {
                let s = &self.doc.source;
                let off = off.min(s.len());
                let row = s[..off].bytes().filter(|&b| b == b'\n').count();
                let line_start = s[..off].rfind('\n').map_or(0, |i| i + 1);
                (row, text_width(&s[line_start..off]))
            }
        }
    }

    /// The source offset under a click at row `row`, `ch` UTF-16 units in.
    fn offset_at(&mut self, row: usize, ch: usize) -> usize {
        self.sync();
        let col = utf16_to_col(&self.row_text(row), ch);
        self.doc.click(row, col, false);
        self.doc.caret
    }

    // ── position mapping for UITextInput (non-mutating; caret untouched) ───────
    // These branch by view exactly as `pos_of_offset` does, so the WYSIWYG map and
    // the raw-source grid each answer in their own coordinates.

    /// The source offset of display column `col` on visual `row` — the inverse of
    /// [`Self::pos_of_offset`] in column space.
    fn offset_of_col(&self, row: usize, col: usize) -> usize {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.offset_of_pos(row, col),
            View::Source => {
                let line = self.row_text(row);
                let (mut c, mut b) = (0usize, 0usize);
                for g in line.graphemes(true) {
                    if c >= col {
                        break;
                    }
                    c += text_width(g);
                    b += g.len();
                }
                self.source_line_start(row) + b
            }
        }
    }

    /// The byte offset where visual `row` begins in the source view.
    fn source_line_start(&self, row: usize) -> usize {
        self.doc.source.split('\n').take(row).map(|l| l.len() + 1).sum()
    }

    /// The next caret stop after `off`, or `None` at the end.
    fn stop_after(&self, off: usize) -> Option<usize> {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.stop_after(off),
            View::Source => {
                let s = &self.doc.source;
                if off >= s.len() {
                    None
                } else {
                    Some(s[off..].grapheme_indices(true).nth(1).map_or(s.len(), |(i, _)| off + i))
                }
            }
        }
    }

    /// The previous caret stop before `off`, or `None` at the start.
    fn stop_before(&self, off: usize) -> Option<usize> {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.stop_before(off),
            View::Source => {
                let s = &self.doc.source;
                let off = off.min(s.len());
                if off == 0 {
                    None
                } else {
                    s[..off].grapheme_indices(true).next_back().map(|(i, _)| i)
                }
            }
        }
    }

    /// Snap `off` to a valid caret stop (WYSIWYG) / char boundary (source).
    fn snap_stop(&self, off: usize) -> usize {
        let s = &self.doc.source;
        let mut off = off.min(s.len());
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.snap_to_stop(off),
            View::Source => {
                while off > 0 && !s.is_char_boundary(off) {
                    off -= 1;
                }
                off
            }
        }
    }

    /// The navigable visual row above `row`, if any.
    fn nav_above(&self, row: usize) -> Option<usize> {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.navigable_above(row),
            View::Source => (row > 0).then(|| row - 1),
        }
    }

    /// The navigable visual row below `row`, if any.
    fn nav_below(&self, row: usize) -> Option<usize> {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.navigable_below(row),
            View::Source => {
                let n = self.doc.source.split('\n').count();
                (row + 1 < n).then_some(row + 1)
            }
        }
    }

    /// Resolve the current document to a renderable frame of style runs. Called
    /// for the first paint, on resize, and by every mutating wrapper so one
    /// boundary crossing both edits and repaints.
    fn view(&mut self) -> DocView {
        self.sync();

        let (ss, se) = self.doc.selection().unwrap_or((usize::MAX, usize::MAX));

        // The two views speak different grids — the WYSIWYG map's resolved glyphs
        // vs the raw source split on newlines — and `caret_pos` branches to match,
        // so the rows must too or the caret lands on the wrong text.
        let rows = match self.doc.view {
            View::Wysiwyg => wysiwyg_rows(&self.doc.vmap, ss, se),
            View::Source => source_rows(&self.doc.source, ss, se),
        };

        let (caret_row, caret_col) = self.doc.caret_pos();
        // Map the caret's display column to a UTF-16 text offset so a native
        // renderer can place it past wide glyphs (see [`DocView::caret_ch`]).
        let caret_ch = col_to_utf16(&self.row_text(caret_row), caret_col);
        // The selection's fixed (anchor) end, in the same row/UTF-16 terms.
        let (has_selection, anchor_row, anchor_ch) = match self.doc.selection() {
            Some(_) => {
                let a = self.doc.anchor.unwrap_or(self.doc.caret);
                let (ar, ac) = self.pos_of_offset(a);
                (true, ar, col_to_utf16(&self.row_text(ar), ac))
            }
            None => (false, caret_row, caret_ch),
        };
        let heading = self.doc.current_heading_level();
        let active = self
            .doc
            .active_inline_marks()
            .iter()
            .map(|k| mark_id(k).to_string())
            .collect();

        DocView {
            rows,
            caret_row: caret_row as u32,
            caret_col: caret_col as u32,
            caret_ch: caret_ch as u32,
            has_selection,
            anchor_row: anchor_row as u32,
            anchor_ch: anchor_ch as u32,
            dirty: self.doc.dirty,
            view: self.doc.view_name().to_string(),
            heading,
            active,
        }
    }
}

#[uniffi::export]
impl LeafDoc {
    /// Parse `source` as `format` (`"markdown"`/`"md"`, `"djot"`/`"dj"`,
    /// `"html"`, `"xml"`) into a live, untitled document.
    #[uniffi::constructor]
    pub fn new(source: String, format: String) -> Result<Arc<Self>, LeafError> {
        let format = match format.to_ascii_lowercase().as_str() {
            "markdown" | "md" => Format::Markdown,
            "djot" | "dj" => Format::Djot,
            "html" | "htm" => Format::Html,
            "xml" => Format::Xml,
            other => return Err(LeafError::UnknownFormat { name: other.to_string() }),
        };
        let doc = Doc::from_source(source, format)
            .map_err(|e| LeafError::Parse { message: e.to_string() })?;
        Ok(Arc::new(LeafDoc { inner: Mutex::new(Inner { doc, width: Some(80) }) }))
    }

    /// Resolve the current document to a renderable frame — the first paint.
    pub fn view(&self) -> DocView {
        self.lock().view()
    }

    /// Set the wrap width (in columns) the viewport implies and repaint. For a
    /// fixed-cell frontend (a terminal); a proportional GUI uses [`set_unwrapped`].
    pub fn set_width(&self, cols: u32) -> DocView {
        let mut g = self.lock();
        g.width = Some((cols as usize).max(1));
        g.view()
    }

    /// Switch to **unwrapped** layout — one visual row per block, no column wrapping —
    /// and repaint. A proportional GUI calls this once at start-up, then wraps each
    /// row at its own pixel width (the caret/hit/selection geometry it derives from
    /// the pixel wrap; core still owns the caret model, in byte offsets). Idempotent
    /// and cheap to leave in place across edits.
    pub fn set_unwrapped(&self) -> DocView {
        let mut g = self.lock();
        g.width = None;
        g.view()
    }

    /// The current source text — for a save (write to disk / iCloud / a document
    /// wrapper) or a source-view display.
    pub fn source(&self) -> String {
        self.lock().doc.source.clone()
    }

    /// The selected text, if any — for a clipboard copy/cut.
    pub fn selected_text(&self) -> Option<String> {
        self.lock().doc.selected_text().map(str::to_string)
    }

    /// Mark the buffer saved after the host persisted [`LeafDoc::source`] its own
    /// way — clears the dirty flag without touching a filesystem.
    pub fn mark_saved(&self) -> DocView {
        let mut g = self.lock();
        g.doc.mark_saved();
        g.view()
    }

    // ── text input ───────────────────────────────────────────────────────────

    pub fn insert(&self, text: String) -> DocView {
        let mut g = self.lock();
        g.doc.insert(&text);
        g.view()
    }

    pub fn paste(&self, text: String) -> DocView {
        let mut g = self.lock();
        g.doc.paste(&text);
        g.view()
    }

    pub fn newline(&self) -> DocView {
        let mut g = self.lock();
        g.doc.newline();
        g.view()
    }

    pub fn backspace(&self) -> DocView {
        let mut g = self.lock();
        g.doc.backspace();
        g.view()
    }

    pub fn delete_forward(&self) -> DocView {
        let mut g = self.lock();
        g.doc.delete_forward();
        g.view()
    }

    pub fn delete_word_back(&self) -> DocView {
        let mut g = self.lock();
        g.doc.delete_word_back();
        g.view()
    }

    pub fn delete_word_forward(&self) -> DocView {
        let mut g = self.lock();
        g.doc.delete_word_forward();
        g.view()
    }

    // ── caret movement ───────────────────────────────────────────────────────
    // Each syncs the grid first (movement reads the stop table / column layout),
    // moves, then repaints — `Inner::view` re-syncs but that's the cached no-op.

    pub fn move_left(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_left(extend);
        g.view()
    }

    pub fn move_right(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_right(extend);
        g.view()
    }

    pub fn move_up(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_up(extend);
        g.view()
    }

    pub fn move_down(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_down(extend);
        g.view()
    }

    pub fn move_word_left(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_word_left(extend);
        g.view()
    }

    pub fn move_word_right(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_word_right(extend);
        g.view()
    }

    pub fn move_home(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_home(extend);
        g.view()
    }

    pub fn move_end(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_end(extend);
        g.view()
    }

    pub fn move_doc_start(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_doc_start(extend);
        g.view()
    }

    pub fn move_doc_end(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_doc_end(extend);
        g.view()
    }

    pub fn select_all(&self) -> DocView {
        let mut g = self.lock();
        g.doc.select_all();
        g.view()
    }

    /// Place the caret from a click, in core's column grid: `row` indexes the
    /// visual [`Row`]s and `col` is the glyph column within it. Core clamps both
    /// to real caret stops. Prefer [`LeafDoc::click_ch`] from a proportional
    /// renderer.
    pub fn click(&self, row: u32, col: u32, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.click(row as usize, col as usize, extend);
        g.view()
    }

    /// Place the caret from a click whose horizontal position is a **UTF-16
    /// offset** into the visual row's text — what `characterIndex(for:)` hands
    /// back. Converted to core's display column before clicking, so a proportional
    /// renderer never reasons about column widths itself.
    pub fn click_ch(&self, row: u32, ch: u32, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        let col = utf16_to_col(&g.row_text(row as usize), ch as usize);
        g.doc.click(row as usize, col, extend);
        g.view()
    }

    /// Select the word under a click (row, `ch`) — the double-click gesture.
    pub fn select_word_ch(&self, row: u32, ch: u32) -> DocView {
        let mut g = self.lock();
        let off = g.offset_at(row as usize, ch as usize);
        g.doc.select_word_at(off);
        g.view()
    }

    /// Select the whole logical text block under a click (row, `ch`) — the
    /// triple-click gesture. Grabs the entire block even where it soft-wraps.
    pub fn select_block_ch(&self, row: u32, ch: u32) -> DocView {
        let mut g = self.lock();
        let off = g.offset_at(row as usize, ch as usize);
        g.doc.select_block_at(off);
        g.view()
    }

    /// Mirror a native selection into the model: `[anchor, focus]` given as
    /// row + UTF-16 offset pairs. Each is resolved to a source offset the way a
    /// click is, then set as the selection's fixed and moving ends. A collapsed
    /// range (`anchor == focus`) just places the caret.
    pub fn set_selection(
        &self,
        anchor_row: u32,
        anchor_ch: u32,
        focus_row: u32,
        focus_ch: u32,
    ) -> DocView {
        let mut g = self.lock();
        let anchor = g.offset_at(anchor_row as usize, anchor_ch as usize);
        let focus = g.offset_at(focus_row as usize, focus_ch as usize);
        g.doc.place_caret(anchor, false);
        if anchor != focus {
            g.doc.place_caret(focus, true);
        }
        g.view()
    }

    // ── rich clipboard (mirrors leaf-tui / leaf-gpui / leaf-wasm) ─────────────

    /// The current selection rendered to HTML by twig — the rich flavor a copy
    /// writes alongside the plain [`LeafDoc::selected_text`]. `None` when nothing
    /// is selected.
    pub fn selection_html(&self) -> Option<String> {
        self.lock().doc.selection_html()
    }

    /// Paste, preferring the clipboard's rich (`text/html`) flavor: twig parses
    /// `html` into the document's own markup and inserts it. Falls back to the
    /// plain `text` when there's no HTML or it doesn't parse.
    pub fn paste_rich(&self, html: Option<String>, text: String) -> DocView {
        let mut g = self.lock();
        let took = html.as_deref().is_some_and(|h| g.doc.paste_html(h));
        if !took {
            g.doc.paste(&text);
        }
        g.view()
    }

    // ── formatting commands (mirror leaf-gpui's EditorCommand) ────────────────

    pub fn toggle_bold(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Strong);
        g.view()
    }

    pub fn toggle_italic(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Emph);
        g.view()
    }

    pub fn toggle_code(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Verbatim);
        g.view()
    }

    pub fn toggle_mark(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Mark);
        g.view()
    }

    pub fn toggle_underline(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Insert);
        g.view()
    }

    pub fn toggle_strike(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Delete);
        g.view()
    }

    pub fn set_paragraph(&self) -> DocView {
        let mut g = self.lock();
        g.doc.set_block(BlockKind::Paragraph);
        g.view()
    }

    /// Toggle the current block to a heading of `level` (1–6); toggling the
    /// active level off returns it to a paragraph, per core.
    pub fn set_heading(&self, level: u32) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_heading(level);
        g.view()
    }

    pub fn toggle_blockquote(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_blockquote();
        g.view()
    }

    pub fn toggle_list(&self, ordered: bool) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_list(ordered);
        g.view()
    }

    pub fn insert_link(&self, destination: String) -> DocView {
        let mut g = self.lock();
        g.doc.insert_link(&destination);
        g.view()
    }

    pub fn undo(&self) -> DocView {
        let mut g = self.lock();
        g.doc.undo();
        g.view()
    }

    pub fn redo(&self) -> DocView {
        let mut g = self.lock();
        g.doc.redo();
        g.view()
    }

    /// Switch between the rendered WYSIWYG surface and the raw source.
    pub fn toggle_view(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_view();
        g.view()
    }
}

// ── UITextInput support ──────────────────────────────────────────────────────
// A `UITextPosition` on the Swift side wraps a source byte offset; these are the
// offset↔geometry, stepping, and range-editing primitives the protocol needs.
// Queries never move the caret — they only read the (synced) visual map — so the
// system can probe positions freely while the model's selection stays put.
#[uniffi::export]
impl LeafDoc {
    /// The caret's source offset (the selection's moving end).
    pub fn caret_offset(&self) -> u32 {
        self.lock().doc.caret as u32
    }

    /// The selection's fixed end (equals the caret when there's no selection).
    pub fn anchor_offset(&self) -> u32 {
        let g = self.lock();
        g.doc.anchor.unwrap_or(g.doc.caret) as u32
    }

    /// The last caret stop in the document — `UITextInput.endOfDocument`.
    pub fn doc_end_offset(&self) -> u32 {
        let mut g = self.lock();
        g.sync();
        let end = g.doc.source.len();
        g.snap_stop(end) as u32
    }

    /// Snap an arbitrary offset to the nearest valid caret stop.
    pub fn snap_offset(&self, off: u32) -> u32 {
        let mut g = self.lock();
        g.sync();
        g.snap_stop(off as usize) as u32
    }

    /// Where a source offset sits on screen: its visual `(row, ch)`.
    pub fn pos_for_offset(&self, off: u32) -> RowCol {
        let mut g = self.lock();
        g.sync();
        let (row, col) = g.pos_of_offset(off as usize);
        let ch = col_to_utf16(&g.row_text(row), col);
        RowCol { row: row as u32, ch: ch as u32 }
    }

    /// The source offset at visual `(row, ch)` — the inverse of
    /// [`Self::pos_for_offset`], for hit-testing a point to a position.
    pub fn offset_for_pos(&self, row: u32, ch: u32) -> u32 {
        let mut g = self.lock();
        g.sync();
        let col = utf16_to_col(&g.row_text(row as usize), ch as usize);
        g.offset_of_col(row as usize, col) as u32
    }

    /// Move `off` by `delta` caret stops (negative = left) — `position(from:offset:)`.
    pub fn step_offset(&self, off: u32, delta: i32) -> u32 {
        let mut g = self.lock();
        g.sync();
        let mut o = g.snap_stop(off as usize);
        if delta >= 0 {
            for _ in 0..delta {
                match g.stop_after(o) {
                    Some(n) => o = n,
                    None => break,
                }
            }
        } else {
            for _ in 0..(-delta) {
                match g.stop_before(o) {
                    Some(p) => o = p,
                    None => break,
                }
            }
        }
        o as u32
    }

    /// The count of caret stops between two offsets (signed) — `offset(from:to:)`.
    pub fn distance_offset(&self, from: u32, to: u32) -> i32 {
        let mut g = self.lock();
        g.sync();
        let (from, to) = (from as usize, to as usize);
        let (mut a, b, sign) = if from <= to { (from, to, 1i32) } else { (to, from, -1i32) };
        a = g.snap_stop(a);
        let mut n = 0i32;
        while a < b {
            match g.stop_after(a) {
                Some(x) => {
                    a = x;
                    n += 1;
                }
                None => break,
            }
        }
        n * sign
    }

    /// The offset one navigable row up/down from `off`, keeping its column —
    /// `position(from:in: .up/.down)`. `None` at the top/bottom edge.
    pub fn vertical_offset(&self, off: u32, down: bool) -> Option<u32> {
        let mut g = self.lock();
        g.sync();
        let (row, col) = g.pos_of_offset(off as usize);
        let target = if down { g.nav_below(row) } else { g.nav_above(row) };
        target.map(|r| g.offset_of_col(r, col) as u32)
    }

    /// The source text between two offsets — `text(in:)`.
    pub fn text_in_range(&self, from: u32, to: u32) -> String {
        let g = self.lock();
        let s = &g.doc.source;
        let (mut a, mut b) = ((from as usize).min(s.len()), (to as usize).min(s.len()));
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        while a > 0 && !s.is_char_boundary(a) {
            a -= 1;
        }
        while b < s.len() && !s.is_char_boundary(b) {
            b += 1;
        }
        s[a..b].to_string()
    }

    /// Set the selection to `[anchor, focus]` by source offsets — the setter behind
    /// `UITextInput.selectedTextRange` and handle dragging.
    pub fn set_selection_offsets(&self, anchor: u32, focus: u32) -> DocView {
        let mut g = self.lock();
        g.doc.place_caret(anchor as usize, false);
        if focus != anchor {
            g.doc.place_caret(focus as usize, true);
        }
        g.view()
    }

    /// Replace the source range `[from, to]` with `text` — `replace(_:withText:)`.
    pub fn replace_range(&self, from: u32, to: u32, text: String) -> DocView {
        let mut g = self.lock();
        g.doc.place_caret(from as usize, false);
        if to != from {
            g.doc.place_caret(to as usize, true);
        }
        g.doc.insert(&text);
        g.view()
    }
}

impl LeafDoc {
    /// Acquire the guard, recovering from a poisoned lock: a panic in `leaf-core`
    /// under one call shouldn't wedge the whole document handle for the app.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The UTF-16 offset into `text` of display column `col`. Walks grapheme clusters
/// exactly as core measures columns ([`text_width`] per cluster), so a wide
/// cluster advances the column by its cells while the offset advances by its
/// UTF-16 length; the two coincide only on plain ASCII.
fn col_to_utf16(text: &str, col: usize) -> usize {
    let mut c = 0usize;
    let mut u = 0usize;
    for g in text.graphemes(true) {
        if c >= col {
            break;
        }
        c += text_width(g);
        u += g.chars().map(char::len_utf16).sum::<usize>();
    }
    u
}

/// The display column of the grapheme boundary at or before UTF-16 offset `off`
/// — the inverse of [`col_to_utf16`], turning a native click position back into
/// core's column. Core then clamps the column to a real caret stop.
fn utf16_to_col(text: &str, off: usize) -> usize {
    let mut c = 0usize;
    let mut u = 0usize;
    for g in text.graphemes(true) {
        if u >= off {
            break;
        }
        u += g.chars().map(char::len_utf16).sum::<usize>();
        c += text_width(g);
    }
    c
}

/// The renderer class id for a semantic role. Heading level is folded into the
/// id (`h1`…`h6`) so a single style rule per level applies.
fn role_name(r: Role) -> String {
    match r {
        Role::Body => "body".into(),
        Role::Heading(level) => format!("h{}", level.clamp(1, 6)),
        Role::Code => "code".into(),
        Role::Link => "link".into(),
        Role::Mark => "mark".into(),
        Role::ListMarker => "list".into(),
        Role::QuoteGutter => "quote".into(),
        Role::Rule => "rule".into(),
        Role::Image => "image".into(),
    }
}

/// The toolbar id for an inline mark — kept in sync with the Swift button ids.
fn mark_id(kind: InlineKind) -> &'static str {
    match kind {
        InlineKind::Strong => "bold",
        InlineKind::Emph => "italic",
        InlineKind::Verbatim => "code",
        InlineKind::Mark => "mark",
        InlineKind::Insert => "underline",
        InlineKind::Delete => "strike",
        InlineKind::Superscript => "superscript",
        InlineKind::Subscript => "subscript",
    }
}

/// The WYSIWYG rows: each visual row's glyphs coalesced into maximal runs of
/// identical `(style, selected)`. A glyph is selected when its source byte lies
/// in `[ss, se)`.
fn wysiwyg_rows(vmap: &VisualMap, ss: usize, se: usize) -> Vec<Row> {
    vmap.rows
        .iter()
        .map(|vrow| {
            // The row's heading level, if any: read off the first heading glyph.
            // A heading block's whole line shares one level, so the first is the
            // row's — what lets the renderer size the entire row.
            let heading = vrow.glyphs.iter().find_map(|g| match g.style.role {
                Role::Heading(level) => Some(level),
                _ => None,
            });

            let mut runs: Vec<Run> = Vec::new();
            let mut buf = String::new();
            let mut cur: Option<(LStyle, bool)> = None;

            for g in &vrow.glyphs {
                let key = (g.style, g.src >= ss && g.src < se);
                match cur {
                    Some(k) if k == key => buf.push(g.ch),
                    _ => {
                        if let Some((style, was_sel)) = cur.take() {
                            runs.push(make_run(std::mem::take(&mut buf), style, was_sel));
                        }
                        cur = Some(key);
                        buf.push(g.ch);
                    }
                }
            }
            if let Some((style, was_sel)) = cur {
                runs.push(make_run(buf, style, was_sel));
            }

            Row {
                runs,
                decoration: vrow.decoration,
                code: vrow.code,
                code_lang: vrow.code_lang.clone(),
                heading,
            }
        })
        .collect()
}

/// The source rows: the raw document split on `'\n'`, every line plain body text
/// with the `[ss, se)` selection carved out as its own run. Backs the source
/// view, whose caret rides raw byte offsets.
fn source_rows(source: &str, ss: usize, se: usize) -> Vec<Row> {
    let body = LStyle::default();
    let mut rows = Vec::new();
    let mut byte = 0usize;

    for raw in source.split('\n') {
        let start = byte;
        let end = start + raw.len();
        // Selection overlap with this line, in line-local byte coordinates.
        let a = ss.clamp(start, end) - start;
        let b = se.clamp(start, end) - start;

        let mut runs = Vec::new();
        if a < b {
            if a > 0 {
                runs.push(make_run(raw[..a].to_string(), body, false));
            }
            runs.push(make_run(raw[a..b].to_string(), body, true));
            if b < raw.len() {
                runs.push(make_run(raw[b..].to_string(), body, false));
            }
        } else if !raw.is_empty() {
            runs.push(make_run(raw.to_string(), body, false));
        }

        rows.push(Row {
            runs,
            decoration: false,
            code: false,
            code_lang: None,
            heading: None, // source view is raw text — no resolved heading rows
        });
        byte = end + 1; // skip the '\n' that `split` consumed
    }
    rows
}

/// Build a [`Run`] from an accumulated string and the core style it was drawn
/// with — the one place role and emphasis flags cross into the view shape.
fn make_run(text: String, style: LStyle, sel: bool) -> Run {
    Run {
        text,
        role: role_name(style.role),
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strike: style.strikethrough,
        sel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Arc<LeafDoc> {
        LeafDoc::new(src.to_string(), "markdown".to_string()).unwrap()
    }

    fn row_text(v: &DocView, row: usize) -> String {
        v.rows[row].runs.iter().map(|r| r.text.clone()).collect()
    }

    #[test]
    fn unwrapped_collapses_a_paragraph_to_one_row() {
        let d = doc("one two three four five six seven eight\n");
        let wrapped = d.set_width(10);
        let unwrapped = d.set_unwrapped();
        assert!(
            unwrapped.rows.len() < wrapped.rows.len(),
            "a narrow column wrap splits the paragraph; unwrapped keeps it whole"
        );
        assert!(
            (0..unwrapped.rows.len()).any(|i| row_text(&unwrapped, i).contains("eight")),
            "the whole paragraph, including its last word, sits on a single unwrapped row"
        );
    }

    #[test]
    fn offsets_round_trip_when_unwrapped() {
        let d = doc("hello world\n");
        d.set_unwrapped();
        // offset -> (row, ch) -> offset is stable, so the pixel-wrapping frontend can
        // map between its visual lines and core's byte-offset caret model.
        let rc = d.pos_for_offset(6); // the 'w' of "world"
        assert_eq!(d.offset_for_pos(rc.row, rc.ch), 6);
    }

    #[test]
    fn set_unwrapped_is_idempotent() {
        let d = doc("a paragraph of some length here\n");
        let first = d.set_unwrapped();
        let second = d.set_unwrapped();
        assert_eq!(first.rows.len(), second.rows.len());
    }
}
