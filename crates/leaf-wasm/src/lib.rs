//! leaf-wasm — the WebAssembly frontend binding for leaf.
//!
//! This is the browser analogue of `leaf-tui`'s `style.rs` + `ui.rs`: it takes
//! `leaf-core`'s frontend-neutral [`Doc`] — the byte-offset caret model and the
//! AST→glyph [`VisualMap`] — and exposes it across the wasm boundary in the
//! shape a web renderer wants. Core stays the single source of truth for the
//! text, the caret math, and the offset⇄position mapping; the JS side only
//! paints glyphs and forwards key/mouse events back in, exactly as the TUI and
//! gpui frontends do.
//!
//! ## The boundary is style *runs*, not glyphs
//!
//! [`Doc::build_visual`] resolves the document to rows of per-character glyphs,
//! each tagged with a semantic [`Role`] and the author's emphasis. Sending one
//! JS object per character would make every keystroke O(document) in boundary
//! crossings. Instead [`LeafDoc::view`] coalesces each row's glyphs into maximal
//! **runs** of identical style (the same merge the TUI does when it builds
//! ratatui `Span`s) and ships those — a handful of objects per line. The JS
//! renderer maps each run's `role` to a CSS class and its emphasis flags to
//! font styling, the web counterpart of `to_ratatui` / `text_run`.
//!
//! ## Core owns the grid; the browser owns the pixels
//!
//! Core lays a row out in whole character *columns* (a terminal cell measure),
//! and every offset⇄position method — [`Doc::caret_pos`], [`Doc::click`],
//! vertical motion — speaks that grid. It wraps each logical line to a column
//! budget and hands back rows, a caret at `(row, col)`, and the up/down goal
//! math, and it stays the sole authority on all of that. What it deliberately
//! does *not* dictate is presentation: a column is a semantic position, not a
//! pixel offset.
//!
//! So the renderer is *proportional*, the web peer of `leaf-gpui`'s `style.rs`:
//! body text in a real proportional family, headings distinguished by **size**
//! (a per-level scale ramp) and weight rather than a recoloured cell, code in a
//! monospace family with a tinted panel. Because the glyphs no longer sit on a
//! fixed pixel grid, the JS side never multiplies `col × cell_width`; it lets the
//! browser shape each row and reads the caret's pixel position back out of the
//! DOM (a collapsed `Range` at the caret column), and hit-tests a click through
//! `caretRangeFromPoint`, translating the DOM node+offset back to core's
//! `(row, col)` by counting glyph columns. Core measures nothing in pixels; the
//! browser positions nothing in the model — the same division of labour gpui
//! keeps between the document and its own visual layout. Each row carries its
//! [`Row::heading`] level so the whole line can be sized as one unit, mirroring
//! how gpui shapes a heading's line at a single larger size.

use leaf_core::style::{Role, Style as LStyle};
use leaf_core::wysiwyg::text_width;
use leaf_core::{BlockKind, Doc, Format, InlineKind, View, VisualMap};
use serde::Serialize;
use tsify_next::Tsify;
use unicode_segmentation::UnicodeSegmentation;
use wasm_bindgen::prelude::*;

/// One maximal span of same-styled glyphs on a visual row — the unit the JS
/// renderer turns into a single styled DOM node.
#[derive(Serialize, Tsify)]
pub struct Run {
    /// The run's text, glyphs concatenated in column order.
    text: String,
    /// The glyph's semantic role as a renderer class id: `body`, `h1`…`h6`,
    /// `code`, `link`, `mark`, `list`, `quote`, `rule`.
    role: String,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    /// Whether this run lies inside the active selection — so the renderer can
    /// paint a selection background without the JS side re-deriving it from
    /// offsets. Selection splits a run the same way a style change does.
    sel: bool,
}

/// One visual line: its styled runs plus the row-level flags a frontend draws
/// chrome from.
#[derive(Serialize, Tsify)]
pub struct Row {
    runs: Vec<Run>,
    /// Drawn but holds no caret (a table rule, a block-gap blank line): the
    /// renderer skips it for click/caret math. See [`leaf_core::VRow`].
    decoration: bool,
    /// A fenced/indented code-block line — the renderer draws a tinted, bordered
    /// panel around each maximal run of these.
    code: bool,
    /// A fenced block's language, carried on the block's first code row only.
    code_lang: Option<String>,
    /// The heading level (1–6) if this row belongs to a heading block, else
    /// `None`. A proportional renderer sizes the *whole* row from this — line
    /// height and all — the web analogue of gpui shaping a heading's line at one
    /// larger size, so an inline `` `code` `` run inside a heading still reads at
    /// the heading's size rather than dropping to body. (The per-run `role`
    /// already carries `h1`…`h6` too, but that can't tell the renderer how tall
    /// to make a row whose runs are mixed.)
    heading: Option<u8>,
}

/// A whole rendered frame: the rows to paint, where the caret sits, and the
/// toolbar state — everything the JS side needs for one repaint, in one object.
///
/// `into_wasm_abi` makes this the *return type* of every view-producing method:
/// the generated `.d.ts` types those methods as `DocView` rather than `any`, so
/// the JS renderer sees the full shape.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct DocView {
    rows: Vec<Row>,
    /// The caret's row: an index into [`Self::rows`].
    caret_row: usize,
    /// The caret's display *column* within its row — core's grid position. Kept
    /// for callers reasoning in columns; a proportional DOM renderer wants
    /// [`Self::caret_ch`] instead.
    caret_col: usize,
    /// The caret's offset within its row's text in **UTF-16 code units** — what a
    /// DOM `Range` counts to. This is `caret_col` mapped through the row's
    /// grapheme widths, so it lands the caret correctly past wide glyphs (CJK,
    /// emoji) where a column and a character index diverge. The renderer builds a
    /// collapsed `Range` at this offset to place the caret.
    caret_ch: usize,
    /// Whether a (non-empty) selection is active. When true, the renderer paints
    /// the browser's native selection over `[anchor_row/anchor_ch, caret]` and
    /// hides its own caret; when false, only the caret shows.
    has_selection: bool,
    /// The selection's *fixed* end (the caret is the moving end), as a row and a
    /// UTF-16 offset — so the renderer can restore a native selection with the
    /// same direction the model has, and a following Shift-motion extends from
    /// the right edge. Equal to the caret position when `has_selection` is false.
    anchor_row: usize,
    anchor_ch: usize,
    /// Whether the buffer differs from the last saved bytes — for a "● modified"
    /// affordance.
    dirty: bool,
    /// `"wysiwyg"` or `"source"`, for a view-toggle affordance.
    view: String,
    /// The heading level at the caret, if any — a toolbar lights H1…H6 from it.
    heading: Option<u32>,
    /// The inline marks active at the caret (`bold`, `italic`, `code`, …) — the
    /// toolbar lights the matching buttons, the same state the TUI prints in its
    /// footer.
    active: Vec<String>,
}

/// The UTF-16 offset into `text` of display column `col` — the position a DOM
/// `Range` counts to. Walks grapheme clusters exactly as core measures columns
/// ([`text_width`] per cluster), so a wide cluster advances the column by its
/// cells while the offset advances by its UTF-16 length; the two coincide only
/// on plain ASCII. A `col` landing inside a wide cluster (it shouldn't — caret
/// columns are cluster starts) resolves to that cluster's start.
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
/// in `text` — the inverse of [`col_to_utf16`], turning a DOM click position
/// back into core's column. Core then clamps the column to a real caret stop.
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
/// id (`h1`…`h6`) so a single CSS rule per level styles it.
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
    }
}

/// The toolbar id for an inline mark — kept in sync with the JS button ids.
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

/// A live leaf document bound for the browser: `leaf_core::Doc` plus the wrap
/// width the current viewport implies. Constructed from an in-memory string and
/// driven entirely through method calls — there is no filesystem behind it.
#[wasm_bindgen]
pub struct LeafDoc {
    doc: Doc,
    /// The wrap width in columns, from the viewport. `build_visual` caches on
    /// `(revision, width)`, so re-syncing when neither moved is free.
    width: usize,
}

#[wasm_bindgen]
impl LeafDoc {
    /// Parse `source` as `format` (`"markdown"`/`"md"`, `"djot"`/`"dj"`,
    /// `"html"`, `"xml"`) into a live, untitled document.
    #[wasm_bindgen(constructor)]
    pub fn new(source: &str, format: &str) -> Result<LeafDoc, JsValue> {
        console_error_panic_hook::set_once();
        let format = match format.to_ascii_lowercase().as_str() {
            "markdown" | "md" => Format::Markdown,
            "djot" | "dj" => Format::Djot,
            "html" | "htm" => Format::Html,
            "xml" => Format::Xml,
            other => return Err(JsValue::from_str(&format!("unknown format: {other}"))),
        };
        let doc = Doc::from_source(source.to_string(), format)
            .map_err(|e| JsValue::from_str(&format!("{e}")))?;
        Ok(LeafDoc { doc, width: 80 })
    }

    /// Rebuild the visual map at the current width. Cheap (cached) when nothing
    /// changed; the guard that lets every movement/click method assume a fresh
    /// grid regardless of the order JS calls them in.
    fn sync(&mut self) {
        self.doc.build_visual(self.width);
    }

    /// The plain text of visual row `row` in the active view — the same string
    /// the renderer concatenates its runs into. It backs the column⇄UTF-16
    /// mapping ([`col_to_utf16`]/[`utf16_to_col`]); the two views draw from
    /// different sources (resolved glyphs vs raw source lines), so it branches
    /// the same way [`LeafDoc::view`] does.
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

    /// The `(row, display-column)` a source offset sits at in the active view —
    /// the counterpart to [`Doc::caret_pos`] for an arbitrary offset (the caret
    /// is `caret_pos`, but the selection's anchor needs the same for any offset).
    /// Branches by view exactly as `caret_pos` does.
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

    /// Resolve the current document to a renderable frame of style runs. Called
    /// for the first paint, on resize, and returned by every mutating method so
    /// one boundary crossing both edits and repaints.
    pub fn view(&mut self) -> Result<DocView, JsValue> {
        self.sync();

        let (ss, se) = self.doc.selection().unwrap_or((usize::MAX, usize::MAX));

        // The two views speak different grids — the WYSIWYG map's resolved glyphs
        // vs the raw source split on newlines — and `caret_pos` below already
        // branches to match, so the rows must too or the caret lands on the wrong
        // text. See `Doc::caret_pos`.
        let rows = match self.doc.view {
            View::Wysiwyg => wysiwyg_rows(&self.doc.vmap, ss, se),
            View::Source => source_rows(&self.doc.source, ss, se),
        };

        let (caret_row, caret_col) = self.doc.caret_pos();
        // Map the caret's display column to a UTF-16 text offset so the DOM
        // renderer can place it past wide glyphs (see [`DocView::caret_ch`]).
        let caret_ch = col_to_utf16(&self.row_text(caret_row), caret_col);
        // The selection's fixed (anchor) end, in the same row/UTF-16 terms, so
        // the renderer can mirror it onto the browser's native selection.
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

        Ok(DocView {
            rows,
            caret_row,
            caret_col,
            caret_ch,
            has_selection,
            anchor_row,
            anchor_ch,
            dirty: self.doc.dirty,
            view: self.doc.view_name().to_string(),
            heading,
            active,
        })
    }

    /// Set the wrap width (in columns) the viewport implies and repaint.
    pub fn set_width(&mut self, cols: usize) -> Result<DocView, JsValue> {
        self.width = cols.max(1);
        self.view()
    }

    /// The current source text — for a "save" (download / localStorage / PUT) or
    /// a source-view display.
    pub fn source(&self) -> String {
        self.doc.source.clone()
    }

    /// The selected text, if any — for a clipboard copy/cut.
    pub fn selected_text(&self) -> Option<String> {
        self.doc.selected_text().map(str::to_string)
    }

    /// Mark the buffer saved after the host persisted [`LeafDoc::source`] its own
    /// way — clears the dirty flag without touching a filesystem.
    pub fn mark_saved(&mut self) -> Result<DocView, JsValue> {
        self.doc.mark_saved();
        self.view()
    }

    // ── text input ──────────────────────────────────────────────────────────

    pub fn insert(&mut self, text: &str) -> Result<DocView, JsValue> {
        self.doc.insert(text);
        self.view()
    }

    pub fn paste(&mut self, text: &str) -> Result<DocView, JsValue> {
        self.doc.paste(text);
        self.view()
    }

    pub fn newline(&mut self) -> Result<DocView, JsValue> {
        self.doc.newline();
        self.view()
    }

    pub fn backspace(&mut self) -> Result<DocView, JsValue> {
        self.doc.backspace();
        self.view()
    }

    pub fn delete_forward(&mut self) -> Result<DocView, JsValue> {
        self.doc.delete_forward();
        self.view()
    }

    pub fn delete_word_back(&mut self) -> Result<DocView, JsValue> {
        self.doc.delete_word_back();
        self.view()
    }

    pub fn delete_word_forward(&mut self) -> Result<DocView, JsValue> {
        self.doc.delete_word_forward();
        self.view()
    }

    // ── caret movement ──────────────────────────────────────────────────────
    // Each syncs the grid first (movement reads the stop table / column layout),
    // moves, then repaints.

    pub fn move_left(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_left(extend);
        self.view()
    }

    pub fn move_right(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_right(extend);
        self.view()
    }

    pub fn move_up(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_up(extend);
        self.view()
    }

    pub fn move_down(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_down(extend);
        self.view()
    }

    pub fn move_word_left(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_word_left(extend);
        self.view()
    }

    pub fn move_word_right(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_word_right(extend);
        self.view()
    }

    pub fn move_home(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_home(extend);
        self.view()
    }

    pub fn move_end(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_end(extend);
        self.view()
    }

    pub fn move_doc_start(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_doc_start(extend);
        self.view()
    }

    pub fn move_doc_end(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_doc_end(extend);
        self.view()
    }

    pub fn select_all(&mut self) -> Result<DocView, JsValue> {
        self.doc.select_all();
        self.view()
    }

    /// Place the caret from a click, in core's column grid: `row` indexes the
    /// visual [`Row`]s and `col` is the glyph column within it. A proportional
    /// renderer derives them by hit-testing — `caretRangeFromPoint` gives the DOM
    /// node+offset under the pointer, which maps to a row and a column count. Core
    /// clamps both to real caret stops.
    pub fn click(&mut self, row: usize, col: usize, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.click(row, col, extend);
        self.view()
    }

    /// Place the caret from a click whose horizontal position is a **UTF-16
    /// offset** into the visual row's text — what a DOM `Range` hands back
    /// (`range.toString().length`). It's converted to core's display column
    /// (they differ by a cell per wide glyph) before clicking, so a proportional
    /// renderer never has to reason about column widths itself. This is the
    /// hit-test counterpart of [`DocView::caret_ch`]; prefer it over [`click`].
    pub fn click_ch(&mut self, row: usize, ch: usize, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        let col = utf16_to_col(&self.row_text(row), ch);
        self.doc.click(row, col, extend);
        self.view()
    }

    /// The source offset under a click at row `row`, `ch` UTF-16 units in — the
    /// same resolution [`click_ch`] does, but returning the offset instead of
    /// moving the caret. It's what the double/triple-click selectors below anchor
    /// on, and it lets a host implement its own gestures (a context menu placing
    /// the caret, say) without a second boundary crossing.
    fn offset_at(&mut self, row: usize, ch: usize) -> usize {
        self.sync();
        let col = utf16_to_col(&self.row_text(row), ch);
        self.doc.click(row, col, false);
        self.doc.caret
    }

    /// Select the word under a click (row, `ch`) — the double-click gesture.
    /// Core reads the word from the source around that offset.
    pub fn select_word_ch(&mut self, row: usize, ch: usize) -> Result<DocView, JsValue> {
        let off = self.offset_at(row, ch);
        self.doc.select_word_at(off);
        self.view()
    }

    /// Select the whole logical text block under a click (row, `ch`) — the
    /// triple-click gesture. Core reads the paragraph/heading span from the AST,
    /// so it grabs the entire block even where it soft-wraps across visual rows.
    pub fn select_block_ch(&mut self, row: usize, ch: usize) -> Result<DocView, JsValue> {
        let off = self.offset_at(row, ch);
        self.doc.select_block_at(off);
        self.view()
    }

    /// Mirror a native browser selection into the model: `[anchor, focus]` given
    /// as row + UTF-16 offset pairs (a DOM `Range`'s ends). Each is resolved to a
    /// source offset the way a click is, then set as the selection's fixed and
    /// moving ends — so `selectionchange` can keep core in step with the
    /// selection the browser drew. A collapsed range (`anchor == focus`) just
    /// places the caret.
    pub fn set_selection(
        &mut self,
        anchor_row: usize,
        anchor_ch: usize,
        focus_row: usize,
        focus_ch: usize,
    ) -> Result<DocView, JsValue> {
        let anchor = self.offset_at(anchor_row, anchor_ch);
        let focus = self.offset_at(focus_row, focus_ch);
        self.doc.place_caret(anchor, false);
        if anchor != focus {
            self.doc.place_caret(focus, true);
        }
        self.view()
    }

    // ── rich clipboard (mirrors leaf-tui / leaf-gpui) ────────────────────────

    /// The current selection rendered to HTML by twig — the rich flavor a copy
    /// writes alongside the plain [`LeafDoc::selected_text`], so pasting into a
    /// word processor keeps the formatting. `None` when nothing is selected.
    pub fn selection_html(&mut self) -> Option<String> {
        self.doc.selection_html()
    }

    /// Paste, preferring the clipboard's rich (`text/html`) flavor: twig parses
    /// `html` into the document's own markup and inserts it. Falls back to the
    /// plain `text` when there's no HTML or it doesn't parse — the same
    /// html-then-plain order the TUI and gpui frontends use.
    pub fn paste_rich(&mut self, html: Option<String>, text: &str) -> Result<DocView, JsValue> {
        let took = html.as_deref().is_some_and(|h| self.doc.paste_html(h));
        if !took {
            self.doc.paste(text);
        }
        self.view()
    }

    // ── formatting commands (mirror leaf-gpui's EditorCommand) ───────────────

    pub fn toggle_bold(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Strong);
        self.view()
    }

    pub fn toggle_italic(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Emph);
        self.view()
    }

    pub fn toggle_code(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Verbatim);
        self.view()
    }

    pub fn toggle_mark(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Mark);
        self.view()
    }

    pub fn toggle_underline(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Insert);
        self.view()
    }

    pub fn toggle_strike(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Delete);
        self.view()
    }

    pub fn set_paragraph(&mut self) -> Result<DocView, JsValue> {
        self.doc.set_block(BlockKind::Paragraph);
        self.view()
    }

    /// Toggle the current block to a heading of `level` (1–6); toggling the
    /// active level off returns it to a paragraph, per core.
    pub fn set_heading(&mut self, level: u32) -> Result<DocView, JsValue> {
        self.doc.toggle_heading(level);
        self.view()
    }

    pub fn toggle_blockquote(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_blockquote();
        self.view()
    }

    pub fn toggle_list(&mut self, ordered: bool) -> Result<DocView, JsValue> {
        self.doc.toggle_list(ordered);
        self.view()
    }

    pub fn insert_link(&mut self, destination: &str) -> Result<DocView, JsValue> {
        self.doc.insert_link(destination);
        self.view()
    }

    pub fn undo(&mut self) -> Result<DocView, JsValue> {
        self.doc.undo();
        self.view()
    }

    pub fn redo(&mut self) -> Result<DocView, JsValue> {
        self.doc.redo();
        self.view()
    }

    /// Switch between the rendered WYSIWYG surface and the raw source.
    pub fn toggle_view(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_view();
        self.view()
    }
}

/// The WYSIWYG rows: each visual row's glyphs coalesced into maximal runs of
/// identical `(style, selected)` — the same span merge the TUI does. A glyph is
/// selected when its source byte lies in `[ss, se)`.
fn wysiwyg_rows(vmap: &VisualMap, ss: usize, se: usize) -> Vec<Row> {
    vmap.rows
        .iter()
        .map(|vrow| {
            // The row's heading level, if any: read off the first heading glyph.
            // A heading block's whole line shares one level, so the first is the
            // row's — this is what lets the renderer size the entire row rather
            // than each run (see [`Row::heading`]).
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
/// with the `[ss, se)` selection carved out as its own run — the browser
/// counterpart of the TUI's `build_lines`. This is what backs the source view,
/// whose caret rides raw byte offsets (see `Doc::caret_pos`).
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
