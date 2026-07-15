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
use twig::{BlockKind, Change, Editor, FlatNode, Format, InlineKind};
use unicode_segmentation::GraphemeCursor;

use crate::wysiwyg::{self, VisualMap};

/// Which view the body shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The raw document with a caret in source bytes.
    Source,
    /// Markup resolved to real styles, caret riding the rendered glyphs.
    Wysiwyg,
}

/// What kind of edit produced an undo group. Same-kind edits in a row coalesce
/// into one undo step (a run of typed characters undoes together); `Other` never
/// coalesces, so a paste, format toggle, or block change is always its own step.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    Other,
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
    /// The kind of the last edit, for coalescing: twig owns the undo *history*
    /// (see `undo`/`redo`), but "what counts as one undo step" is a frontend-UX
    /// call, so leaf decides when a run continues and tells twig to coalesce.
    last_edit_kind: Option<EditKind>,
    /// The source as of the last open/save — `dirty` is `source != clean_source`,
    /// so undoing back to the saved state correctly clears the modified flag.
    clean_source: String,
    /// The "sticky" column vertical motion aims for, in the active view's
    /// column space. Set on the first `move_up`/`move_down` of a run and
    /// reused by every subsequent one in that run, so passing through a
    /// shorter line doesn't permanently forget the original column. Any
    /// horizontal motion or edit clears it.
    goal_col: Option<usize>,
    /// The rendered map for the WYSIWYG view, rebuilt each frame; empty in the
    /// source view. Movement and clicks read it to stay in visible space.
    pub vmap: VisualMap,

    // View geometry the renderer stamps each frame, so mouse events can map a
    // screen cell back to a byte offset.
    pub scroll: usize,
    pub body_origin: (u16, u16),
    pub body_height: u16,
    /// The caret as of the last frame drawn, or `None` before the first.
    ///
    /// Scrolling is the viewport's business, not the caret's: the view follows
    /// the caret when the caret *moves*, but a wheel that doesn't touch the
    /// caret has to be free to scroll away from it — otherwise the view is
    /// pinned to the caret and stops dead at the edge of the document you can
    /// see. Comparing against this is what tells the two apart, and it catches a
    /// caret set by any route, including a frontend assigning the field itself.
    pub drawn_caret: Option<usize>,
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
            clean_source: source.clone(),
            source,
            caret: 0,
            anchor: None,
            dirty: false,
            status: None,
            // leaf opens in the rich-text (WYSIWYG) view by default — the
            // markup-resolved surface is leaf's differentiator. Frontends can
            // still start in source view explicitly (e.g. a CLI flag), and ⌘e/⌥w
            // toggles at runtime.
            view: View::Wysiwyg,
            last_edit_kind: None,
            goal_col: None,
            vmap: VisualMap::default(),
            scroll: 0,
            body_origin: (0, 0),
            body_height: 0,
            drawn_caret: None,
        })
    }

    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            View::Source => View::Wysiwyg,
            View::Wysiwyg => View::Source,
        };
        self.scroll = 0;
        self.status = None;
        // Entering WYSIWYG, the caret may be sitting in now-hidden frontmatter;
        // lift it to the first rendered offset.
        self.clamp_caret();
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
        self.vmap = wysiwyg::build(&nodes, &self.source, Some(width));
        self.clamp_caret();
    }

    /// Build the WYSIWYG map with each block as a single unwrapped row — for a
    /// frontend (the GUI) that wraps at its own proportional pixel width rather
    /// than a fixed character column.
    pub fn build_visual_unwrapped(&mut self) {
        let nodes = self.nodes();
        self.vmap = wysiwyg::build(&nodes, &self.source, None);
        self.clamp_caret();
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
        self.splice(start, end, text, EditKind::Other);
    }

    /// Insert `text` at the caret, replacing the selection if there is one. A
    /// single typed character coalesces with the run of typing before it; a
    /// newline or a multi-character insert (a paste) is its own undo step.
    pub fn insert(&mut self, text: &str) {
        let (s, e) = self.selection().unwrap_or((self.caret, self.caret));
        let kind = if text.chars().take(2).count() == 1 && text != "\n" {
            EditKind::Insert
        } else {
            EditKind::Other
        };
        self.splice(s, e, text, kind);
    }

    /// The Enter key.
    ///
    /// In source view it's a literal newline. In WYSIWYG it's **AST-aware**: it
    /// reads the block the caret is in and splices the source that reparses into
    /// the structurally right thing — because a bare `\n` is only a markdown soft
    /// break (same paragraph), which is why a paragraph needs a blank-line
    /// separator, a list item needs the next marker, and so on.
    ///
    ///   - paragraph / heading  → a new paragraph (blank line)
    ///   - list item            → the next item (same bullet, next number);
    ///                            an *empty* item exits the list
    ///   - block quote          → a new quoted line
    ///   - code block           → a literal newline (stay in the block)
    pub fn newline(&mut self) {
        if self.view == View::Source {
            self.insert("\n");
            return;
        }
        // Enter over a selection replaces it with a paragraph break.
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "\n\n", EditKind::Other);
            return;
        }
        // The block the caret is in. `block_offset_for_caret` nudges off a line
        // end (where the caret sits at the doc level); on a bare line (e.g. an
        // empty list item) fall back to the caret so the enclosing list/quote is
        // still visible in the ancestors.
        let off = self.block_offset_for_caret().unwrap_or(self.caret);
        let kinds: Vec<String> = self
            .editor
            .ancestors_at(off)
            .map(|c| c.into_iter().map(|m| m.kind).collect())
            .unwrap_or_default();
        let has = |k: &str| kinds.iter().any(|x| x == k);

        if has("code_block") {
            self.insert("\n");
            return;
        }
        // Lists: detect from the source marker on the caret's line rather than the
        // ancestors — twig doesn't report an *empty* `- ` line as a `list_item`,
        // and we still want Enter there to exit the list.
        if let Some((line_start, marker)) = self.list_marker_on_line(self.caret) {
            self.list_newline(line_start, marker);
            return;
        }
        if has("block_quote") {
            self.insert("\n> ");
            return;
        }
        self.insert("\n\n");
    }

    /// Enter inside a list: start the next item, or exit the list if the current
    /// item is empty (the standard "double-Enter leaves the list" behaviour).
    fn list_newline(&mut self, line_start: usize, marker: String) {
        let caret = self.caret;
        let content_start = (line_start + marker.len()).min(self.source.len());
        let line_end = self.source[caret..]
            .find('\n')
            .map(|i| caret + i)
            .unwrap_or(self.source.len());
        let item_is_empty = self.source[content_start..line_end.max(content_start)]
            .trim()
            .is_empty();
        if item_is_empty {
            // Exit the list: replace the empty item's marker with a blank line,
            // so the caret lands in a fresh paragraph below the list.
            self.splice(line_start, caret, "\n", EditKind::Other);
        } else {
            self.insert(&format!("\n{}", next_list_marker(&marker)));
        }
    }

    /// Parse a list marker at the start of `off`'s line, e.g. `"- "`, `"  * "`,
    /// `"1. "`, `"3) "`. Returns `(line_start, marker_text)`.
    fn list_marker_on_line(&self, off: usize) -> Option<(usize, String)> {
        let off = off.min(self.source.len());
        let line_start = self.source[..off].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let bytes = self.source.as_bytes();
        let mut i = line_start;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i < bytes.len() && matches!(bytes[i], b'-' | b'*' | b'+') {
            i += 1;
        } else {
            let digits_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == digits_start || !(i < bytes.len() && matches!(bytes[i], b'.' | b')')) {
                return None;
            }
            i += 1; // the . or )
        }
        let after_marker = i;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i == after_marker {
            return None; // a marker needs a trailing space
        }
        Some((line_start, self.source[line_start..i].to_string()))
    }

    pub fn backspace(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
        } else if self.caret > self.caret_floor() {
            // Never delete back across the floor — that would eat hidden
            // frontmatter the WYSIWYG caret can't even see.
            let prev = prev_boundary(&self.source, self.caret).max(self.caret_floor());
            self.splice(prev, self.caret, "", EditKind::Delete);
        }
    }

    pub fn delete_forward(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
        } else if self.caret < self.source.len() {
            let next = next_boundary(&self.source, self.caret);
            self.splice(self.caret, next, "", EditKind::Delete);
        }
    }

    /// Delete from the caret back to the start of the previous word (⌥⌫ /
    /// Ctrl+⌫). Deletes the selection instead when one is active.
    pub fn delete_word_back(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
        } else {
            let start = prev_word(&self.source, self.caret).max(self.caret_floor());
            if start < self.caret {
                self.splice(start, self.caret, "", EditKind::Delete);
            }
        }
    }

    /// Delete from the caret forward to the end of the next word (⌥⌦ /
    /// Ctrl+Del). Deletes the selection instead when one is active.
    pub fn delete_word_forward(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
        } else {
            let end = next_word(&self.source, self.caret);
            if end > self.caret {
                self.splice(self.caret, end, "", EditKind::Delete);
            }
        }
    }

    /// One splice via twig's `edit_range`, then re-anchor the caret from the
    /// returned `Change` and refresh the cached source. A reparse-breaking edit
    /// (rare for Markdown/Djot) leaves the document untouched and reports.
    fn splice(&mut self, start: usize, end: usize, text: &str, kind: EditKind) {
        // twig records an undo step for every edit; when this one continues a
        // run of the same kind (typing, deleting), tell twig to fold it into the
        // step before it so the whole run undoes at once.
        let coalesce = kind != EditKind::Other && self.last_edit_kind == Some(kind);
        match self.editor.edit_range(start, end, text) {
            Ok(change) => {
                if coalesce {
                    let _ = self.editor.coalesce_last_undo();
                }
                self.last_edit_kind = Some(kind);
                self.refresh();
                self.caret = change.new.end;
                self.anchor = None;
                self.goal_col = None;
                self.dirty = self.source != self.clean_source;
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
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                self.anchor = Some(change.new.start);
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
            }
            Err(e) => self.status = Some(format!("{kind:?}: {e}")),
        }
    }

    /// Convert the block at the caret to a heading level or paragraph.
    pub fn set_block(&mut self, kind: BlockKind) {
        match self.block_offset_for_caret() {
            Some(offset) => match self.editor.set_block(offset, kind) {
                Ok(_) => {
                    self.last_edit_kind = None;
                    self.refresh();
                    self.clamp_caret();
                    self.anchor = None;
                    self.dirty = self.source != self.clean_source;
                    self.status = None;
                }
                Err(e) => self.status = Some(format!("{kind:?}: {e}")),
            },
            // A blank line — a fresh, empty paragraph with no AST node to convert.
            // Insert the block's marker so it becomes an (empty) block to type
            // into (twig's `set_block` needs an existing block at the offset).
            None => self.insert_block_prefix(kind),
        }
    }

    /// Whether `off` is inside a text block (paragraph, heading, code block…).
    fn has_block_at(&mut self, off: usize) -> bool {
        self.editor.ancestors_at(off).ok().is_some_and(|chain| {
            chain
                .iter()
                .any(|m| !wysiwyg::is_inline(&m.kind) && !is_block_container(&m.kind))
        })
    }

    /// The offset to hand twig's `set_block`: the caret when it is already inside
    /// a block, otherwise nudged onto the previous character (a caret at a line
    /// end sits at the doc level, outside the block). `None` when the caret is on
    /// a blank line — a new paragraph with no block node to convert.
    fn block_offset_for_caret(&mut self) -> Option<usize> {
        let caret = self.caret.min(self.source.len());
        if self.has_block_at(caret) {
            return Some(caret);
        }
        // Nudge to the previous character — but never across a newline: that would
        // target the previous block, and a blank line genuinely has no block.
        if let Some((i, ch)) = self.source[..caret].char_indices().next_back() {
            if ch != '\n' && self.has_block_at(i) {
                return Some(i);
            }
        }
        None
    }

    /// Insert the source marker for `kind` at the caret, to create a block on an
    /// otherwise-empty line. Markdown/djot spell headings with leading `#`s; a
    /// paragraph needs no marker (a blank line is already a paragraph slot).
    fn insert_block_prefix(&mut self, kind: BlockKind) {
        let prefix = match kind {
            BlockKind::Heading(n) if matches!(self.format, Format::Markdown | Format::Djot) => {
                format!("{} ", "#".repeat(n as usize))
            }
            BlockKind::Paragraph => return,
            _ => {
                self.status = Some(format!("{kind:?}: nothing to convert on an empty line"));
                return;
            }
        };
        self.insert(&prefix);
    }

    /// The heading level of the text block at the caret, or `None` when that
    /// block is not a heading.
    pub fn current_heading_level(&mut self) -> Option<u32> {
        let caret = self.caret;
        self.nodes()
            .into_iter()
            .filter(|n| n.kind == "heading")
            .find(|n| n.span.start <= caret && caret <= n.span.end)
            .and_then(|n| n.level)
    }

    /// Toggle a heading at the caret: if the block is already this heading level,
    /// revert it to a paragraph; otherwise convert it to this heading level.
    /// This gives the heading commands the same toggle feel as bold/italic/code —
    /// re-applying a heading a line already has turns it back into body text.
    pub fn toggle_heading(&mut self, level: u32) {
        if self.current_heading_level() == Some(level) {
            self.set_block(BlockKind::Paragraph);
        } else {
            self.set_block(BlockKind::Heading(level));
        }
    }

    // ── undo / redo ───────────────────────────────────────────────────────────
    // twig owns the history (it owns the buffer); leaf just drives it and
    // re-anchors the caret from the returned `Change`. See `splice` for how a
    // run of keystrokes is coalesced into one step via `coalesce_last_undo`.

    /// Undo the last edit step (⌘Z / ^Z).
    pub fn undo(&mut self) {
        match self.editor.undo() {
            Ok(Some(change)) => self.after_history(change),
            Ok(None) => self.status = Some("nothing to undo".into()),
            Err(e) => self.status = Some(format!("undo: {e}")),
        }
    }

    /// Redo the last undone edit step (⇧⌘Z / ^Y).
    pub fn redo(&mut self) {
        match self.editor.redo() {
            Ok(Some(change)) => self.after_history(change),
            Ok(None) => self.status = Some("nothing to redo".into()),
            Err(e) => self.status = Some(format!("redo: {e}")),
        }
    }

    /// Refresh the cached source and re-anchor the caret to where an undo/redo
    /// landed (the end of the changed region), clearing any active run.
    fn after_history(&mut self, change: Change) {
        self.refresh();
        self.caret = change.new.end.min(self.source.len());
        self.anchor = None;
        self.goal_col = None;
        self.last_edit_kind = None;
        self.dirty = self.source != self.clean_source;
        self.status = None;
        self.clamp_caret();
    }

    pub fn save(&mut self) {
        match std::fs::write(&self.path, self.source.as_bytes()) {
            Ok(()) => {
                self.clean_source = self.source.clone();
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
        self.goal_col = None;
        self.move_to(offset, extend);
        self.clamp_caret();
    }

    /// Select the whole document (⌘A / Ctrl+A) — everything reachable in the
    /// active view, so in WYSIWYG it starts below hidden frontmatter (copy won't
    /// grab the metadata) while the source view still selects the literal whole.
    pub fn select_all(&mut self) {
        self.anchor = Some(self.caret_floor());
        self.caret = self.source.len();
        self.goal_col = None;
        self.last_edit_kind = None;
        self.status = None;
    }

    /// Select the word (or whitespace / punctuation run) at `offset` — the
    /// double-click gesture. Anchors on the run's start with the caret at its
    /// end so a following Shift-motion extends from the far edge.
    pub fn select_word_at(&mut self, offset: usize) {
        let (s, e) = word_range_at(&self.source, offset.min(self.source.len()));
        self.anchor = Some(s);
        self.caret = e;
        self.goal_col = None;
        self.last_edit_kind = None;
        self.status = None;
        self.clamp_caret();
    }

    /// Select the whole enclosing text block (paragraph, heading, list item's
    /// text…) at `offset` — the triple-click gesture. Reads the range straight
    /// from the AST (twig's `content_span`), so it selects the entire *logical*
    /// paragraph even when that paragraph soft-wraps across several visual rows —
    /// where a visual-row-based select breaks down, because one source offset at
    /// a wrap boundary belongs to two rows at once.
    pub fn select_block_at(&mut self, offset: usize) {
        let off = offset.min(self.source.len());
        let range = self
            .editor
            .ancestors_at(off)
            .ok()
            .and_then(|chain| {
                // Ancestors run root → deepest; the deepest node that is neither
                // an inline span nor a multi-block container is the text block
                // the caret sits in (a paragraph, a heading, a code block…).
                chain
                    .into_iter()
                    .rev()
                    .find(|m| !wysiwyg::is_inline(&m.kind) && !is_block_container(&m.kind))
                    .map(|m| m.content_span.unwrap_or(m.span))
            })
            .unwrap_or_else(|| source_line_range(&self.source, off));
        self.anchor = Some(range.start.min(self.source.len()));
        self.caret = range.end.min(self.source.len());
        self.goal_col = None;
        self.last_edit_kind = None;
        self.status = None;
        self.clamp_caret();
    }

    /// The lowest source offset the caret may occupy in the active view. In
    /// WYSIWYG, leading frontmatter is hidden and unreachable, so the floor is
    /// the first rendered offset; the source view reaches everything, so it's 0.
    fn caret_floor(&self) -> usize {
        match self.view {
            View::Wysiwyg => self.vmap.content_start.min(self.source.len()),
            View::Source => 0,
        }
    }

    fn move_to(&mut self, offset: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
        self.caret = offset.min(self.source.len()).max(self.caret_floor());
        self.status = None;
        // A caret move ends the current typing/deletion run, so the next edit
        // starts a fresh undo group rather than coalescing across the gap.
        self.last_edit_kind = None;
    }

    // In the source view, motion walks source bytes / source lines. In the
    // WYSIWYG view it walks the rendered glyph grid (the visual map), which is
    // what steps the caret cleanly over hidden delimiters.

    pub fn move_left(&mut self, extend: bool) {
        self.goal_col = None;
        if !extend {
            if let Some((s, _e)) = self.selection() {
                self.move_to(s, false);
                return;
            }
        }
        let target = match self.view {
            View::Source => {
                if self.caret > 0 {
                    prev_boundary(&self.source, self.caret)
                } else {
                    0
                }
            }
            // Walks caret *stops*, not columns: decoration (a table border, a
            // cell's padding) is stepped over in one press, and a hidden
            // delimiter never holds the caret up.
            View::Wysiwyg => self.vmap.stop_before(self.caret).unwrap_or(self.caret),
        };
        self.move_to(target, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        self.goal_col = None;
        if !extend {
            if let Some((_s, e)) = self.selection() {
                self.move_to(e, false);
                return;
            }
        }
        let target = match self.view {
            View::Source => {
                if self.caret < self.source.len() {
                    next_boundary(&self.source, self.caret)
                } else {
                    self.caret
                }
            }
            View::Wysiwyg => self.vmap.stop_after(self.caret).unwrap_or(self.caret),
        };
        self.move_to(target, extend);
    }

    /// Move to the start of the previous word (⌥← / Ctrl+←). Word boundaries
    /// are computed over the source in both views, since the source is the
    /// document of record and the caret is always a source offset.
    pub fn move_word_left(&mut self, extend: bool) {
        self.goal_col = None;
        let target = prev_word(&self.source, self.caret);
        self.move_to(target, extend);
    }

    /// Move to the end of the next word (⌥→ / Ctrl+→).
    pub fn move_word_right(&mut self, extend: bool) {
        self.goal_col = None;
        let target = next_word(&self.source, self.caret);
        self.move_to(target, extend);
    }

    pub fn move_up(&mut self, extend: bool) {
        let (row, col) = self.caret_pos();
        let goal = *self.goal_col.get_or_insert(col);
        if row == 0 {
            return;
        }
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row - 1, goal),
            View::Wysiwyg => {
                // A table's border rules are drawn but hold no caret, so Up
                // steps over them to the row that does.
                let Some(r) = self.vmap.navigable_above(row) else {
                    return;
                };
                self.vmap.offset_of_pos(r, goal.min(self.vmap.row_len(r)))
            }
        };
        self.move_to(target, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        let (row, col) = self.caret_pos();
        let goal = *self.goal_col.get_or_insert(col);
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row + 1, goal),
            View::Wysiwyg => {
                let Some(r) = self.vmap.navigable_below(row) else {
                    return;
                };
                self.vmap.offset_of_pos(r, goal.min(self.vmap.row_len(r)))
            }
        };
        self.move_to(target, extend);
    }

    pub fn move_home(&mut self, extend: bool) {
        self.goal_col = None;
        let (row, _) = self.caret_pos();
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row, 0),
            View::Wysiwyg => self.vmap.offset_of_pos(row, 0),
        };
        self.move_to(target, extend);
    }

    pub fn move_end(&mut self, extend: bool) {
        self.goal_col = None;
        let (row, _) = self.caret_pos();
        let target = match self.view {
            View::Source => line_end(&self.source, row),
            View::Wysiwyg => self.vmap.offset_of_pos(row, self.vmap.row_len(row)),
        };
        self.move_to(target, extend);
    }

    /// Hop to the next (Tab) or previous (Shift+Tab) table cell, landing at the
    /// start of its text. Returns `false` when the caret isn't in a table, or is
    /// already in the last/first cell — the frontend then does whatever Tab
    /// normally does (indent), so Tab keeps its meaning everywhere else.
    pub fn cell_hop(&mut self, forward: bool) -> bool {
        let off = self.caret;
        let Some(cells) = self.table_cells_at(off) else {
            return false;
        };
        let Some(i) = cells.iter().position(|r| off >= r.start && off <= r.end) else {
            return false;
        };
        let next = if forward { i.checked_add(1) } else { i.checked_sub(1) };
        let Some(target) = next.and_then(|j| cells.get(j)) else {
            return false; // at the table's edge; leave Tab to the frontend
        };
        self.goal_col = None;
        self.move_to(target.start, false);
        true
    }

    /// The content ranges of every cell in the table containing `off`, in
    /// document order — `None` when `off` isn't inside a table's cell. Scoped to
    /// the *one* table, so Tab in the last cell never jumps into another one.
    fn table_cells_at(&mut self, off: usize) -> Option<Vec<std::ops::Range<usize>>> {
        let nodes = self.nodes();
        let in_cell = |n: &FlatNode| {
            n.kind == "cell"
                && n.content_span
                    .as_ref()
                    .is_some_and(|r| off >= r.start && off <= r.end)
        };
        let table = table_of(&nodes, nodes.iter().find(|n| in_cell(n))?)?;
        let mut out: Vec<std::ops::Range<usize>> = nodes
            .iter()
            .filter(|n| n.kind == "cell" && table_of(&nodes, n) == Some(table))
            .filter_map(|n| n.content_span.clone())
            .collect();
        out.sort_by_key(|r| r.start);
        Some(out)
    }

    /// Move the caret to the very start of the document (⌘↑ on macOS,
    /// Ctrl+Home on Windows/Linux).
    pub fn move_doc_start(&mut self, extend: bool) {
        self.goal_col = None;
        self.move_to(0, extend);
    }

    /// Move the caret to the very end of the document (⌘↓ on macOS,
    /// Ctrl+End on Windows/Linux).
    pub fn move_doc_end(&mut self, extend: bool) {
        self.goal_col = None;
        let end = self.source.len();
        self.move_to(end, extend);
    }

    /// Point the caret at the body cell `(row, col)` the mouse landed on.
    pub fn click(&mut self, row: usize, col: usize, extend: bool) {
        self.goal_col = None;
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row, col),
            View::Wysiwyg => self.vmap.offset_of_pos(row, col),
        };
        self.move_to(target, extend);
    }

    /// Settle `scroll` for a frame about to be drawn: follow the caret onto the
    /// screen if it has moved since the last frame, and never scroll past the
    /// last of `rows`.
    ///
    /// Only if it has *moved* — that's the whole point. Revealing the caret on
    /// every frame ties the viewport to it, and a scroll wheel that fights the
    /// caret for the viewport loses: the view snaps back the instant it tries to
    /// pass the caret's row, so the document can't be scrolled beyond what's
    /// already on screen. A caret move is the frontend's cue to follow; a scroll
    /// with the caret sitting still is the reader's cue to leave it alone.
    pub fn follow_caret(&mut self, caret_row: usize, height: usize, rows: usize) {
        if self.drawn_caret != Some(self.caret) {
            if caret_row < self.scroll {
                self.scroll = caret_row;
            } else if height > 0 && caret_row >= self.scroll + height {
                self.scroll = caret_row + 1 - height;
            }
            self.drawn_caret = Some(self.caret);
        }
        self.scroll = self.scroll.min(rows.saturating_sub(1));
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
        // In WYSIWYG the caret can't sit inside hidden frontmatter; lift it (and
        // any selection anchor) to the first rendered offset.
        let floor = self.caret_floor();
        if self.caret < floor {
            self.caret = floor;
        }
        if let Some(a) = self.anchor {
            if a < floor {
                self.anchor = Some(floor);
            }
        }
        while self.caret > 0 && !self.source.is_char_boundary(self.caret) {
            self.caret -= 1;
        }
    }
}

// ── byte-offset ⇄ (row, col) helpers ─────────────────────────────────────────

// Left/right motion and backspace/delete step by *grapheme cluster*, not
// codepoint, so an emoji (a ZWJ sequence) or a base letter plus its combining
// marks moves and deletes as the single character a user sees. Grapheme
// boundaries are a superset of char boundaries, so the caret stays valid for twig.

fn prev_boundary(s: &str, i: usize) -> usize {
    let mut cursor = GraphemeCursor::new(i, s.len(), true);
    cursor.prev_boundary(s, 0).ok().flatten().unwrap_or(0)
}

fn next_boundary(s: &str, i: usize) -> usize {
    let mut cursor = GraphemeCursor::new(i, s.len(), true);
    cursor.next_boundary(s, 0).ok().flatten().unwrap_or(s.len())
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

/// A block that holds other blocks (not a single line of text). `select_block_at`
/// skips these so a triple-click grabs the paragraph, not the whole list/section.
/// The marker for the *next* list item given the current one: a bullet repeats
/// (`"- "` → `"- "`), an ordered marker increments (`"1. "` → `"2. "`), keeping
/// any leading indentation and the delimiter/spacing.
fn next_list_marker(marker: &str) -> String {
    let indent_len = marker.len() - marker.trim_start().len();
    let (indent, rest) = marker.split_at(indent_len);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if let Ok(n) = digits.parse::<u64>() {
        // ordered: bump the number, keep the delimiter + trailing space(s).
        format!("{indent}{}{}", n + 1, &rest[digits.len()..])
    } else {
        marker.to_string()
    }
}

/// The id of the `table` a node lives under, or `None` if it isn't in one.
/// Walks parents rather than assuming `cell`'s grandparent, so a nested table
/// still resolves to the one that actually encloses the cell.
fn table_of(nodes: &[FlatNode], node: &FlatNode) -> Option<usize> {
    let mut cur = node.parent;
    while let Some(id) = cur {
        let n = &nodes[id.0 as usize];
        if n.kind == "table" {
            return Some(id.0 as usize);
        }
        cur = n.parent;
    }
    None
}

fn is_block_container(kind: &str) -> bool {
    matches!(
        kind,
        "doc" | "section"
            | "block_quote"
            | "bullet_list"
            | "ordered_list"
            | "task_list"
            | "list_item"
            | "task_list_item"
    )
}

/// The `[start, end)` byte range of the source line containing `off` (newline
/// excluded) — the fallback when `off` sits outside any AST block (e.g. a blank
/// line between paragraphs).
fn source_line_range(s: &str, off: usize) -> std::ops::Range<usize> {
    let off = off.min(s.len());
    let start = s[..off].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let end = s[off..].find('\n').map(|p| off + p).unwrap_or(s.len());
    start..end
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

    // Source-view document for the source-behaviour tests. `Doc::open` now
    // defaults to WYSIWYG (leaf's default view), so pin the source view here;
    // `wysiwyg_doc` builds the rich-text variant on top of this.
    fn doc_with(name: &str, body: &str) -> Doc {
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_test_{name}.md"));
        std::fs::write(&p, body).unwrap();
        let mut d = Doc::open(p).unwrap();
        d.view = View::Source;
        d
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
    fn select_block_grabs_the_whole_paragraph_from_any_wrapped_row() {
        // Regression: triple-click used move_home/move_end over visual rows, so
        // it only worked on a paragraph's first row (a wrap-boundary offset maps
        // to the earlier row). select_block_at reads the AST, so every offset in
        // the paragraph selects the whole thing.
        let body = "one two three four five six seven eight\n";
        let mut d = doc_with("sel_block", body);
        d.view = View::Wysiwyg;
        d.build_visual(12); // force the paragraph to wrap into several rows
        assert!(d.vmap.num_rows() > 1, "test needs a wrapped paragraph");
        let para = (0, "one two three four five six seven eight".len());
        for off in [0usize, 8, 19, 28, 38] {
            d.caret = 0;
            d.anchor = None;
            d.select_block_at(off);
            assert_eq!(d.selection(), Some(para), "offset {off} should select the paragraph");
        }
    }

    #[test]
    fn select_block_uses_content_span_for_a_heading() {
        let mut d = doc_with("sel_head", "# Title\n\nbody\n");
        d.select_block_at(4); // inside "Title"
        // content_span excludes the "# " marker.
        assert_eq!(d.selected_text(), Some("Title"));
        d.select_block_at(10); // inside "body"
        assert_eq!(d.selected_text(), Some("body"));
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
    fn toggle_code_wraps_then_unwraps_the_selection() {
        let mut d = doc_with("code_rt", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Verbatim);
        assert_eq!(d.source, "a `word` b\n");
        d.toggle(InlineKind::Verbatim);
        assert_eq!(d.source, "a word b\n");
    }

    #[test]
    fn set_block_turns_a_paragraph_into_a_heading_at_the_caret() {
        let mut d = doc_with("head_set", "hello\n");
        d.caret = 2; // caret inside the paragraph, no selection
        d.set_block(BlockKind::Heading(1));
        assert_eq!(d.source, "# hello\n");
    }

    #[test]
    fn set_block_heading_works_in_wysiwyg_view() {
        // The app defaults to WYSIWYG; the caret is a source offset either way.
        let mut d = wysiwyg_doc("head_wys", "hello\n");
        d.caret = 2;
        d.set_block(BlockKind::Heading(1));
        assert_eq!(d.source, "# hello\n");
    }

    #[test]
    fn toggle_heading_applies_switches_and_reverts() {
        let mut d = doc_with("head_toggle", "hello\n");
        d.caret = 2;
        d.toggle_heading(1);
        assert_eq!(d.source, "# hello\n"); // paragraph → H1
        d.toggle_heading(2);
        assert_eq!(d.source, "## hello\n"); // H1 → H2 (different level switches)
        d.toggle_heading(2);
        assert_eq!(d.source, "hello\n"); // same level reverts to paragraph
    }

    #[test]
    fn wysiwyg_one_enter_starts_a_new_paragraph() {
        // Regression: one Enter left the caret between the two newlines, so typing
        // made a soft break (one paragraph) and you needed a second Enter.
        let mut d = wysiwyg_doc("wys_enter", "abc\n");
        d.caret = 3;
        d.newline();
        d.insert("def");
        assert_eq!(d.source, "abc\n\ndef\n"); // two paragraphs, not "abc\ndef\n"
    }

    #[test]
    fn source_view_enter_is_a_single_newline() {
        let mut d = doc_with("src_enter", "abc\n");
        d.caret = 3;
        d.newline();
        assert_eq!(d.source, "abc\n\n");
    }

    #[test]
    fn heading_applies_at_the_end_of_a_paragraph() {
        // The caret at a line end sits at the doc level; set_block must still find
        // the block on that line.
        let mut d = doc_with("head_end", "abc\n");
        d.caret = 3; // end of "abc"
        d.toggle_heading(1);
        assert_eq!(d.source, "# abc\n");
    }

    #[test]
    fn heading_on_an_empty_new_paragraph_creates_one() {
        let mut d = wysiwyg_doc("head_empty", "abc\n");
        d.caret = 3;
        d.newline(); // caret now on a fresh, empty paragraph
        d.toggle_heading(1);
        d.insert("Title");
        assert!(d.source.contains("# Title"), "got {:?}", d.source);
    }

    #[test]
    fn wysiwyg_enter_after_a_heading_makes_a_paragraph() {
        let mut d = wysiwyg_doc("head_enter", "# Title\n");
        d.caret = 7; // end of the heading
        d.newline();
        d.insert("body");
        assert_eq!(d.source, "# Title\n\nbody\n");
    }

    #[test]
    fn wysiwyg_enter_continues_a_bullet_list() {
        let mut d = wysiwyg_doc("wys_bullet", "- item\n");
        d.caret = 6; // end of "item"
        d.newline();
        d.insert("two");
        assert_eq!(d.source, "- item\n- two\n");
    }

    #[test]
    fn wysiwyg_enter_increments_an_ordered_list() {
        let mut d = wysiwyg_doc("wys_ol", "1. one\n");
        d.caret = 6; // end of "one"
        d.newline();
        d.insert("two");
        assert_eq!(d.source, "1. one\n2. two\n");
    }

    #[test]
    fn wysiwyg_enter_on_an_empty_list_item_exits_the_list() {
        let mut d = wysiwyg_doc("wys_exit", "- a\n- \n");
        d.caret = 6; // end of the empty "- " item
        d.newline();
        d.insert("p");
        assert_eq!(d.source, "- a\n\np\n");
    }

    #[test]
    fn wysiwyg_enter_in_a_code_block_is_a_literal_newline() {
        let mut d = wysiwyg_doc("wys_code", "```\nabc\n```\n");
        d.caret = 7; // end of "abc" inside the fence
        d.newline();
        d.insert("def");
        assert_eq!(d.source, "```\nabc\ndef\n```\n");
    }

    #[test]
    fn wysiwyg_enter_continues_a_block_quote() {
        let mut d = wysiwyg_doc("wys_quote", "> quote\n");
        d.caret = 7; // end of "quote"
        d.newline();
        d.insert("more");
        assert_eq!(d.source, "> quote\n> more\n");
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
        // still crossed). Both directions must step through it symmetrically.
        //
        // It's now stepped *over* rather than onto: the blank line between two
        // paragraphs is the boundary being drawn, not a line of the document, so
        // one press of Down crosses it. The goal column survives the crossing —
        // col 3 at the end of "abc" is col 3 at the end of "def".
        let mut d = wysiwyg_doc("wys_down", "abc\n\ndef\n");
        d.caret = 3; // end of "abc" (row 0)
        d.move_down(false);
        assert_eq!(d.caret_pos().0, 2, "Down should reach the second paragraph");
        assert_eq!(d.caret, 8); // end of "def", col 3 kept
        d.move_up(false);
        assert_eq!(d.caret_pos().0, 0, "Up should come back symmetrically");
        assert_eq!(d.caret, 3);
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
    fn wysiwyg_enter_between_paragraphs_lands_on_an_empty_line() {
        // The reported bug: Enter at the end of a paragraph that has another
        // paragraph below put the caret at the *start of the next paragraph* —
        // the empty paragraph it opened had no row, so the caret snapped onto
        // "World". It must now sit on its own empty line, with a blank spacer
        // above it (the paragraph gap).
        let mut d = wysiwyg_doc("wys_gap_mid", "Hello\n\nWorld\n");
        d.caret = 5; // end of "Hello"
        d.newline();
        d.build_visual(80);
        let (row, col) = d.caret_pos();
        assert_eq!(col, 0, "caret should start an empty line, not sit in text");
        assert_eq!(d.vmap.row_len(row), 0, "caret's row must be empty, not 'World'");
        assert!(row >= 2, "a blank spacer row should sit above the caret, got row {row}");
        // The row above the caret is a real (empty) gap, and "Hello" stays put.
        assert_eq!(d.vmap.row_len(row - 1), 0, "the row above the caret is a gap");
        let row0: String = d.vmap.rows[0].glyphs.iter().map(|g| g.ch).collect();
        assert_eq!(row0, "Hello", "the paragraph above the caret must not move");
    }

    #[test]
    fn wysiwyg_enter_at_eof_shows_a_gap_before_typing() {
        // At the document end a single Enter must also show the paragraph gap —
        // a blank spacer row above the caret — so the layout already matches how
        // it will look once the new paragraph has text.
        let mut d = wysiwyg_doc("wys_gap_eof", "Hello");
        d.caret = 5; // end of "Hello", no trailing newline
        d.newline(); // source becomes "Hello\n\n"
        d.build_visual(80);
        let (row, col) = d.caret_pos();
        assert_eq!(col, 0);
        assert!(row >= 2, "caret should sit below a blank spacer, got row {row}");
        assert_eq!(d.vmap.row_len(row - 1), 0, "the row above the caret is a gap");
    }

    #[test]
    fn wysiwyg_typing_after_enter_does_not_shift_the_caret_row() {
        // The spacer is view-only: typing the new paragraph must not reflow the
        // caret onto a different row — the transient view already matched the
        // settled one.
        let mut d = wysiwyg_doc("wys_no_reflow", "Hello\n\nWorld\n");
        d.caret = 5;
        d.newline();
        d.build_visual(80);
        let before = d.caret_pos();
        d.insert("New");
        d.build_visual(80);
        let after = d.caret_pos();
        assert_eq!(
            after.0, before.0,
            "typing must not move the caret to another row ({before:?} -> {after:?})"
        );
    }

    #[test]
    fn wysiwyg_hides_frontmatter_from_the_caret_and_copy() {
        let fm = "---\ntitle: hi\n---\n";
        let body = format!("{fm}# leaf\n\nbody\n");
        let mut d = wysiwyg_doc("wys_fm", &body);
        // Opening lifts the caret out of the now-hidden frontmatter.
        assert_eq!(d.caret, fm.len(), "caret should start at the first real block");
        // Left at the content start can't step back into frontmatter.
        d.move_left(false);
        assert_eq!(d.caret, fm.len(), "left must not enter frontmatter");
        // Doc-start lands on the content floor, not offset 0.
        d.move_doc_start(false);
        assert_eq!(d.caret, fm.len());
        // Select-all + copy never include the frontmatter bytes.
        d.select_all();
        let sel = d.selected_text().unwrap().to_string();
        assert!(!sel.contains("title"), "copy leaked frontmatter: {sel:?}");
        assert!(sel.starts_with("# leaf"), "selection should begin at content: {sel:?}");
    }

    #[test]
    fn wysiwyg_backspace_at_content_start_leaves_frontmatter_intact() {
        // Backspace deletes `prev_boundary..caret` directly; at the first real
        // block that boundary is inside the hidden frontmatter, so it must be a
        // no-op rather than eating the closing `---`.
        let fm = "---\ntitle: hi\n---\n";
        let body = format!("{fm}leaf\n");
        let mut d = wysiwyg_doc("wys_fm_bs", &body);
        assert_eq!(d.caret, fm.len());
        d.backspace();
        assert_eq!(d.source, body, "backspace must not touch frontmatter");
        d.delete_word_back();
        assert_eq!(d.source, body, "word-delete must not touch frontmatter either");
    }

    #[test]
    fn source_view_still_reaches_frontmatter() {
        // The metadata is only *hidden*, never lost: the source view edits and
        // selects it in full, and it's always preserved on save.
        let fm = "---\ntitle: hi\n---\n";
        let body = format!("{fm}# leaf\n");
        let mut d = doc_with("src_fm", &body);
        d.select_all();
        let sel = d.selected_text().unwrap();
        assert!(sel.contains("title"), "source view should select everything");
        d.move_doc_start(false);
        assert_eq!(d.caret, 0, "source view can reach offset 0");
    }

    const TABLE: &str = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n| Fig | 12 |\n";

    #[test]
    fn wysiwyg_right_crosses_a_cell_border_without_stalling() {
        // The border and padding between two cells all share one source offset,
        // so a column-stepping caret would sit on `│` and then stall there
        // forever. Right must step: end of "Name" -> start of "Qty".
        let mut d = wysiwyg_doc("tbl_right", TABLE);
        d.caret = TABLE.find("Name").unwrap() + 4; // just after "Name"
        d.move_right(false);
        assert_eq!(d.caret, TABLE.find("Qty").unwrap(), "should land in the next cell");
        let (r, c) = d.caret_pos();
        assert_eq!(d.vmap.rows[r].glyphs[c].ch, 'Q');
    }

    #[test]
    fn wysiwyg_left_crosses_back_to_the_previous_cell() {
        let mut d = wysiwyg_doc("tbl_left", TABLE);
        d.caret = TABLE.find("Qty").unwrap();
        d.move_left(false);
        assert_eq!(d.caret, TABLE.find("Name").unwrap() + 4, "end of the previous cell");
    }

    #[test]
    fn wysiwyg_down_steps_over_a_table_rule() {
        // Between the header and the first body row sits a `├───┼───┤` rule.
        // It's drawn but holds no caret, so one Down must reach "Pear".
        let mut d = wysiwyg_doc("tbl_down", TABLE);
        d.caret = TABLE.find("Name").unwrap();
        d.move_down(false);
        assert_eq!(d.caret, TABLE.find("Pear").unwrap(), "one Down reaches the body row");
        d.move_down(false);
        assert_eq!(d.caret, TABLE.find("Fig").unwrap());
    }

    #[test]
    fn wysiwyg_tab_walks_the_cells_and_shift_tab_walks_back() {
        let mut d = wysiwyg_doc("tbl_tab", TABLE);
        d.caret = TABLE.find("Name").unwrap();
        assert!(d.cell_hop(true));
        assert_eq!(d.caret, TABLE.find("Qty").unwrap());
        assert!(d.cell_hop(true), "Tab wraps onto the next row's first cell");
        assert_eq!(d.caret, TABLE.find("Pear").unwrap());
        assert!(d.cell_hop(false));
        assert_eq!(d.caret, TABLE.find("Qty").unwrap());
    }

    #[test]
    fn tab_outside_a_table_is_not_a_cell_hop() {
        // `cell_hop` reports false so the frontend can indent as usual.
        let mut d = wysiwyg_doc("tbl_none", "just a paragraph\n");
        d.caret = 4;
        assert!(!d.cell_hop(true));
        assert_eq!(d.caret, 4, "a refused hop leaves the caret alone");
    }

    #[test]
    fn tab_at_the_last_cell_declines_rather_than_leaving_the_table() {
        let mut d = wysiwyg_doc("tbl_edge", TABLE);
        d.caret = TABLE.rfind("12").unwrap(); // the final cell
        assert!(!d.cell_hop(true), "no cell after the last one");
        d.caret = TABLE.find("Name").unwrap();
        assert!(!d.cell_hop(false), "no cell before the first one");
    }

    #[test]
    fn typing_in_a_cell_edits_that_cell() {
        // Editing comes free once offsets map correctly: the caret is a source
        // offset, so a normal splice lands inside the pipe table.
        let mut d = wysiwyg_doc("tbl_type", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4;
        d.insert("s");
        assert!(d.source.contains("| Pears | 3 |"), "got {:?}", d.source);
    }

    #[test]
    fn motion_and_delete_treat_an_emoji_as_one_character() {
        // 👨‍👩‍👧 is a single grapheme built from three emoji joined by ZWJ — 18
        // bytes, several codepoints. Right-arrow must clear it in one step, and
        // backspace must remove the whole cluster, not a stray joiner.
        let family = "👨‍👩‍👧";
        let mut d = doc_with("emoji", &format!("a{family}b\n"));
        d.caret = 1; // just after 'a', before the emoji
        d.move_right(false);
        assert_eq!(d.caret, 1 + family.len(), "one step clears the whole cluster");
        assert_eq!(&d.source[d.caret..d.caret + 1], "b");

        d.backspace(); // delete the emoji as a unit
        assert_eq!(d.source, "ab\n");
        assert_eq!(d.caret, 1);
    }

    #[test]
    fn motion_handles_a_combining_accent_as_one_character() {
        // "e" + U+0301 (combining acute) renders as one é.
        let mut d = doc_with("combining", "e\u{0301}x\n");
        d.caret = 0;
        d.move_right(false);
        assert_eq!(d.caret, "e\u{0301}".len(), "steps past base + combining mark");
    }

    #[test]
    fn undo_then_redo_round_trips_an_edit() {
        let mut d = doc_with("undo", "hello\n");
        d.caret = 5;
        d.insert("!");
        assert_eq!(d.source, "hello!\n");
        d.undo();
        assert_eq!(d.source, "hello\n");
        assert_eq!(d.caret, 5, "undo restores the caret");
        d.redo();
        assert_eq!(d.source, "hello!\n");
    }

    #[test]
    fn a_run_of_typing_undoes_as_one_step() {
        let mut d = doc_with("coalesce", "\n");
        d.caret = 0;
        d.insert("a");
        d.insert("b");
        d.insert("c");
        assert_eq!(d.source, "abc\n");
        d.undo(); // the whole typed run, not just "c"
        assert_eq!(d.source, "\n");
        d.undo(); // nothing left — the run was one step
        assert_eq!(d.source, "\n");
        assert_eq!(d.status.as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn moving_the_caret_starts_a_new_undo_group() {
        let mut d = doc_with("break", "\n");
        d.caret = 0;
        d.insert("a");
        d.insert("b"); // "ab\n", caret at 2
        d.move_left(false); // breaks the run
        d.insert("X"); // "aXb\n"
        assert_eq!(d.source, "aXb\n");
        d.undo();
        assert_eq!(d.source, "ab\n", "first undo removes only the post-move insert");
        d.undo();
        assert_eq!(d.source, "\n", "second undo removes the earlier run");
    }

    #[test]
    fn undo_reverses_a_format_toggle() {
        let mut d = doc_with("fmt_undo", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a **word** b\n");
        d.undo();
        assert_eq!(d.source, "a word b\n");
    }

    #[test]
    fn undo_back_to_the_saved_state_clears_dirty() {
        let mut d = doc_with("dirty_undo", "hello\n");
        assert!(!d.dirty);
        d.caret = 5;
        d.insert("!");
        assert!(d.dirty);
        d.undo();
        assert!(!d.dirty, "undoing to the saved source is not a modification");
    }

    #[test]
    fn a_new_edit_invalidates_redo() {
        let mut d = doc_with("redo_inv", "\n");
        d.caret = 0;
        d.insert("a");
        d.undo();
        d.insert("b"); // diverges — the redo of "a" is now gone
        d.redo();
        assert_eq!(d.source, "b\n");
    }

    #[test]
    fn undo_on_empty_history_is_a_no_op() {
        let mut d = doc_with("undo_empty", "hi\n");
        d.undo();
        assert_eq!(d.source, "hi\n");
        assert_eq!(d.status.as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn vertical_motion_keeps_the_column() {
        let mut d = doc_with("move", "abcd\nef\n");
        d.caret = 3; // "abc|d" on row 0, col 3
        d.move_down(false); // row 1 "ef" only has cols 0..2 -> clamps to end
        assert_eq!(d.caret, 7); // just after "ef"
    }

    // ── goal column ──────────────────────────────────────────────────────────

    #[test]
    fn vertical_motion_goal_column_survives_a_short_line() {
        // Regression: re-deriving the column from the clamped position on
        // every step permanently forgets it once a short line clamps it.
        // Down through "xy" (2 cols) and into "ghijkl" must return to col 4.
        let g = |m, f: fn(&mut Doc)| golden("goalcol", m, f);
        assert_eq!(
            g("abcd|ef\nxy\nghijkl\n", |d| {
                d.move_down(false); // clamps to end of "xy"
                d.move_down(false); // restores col 4 on the long line
            }),
            "abcdef\nxy\nghij|kl\n"
        );
    }

    #[test]
    fn goal_column_state_is_set_by_vertical_motion_and_cleared_by_horizontal() {
        let mut d = doc_with("goalcol_state", "abcdef\nxy\nghijkl\n");
        assert_eq!(d.goal_col, None);
        d.caret = 4; // row 0, col 4
        d.move_down(false); // clamps into "xy"; goal stays the original col
        assert_eq!(d.goal_col, Some(4));
        assert_eq!(d.caret_pos(), (1, 2));

        // A horizontal motion drops the goal column...
        d.move_left(false);
        assert_eq!(d.goal_col, None);

        // ...so the next vertical motion picks up the *new* column (1), not
        // the stale one (4).
        d.move_down(false);
        assert_eq!(d.goal_col, Some(1));
        assert_eq!(d.caret_pos(), (2, 1));
    }

    #[test]
    fn editing_clears_the_goal_column() {
        let mut d = doc_with("goalcol_edit", "abcdef\nxy\nghijkl\n");
        d.caret = 4;
        d.move_down(false);
        assert_eq!(d.goal_col, Some(4));
        d.insert("Z");
        assert_eq!(d.goal_col, None);
    }

    #[test]
    fn vertical_motion_on_an_empty_document_is_a_no_op() {
        let mut d = doc_with("empty_vert", "");
        d.move_down(false);
        assert_eq!(d.caret, 0);
        d.move_up(false);
        assert_eq!(d.caret, 0);
    }

    // ── document start / end ────────────────────────────────────────────────

    #[test]
    fn move_doc_start_and_end_jump_to_the_edges() {
        let g = |m, f: fn(&mut Doc)| golden("doc_edges", m, f);
        assert_eq!(g("hello\nwor|ld\n", |d| d.move_doc_start(false)), "|hello\nworld\n");
        assert_eq!(g("hel|lo\nworld\n", |d| d.move_doc_end(false)), "hello\nworld\n|");
        // Already at the edge: a no-op.
        assert_eq!(g("|hello\n", |d| d.move_doc_start(false)), "|hello\n");
        assert_eq!(g("hello|\n", |d| d.move_doc_end(false)), "hello\n|");
    }

    #[test]
    fn move_doc_start_and_end_extend_the_selection() {
        assert_eq!(
            golden("doc_edges_ext_end", "hello wor|ld\n", |d| d.move_doc_end(true)),
            "hello wor[ld\n|]"
        );
        assert_eq!(
            golden("doc_edges_ext_start", "hello wor|ld\n", |d| d.move_doc_start(true)),
            "[|hello wor]ld\n"
        );
    }

    #[test]
    fn move_doc_start_and_end_on_an_empty_document_are_a_no_op() {
        let mut d = doc_with("empty_edges", "");
        d.move_doc_end(false);
        assert_eq!(d.caret, 0);
        d.move_doc_start(false);
        assert_eq!(d.caret, 0);
    }

    // ── arrow collapses an active selection ─────────────────────────────────

    #[test]
    fn arrow_collapses_selection_to_its_near_edge() {
        let mut d = doc_with("collapse", "hello world\n");

        // Forward selection (anchor before caret): Right -> end, Left -> start.
        d.anchor = Some(2);
        d.caret = 7;
        d.move_right(false);
        assert_eq!((d.caret, d.anchor), (7, None));

        d.anchor = Some(2);
        d.caret = 7;
        d.move_left(false);
        assert_eq!((d.caret, d.anchor), (2, None));

        // Backward selection (anchor after caret): edges are the same
        // regardless of which end the caret started on.
        d.anchor = Some(7);
        d.caret = 2;
        d.move_right(false);
        assert_eq!((d.caret, d.anchor), (7, None));

        d.anchor = Some(7);
        d.caret = 2;
        d.move_left(false);
        assert_eq!((d.caret, d.anchor), (2, None));
    }

    #[test]
    fn arrow_with_extend_keeps_growing_the_selection() {
        let mut d = doc_with("collapse_extend", "hello world\n");
        d.anchor = Some(2);
        d.caret = 7;
        d.move_right(true); // extend: no collapse, caret steps one further
        assert_eq!((d.caret, d.anchor), (8, Some(2)));
    }

    #[test]
    fn arrow_without_a_selection_moves_one_character_as_before() {
        let mut d = doc_with("no_collapse", "hello\n");
        d.caret = 2;
        d.move_right(false);
        assert_eq!(d.caret, 3);
        d.move_left(false);
        assert_eq!(d.caret, 2);
    }

    /// Press Right until it stops, collecting the offsets walked through. Every
    /// caret bug in the WYSIWYG view shows up here as a walk that ends early:
    /// two stops sharing one source offset can't be moved between, so the caret
    /// stalls on the first of them and the walk never reaches the rest.
    fn walk_right(d: &mut Doc) -> Vec<usize> {
        let mut seen = vec![d.caret];
        for _ in 0..2000 {
            let before = d.caret;
            d.move_right(false);
            if d.caret == before {
                break;
            }
            seen.push(d.caret);
        }
        seen
    }

    #[test]
    fn the_caret_crosses_a_soft_break() {
        // A newline inside a paragraph is a `soft_break`, which twig gives no
        // span of its own — the space it renders as used to borrow the offset of
        // the character before it, and a caret can't move without changing
        // offset. Right must walk clean off the end of the first line.
        let mut d = wysiwyg_doc("soft_break_walk", "one two\nthree four\n");
        d.caret = 0;
        let seen = walk_right(&mut d);
        assert_eq!(seen, (0..=18).collect::<Vec<_>>(), "walk stalled: {seen:?}");
    }

    #[test]
    fn the_caret_walks_a_code_block() {
        // Every glyph of a code block used to map to the block's start, so the
        // whole block was a single offset and the caret couldn't move inside it.
        let src = "```rust\nlet x = 1;\nfn f() {}\n```\n";
        let mut d = wysiwyg_doc("code_walk", src);
        d.caret = 0;
        let seen = walk_right(&mut d);
        // The fences are markup: hidden, and no caret stop. The code between
        // them is reached a character at a time.
        let code = src.find("let").unwrap()..src.find("\n```").unwrap();
        for off in code.clone() {
            assert!(seen.contains(&off), "offset {off} unreachable: {seen:?}");
        }
        assert!(seen.contains(&code.end), "no stop after the last line");
    }

    #[test]
    fn the_caret_walks_an_indented_code_block() {
        // An indented block's text has the four-space indent stripped, so it
        // isn't a verbatim slice and its lines have to be re-found. The caret
        // lands on the code, never in the indent.
        let src = "    indented\n    code\n";
        let mut d = wysiwyg_doc("indent_code_walk", src);
        d.caret = 0;
        let seen = walk_right(&mut d);
        assert!(seen.contains(&src.find("indented").unwrap()));
        assert!(seen.contains(&src.find("code").unwrap()));
        assert!(
            !seen.contains(&0) || seen[0] == 0,
            "the caret starts where it was put"
        );
        // Nothing in the stripped indent is a stop.
        for off in [1, 2, 3] {
            assert!(!seen.contains(&off), "landed in the indent at {off}");
        }
    }

    #[test]
    fn the_caret_leaves_a_tight_heading() {
        // "# H" with text directly under it: the heading row's end and the
        // separator row's end are the same offset. Right used to find the
        // separator's copy, set the caret to where it already was, and stop.
        let mut d = wysiwyg_doc("tight_heading_walk", "# H\ntext\n");
        d.caret = 2; // the "H"
        let seen = walk_right(&mut d);
        assert!(seen.len() > 2, "Right stalled at the heading's end: {seen:?}");
        assert!(seen.contains(&8), "never reached the end of \"text\": {seen:?}");
    }

    #[test]
    fn the_caret_skips_the_gap_between_two_paragraphs() {
        // The blank line between two paragraphs is the boundary itself. The
        // caret used to be able to sit on it, and typing there landed in the
        // previous paragraph — "A\n\nB" became "A\nx\nB", one paragraph with a
        // soft break, so the text visibly snapped back up.
        let mut d = wysiwyg_doc("gap_skip", "A\n\nB\n");
        d.caret = 1; // the end of "A"
        d.move_right(false);
        assert_eq!(d.caret, 3, "Right stopped in the gap");
        d.insert("x");
        assert_eq!(d.source, "A\n\nxB\n", "typing landed outside B");
    }

    #[test]
    fn down_from_a_paragraph_lands_on_the_next_one() {
        let mut d = wysiwyg_doc("gap_down", "A\n\nB\n");
        d.caret = 0;
        d.move_down(false);
        assert_eq!(d.caret, 3, "Down stopped in the gap");
    }

    #[test]
    fn clicking_the_gap_lands_on_real_text() {
        // A click can still *reach* the gap — it's drawn, so it's clickable.
        // It has to resolve to somewhere the caret can be.
        let mut d = wysiwyg_doc("gap_click", "A\n\nB\n");
        d.click(1, 0, false); // the gap row
        assert!(d.caret == 1 || d.caret == 3, "click left the caret in the gap at {}", d.caret);
        d.insert("x");
        // Either edge of the boundary is a fair place to land; inside it isn't.
        assert!(
            d.source == "Ax\n\nB\n" || d.source == "A\n\nxB\n",
            "click in the gap typed into the boundary: {:?}",
            d.source
        );
    }

    #[test]
    fn enter_opens_an_empty_paragraph_the_caret_can_type_into() {
        // Enter inserts a paragraph break, which leaves a blank line spare on
        // either side of a new one. That middle line is a real empty paragraph:
        // the caret lands there, and typing makes a paragraph rather than
        // extending a neighbour.
        let mut d = wysiwyg_doc("gap_enter", "A\n\nB\n");
        d.caret = 1;
        d.newline();
        assert_eq!(d.source, "A\n\n\n\nB\n");
        d.build_visual(80);
        let (row, _) = d.caret_pos();
        assert!(d.vmap.row_is_navigable(row), "the caret landed on a gap row");
        d.insert("x");
        assert_eq!(d.source, "A\n\nx\n\nB\n", "the new paragraph merged into a neighbour");
    }

    #[test]
    fn enter_at_the_end_of_the_document_opens_a_paragraph_too() {
        let mut d = wysiwyg_doc("gap_eof", "A\n");
        d.caret = 1;
        d.newline();
        d.build_visual(80);
        let (row, _) = d.caret_pos();
        assert!(d.vmap.row_is_navigable(row), "the caret landed on a gap row");
        d.insert("x");
        assert!(
            d.source.starts_with("A\n\n") && d.source.contains('x'),
            "typing at the end merged into A: {:?}",
            d.source
        );
    }

    #[test]
    fn triple_click_selects_a_paragraph_across_its_soft_breaks() {
        // A paragraph broken over two source lines is one paragraph. Selecting
        // it must not stop at the newline inside it — that newline is markup the
        // rich-text view exists to hide.
        let src = "one two\nthree four\n\nnext\n";
        let mut d = wysiwyg_doc("triple_para", src);
        d.select_block_at(2);
        assert_eq!(d.selected_text(), Some("one two\nthree four"), "stopped at the soft break");
    }

    #[test]
    fn the_wheel_can_scroll_away_from_a_caret_that_stays_put() {
        // The reader scrolls down past the caret's row. Nothing moved the
        // caret, so the view must stay where it was put — the old code revealed
        // the caret every frame, which dragged the view straight back and made
        // the document unscrollable past the caret.
        let mut d = wysiwyg_doc("scroll_free", "a\n\nb\n\nc\n\nd\n\ne\n");
        d.caret = 0;
        d.follow_caret(0, 3, 9); // first frame: the caret is at the top
        d.scroll = 4; // the wheel
        d.follow_caret(0, 3, 9);
        assert_eq!(d.scroll, 4, "the wheel was overruled by a caret that never moved");
    }

    #[test]
    fn moving_the_caret_brings_the_view_back_to_it() {
        let mut d = wysiwyg_doc("scroll_follow", "a\n\nb\n\nc\n\nd\n\ne\n");
        d.caret = 0;
        d.follow_caret(0, 3, 9);
        d.scroll = 6; // scrolled away
        d.move_right(false); // ...and now the caret moves
        let (row, _) = d.caret_pos();
        d.follow_caret(row, 3, 9);
        assert!(d.scroll <= row && row < d.scroll + 3, "caret row {row} off screen at scroll {}", d.scroll);
    }

    #[test]
    fn scrolling_stops_at_the_last_row() {
        let mut d = wysiwyg_doc("scroll_clamp", "a\n\nb\n");
        d.caret = 0;
        d.follow_caret(0, 3, 3); // a first frame, so the caret isn't "new"
        d.scroll = 999; // the wheel, spun hard
        d.follow_caret(0, 3, 3);
        assert_eq!(d.scroll, 2, "scrolled into the void past the document");
    }

    #[test]
    fn every_cell_of_a_wide_table_is_reachable() {
        // A table whose cells are far wider than the surface: the columns are
        // cut to fit and the text wraps inside them, so no cell hangs off the
        // right edge where the caret can never go.
        let src = "| Ingredient | Notes |\n|---|---|\n\
                   | flour milled coarse | sift it twice before folding it in |\n";
        let mut d = wysiwyg_doc("wide_table_walk", src);
        d.build_visual(30);
        d.caret = 0;
        let seen = walk_right(&mut d);
        for word in ["Ingredient", "Notes", "coarse", "folding"] {
            let at = src.find(word).unwrap();
            assert!(seen.contains(&at), "{word:?} at {at} unreachable: {seen:?}");
        }
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

