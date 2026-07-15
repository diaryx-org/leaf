//! The document model: a `twig::Editor` plus a byte-offset caret and selection.
//!
//! Where bough moves a selection through the *tree*, leaf moves a *caret*
//! through the *characters* — a normal text editor's model — and expresses
//! every mutation as one of twig's offset-addressed ops:
//!
//!   - typing / delete  → `edit_range(start, end, text)`   (P0)
//!   - re-anchoring      → the returned `Change`            (P1)
//!   - cursor context    → `node_at` / `ancestors_at`       (P3)
//!   - the toolbar       → `wrap_range`/`toggle_inline`/`set_block`,
//!                         `toggle_block_container`/`insert_link`   (P5)
//!
//! twig reparses after every edit and leaves everything outside the splice
//! byte-for-byte untouched, so the document stays a live, navigable AST while
//! you type into it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use twig::{BlockContainerKind, BlockKind, Change, Editor, FlatNode, Format, InlineKind};
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

/// The caret and selection at one moment — the part of a history step twig's
/// `Change` cannot carry, because the caret is leaf's state and twig only knows
/// about bytes.
#[derive(Clone, Copy)]
struct CaretState {
    caret: usize,
    anchor: Option<usize>,
}

/// The leaf-side half of one undo step, sitting at the same depth as twig's:
/// `before` is where the caret was when the edit began, `after` where the edit
/// left it. Undo restores `before`, redo `after` — an edit is only truly
/// reversed when the caret comes back too, and where the caret *was* is not
/// something the bytes remember.
#[derive(Clone, Copy)]
struct CaretStep {
    before: CaretState,
    after: CaretState,
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
    /// Caret/selection states running in lockstep with twig's undo and redo
    /// stacks — `caret_undo[i]` belongs to twig's i-th undo step, and the two
    /// are pushed, popped, coalesced, and truncated together or not at all.
    ///
    /// Drift here is silent and awful: the stacks stay the same *depth* while
    /// holding states from different timelines, so undo puts the caret somewhere
    /// plausible from an edit that never happened. Every twig history mutation
    /// therefore has exactly one counterpart here — see `push_history` (which
    /// owns the redo truncation a fresh edit forces) and `after_history`.
    caret_undo: Vec<CaretStep>,
    caret_redo: Vec<CaretStep>,
    /// The source as of the last open/save — `dirty` is `source != clean_source`,
    /// so undoing back to the saved state correctly clears the modified flag.
    clean_source: String,
    /// The "sticky" display column vertical motion aims for, in the active
    /// view's grid. Set on the first `move_up`/`move_down` of a run and
    /// reused by every subsequent one in that run, so passing through a
    /// shorter line doesn't permanently forget the original column. Any
    /// horizontal motion or edit clears it.
    ///
    /// A column, not a character index: dropping down a line of `你好` onto one
    /// of ASCII has to land under the glyph the caret was drawn beneath, which
    /// is the only thing the user can see to aim by. Where the goal falls inside
    /// a wide character on the target line, the mapping resolves it to that
    /// character — the caret lands on it rather than between its cells.
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
            caret_undo: Vec::new(),
            caret_redo: Vec::new(),
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

    /// Insert typed `text` at the caret, replacing the selection if there is one.
    /// A single typed character coalesces with the run of typing before it; a
    /// newline or a multi-character insert is its own undo step.
    ///
    /// Typed input only — clipboard text goes through [`paste`](Self::paste).
    pub fn insert(&mut self, text: &str) {
        let (s, e) = self.selection().unwrap_or((self.caret, self.caret));
        let kind = if text.chars().take(2).count() == 1 && text != "\n" {
            EditKind::Insert
        } else {
            EditKind::Other
        };
        self.splice(s, e, text, kind);
    }

    /// Insert clipboard `text` at the caret, replacing the selection if there is
    /// one — always its own undo step, whatever its length.
    ///
    /// Provenance is the whole point, and only the caller has it. `insert` reads
    /// a lone character as a keystroke and folds it into the run around it,
    /// which is right for typing and wrong for a one-character paste: that paste
    /// would vanish mid-run on an undo it was never part of, and the characters
    /// the user actually typed would go with it. Length can't tell the two
    /// apart — `⌘V` of `x` and typing `x` are the same string — so the door the
    /// caller comes through is what says which happened.
    pub fn paste(&mut self, text: &str) {
        let (s, e) = self.selection().unwrap_or((self.caret, self.caret));
        self.splice(s, e, text, EditKind::Other);
    }

    // ── indentation ──────────────────────────────────────────────────────────

    /// One indent level.
    ///
    /// Two spaces, not the four both frontends type for Tab today, because in a
    /// markdown document four columns isn't a width — it's a *meaning*. Four
    /// spaces at the head of a line is markdown's indented-code-block marker, so
    /// one Tab on a paragraph would reparse it into code and style it as such;
    /// two cannot, and the line stays the prose it was. Two is also exactly
    /// where a `- ` bullet's content starts, so an indented line lands under its
    /// parent item's text instead of beside it — the column a list-aware indent
    /// has to hit anyway, which keeps this width from being relitigated later.
    const INDENT: &'static str = "  ";

    /// Indent the selected lines — or the caret's line, with no selection — by
    /// one level (Tab).
    pub fn indent(&mut self) {
        self.reindent(true);
    }

    /// Take one indent level back off the selected lines, or the caret's line
    /// (Shift+Tab). A line with no indentation is left exactly as it is.
    ///
    /// A line with *less* than a full level gives back what it has rather than
    /// refusing: outdent's job is to walk a line left, and real documents — hand
    /// written, or reflowed by some other editor — are full of indentation that
    /// was never a clean multiple of anything. Refusing there would strand the
    /// line at a depth Shift+Tab couldn't undo.
    pub fn outdent(&mut self) {
        self.reindent(false);
    }

    /// The body of [`indent`](Self::indent) / [`outdent`](Self::outdent).
    ///
    /// One splice across the whole line range, never one per line: a Tab is one
    /// thing the user did, so it has to be one undo step and one reparse. Per
    /// line, twig would reparse the document once per line and leave a stack of
    /// steps that Shift+⌘Z walks back one line at a time.
    fn reindent(&mut self, add: bool) {
        let (sel_start, sel_end) = self.selection().unwrap_or((self.caret, self.caret));
        let start = source_line_range(&self.source, sel_start).start;
        let end = source_line_range(&self.source, sel_end).end;
        let region = self.source[start..end].to_string();
        let lines: Vec<&str> = region.split('\n').collect();
        // A blank line has no text to move, and padding it would leave nothing
        // but trailing whitespace — but Tab on a blank line *is* a request for
        // indentation to type into, so the skip only applies where the op has
        // other lines to do real work on.
        let skip_blank = add && lines.len() > 1;

        let mut out = String::with_capacity(region.len() + lines.len() * Self::INDENT.len());
        let mut deltas: Vec<isize> = Vec::with_capacity(lines.len());
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let delta = if add {
                if skip_blank && line.trim().is_empty() {
                    out.push_str(line);
                    0
                } else {
                    out.push_str(Self::INDENT);
                    out.push_str(line);
                    Self::INDENT.len() as isize
                }
            } else {
                let strip = outdent_width(line);
                out.push_str(&line[strip..]);
                -(strip as isize)
            };
            deltas.push(delta);
        }
        // Nothing to give back. Returning before the splice keeps an outdent at
        // column zero from spending an undo step on a document it never changed.
        if deltas.iter().all(|d| *d == 0) {
            return;
        }

        // Every line's text keeps its offset *within the line*, so the caret is
        // remapped by its column, not by its byte offset — which the prefixes on
        // the lines above it have already invalidated.
        let remap = |off: usize| -> usize {
            let (mut old_ls, mut new_ls) = (start, start);
            for (line, delta) in lines.iter().zip(&deltas) {
                let old_le = old_ls + line.len();
                let new_len = (line.len() as isize + delta) as usize;
                if off <= old_le {
                    let col = (off - old_ls) as isize;
                    return new_ls + ((col + delta).max(0) as usize).min(new_len);
                }
                old_ls = old_le + 1;
                new_ls += new_len + 1;
            }
            start + out.len()
        };
        let placed = match self.selection() {
            // Keep the rewritten region selected, the way a container toggle
            // keeps its own: it leaves a second Tab aimed at the same lines
            // rather than at whatever the shifted offsets now happen to cover.
            Some(_) => (start + out.len(), Some(start)),
            None => (remap(self.caret), None),
        };

        // A rolled-back splice leaves the old source in place, where every offset
        // computed above addresses text that was never written.
        if !self.splice(start, end, &out, EditKind::Other) {
            return;
        }
        // `splice` re-anchors to the end of the `Change`, which for a whole-region
        // rewrite is the last line's end — nowhere the caret was. Place it, then
        // tell the history where it really ended up, or a redo would replay the
        // caret splice left behind instead of this one.
        self.caret = placed.0.min(self.source.len());
        self.anchor = placed.1;
        self.clamp_caret();
        self.sync_history_after();
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
            let start = self.word_left_from(self.caret).max(self.caret_floor());
            if start < self.caret {
                let (s, e) = self.widen_over_emptied_inlines(start, self.caret);
                self.splice(s, e, "", EditKind::Delete);
            }
        }
    }

    /// Delete from the caret forward to the end of the next word (⌥⌦ /
    /// Ctrl+Del). Deletes the selection instead when one is active.
    pub fn delete_word_forward(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
        } else {
            let end = self.word_right_from(self.caret);
            if end > self.caret {
                let (s, e) = self.widen_over_emptied_inlines(self.caret, end);
                self.splice(s, e, "", EditKind::Delete);
            }
        }
    }

    /// Grow a WYSIWYG word-delete to swallow any inline node it empties.
    ///
    /// A glyph-space range covers what the user can see, which for `**bold**` is
    /// the word and never the delimiters around it — so deleting the word on its
    /// own leaves `a **** c`, markup wrapped around nothing. They asked for the
    /// word, and the styling was the word's; the two go together. Only the
    /// node's delimiters are taken, and those are hidden here anyway, so nothing
    /// visible outside the range is lost.
    ///
    /// Repeated to a fixed point: emptying `***bold***` empties the emph inside
    /// the strong, and only then is the strong empty too.
    fn widen_over_emptied_inlines(&mut self, start: usize, end: usize) -> (usize, usize) {
        if self.view == View::Source {
            return (start, end);
        }
        let nodes = self.nodes();
        let (mut s, mut e) = (start, end);
        loop {
            let mut grew = false;
            for n in nodes.iter().filter(|n| wysiwyg::is_inline(&n.kind)) {
                let Some(text) = inline_content_span(n, &self.source) else {
                    continue;
                };
                // Some of its text survives, so the node still has a job.
                if text.start < s || text.end > e {
                    continue;
                }
                if n.span.start < s || n.span.end > e {
                    s = s.min(n.span.start);
                    e = e.max(n.span.end);
                    grew = true;
                }
            }
            if !grew {
                return (s, e);
            }
        }
    }

    /// One splice via twig's `edit_range`, then re-anchor the caret from the
    /// returned `Change` and refresh the cached source. A reparse-breaking edit
    /// (rare for Markdown/Djot) leaves the document untouched and reports.
    ///
    /// Returns whether the edit landed — for a caller that has offsets of its
    /// own to place afterwards, which a rolled-back splice would leave pointing
    /// into text that never came to exist.
    fn splice(&mut self, start: usize, end: usize, text: &str, kind: EditKind) -> bool {
        // twig records an undo step for every edit; when this one continues a
        // run of the same kind (typing, deleting), tell twig to fold it into the
        // step before it so the whole run undoes at once.
        let coalesce = kind != EditKind::Other && self.last_edit_kind == Some(kind);
        let before = self.snapshot();
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
                self.push_history(before, coalesce);
                true
            }
            // The edit was rolled back, so twig's history did not move and
            // neither may ours: pushing here would leave a step with no edit
            // under it and shift every later undo onto the wrong caret.
            Err(e) => {
                self.status = Some(format!("edit: {e}"));
                false
            }
        }
    }

    fn snapshot(&self) -> CaretState {
        CaretState {
            caret: self.caret,
            anchor: self.anchor,
        }
    }

    /// Record the caret state around one successful twig edit. `before` is the
    /// snapshot taken before the op ran; the caret as it stands *now* is the
    /// step's `after`, so call this once the op has placed the caret where it
    /// finally means to leave it.
    ///
    /// `coalesce` must be the same flag handed to twig's `coalesce_last_undo`:
    /// folding two twig steps into one has to fold two of ours into one as well,
    /// which is a matter of *not* pushing and instead stretching the open step's
    /// `after` over the new edit. The run's original `before` stays put — a
    /// coalesced run undoes as one step, so it restores the caret from before
    /// the whole run, not before its last keystroke.
    fn push_history(&mut self, before: CaretState, coalesce: bool) {
        let after = self.snapshot();
        // Any fresh edit makes twig drop its redo stack; ours goes with it, or a
        // later redo would replay a caret from the branch that edit abandoned.
        self.caret_redo.clear();
        match self.caret_undo.last_mut().filter(|_| coalesce) {
            Some(open) => open.after = after,
            None => self.caret_undo.push(CaretStep { before, after }),
        }
    }

    /// Re-point the open history step's `after` at the caret as it now stands —
    /// for an op that splices and then places the caret itself, whose final
    /// caret isn't the one `splice` re-anchored from the `Change`.
    fn sync_history_after(&mut self) {
        let after = self.snapshot();
        if let Some(open) = self.caret_undo.last_mut() {
            open.after = after;
        }
    }

    /// Toggle an inline mark over the selection (Bold / Italic / Code / …). Keeps
    /// the toggled region selected so a second press cleanly reverses it.
    pub fn toggle(&mut self, kind: InlineKind) {
        let Some((s, e)) = self.selection() else {
            self.status = Some("select text first".into());
            return;
        };
        let before = self.snapshot();
        match self.editor.toggle_inline(s, e, kind) {
            Ok(change) => {
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                self.anchor = Some(change.new.start);
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.push_history(before, false);
            }
            Err(e) => self.status = Some(format!("{kind:?}: {e}")),
        }
    }

    /// Convert the block at the caret to a heading level or paragraph.
    pub fn set_block(&mut self, kind: BlockKind) {
        let before = self.snapshot();
        match self.block_offset_for_caret() {
            Some(offset) => match self.editor.set_block(offset, kind) {
                Ok(_) => {
                    self.last_edit_kind = None;
                    self.refresh();
                    self.clamp_caret();
                    self.anchor = None;
                    self.dirty = self.source != self.clean_source;
                    self.status = None;
                    self.push_history(before, false);
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

    /// Toggle a block quote around the selection, or around the block at the
    /// caret — the toolbar's Quote button.
    pub fn toggle_blockquote(&mut self) {
        self.toggle_container(BlockContainerKind::BlockQuote);
    }

    /// Toggle a numbered (`ordered`) or bulleted list over the selection, or
    /// over the block at the caret — one op with the kind as a flag, the way
    /// `toggle_heading` takes its level, so a frontend needs no twig type to
    /// name the two buttons.
    ///
    /// Pressing the *other* list's button while in a list converts in place
    /// rather than nesting, so the pair reads as one three-state control
    /// (bulleted / numbered / neither) rather than two independent wrappers.
    pub fn toggle_list(&mut self, ordered: bool) {
        self.toggle_container(if ordered {
            BlockContainerKind::OrderedList
        } else {
            BlockContainerKind::BulletList
        });
    }

    /// One `toggle_block_container` over the block-level target.
    ///
    /// leaf says *where*; twig decides everything else — which blocks the range
    /// covers, whether that means wrapping, unwrapping, nesting or converting,
    /// and how this document's format spells the prefix. The rule that a
    /// container only comes off when the range covers every block it holds is
    /// what the re-anchoring below is built around.
    fn toggle_container(&mut self, kind: BlockContainerKind) {
        let selected = self.selection();
        // Without a selection the target is the caret's own block, resolved the
        // way `set_block` resolves it — a caret at a line end sits at the doc
        // level and has to be nudged back onto the block it looks like it's in.
        // An empty range is enough: twig widens to the whole lines it touches.
        // A blank line resolves to nothing twig can wrap, and its `NotFound`
        // says so.
        let (start, end) = match selected {
            Some(range) => range,
            None => {
                let off = self.block_offset_for_caret().unwrap_or(self.caret);
                (off, off)
            }
        };
        let before = self.snapshot();
        match self.editor.toggle_block_container(start, end, kind) {
            Ok(change) => {
                // Read the caret's place out of the *pre-edit* source, before
                // `refresh` swaps that source out from under it.
                let place = selected.is_none().then(|| self.caret_line_tail(&change.old));
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                match place {
                    // Select what the container now holds, the way `toggle`
                    // keeps its marked region selected — and for a stronger
                    // reason than symmetry: a container comes *off* only a range
                    // covering every block it holds, so a selection left on its
                    // old bytes (now short by a prefix per line) would nest on
                    // the second press instead of reversing the first.
                    None => {
                        self.anchor = Some(change.new.start);
                        self.caret = change.new.end;
                    }
                    Some(place) => {
                        self.anchor = None;
                        self.caret = self.line_tail_offset(&change.new, place);
                    }
                }
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.push_history(before, false);
            }
            Err(e) => self.status = Some(format!("{kind:?}: {e}")),
        }
    }

    /// The caret's place inside the region a container toggle is rewriting, in
    /// the only terms the rewrite preserves: which of the region's lines it sits
    /// on, and how many bytes of that line lie ahead of it.
    ///
    /// A container's markup goes in at column 0 and never touches what follows
    /// on the line, so that pair survives the edit exactly where a byte offset
    /// does not — a caret left on its old offset slides back by one prefix per
    /// line above it, which on a hard-wrapped paragraph parks it *inside* the
    /// `> ` it just asked for.
    fn caret_line_tail(&self, old: &std::ops::Range<usize>) -> (usize, usize) {
        let caret = self.caret.clamp(old.start, old.end);
        let line = self.source[old.start..caret].matches('\n').count();
        let end = self.source[caret..old.end]
            .find('\n')
            .map_or(old.end, |i| caret + i);
        (line, end - caret)
    }

    /// [`caret_line_tail`](Self::caret_line_tail) undone against the rewritten
    /// region: the offset `tail` bytes back from the end of the region's `line`.
    ///
    /// Both walks are clamped rather than trusted, because the one op that does
    /// *not* keep a region's lines one-to-one is stripping a list — twig blows
    /// the items back apart with blank lines between them — and a caret landing
    /// on the nearest line of the right item beats one landing out of the region
    /// entirely.
    fn line_tail_offset(&self, new: &std::ops::Range<usize>, (line, tail): (usize, usize)) -> usize {
        let region = &self.source[new.start.min(self.source.len())..new.end.min(self.source.len())];
        let mut start = 0;
        for _ in 0..line {
            match region[start..].find('\n') {
                Some(i) => start += i + 1,
                None => break,
            }
        }
        let end = region[start..].find('\n').map_or(region.len(), |i| start + i);
        new.start + end.saturating_sub(tail).max(start)
    }

    /// Link the selection to `destination` — the toolbar's Link button. With no
    /// selection it acts at the caret, which re-points a link the caret is
    /// already standing in (twig replaces an existing link's destination and
    /// keeps its text) and otherwise spells a link that has no text of its own:
    /// an autolink (`<https://x.dev>`) where the destination is one, and
    /// `[destination](destination)` where it isn't.
    ///
    /// `destination` reaches twig raw. Escaping it is format knowledge and the
    /// two formats genuinely disagree — Markdown ends a destination at the first
    /// space and moves it into `<…>`, djot reads that `<…>` as part of the URL
    /// itself — so the side holding the document is the side that gets to spell
    /// it. A destination twig can't carry at all (one with a newline) comes back
    /// as an error rather than a quietly rewritten URL.
    pub fn insert_link(&mut self, destination: &str) {
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let before = self.snapshot();
        match self.editor.insert_link(start, end, destination) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                match self.link_text_span(change.new.start) {
                    // A link with text of its own: select it, so typing replaces
                    // a `[dest](dest)`'s stand-in label and a second press
                    // re-points what the first one linked.
                    Some(text) => {
                        self.anchor = (text.start != text.end).then_some(text.start);
                        self.caret = text.end;
                    }
                    // An autolink is finished the moment it's written — its text
                    // *is* the URL. Leaving it selected would aim the next press
                    // at the one shape twig still wraps instead of re-points.
                    None => {
                        self.anchor = None;
                        self.caret = change.new.end;
                    }
                }
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.push_history(before, false);
            }
            Err(e) => self.status = Some(format!("link: {e}")),
        }
    }

    /// The destination of the link under the caret — what a Link prompt shows so
    /// ⌘K on an existing link edits its URL instead of asking for it again.
    /// `None` when the caret stands in no link.
    ///
    /// An autolink carries no separate destination: its text *is* the URL, so
    /// that's what comes back for one.
    pub fn link_destination_at_caret(&mut self) -> Option<String> {
        let off = self.caret;
        self.nodes()
            .into_iter()
            .filter(|n| matches!(n.kind.as_str(), "link" | "url" | "email"))
            .filter(|n| n.span.start <= off && off < n.span.end)
            .max_by_key(|n| n.span.start)
            .and_then(|n| n.destination.or(n.text))
    }

    /// The source range of the text inside the link covering `off` — what sits
    /// between its `[` and `]`. `None` when twig reports no link there.
    fn link_text_span(&mut self, off: usize) -> Option<std::ops::Range<usize>> {
        self.nodes()
            .into_iter()
            // Two links can touch (`[a](x)[b](y)`), and then one's `span.end` is
            // the other's `span.start`; the link that starts latest at or before
            // `off` is the one `off` is actually in.
            .filter(|n| n.kind == "link" && n.span.start <= off && off < n.span.end)
            .max_by_key(|n| n.span.start)
            .and_then(|n| n.content_span)
    }

    // ── undo / redo ───────────────────────────────────────────────────────────
    // twig owns the history of *bytes* (it owns the buffer); leaf drives it and
    // keeps the matching history of *carets*, which twig's `Change` can't carry
    // because the caret was never twig's to know. The two stacks move as one —
    // see `push_history` for the pushing, coalescing, and redo truncation, and
    // `splice` for how a run of keystrokes becomes a single step.

    /// Undo the last edit step (⌘Z / ^Z), putting the caret and selection back
    /// where they were when that step began.
    pub fn undo(&mut self) {
        match self.editor.undo() {
            Ok(Some(change)) => {
                let step = self.caret_undo.pop();
                self.after_history(change, step.map(|s| s.before));
                if let Some(step) = step {
                    self.caret_redo.push(step);
                }
            }
            Ok(None) => self.status = Some("nothing to undo".into()),
            Err(e) => self.status = Some(format!("undo: {e}")),
        }
    }

    /// Redo the last undone edit step (⇧⌘Z / ^Y), putting the caret and
    /// selection back where that step originally left them.
    pub fn redo(&mut self) {
        match self.editor.redo() {
            Ok(Some(change)) => {
                let step = self.caret_redo.pop();
                self.after_history(change, step.map(|s| s.after));
                if let Some(step) = step {
                    self.caret_undo.push(step);
                }
            }
            Ok(None) => self.status = Some("nothing to redo".into()),
            Err(e) => self.status = Some(format!("redo: {e}")),
        }
    }

    /// Refresh the cached source and put the caret back where the step being
    /// undone/redone had it, clearing any active run.
    ///
    /// `restore` is that remembered state; `change` is only the fallback for a
    /// step with no record of its own — a caret at the end of the restored text,
    /// which is where this always landed before the states were kept. It is the
    /// edit site, not where the user was standing, so it's a floor and not the
    /// behaviour: undoing should hand back the document *and* the place you were
    /// working, which for an edit made anywhere but under the caret are two
    /// different places.
    fn after_history(&mut self, change: Change, restore: Option<CaretState>) {
        self.refresh();
        match restore {
            Some(state) => {
                self.caret = state.caret.min(self.source.len());
                self.anchor = state.anchor.map(|a| a.min(self.source.len()));
            }
            None => {
                self.caret = change.new.end.min(self.source.len());
                self.anchor = None;
            }
        }
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
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
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
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// Move to the start of the previous word (⌥← / Ctrl+←).
    pub fn move_word_left(&mut self, extend: bool) {
        self.goal_col = None;
        let before = self.caret;
        let target = self.word_left_from(self.caret);
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// Move to the end of the next word (⌥→ / Ctrl+→).
    pub fn move_word_right(&mut self, extend: bool) {
        self.goal_col = None;
        let before = self.caret;
        let target = self.word_right_from(self.caret);
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    // Word boundaries are found in the space the *view* is in. The source view
    // walks the source, because there the source is what's rendered. WYSIWYG
    // walks the rendered text instead: `**` is invisible to the user, so it has
    // to be invisible to word motion too — a caret parked inside one draws in
    // the column after `bold` and types two bytes earlier, and a word-delete
    // that stops there shreds the markup into `a ** c`.

    /// The word boundary to the left of `off` in the active view's space.
    fn word_left_from(&self, off: usize) -> usize {
        match self.view {
            View::Source => prev_word(&self.source, off),
            View::Wysiwyg => self.glyph_word_left(off),
        }
    }

    /// The word boundary to the right of `off` in the active view's space.
    fn word_right_from(&self, off: usize) -> usize {
        match self.view {
            View::Source => next_word(&self.source, off),
            View::Wysiwyg => self.glyph_word_right(off),
        }
    }

    /// The character class of the glyph drawn at stop `off`.
    ///
    /// Read from the source, because a stop points at the source byte its glyph
    /// came from — the source *is* where the rendered character is written. What
    /// makes the walk glyph space rather than source space is that it only ever
    /// visits stops, and the hidden bytes between them have none.
    fn class_at(&self, off: usize) -> Class {
        self.source
            .get(off..)
            .and_then(|s| s.chars().next())
            .map_or(Class::Space, classify)
    }

    /// [`next_word`] in glyph space: skip any leading separators, then consume
    /// the following word run, with the stop table standing in for the source's
    /// characters.
    fn glyph_word_right(&self, from: usize) -> usize {
        let Some(mut off) = self.vmap.stop_at_or_after(from) else {
            return from;
        };
        let mut in_word = false;
        loop {
            match self.class_at(off) {
                Class::Word => in_word = true,
                _ if in_word => return off,
                _ => {}
            }
            match self.vmap.stop_after(off) {
                Some(next) => off = next,
                None => return off,
            }
        }
    }

    /// [`prev_word`] in glyph space: skip separators walking left, then consume
    /// the preceding word run.
    fn glyph_word_left(&self, from: usize) -> usize {
        let Some(mut off) = self.vmap.stop_at_or_before(from) else {
            return from;
        };
        let mut in_word = false;
        while let Some(prev) = self.vmap.stop_before(off) {
            match self.class_at(prev) {
                Class::Word => in_word = true,
                _ if in_word => return off,
                _ => {}
            }
            off = prev;
        }
        off
    }

    /// After a motion that walks the visual map, the caret must be *on* the map.
    /// A stop is the only offset where the caret draws and edits in the same
    /// place, and it's the invariant both a caret parked inside an emoji and one
    /// parked inside a `**` were quietly breaking.
    ///
    /// Only when the caret actually moved: a walk with nowhere to go leaves it
    /// where it was, which is wherever the floor or a frontend put it rather
    /// than somewhere this motion chose.
    fn debug_assert_on_a_stop(&self, before: usize) {
        debug_assert!(
            self.view != View::Wysiwyg
                || self.vmap.num_rows() == 0
                || self.caret == before
                || self.vmap.is_stop(self.caret),
            "motion left the caret at {}, which is not a caret stop: it would draw in \
             one place and type in another",
            self.caret
        );
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
                self.vmap.offset_of_pos(r, goal.min(self.vmap.row_width(r)))
            }
        };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
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
                self.vmap.offset_of_pos(r, goal.min(self.vmap.row_width(r)))
            }
        };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    pub fn move_home(&mut self, extend: bool) {
        self.goal_col = None;
        let (row, _) = self.caret_pos();
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row, 0),
            View::Wysiwyg => self.vmap.offset_of_pos(row, 0),
        };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    pub fn move_end(&mut self, extend: bool) {
        self.goal_col = None;
        let (row, _) = self.caret_pos();
        let target = match self.view {
            View::Source => line_end(&self.source, row),
            View::Wysiwyg => self.vmap.offset_of_pos(row, self.vmap.row_width(row)),
        };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
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

    /// Point the caret at the body cell `(row, col)` the mouse landed on —
    /// `col` being a cell of the terminal grid, which is what a display column
    /// is. A click on the far cell of a wide character lands at that
    /// character's start; the mapping's own doc-comments carry the rule.
    pub fn click(&mut self, row: usize, col: usize, extend: bool) {
        self.goal_col = None;
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row, col),
            View::Wysiwyg => self.vmap.offset_of_pos(row, col),
        };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
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

    /// The caret's screen position `(row, col)` in the active view's grid, with
    /// `col` a display column: the cell to draw the caret in, which on a line of
    /// `你好` or emoji is not the count of characters before it.
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

/// The source range of an inline node's own visible text — the part of it a
/// WYSIWYG caret can reach, as against the delimiters that only spell it.
/// `None` for a node with no interior to empty (a `str`, a break).
///
/// twig reports no `content_span` for `verbatim`/`inline_math`, whose text sits
/// one delimiter in from the span — the same place the renderer maps it to. A
/// longer fence (`` ``a`` ``) breaks that assumption, so the guess is checked
/// against the source rather than trusted: a range guessed wrong here is text
/// deleted wrong.
fn inline_content_span(n: &FlatNode, source: &str) -> Option<std::ops::Range<usize>> {
    if let Some(span) = n.content_span.clone() {
        return Some(span);
    }
    match n.kind.as_str() {
        "verbatim" | "inline_math" => {
            let text = n.text.as_ref()?;
            let start = n.span.start + 1;
            let range = start..start + text.len();
            (source.get(range.clone()) == Some(text.as_str())).then_some(range)
        }
        _ => None,
    }
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

/// How many leading bytes an outdent takes off `line`: a whole indent level
/// where the line has one, and whatever it has where it has less.
///
/// A leading tab counts as a level on its own. It's indentation some other
/// editor wrote, and one tab is one level everywhere it came from — measuring it
/// in spaces it doesn't contain would leave it untouchable.
fn outdent_width(line: &str) -> usize {
    if line.starts_with('\t') {
        return 1;
    }
    line.bytes()
        .take(Doc::INDENT.len())
        .take_while(|b| *b == b' ')
        .count()
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

/// `(row, col)` of byte offset `off`, `col` counted in *display columns* from
/// the line's start — terminal cells, not characters, so the column names the
/// cell the caret is drawn in even on a line of `你好` or emoji.
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
    (row, wysiwyg::text_width(&s[line_start..off]))
}

/// The byte offset at display column `col` of `row` (clamped to that line's
/// end) — the inverse of [`offset_to_row_col`], which it has to agree with.
///
/// A column landing *inside* a character — the second cell of `你`, or any cell
/// but the first of an emoji — resolves to that character's start, which is the
/// column the caret would have been drawn at to begin with. So both cells of a
/// wide character mean the character, and every offset survives the round trip
/// out to a column and back. The walk steps by grapheme cluster for the same
/// reason the caret does: a cluster is the character, and the cells belong to it
/// rather than to the codepoints spelling it.
fn row_col_to_offset(s: &str, row: usize, col: usize) -> usize {
    let start = line_start(s, row);
    let end = line_end_from(s, start);
    let mut off = start;
    let mut at = 0; // the display column `off` sits at
    while off < end {
        let next = next_boundary(s, off).min(end);
        let cells = wysiwyg::text_width(&s[off..next]);
        if at + cells > col {
            break; // `col` is one of this cluster's own cells
        }
        at += cells;
        off = next;
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

    /// A document open in `view`. WYSIWYG motion reads the visual map, which the
    /// renderer stamps each frame, so the map is built here too — a WYSIWYG doc
    /// without one is a view no user is ever in.
    fn doc_in(view: View, name: &str, body: &str) -> Doc {
        // The fixture name doubles as the temp file's, so two tests picking the
        // same one raced under the parallel runner and read each other's body —
        // a green suite proving the wrong thing. The counter makes that
        // unreachable rather than asking every future caller to notice.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_test_{name}_{seq}.md"));
        std::fs::write(&p, body).unwrap();
        let mut d = Doc::open(p).unwrap();
        d.view = view;
        if view == View::Wysiwyg {
            d.build_visual(80);
        }
        d
    }

    // Source-view document for the source-behaviour tests. `Doc::open` now
    // defaults to WYSIWYG (leaf's default view), so pin the source view here;
    // `wysiwyg_doc` builds the rich-text variant on top of this.
    fn doc_with(name: &str, body: &str) -> Doc {
        doc_in(View::Source, name, body)
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
        golden_in(View::Source, name, marked, action)
    }

    /// [`golden`] in a chosen view — the editing ops are the view's to share, so
    /// the same fixture has to read the same way in both.
    fn golden_in(view: View, name: &str, marked: &str, action: impl FnOnce(&mut Doc)) -> String {
        let (src, caret) = parse_caret(marked);
        let mut d = doc_in(view, name, &src);
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

    // ── block containers (quote / list) ──────────────────────────────────────

    #[test]
    fn toggle_blockquote_wraps_the_block_at_the_caret_and_reverses() {
        let g = |m, f: fn(&mut Doc)| golden("quote", m, f);
        assert_eq!(g("hel|lo\n", |d| d.toggle_blockquote()), "> hel|lo\n");
        assert_eq!(g("> hel|lo\n", |d| d.toggle_blockquote()), "hel|lo\n");
        // A caret at a line end sits at the doc level; the block is still found.
        assert_eq!(g("hello|\n", |d| d.toggle_blockquote()), "> hello|\n");
    }

    #[test]
    fn toggle_blockquote_keeps_the_caret_in_a_hard_wrapped_paragraph() {
        // Every source line of the paragraph gets its own `> `, so a caret left
        // on its old byte offset falls one prefix per line above it too far
        // back — inside the markup it just asked for rather than in its word.
        assert_eq!(
            golden("quote_wrap", "aaa\nb|bb\nccc\n", |d| d.toggle_blockquote()),
            "> aaa\n> b|bb\n> ccc\n"
        );
    }

    #[test]
    fn toggle_blockquote_works_in_wysiwyg_view() {
        let g = |n, m, f: fn(&mut Doc)| golden_in(View::Wysiwyg, n, m, f);
        assert_eq!(g("q_wys", "hel|lo\n", |d| d.toggle_blockquote()), "> hel|lo\n");
        assert_eq!(g("q_wys2", "> hel|lo\n", |d| d.toggle_blockquote()), "hel|lo\n");
    }

    #[test]
    fn toggle_list_makes_a_list_and_converts_between_the_kinds() {
        let g = |m, f: fn(&mut Doc)| golden("list", m, f);
        assert_eq!(g("hel|lo\n", |d| d.toggle_list(false)), "- hel|lo\n");
        assert_eq!(g("hel|lo\n", |d| d.toggle_list(true)), "1. hel|lo\n");
        // The *other* kind converts in place instead of nesting, which is what
        // makes the two buttons one three-state control.
        assert_eq!(g("- hel|lo\n", |d| d.toggle_list(true)), "1. hel|lo\n");
        assert_eq!(g("1. hel|lo\n", |d| d.toggle_list(false)), "- hel|lo\n");
        // Its own kind, over the only item the list holds, takes it off.
        assert_eq!(g("- hel|lo\n", |d| d.toggle_list(false)), "hel|lo\n");
    }

    #[test]
    fn toggle_list_works_in_wysiwyg_view() {
        let g = |n, m, f: fn(&mut Doc)| golden_in(View::Wysiwyg, n, m, f);
        assert_eq!(g("l_wys", "hel|lo\n", |d| d.toggle_list(true)), "1. hel|lo\n");
        assert_eq!(g("l_wys2", "1. hel|lo\n", |d| d.toggle_list(false)), "- hel|lo\n");
        assert_eq!(g("l_wys3", "- hel|lo\n", |d| d.toggle_list(false)), "hel|lo\n");
    }

    #[test]
    fn a_list_over_a_selection_numbers_each_block_and_stays_selected() {
        // The selection has to grow with the markup: twig takes a container off
        // only a range covering every block it holds, so the second press can
        // reverse the first only if the result is what's selected.
        let mut d = doc_with("list_sel", "abc\n\ndef\n");
        d.select_all();
        d.toggle_list(true);
        assert_eq!(d.source, "1. abc\n\n2. def\n");
        assert_eq!(d.selection(), Some((0, d.source.len())));
        d.toggle_list(true);
        assert_eq!(d.source, "abc\n\ndef\n");
    }

    #[test]
    fn toggle_blockquote_nests_a_partly_covered_quote() {
        // twig's rule: covering only some of a container's blocks nests, because
        // taking the quote off would drag its uncovered siblings out with it.
        let mut d = doc_with("quote_nest", "> a\n>\n> b\n");
        d.caret = 2; // in the first quoted paragraph only
        d.toggle_blockquote();
        assert_eq!(d.source, "> > a\n>\n> b\n");
    }

    #[test]
    fn a_container_toggle_on_a_blank_line_reports_and_changes_nothing() {
        let mut d = doc_with("quote_blank", "\nabc\n");
        d.caret = 0; // a blank line is no block for twig to wrap
        d.toggle_blockquote();
        assert_eq!(d.source, "\nabc\n");
        assert!(d.status.is_some(), "twig's error should reach the status line");
        assert!(!d.dirty);
    }

    #[test]
    fn a_container_toggle_is_one_undo_step() {
        let mut d = doc_with("quote_undo", "hello\n");
        d.caret = 3;
        d.insert("X"); // a typing run the structural edit must not fold into
        d.toggle_blockquote();
        assert_eq!(d.source, "> helXlo\n");
        d.undo();
        assert_eq!(d.source, "helXlo\n");
    }

    // ── links ────────────────────────────────────────────────────────────────

    #[test]
    fn insert_link_wraps_the_selection_and_leaves_its_text_selected() {
        let mut d = doc_with("link_sel", "word here\n");
        d.anchor = Some(0);
        d.caret = 4;
        d.insert_link("http://x.dev");
        assert_eq!(d.source, "[word](http://x.dev) here\n");
        // The text, not the destination — so a second press re-points the link
        // the first one made rather than nesting one inside it.
        assert_eq!(d.selected_text(), Some("word"));
        d.insert_link("http://y.dev");
        assert_eq!(d.source, "[word](http://y.dev) here\n");
        assert_eq!(d.selected_text(), Some("word"));
    }

    #[test]
    fn insert_link_repoints_the_link_at_a_bare_caret() {
        let mut d = doc_with("link_repoint", "[word](http://x.dev)\n");
        d.caret = 3; // in the link's text, nothing selected
        d.insert_link("http://y.dev");
        assert_eq!(d.source, "[word](http://y.dev)\n");
        assert_eq!(d.selected_text(), Some("word"));
    }

    #[test]
    fn insert_link_on_an_empty_range_autolinks_a_url() {
        // A link with no text of its own is an autolink, and twig spells it —
        // `<…>` is the canonical form and needs no text typed into it, so the
        // caret lands after it rather than selecting a finished link.
        let mut d = doc_with("link_empty", "\n");
        d.caret = 0;
        d.insert_link("http://x.dev");
        assert_eq!(d.source, "<http://x.dev>\n");
        assert_eq!(d.selection(), None);
        assert_eq!(d.caret, 14);
    }

    #[test]
    fn insert_link_on_an_empty_range_falls_back_for_a_non_url() {
        // `<./notes.md>` is literal text in both formats and `<foo>` is raw HTML
        // in Markdown, so a destination that can't autolink doubles as the text
        // instead — which is then selected, ready to be typed over.
        let mut d = doc_with("link_rel", "\n");
        d.caret = 0;
        d.insert_link("./notes.md");
        assert_eq!(d.source, "[./notes.md](./notes.md)\n");
        assert_eq!(d.selection(), Some((1, 11)));
        d.insert("Notes");
        assert_eq!(d.source, "[Notes](./notes.md)\n");
    }

    #[test]
    fn insert_link_repoints_the_autolink_the_caret_stands_in() {
        // The autolink's text is its URL, so re-pointing replaces the whole
        // node — the caret must not splice a second link inside the first.
        let mut d = doc_with("link_repoint_auto", "see <https://x.dev> ok\n");
        d.caret = 10;
        d.insert_link("https://y.dev");
        assert_eq!(d.source, "see <https://y.dev> ok\n");
    }

    #[test]
    fn link_destination_at_caret_reads_both_spellings() {
        let mut d = doc_with("link_dest", "see [t](https://x.dev) ok\n");
        d.caret = 5;
        assert_eq!(d.link_destination_at_caret().as_deref(), Some("https://x.dev"));
        d.caret = 0;
        assert_eq!(d.link_destination_at_caret(), None);

        // An autolink has no `destination`; its text is the URL.
        let mut a = doc_with("link_dest_auto", "see <https://x.dev> ok\n");
        a.caret = 10;
        assert_eq!(a.link_destination_at_caret().as_deref(), Some("https://x.dev"));
        a.caret = 21;
        assert_eq!(a.link_destination_at_caret(), None);
    }

    #[test]
    fn insert_link_hands_the_destination_to_twig_raw() {
        // Escaping is twig's, and format-specific: Markdown ends a destination
        // at the first space and needs the `<…>` form, where djot would read
        // those angle brackets as part of the URL.
        let mut d = doc_with("link_space", "word\n");
        d.anchor = Some(0);
        d.caret = 4;
        d.insert_link("a b");
        assert_eq!(d.source, "[word](<a b>)\n");
    }

    #[test]
    fn insert_link_reports_a_destination_no_format_can_carry() {
        let mut d = doc_with("link_bad", "word\n");
        d.anchor = Some(0);
        d.caret = 4;
        d.insert_link("a\nb");
        assert_eq!(d.source, "word\n"); // untouched, not quietly rewritten
        assert!(d.status.is_some(), "InvalidArgument should reach the status line");
        assert!(!d.dirty);
    }

    #[test]
    fn insert_link_works_in_wysiwyg_view() {
        let mut d = wysiwyg_doc("link_wys", "word here\n");
        d.anchor = Some(0);
        d.caret = 4;
        d.insert_link("http://x.dev");
        assert_eq!(d.source, "[word](http://x.dev) here\n");
        assert_eq!(d.selected_text(), Some("word"));
        // The map the caret has to keep riding is rebuilt each frame; motion
        // over the fresh one must still land on a real stop (the debug_assert).
        d.build_visual(80);
        d.move_right(false);
        d.move_left(false);
    }

    #[test]
    fn click_maps_a_row_col_to_a_byte_offset() {
        let mut d = doc_with("click", "ab\ncd\n");
        d.click(1, 1, false); // row 1 ("cd"), col 1 -> the 'd'
        assert_eq!(d.caret, 4);
    }

    fn wysiwyg_doc(name: &str, body: &str) -> Doc {
        doc_in(View::Wysiwyg, name, body)
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
        assert_eq!(d.vmap.row_width(row), 0, "caret's row must be empty, not 'World'");
        assert!(row >= 2, "a blank spacer row should sit above the caret, got row {row}");
        // The row above the caret is a real (empty) gap, and "Hello" stays put.
        assert_eq!(d.vmap.row_width(row - 1), 0, "the row above the caret is a gap");
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
        assert_eq!(d.vmap.row_width(row - 1), 0, "the row above the caret is a gap");
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
    fn a_one_character_paste_is_its_own_undo_step() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "paste_step", "ab\n");
            d.caret = 0;
            d.insert("x");
            d.insert("y"); // a run of typing
            d.paste("z"); // one character, but pasted — not part of that run
            assert_eq!(d.source, "xyzab\n");
            d.undo();
            assert_eq!(d.source, "xyab\n", "the paste undoes on its own");
            assert_eq!(d.caret, 2, "and hands back the caret it found");
            d.undo();
            assert_eq!(d.source, "ab\n", "the typed run is still one step under it");
        }
    }

    #[test]
    fn the_same_character_typed_still_joins_the_run() {
        // The other half of the pair: `z` is a keystroke here and a paste above,
        // and the two undo differently. Nothing about the *string* says which —
        // which is why provenance has to come from the door the caller uses.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "typed_run", "ab\n");
            d.caret = 0;
            d.insert("x");
            d.insert("y");
            d.insert("z");
            d.undo();
            assert_eq!(d.source, "ab\n", "one run, one step");
        }
    }

    #[test]
    fn undo_restores_the_caret_to_where_it_was_not_to_the_edit_site() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "undo_caret", "hello world\n");
            d.caret = 11; // standing at the end of "world", away from the edit
            d.edit(0, 5, "goodbye");
            assert_eq!(d.source, "goodbye world\n");
            d.undo();
            assert_eq!(d.source, "hello world\n");
            // The undone edit ends at offset 5; the user was at 11.
            assert_eq!(d.caret, 11, "the caret comes back with the bytes");
        }
    }

    #[test]
    fn undo_restores_the_selection_the_edit_replaced() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "undo_sel", "a word b\n");
            d.anchor = Some(2);
            d.caret = 6; // "word" selected
            d.insert("X");
            assert_eq!(d.source, "a X b\n");
            d.undo();
            assert_eq!(d.source, "a word b\n");
            assert_eq!(d.selection(), Some((2, 6)), "the selection comes back too");
        }
    }

    #[test]
    fn redo_restores_the_caret_the_edit_left_behind() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "redo_caret", "hello world\n");
            d.caret = 11;
            d.edit(0, 5, "goodbye");
            assert_eq!(d.caret, 7, "the edit left the caret after its new text");
            d.undo();
            d.redo();
            assert_eq!(d.source, "goodbye world\n");
            assert_eq!(d.caret, 7, "redo puts it back where the edit had it");
        }
    }

    #[test]
    fn undoing_a_typed_run_restores_the_caret_from_before_the_whole_run() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "run_caret", "hi\n");
            d.caret = 2;
            d.insert("a");
            d.insert("b");
            d.insert("c");
            assert_eq!(d.source, "hiabc\n");
            d.undo();
            assert_eq!(d.source, "hi\n");
            assert_eq!(d.caret, 2, "before the run, not before its last keystroke");
            d.redo();
            assert_eq!(d.caret, 5, "and redo restores the end of the whole run");
        }
    }

    #[test]
    fn undo_restores_the_caret_across_a_format_toggle() {
        // A toggle reaches twig without going through `splice`, so it has to
        // record its own step — miss it and every stack depth below it is off by
        // one, and undo starts handing back another edit's caret.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "fmt_caret", "a word b\n");
            d.caret = 8;
            d.anchor = Some(2);
            d.caret = 6;
            d.toggle(InlineKind::Strong);
            assert_eq!(d.source, "a **word** b\n");
            d.undo();
            assert_eq!(d.source, "a word b\n");
            assert_eq!(d.selection(), Some((2, 6)), "the toggled selection comes back");
        }
    }

    #[test]
    fn an_edit_after_an_undo_truncates_the_caret_history_with_twigs() {
        // The drift that would never announce itself: twig drops its redo stack
        // on any fresh edit, so a leaf redo entry that outlives it would restore
        // a caret from the timeline that edit abandoned.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "redo_trunc", "hello world\n");
            d.caret = 11;
            d.edit(0, 5, "goodbye"); // step A, caret 11 → 7
            d.undo();
            assert_eq!(d.caret, 11);
            d.caret = 0;
            d.insert("X"); // diverges: A's redo is gone from twig
            assert_eq!(d.source, "Xhello world\n");
            assert_eq!(d.caret_undo.len(), 1, "A's step went with A's redo");
            assert!(d.caret_redo.is_empty(), "the abandoned branch is gone");

            d.redo();
            assert_eq!(d.source, "Xhello world\n", "nothing to redo onto");
            assert_eq!(d.status.as_deref(), Some("nothing to redo"));
            d.undo();
            assert_eq!(d.source, "hello world\n");
            assert_eq!(d.caret, 0, "the surviving step's caret, not the dropped one");
        }
    }

    #[test]
    fn indent_and_outdent_move_the_caret_line_with_its_text() {
        for view in [View::Source, View::Wysiwyg] {
            let g = |m, f: fn(&mut Doc)| golden_in(view, "indent_line", m, f);
            assert_eq!(g("he|llo\n", |d| d.indent()), "  he|llo\n");
            assert_eq!(g("  he|llo\n", |d| d.outdent()), "he|llo\n");
            // Indentation the caret is standing *in* collapses to the line start
            // rather than dragging the caret into the text.
            assert_eq!(g("| hello\n", |d| d.outdent()), "|hello\n");
            // A line with none to give back is left exactly as it was.
            assert_eq!(g("he|llo\n", |d| d.outdent()), "he|llo\n");
            // Less than a full level gives back what it has.
            assert_eq!(g(" he|llo\n", |d| d.outdent()), "he|llo\n");
            // A tab is one level however many spaces it isn't.
            assert_eq!(g("\the|llo\n", |d| d.outdent()), "he|llo\n");
        }
    }

    #[test]
    fn one_indent_level_leaves_a_paragraph_a_paragraph() {
        // Why the level is two spaces and not the four both frontends type
        // today. Four is markdown's indented-code-block marker, so a Tab on a
        // paragraph would silently restyle it as code — a width that changes
        // what the document *means* isn't an indent. Pinned because the number
        // is the kind of thing a later list-aware pass would reach for.
        let mut d = doc_with("indent_kind", "hello\n");
        d.caret = 2;
        d.indent();
        assert_eq!(d.source, "  hello\n");
        assert!(
            d.nodes().iter().any(|n| n.kind == "para"),
            "still prose after a Tab"
        );
        assert!(!d.nodes().iter().any(|n| n.kind == "code_block"));

        // The four-space level this replaces, for contrast: same text, and twig
        // reparses the paragraph into a code block.
        let mut wide = doc_with("indent_kind_4", "    hello\n");
        wide.build_visual(80);
        assert!(
            wide.nodes().iter().any(|n| n.kind == "code_block"),
            "four spaces is a code block, not an indented paragraph"
        );
    }

    #[test]
    fn indent_nests_a_list_item_under_its_parent() {
        // Not list-*aware* (that needs container-aware indentation in twig), but
        // the plain line shift already lands a bullet at the column that nests
        // it — which is the same width the aware version will need.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_nest", "- a\n- b\n");
            d.caret = 6; // on the second item
            d.indent();
            assert_eq!(d.source, "- a\n  - b\n");
            let lists = d.nodes().iter().filter(|n| n.kind == "bullet_list").count();
            assert_eq!(lists, 2, "the indented item is a nested list");
        }
    }

    #[test]
    fn outdent_with_nothing_to_give_back_records_no_undo_step() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "outdent_noop", "hello\n");
            d.caret = 2;
            d.outdent();
            assert_eq!(d.source, "hello\n");
            assert!(!d.dirty, "a no-op is not a modification");
            assert!(d.caret_undo.is_empty(), "and spends no undo step");
        }
    }

    #[test]
    fn indent_shifts_every_selected_line_and_keeps_them_selected() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_sel", "one\n\ntwo\n");
            d.anchor = Some(0);
            d.caret = 7; // through "two"
            d.indent();
            assert_eq!(
                d.source, "  one\n\n  two\n",
                "the blank line keeps no trailing pad"
            );
            // Selected, so a second Tab lands on the same lines rather than on
            // whatever the shifted offsets now cover.
            assert_eq!(d.selection(), Some((0, 12)));
            d.indent();
            assert_eq!(d.source, "    one\n\n    two\n");
        }
    }

    #[test]
    fn outdent_takes_what_each_line_has_and_leaves_the_rest_alone() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "outdent_sel", "  two\n one\nnone\n");
            d.anchor = Some(0);
            d.caret = 15;
            d.outdent();
            assert_eq!(d.source, "two\none\nnone\n");
        }
    }

    #[test]
    fn a_tab_undoes_as_one_step_however_many_lines_it_moved() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_undo", "one\n\ntwo\n");
            d.anchor = Some(0);
            d.caret = 7;
            d.indent();
            assert_eq!(d.source, "  one\n\n  two\n");
            d.undo();
            assert_eq!(d.source, "one\n\ntwo\n", "one step, not one per line");
            assert_eq!(d.selection(), Some((0, 7)), "with the selection it was aimed at");
            d.redo();
            assert_eq!(d.source, "  one\n\n  two\n");
            assert_eq!(
                d.selection(),
                Some((0, 12)),
                "redo replays the caret the indent placed, not the one splice left"
            );
        }
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

    // ── view parity ──────────────────────────────────────────────────────────
    // `doc_with` pins the source view, so everything above tests a view users
    // never start in — `Doc::open` opens in WYSIWYG. These run the motion and
    // deletion golden cases through *both*, plus the WYSIWYG cases the two
    // can't share: where the source carries markup the rendered text is a
    // different string, and the views agreeing would itself be the bug.

    const VIEWS: [(View, &str); 2] = [(View::Source, "source"), (View::Wysiwyg, "wysiwyg")];

    /// Run `action` in both views on one `|`-marked fixture and assert they
    /// agree. Plain prose only: with no markup to hide, WYSIWYG renders the
    /// source verbatim, so the two views are looking at the same text and any
    /// disagreement is one of them having lost the plot.
    fn both_views(name: &str, marked: &str, action: fn(&mut Doc)) -> String {
        let (src, caret) = parse_caret(marked);
        let run = |view: View, tag: &str| {
            let mut d = doc_in(view, &format!("{name}_{tag}"), &src);
            d.caret = caret;
            action(&mut d);
            render_caret(&d)
        };
        let source = run(VIEWS[0].0, VIEWS[0].1);
        let wysiwyg = run(VIEWS[1].0, VIEWS[1].1);
        assert_eq!(source, wysiwyg, "the views disagree on {marked:?}");
        source
    }

    #[test]
    fn word_motion_agrees_across_the_views_on_plain_prose() {
        let g = both_views;
        assert_eq!(g("par_wl", "hello wor|ld", |d| d.move_word_left(false)), "hello |world");
        assert_eq!(g("par_wl2", "hello| world", |d| d.move_word_left(false)), "|hello world");
        assert_eq!(g("par_wr", "hel|lo world", |d| d.move_word_right(false)), "hello| world");
        assert_eq!(g("par_wr2", "hello| world", |d| d.move_word_right(false)), "hello world|");
        assert_eq!(g("par_punct", "|foo.bar", |d| d.move_word_right(false)), "foo|.bar");
        assert_eq!(
            g("par_ext", "hello |world", |d| d.move_word_right(true)),
            "hello [world|]"
        );
    }

    #[test]
    fn word_deletion_agrees_across_the_views_on_plain_prose() {
        let g = both_views;
        assert_eq!(g("par_db", "hello world|", |d| d.delete_word_back()), "hello |");
        assert_eq!(g("par_df", "hello |world", |d| d.delete_word_forward()), "hello |");
        assert_eq!(g("par_db2", "foo |bar baz", |d| d.delete_word_back()), "|bar baz");
        assert_eq!(g("par_utf8", "café |ok", |d| d.delete_word_back()), "|ok");
    }

    #[test]
    fn character_motion_and_deletion_agree_across_the_views_on_plain_prose() {
        let g = both_views;
        assert_eq!(g("par_r", "he|llo", |d| d.move_right(false)), "hel|lo");
        assert_eq!(g("par_l", "he|llo", |d| d.move_left(false)), "h|ello");
        assert_eq!(g("par_bs", "hel|lo", |d| d.backspace()), "he|lo");
        assert_eq!(g("par_del", "hel|lo", |d| d.delete_forward()), "hel|o");
    }

    #[test]
    fn wysiwyg_motion_steps_a_grapheme_cluster_the_way_the_source_view_does() {
        // The reproduction: the stop table was built one stop per `char`, so
        // Right parked the caret 4 bytes into a ZWJ sequence — a place the
        // source view, which steps by grapheme, can't reach and backspace can't
        // survive. The two views must land on the same offset.
        let family = "👨‍👩‍👧"; // three emoji strung together with joiners: one cluster
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("cluster_{tag}"), &format!("a{family}b\n"));
            d.caret = 1;
            d.move_right(false);
            assert_eq!(d.caret, 1 + family.len(), "{tag} parked inside the cluster");

            // ...and the edit that used to sever a joiner off the front of it.
            d.backspace();
            assert_eq!(d.source, "ab\n", "{tag} split the cluster");
            assert_eq!(d.caret, 1);
        }
    }

    #[test]
    fn wysiwyg_motion_treats_a_combining_accent_as_one_character() {
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("combining_{tag}"), "e\u{0301}x\n");
            d.caret = 0;
            d.move_right(false);
            assert_eq!(d.caret, "e\u{0301}".len(), "{tag} stopped on the combining mark");
        }
    }

    #[test]
    fn no_wysiwyg_motion_can_park_the_caret_inside_a_cluster() {
        // The general form: whatever route the caret takes through a document
        // full of clusters, it never lands between the codepoints of one — so no
        // motion-then-backspace sequence can leave a dangling joiner behind.
        use unicode_segmentation::UnicodeSegmentation;

        let src = "a👨‍👩‍👧b e\u{0301}mo👨‍👩‍👧ji\n\nnext 👩‍🚀 line\n";
        let mut d = wysiwyg_doc("cluster_walk", src);
        d.caret = 0;
        let boundaries: Vec<usize> = src
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(src.len()))
            .collect();
        for off in walk_right(&mut d) {
            assert!(
                boundaries.contains(&off),
                "Right stopped at {off}, inside a grapheme cluster"
            );
        }
    }

    #[test]
    fn wysiwyg_word_motion_stays_out_of_hidden_delimiters() {
        // The reproduction: ⌥→ from inside the opening `**` computed its
        // boundary over the raw source and landed on byte 8 — inside the
        // *closing* `**`, which `caret_pos` draws at column 6, immediately after
        // "bold". The caret drew past the bold word and sat inside it.
        let mut d = wysiwyg_doc("wys_word_delim", "a **bold** c\n");
        d.caret = 2;
        d.move_word_right(false);
        assert!(d.vmap.is_stop(d.caret), "landed at {}, not a caret stop", d.caret);
        assert_eq!(d.caret, 10, "should land on the space after \"bold\"");
        // The rendered row is "a bold c": column 6 is the space just past "bold",
        // and now the caret is really there rather than only drawn there.
        assert_eq!(d.caret_pos(), (0, 6));

        // ...and back again: ⌥← returns to the "b", not into the opening `**`.
        d.move_word_left(false);
        assert_eq!(d.caret, 4);
        assert_eq!(d.caret_pos(), (0, 2));
    }

    #[test]
    fn wysiwyg_word_delete_takes_the_markup_with_the_word() {
        // The reproduction: ⌥⌫ from after "bold" walked the raw source, stopped
        // inside the closing `**`, and left "a ** c\n" — delimiters with no
        // opener. Glyph space covers the word alone, which would leave
        // "a **** c": markup wrapped around nothing. The word and the styling
        // that was only ever the word's go together.
        let mut d = wysiwyg_doc("wys_word_del_back", "a **bold** c\n");
        d.caret = 10;
        d.delete_word_back();
        assert_eq!(d.source, "a  c\n");
        assert_eq!(d.caret, 2);

        let mut d = wysiwyg_doc("wys_word_del_fwd", "a **bold** c\n");
        d.caret = 4; // the "b"
        d.delete_word_forward();
        assert_eq!(d.source, "a  c\n");
    }

    #[test]
    fn wysiwyg_word_delete_empties_a_nested_mark_and_a_code_span_too() {
        let src = "a ***bold*** c\n";
        let mut d = wysiwyg_doc("wys_word_del_nest", src);
        d.caret = src.find(" c").unwrap();
        d.delete_word_back();
        assert_eq!(d.source, "a  c\n", "the emph inside the strong empties it too");

        let src = "a `code` c\n";
        let mut d = wysiwyg_doc("wys_word_del_code", src);
        d.caret = src.find(" c").unwrap();
        d.delete_word_back();
        assert_eq!(d.source, "a  c\n");
    }

    #[test]
    fn wysiwyg_word_delete_keeps_a_mark_that_still_has_text() {
        // Only an *emptied* node goes. Take one word of two and the `**` still
        // has a job to do.
        let src = "a **two words** c\n";
        let mut d = wysiwyg_doc("wys_word_del_partial", src);
        d.caret = src.find(" words").unwrap();
        d.delete_word_back();
        assert_eq!(d.source, "a ** words** c\n");
    }

    #[test]
    fn source_view_word_motion_still_walks_the_markup() {
        // The other half of the decision: in the source view the `**` are
        // characters like any other — they're on the screen, so word motion has
        // to stop at them and a word-delete has to leave them behind. Only
        // WYSIWYG hides them, so only WYSIWYG steps over them.
        let g = |n, m, f: fn(&mut Doc)| golden(n, m, f);
        assert_eq!(
            g("src_word_motion", "a |**bold** c\n", |d| d.move_word_right(false)),
            "a **bold|** c\n"
        );
        // The same caret as the WYSIWYG reproduction, and the opposite outcome:
        // here "a ** c\n" is right, because `bold**` is what's to the left of it.
        assert_eq!(
            g("src_word_del", "a **bold**| c\n", |d| d.delete_word_back()),
            "a **| c\n"
        );
    }

    #[test]
    fn every_wysiwyg_motion_lands_on_a_caret_stop() {
        // The single invariant both bugs violated: the caret draws and edits at
        // the same place only when it's on a stop. `debug_assert_on_a_stop`
        // makes the same claim in-place; this pins it from the outside, over a
        // document with every kind of thing the map has to be careful about.
        let src = "# Title\n\na **bold** e\u{0301}mo👨‍👩‍👧ji `x` c\n\n\
                   - item one\n\n| A | B |\n|---|---|\n| x | y |\n";
        let mut d = wysiwyg_doc("stop_invariant", src);
        let motions: [(&str, fn(&mut Doc)); 8] = [
            ("right", |d| d.move_right(false)),
            ("left", |d| d.move_left(false)),
            ("word_right", |d| d.move_word_right(false)),
            ("word_left", |d| d.move_word_left(false)),
            ("down", |d| d.move_down(false)),
            ("up", |d| d.move_up(false)),
            ("home", |d| d.move_home(false)),
            ("end", |d| d.move_end(false)),
        ];
        let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
        assert!(stops.len() > 20, "fixture should have plenty of stops");
        for start in stops {
            for (name, motion) in &motions {
                d.caret = start;
                d.anchor = None;
                motion(&mut d);
                assert!(
                    d.vmap.is_stop(d.caret),
                    "{name} from {start} landed at {} — not a caret stop",
                    d.caret
                );
            }
        }
    }
    // ── display columns ──────────────────────────────────────────────────────
    // A `col` is a terminal cell, not a character. The two are the same number
    // for the ASCII the fixtures above are written in, which is how they came
    // apart in the first place: `你` is one character drawn in two cells, so a
    // column counted in characters names a cell the text isn't in — one earlier
    // for every wide character to its left.

    #[test]
    fn a_wide_character_is_two_columns_wide() {
        // The reproduction: `你` is one char and two cells, so the caret just
        // past it drew at column 1 — inside the character it had already left.
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("wide_col_{tag}"), "你好\n");
            d.caret = "你".len();
            assert_eq!(d.caret_pos(), (0, 2), "{tag}: caret drew inside 你");
            d.caret = "你好".len();
            assert_eq!(d.caret_pos(), (0, 4), "{tag}");
        }
    }

    #[test]
    fn a_cluster_is_as_wide_as_it_is_drawn_not_as_its_codepoints_measure() {
        // `👨‍👩‍👧` is five codepoints — two-cell, joiner, two-cell, joiner,
        // two-cell — measuring six cells one at a time, but the character they
        // spell is drawn in two. Width belongs to the cluster, not the glyph,
        // and the frontends measure it the same way.
        let family = "👨‍👩‍👧";
        for (view, tag) in VIEWS {
            let src = format!("a{family}b\n");
            let mut d = doc_in(view, &format!("wide_cluster_{tag}"), &src);
            d.caret = 1 + family.len();
            assert_eq!(d.caret_pos(), (0, 3), "{tag}: 'a' is one cell, the family two");
        }
    }

    #[test]
    fn both_cells_of_a_wide_character_mean_the_character() {
        // Clicking the far half of `好` is still clicking `好`: half a character
        // is not a place the caret can be, so it comes to rest at the
        // character's start — the column it would have been drawn at anyway.
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("wide_click_{tag}"), "你好\n");
            for col in [2, 3] {
                d.caret = 0;
                d.click(0, col, false);
                assert_eq!(d.caret, "你".len(), "{tag}: click at col {col}");
                assert_eq!(d.caret_pos(), (0, 2), "{tag}: click at col {col}");
            }
            // Past the last cell is the line's end, as it is for ASCII.
            d.click(0, 9, false);
            assert_eq!(d.caret, "你好".len(), "{tag}: click past the end");
        }
    }

    #[test]
    fn every_offset_survives_the_trip_out_to_a_column_and_back() {
        // The mapping is only a mapping if it inverts: the cell the caret is
        // drawn in has to be the cell that brings it back to the same offset.
        // Over a fixture where a character may be one cell or two, and one
        // codepoint or five.
        use unicode_segmentation::UnicodeSegmentation;

        let src = "ab 你好 c\n\n👨‍👩‍👧 e\u{0301}x 漢字\n\nplain ascii\n";

        let mut d = doc_in(View::Source, "roundtrip_source", src);
        // Every offset the source view's caret can occupy: it steps by grapheme
        // cluster, so those are its boundaries.
        for (off, _) in src.grapheme_indices(true).chain(std::iter::once((src.len(), ""))) {
            d.caret = off;
            let (row, col) = d.caret_pos();
            d.click(row, col, false);
            assert_eq!(d.caret, off, "source: {off} → ({row}, {col}) → {}", d.caret);
        }

        // And in WYSIWYG, where the offsets the caret can occupy are the map's
        // stops rather than every boundary.
        let mut d = doc_in(View::Wysiwyg, "roundtrip_wysiwyg", src);
        let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
        assert!(stops.len() > 20, "fixture should have plenty of stops");
        for off in stops {
            d.caret = off;
            let (row, col) = d.caret_pos();
            d.click(row, col, false);
            assert_eq!(d.caret, off, "wysiwyg: {off} → ({row}, {col}) → {}", d.caret);
        }
    }

    #[test]
    fn vertical_motion_aims_at_a_column_the_reader_can_see() {
        // Down from under `世` lands under the glyph in that cell, not two
        // characters further along the line. The goal is a column, so a line of
        // wide characters and a line of ASCII line up the way they're drawn.
        //
        // The gap differs by view: a bare newline inside a paragraph is a soft
        // break, which WYSIWYG draws as a space on a single row. The views share
        // a grid only where the source's lines are the renderer's rows too.
        for (view, tag) in VIEWS {
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let src = format!("你好世{gap}abcdef\n");
            let mut d = doc_in(view, &format!("goal_wide_{tag}"), &src);
            d.caret = "你好".len();
            assert_eq!(d.caret_pos().1, 4, "{tag}: `世` is drawn at column 4");
            d.move_down(false);
            assert_eq!(d.caret_pos().1, 4, "{tag}: goal column lost");
            assert!(d.source[d.caret..].starts_with('e'), "{tag}: landed on the wrong glyph");
        }
    }

    #[test]
    fn a_goal_column_landing_inside_a_wide_character_lands_on_it() {
        // Down from column 3 onto `你好`, whose characters start at columns 0
        // and 2: column 3 is the *second* cell of `好`. There is nowhere to be
        // between the cells of one character, so the caret rests on it — and on
        // its start, which is the only offset there that is a caret stop.
        for (view, tag) in VIEWS {
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let src = format!("abcdef{gap}你好\n");
            let mut d = doc_in(view, &format!("goal_inside_{tag}"), &src);
            let line = src.find('你').unwrap();
            d.caret = 3;
            d.move_down(false);
            assert_eq!(d.caret, line + "你".len(), "{tag}: landed off `好`'s start");
            assert_eq!(d.caret_pos().1, 2, "{tag}: drew between `好`'s cells");
        }
    }

    #[test]
    fn a_caret_in_a_table_cell_of_wide_text_draws_where_the_text_is() {
        // The column the cell's text is laid out in is measured in cells, so the
        // caret walking that text has to be too — the two agreeing is the whole
        // point of the grid staying square.
        let mut d = wysiwyg_doc("table_wide", "| A | B |\n|---|---|\n| 你好 | y |\n");
        let at = d.source.find("你").unwrap();
        d.caret = at;
        let (row, col) = d.caret_pos();
        // `│ ` opens the row, so the cell's text starts at column 2; `好` is two
        // cells further along.
        assert_eq!(col, 2, "the cell's first character");
        d.move_right(false);
        assert_eq!(d.caret_pos(), (row, 4), "`好` is drawn past `你`'s two cells");
        assert_eq!(d.caret, at + "你".len());
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



