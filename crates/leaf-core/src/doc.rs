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

// `PathBuf` names the `path` field and the untitled marker on every build;
// `Path` is only touched by the filesystem I/O gated behind the `fs` feature.
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(feature = "fs")]
use std::path::Path;

use anyhow::{Result, anyhow};
#[cfg(feature = "fs")]
use anyhow::Context;
use twig::{
    BlockContainerKind, BlockKind, Change, Editor, FlatNode, Format, InlineKind,
    MarkdownExtensions, NodeId,
};
use unicode_segmentation::GraphemeCursor;

use crate::html;
use crate::wysiwyg::{self, VisualMap};

/// Which view the body shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The raw document with a caret in source bytes.
    Source,
    /// Markup resolved to real styles, caret riding the rendered glyphs.
    Wysiwyg,
}

/// What the file behind a document looks like right now, against the bytes leaf
/// last read from it or wrote to it — the question a frontend asks before it
/// saves (a `Changed` file plus a `dirty` document is an overwrite about to
/// happen) or when its window regains focus. See [`Doc::disk_state`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskState {
    /// The file holds exactly the bytes leaf last read or wrote.
    Unchanged,
    /// Someone else wrote the file since. Saving overwrites their work; see
    /// [`Doc::reload`] for the other direction.
    Changed,
    /// The file is gone — deleted or renamed away. A save recreates it.
    Missing,
    /// There is a path, but the file couldn't be read (permissions, a directory
    /// in the way): leaf can't tell, and won't guess.
    Unreadable,
    /// No file behind this document yet — see [`Doc::blank`]. Nothing can have
    /// changed under a document that was never on disk.
    Untitled,
}

/// The inline marks in force at a point in the document — what a toolbar
/// lights up. A `Copy` bitset rather than a `HashSet`, because
/// [`Doc::active_inline_marks`] is called on every frame that draws a toolbar
/// and a set that allocates to answer "is Bold on?" is a set that shouldn't.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineMarks(u8);

impl InlineMarks {
    /// Every kind, in the order [`InlineMarks::iter`] yields them.
    const ALL: [InlineKind; 8] = [
        InlineKind::Strong,
        InlineKind::Emph,
        InlineKind::Verbatim,
        InlineKind::Mark,
        InlineKind::Superscript,
        InlineKind::Subscript,
        InlineKind::Insert,
        InlineKind::Delete,
    ];

    pub const fn empty() -> Self {
        InlineMarks(0)
    }

    /// Private: the set is an *answer*, and adding a mark to it doesn't mark
    /// anything ([`Doc::toggle`] does that). `FromIterator` is the way in.
    fn insert(&mut self, kind: InlineKind) {
        self.0 |= Self::bit(kind);
    }

    /// Whether `kind` is in force — the toolbar's "is Bold active?".
    pub fn contains(self, kind: InlineKind) -> bool {
        self.0 & Self::bit(kind) != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The marks in force, for a frontend that renders whatever is on rather
    /// than asking after a fixed list.
    pub fn iter(self) -> impl Iterator<Item = InlineKind> {
        Self::ALL.into_iter().filter(move |&k| self.contains(k))
    }

    fn bit(kind: InlineKind) -> u8 {
        1 << match kind {
            InlineKind::Strong => 0,
            InlineKind::Emph => 1,
            InlineKind::Verbatim => 2,
            InlineKind::Mark => 3,
            InlineKind::Superscript => 4,
            InlineKind::Subscript => 5,
            InlineKind::Insert => 6,
            InlineKind::Delete => 7,
        }
    }
}

impl FromIterator<InlineKind> for InlineMarks {
    fn from_iter<I: IntoIterator<Item = InlineKind>>(iter: I) -> Self {
        let mut m = InlineMarks::empty();
        for k in iter {
            m.insert(k);
        }
        m
    }
}

/// What kind of edit produced an undo group. Same-kind edits in a row coalesce
/// into one undo step (a run of typed characters undoes together); `Other` never
/// coalesces, so a paste, format toggle, or block change is always its own step.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    /// One step of an IME composition — see [`Doc::edit_composing`]. Its own kind
    /// rather than `Insert`'s because a composition is not typing: each step
    /// *replaces* the last (`か` → `かん` → `感`), so the run has to coalesce even
    /// though no two steps insert the same bytes, and it must not fold into the
    /// typed characters on either side of it.
    Compose,
    Other,
}

/// The caret and selection at one moment — the part of a history step twig's
/// `Change` cannot carry, because the caret is leaf's state and twig only knows
/// about bytes. leaf serializes it into the opaque per-state blob twig now
/// stores in its own undo history (see `record_caret`), so undo and redo hand
/// back the caret that matches the source they restore.
#[derive(Clone, Copy)]
struct CaretState {
    caret: usize,
    anchor: Option<usize>,
}

impl CaretState {
    /// Pack into the fixed 17-byte blob leaf hands twig: the caret as a u64,
    /// then an anchor-present flag and the anchor. twig copies these bytes and
    /// never reads them.
    fn to_blob(self) -> [u8; 17] {
        let mut b = [0u8; 17];
        b[..8].copy_from_slice(&(self.caret as u64).to_le_bytes());
        if let Some(a) = self.anchor {
            b[8] = 1;
            b[9..].copy_from_slice(&(a as u64).to_le_bytes());
        }
        b
    }

    /// Recover a state from twig's blob, or `None` when it is empty or the wrong
    /// length — a state twig restored that never had a caret set on it, which
    /// leaves the caller to fall back to the edit site.
    fn from_blob(b: &[u8]) -> Option<Self> {
        let b: &[u8; 17] = b.try_into().ok()?;
        let caret = u64::from_le_bytes(b[..8].try_into().unwrap()) as usize;
        let anchor =
            (b[8] != 0).then(|| u64::from_le_bytes(b[9..].try_into().unwrap()) as usize);
        Some(CaretState { caret, anchor })
    }
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
    /// A hash of the bytes leaf last read from `path` or wrote to it; `None`
    /// while the document has no file behind it. [`Doc::disk_state`] compares
    /// the file against this to catch an edit made *outside* leaf before a save
    /// silently overwrites it — `clean_source` only knows what leaf itself did.
    ///
    /// A hash, not an mtime: mtime is the cheap answer and the wrong one — two
    /// writes inside one filesystem timestamp tick are indistinguishable, a
    /// clock that steps backwards (or a writer that restores an mtime) hides a
    /// real change, and a `touch` invents one. The whole point of the watermark
    /// is to not clobber someone's work, so it reads the bytes and compares what
    /// is actually there. That costs a file read per question, which is why the
    /// question is asked on a user event (focus, save) and not every frame.
    disk_hash: Option<u64>,
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
    /// The rendered map for the WYSIWYG view; empty in the source view. Movement
    /// and clicks read it to stay in visible space.
    pub vmap: VisualMap,
    /// Everything the map is built from, as one number: bumped whenever the
    /// document's text changes, and never by a motion, a selection, or a save.
    /// A frontend can hold work against it — see [`Doc::revision`].
    revision: u64,
    /// What `vmap` was built from, or `None` before the first build. The map is
    /// a pure function of `(revision, wrap)`, so when those haven't moved,
    /// rebuilding it produces the identical map — see [`Doc::build_visual`].
    vmap_key: Option<(u64, Option<usize>)>,
    /// Per-block row cache backing the incremental rebuild: when the text
    /// changes, only the top-level blocks whose bytes moved are re-rendered and
    /// the rest are reused shifted (see [`wysiwyg::BlockCache`]). Persists across
    /// builds; a pure accelerator, so it's never read for correctness.
    block_cache: wysiwyg::BlockCache,
    /// How many visual rows each block image reserves, keyed by its destination —
    /// set by the frontend through [`Doc::set_image_rows`] once it has decoded and
    /// measured the pictures. Core does no image I/O, so this is the only way it
    /// learns a picture's height; a destination not in the map reserves the bare
    /// one-row placeholder. Threaded into the builder so [`wysiwyg::build_cached`]
    /// sizes each placeholder, and folded into `vmap_key` so a height change
    /// rebuilds the map.
    image_rows: HashMap<String, usize>,

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

/// The Markdown extensions every leaf document is parsed with. Only
/// `html_elements` departs from twig's defaults: it promotes embedded raw HTML
/// (`<img>`, `<picture>`, `<source>`, …) into semantic AST nodes, so a picture
/// becomes a real `image` node the frontends can frame and rasterize instead of
/// opaque `raw_block` text. The flag is inert for non-Markdown formats, so it's
/// safe to pass unconditionally. Threading it through every constructor (not
/// just `open`) keeps `from_source`, `blank`, and `reload` parsing the same
/// document the same way — twig reparses with these same flags after each edit.
fn parse_extensions() -> MarkdownExtensions {
    MarkdownExtensions { html_elements: true, ..Default::default() }
}

/// Build an editor over `bytes` in `format` with leaf's [`parse_extensions`],
/// mapping twig's error into the `anyhow` context every constructor shares.
fn new_editor(bytes: &[u8], format: Format) -> Result<Editor> {
    Editor::new_ext(bytes, format, parse_extensions()).map_err(|e| anyhow!("twig parse: {e}"))
}

impl Doc {
    #[cfg(feature = "fs")]
    pub fn open(path: PathBuf) -> Result<Self> {
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let format = detect_format(&path)?;
        let editor = new_editor(&bytes, format)?;
        let source = String::from_utf8(bytes).map_err(|_| anyhow!("document is not UTF-8"))?;
        let disk_hash = Some(hash_bytes(source.as_bytes()));
        // Store the document's *absolute* path. A relative one (`leaf README.md`)
        // has an empty parent, so a frontend can't resolve a relative image
        // destination (`![](pic.png)`) against the document's directory and the
        // picture silently falls back to its text placeholder. `absolute` is
        // purely lexical — it prefixes the current directory and normalizes, but
        // reads nothing and resolves no symlinks — so `file_name` and save are
        // unchanged; it only gives `path.parent()` something to join against.
        let path = std::path::absolute(&path).unwrap_or(path);
        Ok(Doc::from_parts(editor, format, path, source, disk_hash))
    }

    /// Build a document from an in-memory string, the format named explicitly —
    /// the portable, filesystem-free counterpart to [`Doc::open`] (which reads a
    /// path and sniffs the format from its extension). A wasm or FFI host, which
    /// has no path to read, uses this: it hands over bytes it fetched however it
    /// could, and later persists [`Doc::source`] however it can (a browser
    /// download, `localStorage`, a backend `PUT`) and calls [`Doc::mark_saved`].
    ///
    /// No file backs the result, so it starts untitled ([`Doc::is_untitled`] is
    /// true) exactly like a [`Doc::blank`] that has been given content.
    pub fn from_source(source: String, format: Format) -> Result<Self> {
        let editor = new_editor(source.as_bytes(), format)?;
        Ok(Doc::from_parts(editor, format, PathBuf::new(), source, None))
    }

    /// An untitled, empty document — the `+` button and a `leaf` launched with
    /// no file argument. Nothing on disk backs it until a [`Doc::save_as`].
    ///
    /// It is Markdown, because a format has to be chosen before a name exists to
    /// read one from: `detect_format` reads the extension and an untitled
    /// document has neither. Markdown is what leaf's own files are, what its
    /// block markers are already written for (`insert_block_prefix`), and the
    /// extension a Save As will overwhelmingly pick — a wrong guess here would
    /// mean typing djot into a buffer parsing it as Markdown. Note that Save As
    /// *doesn't* revisit this: see [`Doc::save_as`].
    pub fn blank() -> Result<Self> {
        let format = Format::Markdown;
        let editor = new_editor(b"", format)?;
        // An empty `path` is the untitled marker (`path` is a public `PathBuf`
        // field two frontends already read; making it an `Option` to say this
        // would break both). `is_untitled` is the question to ask, not the
        // representation to copy.
        Ok(Doc::from_parts(editor, format, PathBuf::new(), String::new(), None))
    }

    /// The fields every constructor agrees on, so `open` and `blank` can't drift
    /// apart in the ones neither of them has an opinion about.
    fn from_parts(
        editor: Editor,
        format: Format,
        path: PathBuf,
        source: String,
        disk_hash: Option<u64>,
    ) -> Self {
        Doc {
            editor,
            format,
            path,
            disk_hash,
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
            revision: 0,
            // No map yet — the first `build_visual` always builds.
            vmap_key: None,
            block_cache: wysiwyg::BlockCache::default(),
            image_rows: HashMap::new(),
            scroll: 0,
            body_origin: (0, 0),
            body_height: 0,
            drawn_caret: None,
        }
    }

    /// Whether this document has no file behind it yet — a [`Doc::blank`] that
    /// has never been saved. The question a ⌘S handler asks to know it should
    /// open a Save As picker instead ([`Doc::save`] won't guess a name), and the
    /// header asks to know the name it shows is a placeholder.
    pub fn is_untitled(&self) -> bool {
        self.path.as_os_str().is_empty()
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
    /// Build the WYSIWYG map, wrapped at `width` display columns.
    ///
    /// Cheap to call every frame, which is what both frontends do: the map is a
    /// pure function of the document and the wrap width, so a call that would
    /// rebuild the same map returns the one already built. Only an edit (or a
    /// resize) pays.
    ///
    /// That isn't a micro-optimisation. A frontend repaints for reasons that have
    /// nothing to do with the text — a blinking caret, a scroll, a focus change —
    /// and rebuilding here is O(document): 23 ms on a 1 MB file, of which 5 ms is
    /// marshalling twig's AST across the C ABI. Paid twice a second by the GUI's
    /// blink timer, that was 14% of a core spent redrawing an unchanged document.
    /// (`cargo run --release -p leaf-core --example bench` for the numbers.)
    pub fn build_visual(&mut self, width: usize) {
        self.build_map(Some(width));
    }

    /// Build the WYSIWYG map with each block as a single unwrapped row — for a
    /// frontend (the GUI) that wraps at its own proportional pixel width rather
    /// than a fixed character column.
    pub fn build_visual_unwrapped(&mut self) {
        self.build_map(None);
    }

    /// Tell the model how many visual rows each block image should reserve, keyed
    /// by the image's destination. A terminal frontend calls this once it has
    /// decoded and measured its pictures — core does no image I/O, so this is the
    /// only way it learns a height — and the next [`Doc::build_visual`] lays each
    /// placeholder out that tall (the label row plus blank filler rows the
    /// frontend paints the raster over). A destination left out of the map falls
    /// back to the bare one-row placeholder, which is also what a frontend that
    /// can't draw pictures (or lays them out in its own units, like the GUI) gets
    /// by never calling this.
    ///
    /// Cheap to call every frame with the same map: only a *change* invalidates
    /// the built map (and the block-row cache, since a height isn't part of a
    /// block's bytes and so wouldn't otherwise re-render it). Steady state is a
    /// no-op, so a frontend can just hand over its current measurements each frame.
    pub fn set_image_rows(&mut self, rows: HashMap<String, usize>) {
        if self.image_rows == rows {
            return;
        }
        self.image_rows = rows;
        // A height lives outside the block's source bytes, so the content-keyed
        // block cache would hand back the old-height rows on a hit. Drop it (and
        // the splice layout it carries) so the next build re-renders every block
        // at the new heights, and force that build by clearing the map key.
        self.block_cache = wysiwyg::BlockCache::default();
        self.vmap_key = None;
    }

    /// The revision the document's text is at — bumped by every edit, undo,
    /// redo, and reload, and by nothing else. A frontend caches against this to
    /// tell a repaint that needs new work from one that doesn't.
    ///
    /// It counts *edits*, not distinct texts: typing `x` and deleting it again
    /// lands on the same text two revisions later. Work is only ever rebuilt
    /// needlessly, never wrongly reused.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The map, built at most once per `(revision, wrap)`. `clamp_caret` still
    /// runs on every call: the caret moves without the document changing, and
    /// keeping it on a legal stop is this function's job either way.
    fn build_map(&mut self, wrap: Option<usize>) {
        let key = (self.revision, wrap);
        if self.vmap_key != Some(key) {
            // Enumerate the top-level blocks cheaply — no whole-arena marshal.
            // A subtree is pulled only for the block(s) that actually changed, so
            // the FFI marshal shrinks from O(document) to O(edited block).
            let top = self.editor.child_spans(None).unwrap_or_default();

            // Fast path: when twig reports a dirty byte range, try to patch the
            // previous map in place — a single-block edit moves the prefix,
            // shifts the suffix, and re-renders only one block. `build_spliced`
            // returns `None` (and we fall back to the always-correct full rebuild)
            // whenever the edit reshaped the block structure, hit a table, or
            // there's no previous map to patch.
            let spliced = match self.editor.dirty_range() {
                Some(dirty) => {
                    let prev = std::mem::take(&mut self.vmap);
                    let source = &self.source;
                    let cache = &mut self.block_cache;
                    let image_rows = &self.image_rows;
                    let editor = &mut self.editor;
                    wysiwyg::build_spliced(prev, source, wrap, &top, dirty, image_rows, cache, |id| {
                        editor.subtree(NodeId(id)).unwrap_or_default()
                    })
                }
                None => None,
            };
            self.vmap = spliced.unwrap_or_else(|| {
                let source = &self.source;
                let cache = &mut self.block_cache;
                let image_rows = &self.image_rows;
                let editor = &mut self.editor;
                wysiwyg::build_cached(&top, source, wrap, image_rows, cache, |id| {
                    editor.subtree(NodeId(id)).unwrap_or_default()
                })
            });
            // Acknowledge the dirty range so the next edit's range starts fresh.
            self.editor.clear_dirty();
            self.vmap_key = Some(key);
        }
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

    /// The name to show for this document. An untitled one has no file to name
    /// it, and both frontends put this straight on screen — an empty path
    /// renders as an empty header, so it says so instead.
    pub fn file_name(&self) -> String {
        if self.is_untitled() {
            return "untitled".into();
        }
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

    /// Replace `[start, end)` with `text` as one step of an IME composition —
    /// the same splice as [`edit`](Self::edit), but marked so the run of steps
    /// folds into a single undo.
    ///
    /// A composition is *one* act of writing. Typing `かんじ` and picking 感じ is a
    /// dozen calls here, each replacing the last one's provisional bytes, and an
    /// undo step per call means undoing a word means pressing ⌘Z until the reading
    /// unspools backwards through kana — the intermediate states were never text
    /// the user wrote. Only the frontend knows a call is provisional (the bytes
    /// look like any other edit), so the door the caller comes through is what
    /// says so, exactly as it is for [`paste`](Self::paste) versus
    /// [`insert`](Self::insert).
    ///
    /// Pair with [`end_composition`](Self::end_composition), or the *next*
    /// composition folds into this one.
    pub fn edit_composing(&mut self, start: usize, end: usize, text: &str) {
        self.splice(start, end, text, EditKind::Compose);
    }

    /// Close the open composition run, so the next one is its own undo step.
    /// Call when the IME commits or withdraws a composition.
    ///
    /// Only clears a *composition* run: a frontend that reports an end it never
    /// began (some IMEs unmark unprompted) would otherwise split the run of
    /// typing around it into two undo steps for no reason the user can see.
    pub fn end_composition(&mut self) {
        if self.last_edit_kind == Some(EditKind::Compose) {
            self.last_edit_kind = None;
        }
    }

    // ── the clipboard's rich flavor ──────────────────────────────────────────

    /// The selection rendered as HTML, for the clipboard's `text/html` flavor —
    /// what lets a paste into Docs/Mail/Slack keep its formatting. `None` when
    /// nothing is selected, or when the selection doesn't render (the caller
    /// still has [`selected_text`](Self::selected_text), which is what to publish
    /// as `text/plain` either way).
    ///
    /// **The fragment is a source substring, and that is the honest limit here.**
    /// It's parsed standalone, so a selection whose meaning depends on its
    /// surroundings converts as what it literally says rather than what it looks
    /// like on screen: half a list item is a paragraph, a row torn out of a table
    /// is the text of a row, the `**` of a bold run selected without its closing
    /// `**` is two asterisks. Every one of those still *renders* — there's no
    /// error to report — it just renders as the fragment and not as the document.
    /// Widening the range to whole blocks would publish text the user didn't
    /// select, which is a worse lie than a fragment being a fragment; the plain
    /// flavor has the same substring, so the two flavors at least agree.
    pub fn selection_html(&mut self) -> Option<String> {
        let (start, end) = self.selection()?;
        let inline = self.selection_is_inline(start, end);
        let html = html::render_fragment(&self.source[start..end], self.format)?;
        Some(match inline {
            true => html::strip_sole_paragraph(html),
            false => html,
        })
    }

    /// Paste the clipboard's `text/html` flavor, converting it to this document's
    /// format first. Its own undo step, like any [`paste`](Self::paste).
    ///
    /// Returns whether it landed. `false` means the HTML didn't convert to
    /// anything worth pasting — the caller should fall back to the plain flavor
    /// rather than treat it as an error. The `html` module has the full list of
    /// what that covers: a table twig won't build, markup it doesn't recognise,
    /// an empty result.
    pub fn paste_html(&mut self, html: &str) -> bool {
        match html::parse_fragment(html, self.format) {
            Some(source) => {
                self.paste(&source);
                true
            }
            None => false,
        }
    }

    /// Does the selection live *inside* a single top-level block?
    ///
    /// The question [`selection_html`](Self::selection_html) needs and the
    /// fragment can't answer: `**bold**` renders as `<p><strong>bold</strong></p>`
    /// whether the user selected one word of a sentence or a whole paragraph, and
    /// only the document knows which. Selecting a word and pasting into Docs
    /// should extend the line you paste into; selecting the paragraph should make
    /// a paragraph. So a selection strictly within one block is inline (its `<p>`
    /// is an artifact of standalone parsing), and one that covers a whole block —
    /// or spans two — keeps its structure.
    ///
    /// Reads the block from twig rather than guessing from the bytes:
    /// `ancestors_at` is `[doc, block, …inline]`, so index 1 is the top-level
    /// block containing an offset, and two ends inside the same one cannot have
    /// crossed a block boundary.
    fn selection_is_inline(&mut self, start: usize, end: usize) -> bool {
        // The last *character*, not `end - 1`: the selection's end is exclusive
        // and may sit mid-codepoint's-worth of bytes past the last char.
        let Some((off, _)) = self.source[start..end].char_indices().next_back() else {
            return false;
        };
        let (Some(head), Some(tail)) = (self.top_block_span(start), self.top_block_span(start + off))
        else {
            return false;
        };
        head == tail && !(start <= head.start && end >= head.end)
    }

    /// The byte span of the top-level block containing `offset`, or `None` at an
    /// offset that belongs to no block (the blank line between two of them).
    fn top_block_span(&mut self, offset: usize) -> Option<std::ops::Range<usize>> {
        self.editor
            .ancestors_at(offset)
            .ok()?
            .get(1)
            .map(|m| m.span.clone())
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
        // re-record the caret so this is the state redo restores, not the one
        // `splice` left behind from the `Change`.
        self.caret = placed.0.min(self.source.len());
        self.anchor = placed.1;
        self.clamp_caret();
        self.record_caret();
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
        // Lists: an empty item exits the list, a non-empty one opens the next.
        // Gate on the AST, not the marker bytes alone — a `- ` line reads as a
        // list marker byte-for-byte whether or not it is one, and `text\n- \n`
        // is a *setext heading*, not a list. twig does report an empty item as a
        // childless `list_item`; the marker text is still read from source to
        // spell the next item's bullet. Ask the AST at the marker, not the
        // caret: on a bare `- ` line the caret sits on the trailing newline,
        // past the item's span, where the enclosing `list_item` is out of reach.
        if let Some((line_start, marker)) = self.list_marker_on_line(self.caret)
            && self.is_inside_list(line_start)
        {
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

    /// Whether `off` falls inside a list item, per the AST — the honest test
    /// for "is this a list line," which the `- ` marker bytes alone can't answer
    /// (they read identically in a setext underline). Pass the marker offset,
    /// not the caret: on a bare `- ` line the caret rests on the trailing
    /// newline, one past the item's span, where its `list_item` is out of reach.
    fn is_inside_list(&mut self, off: usize) -> bool {
        self.editor
            .ancestors_at(off)
            .map(|c| c.into_iter().any(|m| m.kind == "list_item" || m.kind == "task_list_item"))
            .unwrap_or(false)
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

    /// Delete from the caret back to the start of its line (⌘⌫). Deletes the
    /// selection instead when one is active, as every other delete here does.
    ///
    /// The line is the view's own — the one Home and End work on, so in WYSIWYG
    /// a soft-wrapped row is a line. It is not Home's *target*, though: Home
    /// stops at the first character and this takes the indentation with it, the
    /// way Cocoa's `deleteToBeginningOfLine:` does. Stopping at the text would
    /// leave an indent behind that nothing can then ask to delete, where a caret
    /// left at column 0 is one press of Home away from either.
    pub fn delete_to_line_start(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
            return;
        }
        // Never back across the floor: hidden frontmatter isn't on this line, or
        // on any line the WYSIWYG caret can see.
        let (start, _) = self.line_span();
        let start = start.max(self.caret_floor());
        if start < self.caret {
            let (s, e) = self.widen_over_emptied_inlines(start, self.caret);
            self.splice(s, e, "", EditKind::Delete);
        }
    }

    /// Kill from the caret to the end of its line (^K). Deletes the selection
    /// instead when one is active.
    ///
    /// At the end of the line it does nothing, rather than pulling the line
    /// below up into this one. Joining has no meaning to give it in both views
    /// at once: a WYSIWYG line ends at a soft wrap as often as at a newline, and
    /// there is nothing there to delete, while the newline a *source* line ends
    /// with is only half of the blank line that separates two paragraphs —
    /// deleting one leaves a soft break, which is not the join it looks like.
    /// The views agreeing is worth more than emacs' second press, and Delete is
    /// already the key that joins.
    pub fn delete_to_line_end(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
            return;
        }
        let (_, end) = self.line_span();
        if end > self.caret {
            let (s, e) = self.widen_over_emptied_inlines(self.caret, end);
            self.splice(s, e, "", EditKind::Delete);
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
        // Hand twig the pre-edit caret before the splice, so the undo step it
        // retires carries where the caret was standing.
        self.record_caret();
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
                // And the post-edit caret, so a later redo restores it.
                self.record_caret();
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

    /// Hand twig the current caret and selection as the blob for the live
    /// document state. Called before an edit — so the step twig retires records
    /// where the caret was, and undo can restore it — and again once the op has
    /// placed the caret, so redo restores where the edit left it.
    ///
    /// This is the whole of leaf's undo-caret bookkeeping now. twig carries the
    /// caret through its own history, so coalescing falls out for free (folding
    /// two twig steps into one drops the intermediate blob, keeping the run's
    /// first) and the parallel stacks that had to march in lockstep — and could
    /// silently drift out of it — are gone.
    fn record_caret(&mut self) {
        let _ = self.editor.set_caret_blob(&self.snapshot().to_blob());
    }

    /// Toggle an inline mark over the selection (Bold / Italic / Code / …). Keeps
    /// the toggled region selected so a second press cleanly reverses it.
    pub fn toggle(&mut self, kind: InlineKind) {
        let Some((s, e)) = self.selection() else {
            self.status = Some("select text first".into());
            return;
        };
        self.record_caret();
        match self.editor.toggle_inline(s, e, kind) {
            Ok(change) => {
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                self.anchor = Some(change.new.start);
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("{kind:?}: {e}")),
        }
    }

    /// Convert the block at the caret to a heading level or paragraph.
    pub fn set_block(&mut self, kind: BlockKind) {
        self.record_caret();
        match self.block_offset_for_caret() {
            Some(offset) => match self.editor.set_block(offset, kind) {
                Ok(_) => {
                    self.last_edit_kind = None;
                    self.refresh();
                    self.clamp_caret();
                    self.anchor = None;
                    self.dirty = self.source != self.clean_source;
                    self.status = None;
                    self.record_caret();
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

    /// The inline marks in force at the caret (or over the selection) — what a
    /// toolbar draws lit, and the block-level [`Doc::current_heading_level`]'s
    /// inline counterpart. Cheap enough to call every frame: one twig
    /// `ancestors_at` query per caret (two with a selection), each walking root
    /// → deepest node at one offset. It never snapshots the tree the way
    /// `current_heading_level` does, and the returned set is a `Copy` bitset, so
    /// the only allocation is twig's own small ancestor `Vec`.
    ///
    /// **A selection reports a mark only when the mark covers *all* of it.**
    /// That's what every real toolbar means by an active button — Bold lit over
    /// a half-bold selection would claim a press turns bold *off*, when
    /// [`Doc::toggle`] hands the range to twig and gets the whole thing bolded.
    /// Whole-coverage is asked as "is the same mark node standing over both the
    /// first and the last character?": inline nodes are contiguous, so one node
    /// covering both ends covers every byte between them. Two touching runs
    /// (`**a****b**`) are two nodes, and correctly light nothing.
    ///
    /// At a bare caret a mark is active when the caret stands inside the mark's
    /// span — `span.start <= caret < span.end`, delimiters included, which is
    /// what makes the boundaries behave. In `a **bold** b` the offsets from the
    /// opening `*` (2) through the last byte of the closing `**` (9) are all
    /// bold, so the WYSIWYG caret both before `b` and after `d` (the delimiters
    /// are hidden, and those offsets are 4 and 8) reports bold — matching where
    /// typing would actually land inside the marked run. The offset one past the
    /// mark (10) is the text after it and reports nothing, at the end of the
    /// buffer exactly as in the middle.
    pub fn active_inline_marks(&mut self) -> InlineMarks {
        let Some((start, end)) = self.selection() else {
            return self.marks_at(self.caret).into_iter().map(|(k, _)| k).collect();
        };
        // The selection's *last character*, not its exclusive end: `end` is the
        // offset one past the selection, which for a selection ending exactly at
        // a mark's close is already outside it (`[4,10)` of `a **bold** b` is
        // entirely bold, but offset 10 is the space after).
        let last = prev_boundary(&self.source, end);
        let head = self.marks_at(start);
        let tail = self.marks_at(last);
        head.into_iter()
            .filter(|m| tail.contains(m))
            .map(|(k, _)| k)
            .collect()
    }

    /// The inline marks whose span covers `off`, each with the id of the node
    /// carrying it — the id is what lets a selection tell one mark node from
    /// another of the same kind.
    fn marks_at(&mut self, off: usize) -> Vec<(InlineKind, u32)> {
        let off = off.min(self.source.len());
        self.editor
            .ancestors_at(off)
            .unwrap_or_default()
            .into_iter()
            // `span.end` is the offset one *past* the mark, so it isn't in it.
            // twig already resolves a boundary to whatever starts there — in
            // `**bold** x` offset 8 is the following text, not the strong — but
            // when nothing follows, the tie has nobody to break for and the
            // chain still ends at the mark. That would make the answer at the
            // last offset of the document depend on whether the file happens to
            // end in a newline; the rule is `span.start <= off < span.end`, and
            // it's the same rule at the end of a buffer as in the middle.
            .filter(|m| off < m.span.end)
            .filter_map(|m| inline_kind(&m.kind).map(|k| (k, m.node_id)))
            .collect()
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
        self.record_caret();
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
                self.record_caret();
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
        self.record_caret();
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
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("link: {e}")),
        }
    }

    /// Insert a block-level image at the caret: `![alt](destination)`. Any
    /// selection becomes the alt text (so "select a caption, insert image" labels
    /// it); with no selection, `alt` is used — empty for none. The caret lands
    /// just past the inserted image.
    ///
    /// Markdown and Djot spell an image identically, so this is a plain
    /// span-splice through [`edit`](Self::edit) rather than a twig editor op —
    /// there is no `insert_image` in twig's surface the way there is an
    /// `insert_link`. One consequence: `destination` and `alt` are inserted
    /// verbatim, so a `]`/`)` in either can break the markup. A frontend that
    /// takes these from a prompt should keep them tame; format-correct escaping
    /// (which twig's `insert_link` does because it owns the spelling) is a future
    /// refinement.
    pub fn insert_image(&mut self, destination: &str, alt: &str) {
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        // A selection is the alt text; otherwise the caller's `alt`.
        let alt_text = self
            .selected_text()
            .map(str::to_string)
            .unwrap_or_else(|| alt.to_string());
        let markup = format!("![{alt_text}]({destination})");
        // `edit` (via `splice`) records the undo caret, refreshes, and lands the
        // caret at the end of the inserted text — just past the image, which is
        // where a caret belongs after inserting one.
        self.edit(start, end, &markup);
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

    /// The destination of the image under the caret — what an image prompt shows
    /// so editing an existing image starts from its current URL instead of blank,
    /// the image analogue of [`link_destination_at_caret`](Self::link_destination_at_caret).
    /// `None` when the caret stands in no image. A caret resting just after a
    /// block image (its trailing stop) is still "in" it — the half-open span test
    /// excludes that offset, which is the intended precision: past the image is
    /// past it.
    pub fn image_destination_at_caret(&mut self) -> Option<String> {
        let off = self.caret;
        self.nodes()
            .into_iter()
            .filter(|n| n.kind == "image")
            .filter(|n| n.span.start <= off && off < n.span.end)
            .max_by_key(|n| n.span.start)
            .and_then(|n| n.destination)
    }

    /// The language of the fenced code block the caret stands in — what a
    /// language prompt shows so editing it starts from the current value rather
    /// than blank. `None` when the caret is in no code block, or in one whose
    /// fence carries no language (or an indented block, which has no fence).
    pub fn code_language_at_caret(&mut self) -> Option<String> {
        let start = self.code_block_start_at_caret()?;
        wysiwyg::code_language(&self.source, start)
    }

    /// Whether the caret stands in a fenced code block — the one a language
    /// prompt could edit. A frontend gates its "set language" affordance on this
    /// (an indented block, which can't carry a language, reports `false`).
    pub fn caret_in_fenced_code(&mut self) -> bool {
        self.code_block_start_at_caret()
            .is_some_and(|start| wysiwyg::code_info_span(&self.source, start).is_some())
    }

    /// Set (or clear, with `""`) the language of the fenced code block the caret
    /// is in — the prompt's confirm. Replaces the fence's info string in place;
    /// a no-op when the caret is in no fenced block.
    pub fn set_code_language(&mut self, lang: &str) {
        let Some(start) = self.code_block_start_at_caret() else {
            return;
        };
        let Some(span) = wysiwyg::code_info_span(&self.source, start) else {
            return;
        };
        // Trim what the user typed: an info string is a single token, and a
        // stray space would render as part of the label and re-open the prompt
        // with it next time.
        self.splice(span.start, span.end, lang.trim(), EditKind::Other);
    }

    /// The `span.start` of the code block covering the caret — the anchor
    /// [`wysiwyg::code_info_span`] reads the fence from. `None` when the caret is
    /// in none.
    fn code_block_start_at_caret(&mut self) -> Option<usize> {
        let off = self.caret;
        self.nodes()
            .into_iter()
            .filter(|n| n.kind == "code_block" && n.span.start <= off && off <= n.span.end)
            .max_by_key(|n| n.span.start)
            .map(|n| n.span.start)
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
    // twig owns the history of *bytes* (it owns the buffer) and now carries the
    // caret through it too: `record_caret` stashes each state's caret in twig's
    // opaque per-step blob, and undo/redo hand it back with the source they
    // restore. So leaf keeps no history of its own — no parallel stacks to march
    // in lockstep and silently drift out of it.

    /// Undo the last edit step (⌘Z / ^Z), putting the caret and selection back
    /// where they were when that step began.
    pub fn undo(&mut self) {
        match self.editor.undo() {
            Ok(Some(change)) => self.after_history(change),
            Ok(None) => self.status = Some("nothing to undo".into()),
            Err(e) => self.status = Some(format!("undo: {e}")),
        }
    }

    /// Redo the last undone edit step (⇧⌘Z / ^Y), putting the caret and
    /// selection back where that step originally left them.
    pub fn redo(&mut self) {
        match self.editor.redo() {
            Ok(Some(change)) => self.after_history(change),
            Ok(None) => self.status = Some("nothing to redo".into()),
            Err(e) => self.status = Some(format!("redo: {e}")),
        }
    }

    /// Refresh the cached source and put the caret back where the step being
    /// undone/redone had it, clearing any active run.
    ///
    /// The caret comes from twig's blob for the restored state (what
    /// `record_caret` stored). `change` is only the fallback for a state with no
    /// blob — a caret at the end of the restored text, which is where this always
    /// landed before the blobs were kept. It is the edit site, not where the user
    /// was standing, so it's a floor and not the behaviour: undoing should hand
    /// back the document *and* the place you were working, which for an edit made
    /// anywhere but under the caret are two different places.
    fn after_history(&mut self, change: Change) {
        self.refresh();
        match self.editor.caret_blob().ok().and_then(|b| CaretState::from_blob(&b)) {
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

    // ── the file ──────────────────────────────────────────────────────────────

    #[cfg(feature = "fs")]
    pub fn save(&mut self) {
        if self.is_untitled() {
            // No path to write and no name to invent: ⌘S on an untitled document
            // is a Save As, and only a frontend has a picker to ask with. Say so
            // rather than failing at the filesystem with an empty path.
            self.status = Some("untitled — save as…".into());
            return;
        }
        let path = self.path.clone();
        if self.write(&path) {
            self.mark_saved();
        }
    }

    /// Save As: write the document to `path` and *move* it there — `self.path`
    /// becomes `path`, and every later [`Doc::save`] writes the new file. That's
    /// what Save As means; a copy would leave the user editing a document whose
    /// name is no longer where their keystrokes go.
    ///
    /// The move only happens if the bytes actually landed. A failed write leaves
    /// the path, `dirty`, and the disk watermark exactly as they were, with the
    /// same `save failed: …` status a failed [`Doc::save`] sets — the document
    /// must never come away believing it was saved.
    ///
    /// An existing `path` is overwritten, and the caller is the one that knows
    /// whether to ask first: a Save As picker has already run that prompt, and a
    /// second confirmation from down here would be the same question twice.
    ///
    /// `format` does **not** follow the new extension. The buffer is parsed as
    /// the format it was opened with, and re-reading it as another one is a
    /// conversion — a different, lossy operation that would throw away the undo
    /// history — not a rename. So `notes.md` saved as `notes.dj` holds Markdown
    /// in a `.dj` file, and `format_name()` keeps honestly saying `markdown`
    /// until it's reopened.
    #[cfg(feature = "fs")]
    pub fn save_as(&mut self, path: PathBuf) {
        if !self.write(&path) {
            return;
        }
        self.path = path;
        self.mark_saved();
    }

    /// Put `source` on disk at `path`, reporting whether it got there. The one
    /// place leaf writes a document, so a save and a Save As can't disagree
    /// about what a failure looks like.
    #[cfg(feature = "fs")]
    fn write(&mut self, path: &Path) -> bool {
        match std::fs::write(path, self.source.as_bytes()) {
            Ok(()) => true,
            Err(e) => {
                self.status = Some(format!("save failed: {e}"));
                false
            }
        }
    }

    /// Re-base the document's saved watermark to the current bytes: clears
    /// `dirty`, records `source` as the new clean state (so undoing back to here
    /// clears the flag again), and re-stamps the on-disk hash.
    ///
    /// [`Doc::save`]/[`Doc::save_as`] call this after a write lands. It is also
    /// the hook a **filesystem-free host** calls itself once it has persisted
    /// [`Doc::source`] its own way (a browser download, `localStorage`, a backend
    /// `PUT`) — which is why it is public and touches no filesystem: the bytes
    /// are already where that host wants them, and this just tells the model they
    /// are safe.
    pub fn mark_saved(&mut self) {
        self.clean_source = self.source.clone();
        self.dirty = false;
        // The bytes on disk are now ours, so this is the new watermark: without
        // re-stamping it, every save would report its own work as an external
        // change forever after.
        self.disk_hash = Some(hash_bytes(self.source.as_bytes()));
        self.status = Some(format!("saved {}", self.file_name()));
    }

    /// What the file looks like now against the bytes leaf last read or wrote.
    ///
    /// Reads the file and hashes it (see `disk_hash` for why it isn't an mtime),
    /// so this is a filesystem round-trip, not a per-frame question — ask it
    /// when a window regains focus, on a timer, or before a save.
    ///
    /// This *only* reports the file. Whether the document also has unsaved edits
    /// is `dirty`, and the interesting case is the conjunction: `dirty` plus
    /// [`DiskState::Changed`] means a save overwrites someone's work and a
    /// [`Doc::reload`] discards the user's. leaf-core deliberately won't choose —
    /// it has no way to ask — so it hands a frontend both halves and lets it put
    /// the question to the person who can answer it.
    #[cfg(feature = "fs")]
    pub fn disk_state(&self) -> DiskState {
        let Some(want) = self.disk_hash else {
            return DiskState::Untitled;
        };
        match std::fs::read(&self.path) {
            Ok(bytes) if hash_bytes(&bytes) == want => DiskState::Unchanged,
            Ok(_) => DiskState::Changed,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DiskState::Missing,
            Err(_) => DiskState::Unreadable,
        }
    }

    /// Re-read the file and replace the document with what's there — the other
    /// answer to a [`DiskState::Changed`].
    ///
    /// **Discards unsaved changes and the undo history, unconditionally.** It
    /// doesn't check `dirty` first: a frontend that wants to protect unsaved
    /// work asks (`dirty` + [`Doc::disk_state`]) *before* calling this, and one
    /// reloading a clean document shouldn't have to argue with a guard. The
    /// history goes because twig's undo stack belongs to the buffer, and these
    /// are different bytes — replaying a step recorded against the old ones onto
    /// them would corrupt the document, and nothing here can honestly rebase it.
    ///
    /// The caret keeps its byte offset, clamped to the new length; the selection
    /// is dropped. Anything cleverer would be a lie: leaf doesn't know how the
    /// file changed, so it can't know where the caret "still" is. Clamping keeps
    /// it where the user left it in the common case (a change further down the
    /// file, or none in the text they're sitting in), and never puts it
    /// somewhere invalid. A selection has two such offsets and no such excuse —
    /// silently reinterpreting one over changed bytes would arm the *next*
    /// keystroke to delete something the user never selected.
    ///
    /// Nothing is touched unless the whole reload succeeds; a failure leaves the
    /// document alone with a status.
    #[cfg(feature = "fs")]
    pub fn reload(&mut self) {
        if self.is_untitled() {
            self.status = Some("no file to reload".into());
            return;
        }
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) => {
                self.status = Some(format!("reload failed: {e}"));
                return;
            }
        };
        let Ok(source) = String::from_utf8(bytes) else {
            self.status = Some("reload failed: file is not UTF-8".into());
            return;
        };
        // Reparse rather than splice the difference in: leaf doesn't know what
        // changed, and `format` is the format this document is, not what the
        // (unchanged) name now says — see `save_as`.
        let editor = match new_editor(source.as_bytes(), self.format) {
            Ok(ed) => ed,
            Err(e) => {
                self.status = Some(format!("reload failed: {e}"));
                return;
            }
        };
        self.editor = editor;
        self.disk_hash = Some(hash_bytes(source.as_bytes()));
        self.clean_source = source.clone();
        self.source = source;
        // Reload replaces the text without going through `refresh`, so it has to
        // move the revision itself or every frontend would keep painting the old
        // file from cache.
        self.revision += 1;
        self.caret = self.caret.min(self.source.len());
        self.anchor = None;
        self.goal_col = None;
        self.last_edit_kind = None;
        self.dirty = false;
        self.status = Some(format!("reloaded {}", self.file_name()));
        self.clamp_caret();
    }

    /// Re-read the source from twig after it has changed the document. The one
    /// funnel every edit, undo, and redo comes through — so it's where the
    /// revision moves, and anything cached against the text dies here.
    fn refresh(&mut self) {
        if let Ok(s) = self.editor.source_str() {
            self.source = s;
        }
        self.revision += 1;
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
        let before = self.caret;
        // A pixel hit-test can land between the visible caret stops — in the
        // blank gap a paragraph break is drawn with, or inside a hidden delimiter.
        // Snap to the nearest real stop so the caret can't come to rest where it
        // would draw in one place and type in another. The `(row, col)` click
        // path (`click`) already snaps this way through `offset_of_pos`; the
        // source view reaches every byte, so it snaps to nothing.
        let target = match self.view {
            View::Wysiwyg => self.vmap.snap_to_stop(offset.min(self.source.len())),
            View::Source => offset,
        };
        self.move_to(target, extend);
        self.clamp_caret();
        self.debug_assert_on_a_stop(before);
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

    // Up and Down run off the ends of the document rather than stopping dead at
    // them: Up from the first row lands at the document's start, Down from the
    // last at its end. That's Cocoa's rule (`moveUp:`/`moveDown:` past the edge
    // are `moveToBeginningOfDocument:`/`moveToEndOfDocument:`), and holding ↓
    // reaching the end of the text is what a reader means by it.
    //
    // The views used to disagree here by accident rather than by decision: the
    // source view fell into the edge behaviour through `row_col_to_offset`
    // clamping an out-of-range row to the end of the string, while WYSIWYG had
    // no row below to walk to and did nothing at all. They share the rule now,
    // each in its own space — the source view reaches every byte, WYSIWYG only
    // the offsets it draws.

    pub fn move_up(&mut self, extend: bool) {
        let (row, col) = self.caret_pos();
        let goal = self.goal_col.unwrap_or(col);
        let target = match self.view {
            View::Source => match row.checked_sub(1) {
                Some(r) => row_col_to_offset(&self.source, r, goal),
                None => self.reachable_start(),
            },
            // A table's border rules are drawn but hold no caret, so Up steps
            // over them to the row that does.
            View::Wysiwyg => match self.vmap.navigable_above(row) {
                Some(r) => self.row_target(r, goal),
                None => self.reachable_start(),
            },
        };
        self.step_vertical(target, goal, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        let (row, col) = self.caret_pos();
        let goal = self.goal_col.unwrap_or(col);
        let target = match self.view {
            View::Source => match self.source_row_below(row) {
                Some(r) => row_col_to_offset(&self.source, r, goal),
                None => self.reachable_end(),
            },
            View::Wysiwyg => match self.vmap.navigable_below(row) {
                Some(r) => self.row_target(r, goal),
                None => self.reachable_end(),
            },
        };
        self.step_vertical(target, goal, extend);
    }

    /// Land a vertical motion at `target`, latching the `goal` column it aimed
    /// with so the rest of the run keeps aiming there.
    ///
    /// A motion with nowhere to go changes *nothing*, the goal column included:
    /// the latch used to run before the early return at the top of the document,
    /// so an Up that did nothing still armed a column, and the next Down aimed
    /// at one the caret had never been in.
    fn step_vertical(&mut self, target: usize, goal: usize, extend: bool) {
        let before = self.caret;
        if target == before {
            return;
        }
        self.goal_col = Some(goal);
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// The source line below `row`, or `None` when `row` is the last one. Lines
    /// are counted by newline, so a trailing one leaves a real, empty last line
    /// for the caret to sit on — the document ends below it, not on it.
    fn source_row_below(&self, row: usize) -> Option<usize> {
        let last = self.source.bytes().filter(|&b| b == b'\n').count();
        (row < last).then_some(row + 1)
    }

    /// Where a vertical motion aiming at the `goal` column lands on visual row
    /// `r`: the column clamped to the row, mapped to its offset, then held
    /// inside the row's own [bounds](Self::row_bounds) — a wrapped row's last
    /// column belongs to the row below, and a gutter's column 0 points at the
    /// block rather than at this row.
    fn row_target(&self, r: usize, goal: usize) -> usize {
        let (start, end) = self.row_bounds(r);
        self.vmap
            .offset_of_pos(r, goal.min(self.vmap.row_width(r)))
            .clamp(start, end)
    }

    /// The first and last offsets the caret can reach in the active view.
    ///
    /// Not the same span in both: the source view shows every byte, so it can
    /// reach every byte. WYSIWYG reaches only what it draws — hidden frontmatter
    /// sits below the first stop, and a document's trailing newline is drawn
    /// nowhere and so sits past the last.
    fn reachable_start(&self) -> usize {
        match self.view {
            View::Source => 0,
            View::Wysiwyg => self.vmap.stop_at_or_after(0).unwrap_or(self.caret),
        }
    }

    fn reachable_end(&self) -> usize {
        match self.view {
            View::Source => self.source.len(),
            View::Wysiwyg => self.vmap.stop_at_or_before(self.source.len()).unwrap_or(self.caret),
        }
    }

    /// The `[start, end]` offsets visual row `r` *draws* — everything on it,
    /// including the space a soft wrap ate off its end, which is drawn on this
    /// row however much the offset past it belongs to the next one.
    fn row_span(&self, r: usize) -> (usize, usize) {
        let start = self
            .vmap
            .row_start(r)
            .unwrap_or_else(|| self.vmap.offset_of_pos(r, 0));
        let end = self.vmap.offset_of_pos(r, self.vmap.row_width(r));
        (start.min(end), end)
    }

    /// [`row_span`](Self::row_span) narrowed to where the caret can stand: a
    /// soft wrap's shared offset opens the row below (see `pos_of_offset`), so
    /// this row's last position is the one before it — the offset before the
    /// space the wrap ate, where the caret draws just past the row's last word
    /// and types there too.
    ///
    /// Aiming at the shared offset instead is what stalled End: it is the row's
    /// last *column*, so End pressed on the row reached it and then read back as
    /// the row below's start, where a second press ran on to that row's end and
    /// the next to the one after — End walking down the paragraph a row a press.
    fn row_bounds(&self, r: usize) -> (usize, usize) {
        let (start, end) = self.row_span(r);
        let wraps = self
            .vmap
            .navigable_below(r)
            .and_then(|b| self.vmap.row_start(b))
            .is_some_and(|off| off == end);
        match wraps {
            true => (start, self.vmap.stop_before(end).unwrap_or(end).max(start)),
            false => (start, end),
        }
    }

    /// The `[start, end]` of the line Home and End aim at: the visual row in
    /// WYSIWYG, the logical line in the source view. Both ends are caret stops.
    ///
    /// A soft-wrapped row is a line here, because it is one to the eye and the
    /// eye is what these keys are aimed by — a reader pressing End means the end
    /// of the line they can see. (`select_block_at` wants the opposite and reads
    /// the AST for it: a triple-click grabs the whole paragraph, however many
    /// rows it folds into.)
    fn line_bounds(&self) -> (usize, usize) {
        let (row, _) = self.caret_pos();
        match self.view {
            View::Source => {
                let start = line_start(&self.source, row);
                (start, line_end_from(&self.source, start))
            }
            View::Wysiwyg => self.row_bounds(row),
        }
    }

    /// The same line as [`line_bounds`](Self::line_bounds), as far as it is
    /// *drawn* — what a kill takes.
    ///
    /// The two part only at a soft wrap, over the space the wrap ate: the caret
    /// can't stand after it (that offset opens the row below, and End stopping
    /// there would walk), but it is on this row, and a kill that spared it would
    /// leave a double space behind where the row's text had been. Deleting it
    /// joins nothing — a wrap is drawn, not written.
    fn line_span(&self) -> (usize, usize) {
        let (row, _) = self.caret_pos();
        match self.view {
            View::Source => self.line_bounds(),
            View::Wysiwyg => self.row_span(row),
        }
    }

    /// The first offset in `[start, end]` holding something other than
    /// whitespace, or `end` when the line holds nothing else — where Home aims.
    ///
    /// Walks the space the view is in, as word motion does: WYSIWYG steps stops,
    /// so a hidden delimiter is never taken for the line's first character (nor
    /// landed on), and the source view steps the source it is showing.
    fn first_non_space(&self, start: usize, end: usize) -> usize {
        let mut off = start;
        while off < end {
            if self.class_at(off) != Class::Space {
                return off;
            }
            off = match self.view {
                View::Source => next_boundary(&self.source, off),
                View::Wysiwyg => match self.vmap.stop_after(off) {
                    Some(next) => next,
                    None => return end,
                },
            };
        }
        end
    }

    /// Home: to the first character on the line, or to column 0 when the caret
    /// is already on it — the two-press toggle every editor spells this way.
    /// The indentation is somewhere the caret has to be able to reach and almost
    /// never where a reader is headed, so it costs the second press.
    pub fn move_home(&mut self, extend: bool) {
        self.goal_col = None;
        let (start, end) = self.line_bounds();
        let text = self.first_non_space(start, end);
        let target = if self.caret == text { start } else { text };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// End: to the end of the line.
    pub fn move_end(&mut self, extend: bool) {
        self.goal_col = None;
        let (_, end) = self.line_bounds();
        let before = self.caret;
        self.move_to(end, extend);
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

    #[test]
    fn blockquote_after_a_list_is_not_bulleted() {
        // twig nests a following top-level block quote under the `bullet_list`
        // (a direct child, not a `list_item`). The map must render it de-nested —
        // `│ quote`, never `• │ quote` — with a blank separator, like any block
        // that follows a list. Regression for the "combined list + blockquote" bug.
        let mut d = doc_in(View::Wysiwyg, "bq_after_list", "- item\n\n> quote\n");
        d.build_visual(80);
        let rows: Vec<String> = d
            .vmap
            .rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
            .collect();
        assert!(
            rows.iter().any(|r| r == "│ quote"),
            "block quote should render on its own gutter, got rows: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains('•') && r.contains('│')),
            "no row should carry both a bullet and a quote gutter, got rows: {rows:?}"
        );
    }

    // ── the map is built at most once per (revision, wrap) ───────────────────
    //
    // A frontend repaints for reasons that have nothing to do with the text — a
    // blinking caret, a scroll — and rebuilding the map is O(document). These
    // pin *that the cache fires*, which a passing suite can't tell you: a cache
    // that never hits is invisible to every other test in this file.
    //
    // The probe is to wreck the built map and ask for it again. A rebuild
    // repairs it; a cache hit hands the wreckage straight back. Nothing else
    // can distinguish the two from outside.

    #[test]
    fn a_rebuild_with_nothing_changed_reuses_the_map() {
        let mut d = doc_in(View::Wysiwyg, "cache_hit", "# Title\n\nbody\n");
        d.build_visual(80);
        assert!(!d.vmap.rows.is_empty());
        d.vmap.rows.clear(); // wreck it
        d.build_visual(80);
        assert!(
            d.vmap.rows.is_empty(),
            "the map was rebuilt though nothing changed — the cache never fired"
        );
    }

    #[test]
    fn an_edit_rebuilds_the_map() {
        let mut d = doc_in(View::Wysiwyg, "cache_edit", "# Title\n\nbody\n");
        d.build_visual(80);
        let before = d.revision();
        d.vmap.rows.clear();
        d.insert("x");
        d.build_visual(80);
        assert!(d.revision() > before, "an edit must move the revision");
        assert!(
            !d.vmap.rows.is_empty(),
            "an edited document must not paint from a stale map"
        );
    }

    #[test]
    fn a_width_change_rebuilds_the_map() {
        // The map is a function of the wrap width too, so a resize is a miss
        // even though the text is untouched.
        let mut d = doc_in(View::Wysiwyg, "cache_width", "one two three four five six\n");
        d.build_visual(80);
        d.vmap.rows.clear();
        d.build_visual(12);
        assert!(!d.vmap.rows.is_empty(), "a resize must rebuild the map");
        // And the unwrapped map is its own key, not the same as any width.
        d.vmap.rows.clear();
        d.build_visual_unwrapped();
        assert!(!d.vmap.rows.is_empty(), "unwrapped is a different map");
    }

    #[test]
    fn a_motion_does_not_rebuild_the_map() {
        // The whole point: moving the caret changes nothing the map is built
        // from. If a motion bumped the revision, every arrow key would cost a
        // full rebuild and the cache would be worthless.
        let mut d = doc_in(View::Wysiwyg, "cache_motion", "# Title\n\nbody text\n");
        d.build_visual(80);
        let rev = d.revision();
        d.move_right(false);
        d.move_right(true);
        d.move_down(false);
        assert_eq!(d.revision(), rev, "a motion must not move the revision");
        d.vmap.rows.clear();
        d.build_visual(80);
        assert!(d.vmap.rows.is_empty(), "a motion should not rebuild the map");
    }

    #[test]
    fn saving_does_not_rebuild_the_map() {
        // Saving changes `dirty`, not the text.
        let mut d = doc_in(View::Wysiwyg, "cache_save", "# Title\n\nbody\n");
        d.insert("x");
        d.build_visual(80);
        let rev = d.revision();
        d.save();
        assert_eq!(d.revision(), rev, "a save must not move the revision");
        assert!(!d.dirty, "the save should have cleaned the document");
    }

    #[test]
    fn a_reload_rebuilds_the_map() {
        // Reload replaces the text without going through `refresh`, so it has to
        // move the revision itself — else the editor paints the old file.
        let mut d = doc_in(View::Wysiwyg, "cache_reload", "# Title\n\nbody\n");
        d.build_visual(80);
        let rev = d.revision();
        std::fs::write(&d.path, "# Other\n\nwholly new\n").unwrap();
        d.reload();
        assert!(d.revision() > rev, "a reload must move the revision");
        d.build_visual(80);
        let text: String = d
            .vmap
            .rows
            .iter()
            .flat_map(|r| r.glyphs.iter().map(|g| g.ch))
            .collect();
        assert!(
            text.contains("wholly new"),
            "the reloaded text should be on screen, got {text:?}"
        );
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

    // ── Home / End ───────────────────────────────────────────────────────────

    #[test]
    fn home_toggles_between_the_line_s_text_and_its_margin() {
        // Source: the indentation is what the toggle is for. WYSIWYG resolves an
        // indent to the markup it spells everywhere it means one, so the fixture
        // with whitespace left to walk is a code block, which is verbatim.
        let g = |m, f: fn(&mut Doc)| golden("smart_home", m, f);
        assert_eq!(g("    inden|ted", |d| d.move_home(false)), "    |indented");
        assert_eq!(g("    |indented", |d| d.move_home(false)), "|    indented");
        assert_eq!(g("|    indented", |d| d.move_home(false)), "    |indented");
        // A line with no indentation has one place to go, so the toggle is a
        // no-op rather than a trip to nowhere.
        assert_eq!(g("hel|lo", |d| d.move_home(false)), "|hello");
        assert_eq!(g("|hello", |d| d.move_home(false)), "|hello");

        let mut d = wysiwyg_doc("smart_home_wys", "```\n    indented\n```\n");
        let indent = d.source.find("    indented").unwrap();
        d.caret = indent + 6; // inside "indented"
        d.move_home(false);
        assert_eq!(d.caret, indent + 4, "wysiwyg: Home aims at the code line's text");
        d.move_home(false);
        assert_eq!(d.caret, indent, "wysiwyg: the second press takes the indent");
        d.move_home(false);
        assert_eq!(d.caret, indent + 4, "wysiwyg: the toggle swaps back");
    }

    #[test]
    fn end_takes_the_line_the_view_is_showing() {
        // The line differs by view for the same document, and that is the point:
        // a bare newline inside a paragraph is a soft break, which WYSIWYG draws
        // as a space on one row and the source view as two lines.
        let mut d = doc_with("end_src", "one two\nthree\n");
        d.caret = 1;
        d.move_end(false);
        assert_eq!(d.caret, 7, "source: the end of the source line");

        let mut d = wysiwyg_doc("end_wys", "one two\nthree\n");
        d.caret = 1;
        d.move_end(false);
        assert_eq!(d.caret, 13, "wysiwyg: the end of the row, soft break and all");
    }

    #[test]
    fn home_and_end_extend_the_selection_when_asked() {
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("home_end_ext_{tag}"), "hello world");
            d.caret = 6;
            d.move_end(true);
            assert_eq!(d.selection(), Some((6, 11)), "{tag}: End extends");
            let mut d = doc_in(view, &format!("home_ext_{tag}"), "hello world");
            d.caret = 6;
            d.move_home(true);
            assert_eq!(d.selection(), Some((0, 6)), "{tag}: Home extends");
        }
    }

    // ── kill to the line's start / end ───────────────────────────────────────

    #[test]
    fn kill_to_the_line_start_and_end_in_both_views() {
        for (view, tag) in VIEWS {
            // The gap that reads as a paragraph break in each view: the source
            // view's lines are the renderer's rows only where the source says so.
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let mut d = doc_in(view, &format!("kill_end_{tag}"), &format!("one two{gap}three\n"));
            d.caret = 3;
            d.delete_to_line_end();
            assert_eq!(d.source, format!("one{gap}three\n"), "{tag}: ^K to the line's end");
            assert_eq!(d.caret, 3, "{tag}: the caret stays where it kills from");

            let mut d = doc_in(view, &format!("kill_start_{tag}"), &format!("one two{gap}three\n"));
            d.caret = 7; // the end of the first line
            d.delete_to_line_start();
            assert_eq!(d.source, format!("{gap}three\n"), "{tag}: ⌘⌫ to the line's start");
            assert_eq!(d.caret, 0, "{tag}");
        }
    }

    #[test]
    fn a_kill_at_the_line_s_edge_leaves_the_lines_joined() {
        // The decision: at the boundary both kills do nothing, rather than
        // eating the line break. "Line" is the view's own — in WYSIWYG it ends
        // at a soft wrap as often as at a newline, where there is nothing
        // written to delete — and a source newline is only half of the blank
        // line between two paragraphs, so taking it leaves a soft break rather
        // than the join it looks like. Backspace and Delete are the keys for it.
        for (view, tag) in VIEWS {
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let src = format!("one{gap}three\n");
            let mut d = doc_in(view, &format!("kill_edge_end_{tag}"), &src);
            d.caret = 3; // the end of "one"
            d.delete_to_line_end();
            assert_eq!(d.source, src, "{tag}: ^K at the line's end joined it to the next");

            let mut d = doc_in(view, &format!("kill_edge_start_{tag}"), &src);
            d.caret = 3 + gap.len(); // the start of "three"
            d.delete_to_line_start();
            assert_eq!(d.source, src, "{tag}: ⌘⌫ at the line's start joined it to the last");
        }
    }

    #[test]
    fn a_kill_takes_the_selection_when_there_is_one() {
        // What every other delete here does with one, so these two as well.
        for (view, tag) in VIEWS {
            for (name, kill) in [
                ("end", (|d: &mut Doc| d.delete_to_line_end()) as fn(&mut Doc)),
                ("start", |d: &mut Doc| d.delete_to_line_start()),
            ] {
                let mut d = doc_in(view, &format!("kill_sel_{name}_{tag}"), "one two three\n");
                d.anchor = Some(4);
                d.caret = 7; // "two"
                kill(&mut d);
                assert_eq!(d.source, "one  three\n", "{tag}: {name} ignored the selection");
                assert_eq!(d.selection(), None, "{tag}: {name}");
            }
        }
    }

    #[test]
    fn a_kill_takes_the_markup_it_empties_with_it() {
        // The same hazard a word-delete has: a WYSIWYG range covers what the
        // user can see, which for `**bold**` is the word and never the
        // delimiters, so a kill that stopped at the text would leave `a ****` —
        // markup wrapped around nothing.
        let mut d = wysiwyg_doc("kill_widen", "a **bold**\n");
        d.caret = d.source.find("bold").unwrap();
        d.delete_to_line_end();
        assert_eq!(d.source, "a \n");
    }

    #[test]
    fn a_kill_is_undone_in_one_step() {
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("kill_undo_{tag}"), "one two three\n");
            d.caret = 3;
            d.delete_to_line_end();
            assert_eq!(d.source, "one\n", "{tag}");
            d.undo();
            assert_eq!(d.source, "one two three\n", "{tag}: a kill takes one undo");
        }
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
    fn wysiwyg_enter_does_not_mistake_a_setext_underline_for_a_list() {
        // `text\n- \n` is a setext heading — the `- ` is its underline, not a
        // list item, though it reads as a `- ` marker byte-for-byte. Enter must
        // not take the list-exit path (which would splice the `- ` away as if
        // leaving an empty item); the AST guard sends it to a normal break and
        // leaves the underline intact.
        let mut d = wysiwyg_doc("wys_setext", "text\n- \n");
        assert!(
            d.nodes().iter().any(|n| n.kind == "heading"),
            "precondition: twig parses this as a heading, not a list",
        );
        d.caret = 7; // on the `- ` underline line
        d.newline();
        assert!(
            d.source.contains("- "),
            "the setext underline survives, not spliced away as a list item: {:?}",
            d.source,
        );
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
    fn insert_image_at_the_caret_spells_the_markup_and_lands_past_it() {
        let mut d = doc_with("img_caret", "before after\n");
        d.caret = 7; // between "before " and "after"
        d.insert_image("cat.png", "a cat");
        assert_eq!(d.source, "before ![a cat](cat.png)after\n");
        // The caret sits just past the inserted image, nothing selected.
        assert_eq!(d.selection(), None);
        assert_eq!(d.caret, 7 + "![a cat](cat.png)".len());
    }

    #[test]
    fn insert_image_uses_the_selection_as_alt_text() {
        let mut d = doc_with("img_sel", "caption here\n");
        d.anchor = Some(0);
        d.caret = 7; // "caption"
        d.insert_image("p.png", "ignored fallback");
        assert_eq!(d.source, "![caption](p.png) here\n");
    }

    #[test]
    fn insert_image_with_no_alt_leaves_empty_brackets() {
        let mut d = doc_with("img_noalt", "\n");
        d.caret = 0;
        d.insert_image("logo.svg", "");
        assert_eq!(d.source, "![](logo.svg)\n");
    }

    #[test]
    fn image_destination_at_caret_reads_the_image_under_the_caret() {
        let mut d = doc_with("img_read", "![a cat](cat.png)\n");
        d.caret = 3; // inside the image markup
        assert_eq!(d.image_destination_at_caret(), Some("cat.png".to_string()));
        // Past the image, the caret is in no image.
        d.caret = "![a cat](cat.png)".len();
        assert_eq!(d.image_destination_at_caret(), None);
    }

    #[test]
    fn set_image_rows_reserves_blank_filler_rows_the_frontend_paints_over() {
        // The image is one placeholder row by default, and `set_image_rows` grows
        // it to the height the frontend measured: the label row plus blank
        // `decoration` fillers that hold the vertical space a raster is drawn into.
        let mut d = wysiwyg_doc("img_rows", "intro\n\n![a cat](cat.png)\n\nend\n");
        assert_eq!(d.vmap.images.len(), 1);
        let img_row = d.vmap.images[0].rows_span.start;
        assert_eq!(d.vmap.images[0].rows_span, img_row..img_row + 1, "default is one row");

        d.set_image_rows(HashMap::from([("cat.png".to_string(), 4)]));
        d.build_visual(80);
        assert_eq!(d.vmap.images.len(), 1, "still one image, now taller");
        let span = d.vmap.images[0].rows_span.clone();
        assert_eq!(span.end - span.start, 4, "reserves the four rows asked for");
        // The label row carries the mark and its glyphs; the three below are blank
        // decoration — drawn, but no caret and no text.
        assert!(d.vmap.rows[span.start].image.is_some(), "mark rides the first row");
        for r in (span.start + 1)..span.end {
            assert!(d.vmap.rows[r].decoration, "filler row {r} is decoration");
            assert!(d.vmap.rows[r].glyphs.is_empty(), "filler row {r} is blank");
            assert!(d.vmap.rows[r].image.is_none(), "only the first row is marked");
        }
    }

    #[test]
    fn a_taller_image_adds_no_caret_stops_and_motion_steps_over_its_fillers() {
        // The extra rows are pure spacers: the caret's only homes stay the stop in
        // front of the image and the one just past it, so walking the document top
        // to bottom visits the same offsets whether the image is 1 row or 5.
        let body = "ab\n\n![x](p.png)\n\ncd\n";
        let stops_at = |rows: usize| -> Vec<usize> {
            let mut d = wysiwyg_doc("img_stops", body);
            if rows > 1 {
                d.set_image_rows(HashMap::from([("p.png".to_string(), rows)]));
                d.build_visual(80);
            }
            d.caret = 0;
            let mut seen = vec![d.caret];
            loop {
                d.move_right(false);
                if *seen.last().unwrap() == d.caret {
                    break;
                }
                seen.push(d.caret);
            }
            seen
        };
        assert_eq!(stops_at(1), stops_at(5), "reserving rows must not add stops");
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
    fn code_language_reads_and_edits_through_the_fence() {
        let mut d = doc_with("code_lang", "```rust\nlet x = 1;\n```\n");
        d.caret = 10; // inside the code body
        assert_eq!(d.code_language_at_caret().as_deref(), Some("rust"));
        assert!(d.caret_in_fenced_code());

        d.set_code_language("python");
        assert!(d.source.starts_with("```python\n"), "source: {:?}", d.source);
        assert_eq!(d.code_language_at_caret().as_deref(), Some("python"));

        // Clearing it leaves a bare fence and no label.
        d.set_code_language("");
        assert!(d.source.starts_with("```\n"), "source: {:?}", d.source);
        assert_eq!(d.code_language_at_caret(), None);

        // A caret outside any code block edits nothing.
        let mut p = doc_with("code_lang_none", "just prose\n");
        assert!(!p.caret_in_fenced_code());
        p.set_code_language("rust");
        assert_eq!(p.source, "just prose\n");
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

    // A pixel-hit-test placement (the GUI's `place_caret`) must land on a caret
    // stop just as the `(row, col)` click path does, so the caret can never come
    // to rest in the blank gap between two paragraphs — where it would draw in one
    // place and type in another.
    #[test]
    fn place_caret_snaps_out_of_the_blank_gap_between_paragraphs() {
        // "A\n\nB": offset 2 is the gap the paragraph break is drawn with, not a
        // caret stop (stops are 0,1,3,4).
        let mut d = wysiwyg_doc("place_gap", "A\n\nB");
        assert!(!d.vmap.is_stop(2), "offset 2 should be an unreachable gap");
        d.place_caret(2, false);
        assert!(d.vmap.is_stop(d.caret), "caret {} is not a stop", d.caret);
        assert_eq!(d.caret, 1, "should snap to the end of the paragraph above");
    }

    #[test]
    fn place_caret_dragging_through_the_gap_keeps_selection_on_stops() {
        let mut d = wysiwyg_doc("place_gap_drag", "A\n\nB");
        d.place_caret(0, false); // anchor at the start of "A"
        d.place_caret(2, true); // drag into the gap
        assert!(d.vmap.is_stop(d.caret), "caret {} is not a stop", d.caret);
        let (s, e) = d.selection().expect("a selection");
        assert!(d.vmap.is_stop(s) && d.vmap.is_stop(e), "selection {s}..{e} off a stop");
    }

    #[test]
    fn place_caret_on_a_real_stop_is_left_untouched() {
        let mut d = wysiwyg_doc("place_stop", "A\n\nB");
        d.place_caret(3, false); // the start of "B" — a genuine stop
        assert_eq!(d.caret, 3);
    }

    // An *empty paragraph* (two blank lines, an intentional blank line the user
    // opened) is a real caret stop, unlike the gap — a click into it must stay.
    #[test]
    fn place_caret_rests_in_an_empty_paragraph() {
        let mut d = wysiwyg_doc("place_empty_para", "A\n\n\n\nB");
        let empty = 3; // the navigable empty row's offset (stops: 0,1,3,5,6)
        assert!(d.vmap.is_stop(empty));
        d.place_caret(empty, false);
        assert_eq!(d.caret, empty);
    }

    fn wysiwyg_doc(name: &str, body: &str) -> Doc {
        doc_in(View::Wysiwyg, name, body)
    }

    /// A from-scratch, cache-free WYSIWYG map for `source` — the ground truth the
    /// incremental (`build_spliced` / `build_cached`) path must always match.
    fn reference_map(source: &str) -> crate::wysiwyg::VisualMap {
        let mut ed = twig::Editor::new_str(source, Format::Markdown).unwrap();
        let nodes = ed.nodes().unwrap();
        crate::wysiwyg::build(&nodes, source, None, &std::collections::HashMap::new())
    }

    fn maps_differ(a: &crate::wysiwyg::VisualMap, b: &crate::wysiwyg::VisualMap) -> bool {
        if a.rows.len() != b.rows.len() {
            return true;
        }
        for (ra, rb) in a.rows.iter().zip(&b.rows) {
            if ra.end_src != rb.end_src || ra.glyphs.len() != rb.glyphs.len() {
                return true;
            }
            for (ga, gb) in ra.glyphs.iter().zip(&rb.glyphs) {
                if ga.ch != gb.ch || ga.src != gb.src {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn incremental_build_matches_a_fresh_build_across_edits() {
        // Every `Doc` edit rebuilds through `build_spliced` (the single-block
        // fast path, gated on twig's `dirty_range`) or falls back to
        // `build_cached`. After each edit the map must be byte-identical to a
        // from-scratch build — this is the correctness net under the splice.
        let docs = [
            "# Title\n\nThe quick brown fox jumps.\n\nAnother paragraph here.\n\n- a\n- b\n",
            "para one\n\n> quote **bold** text\n> continued line\n\ntail paragraph\n",
            "alpha\n\nbeta\n\ngamma\n\ndelta\n\nepsilon\n\nzeta\n",
        ];
        // A deterministic mix: mostly single characters (which stay inside one
        // block → splice), plus edits that reshape structure (a paragraph break,
        // a heading marker, a code fence → fallback), so both paths are exercised.
        let inserts = ["x", "y", "\n\n", "#", "`", " ", "z"];
        for src in docs {
            let mut d = wysiwyg_doc("diff", src);
            d.build_visual_unwrapped();
            wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "initial");

            for step in 0..60usize {
                let len = d.source.len();
                let raw = (step * 13 + 5) % (len + 1);
                let pos = (raw..=len).find(|&i| d.source.is_char_boundary(i)).unwrap();
                let pre = d.source.clone();
                let action;
                if step % 3 == 0 && pos < len {
                    let end = (pos + 1..=len).find(|&i| d.source.is_char_boundary(i)).unwrap();
                    action = format!("delete [{pos},{end})");
                    d.edit(pos, end, "");
                } else {
                    let ins = inserts[step % inserts.len()];
                    action = format!("insert {ins:?} @ {pos}");
                    d.edit(pos, pos, ins);
                }
                d.build_visual_unwrapped();
                if maps_differ(&d.vmap, &reference_map(&d.source)) {
                    panic!(
                        "FIRST MISMATCH at step {step}: {action}\n  pre  = {pre:?}\n  post = {:?}",
                        d.source
                    );
                }
            }
        }
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
        // The second Up and the second Down here run off the ends of the
        // document, which is no longer a place a press is swallowed: they carry
        // the caret to the start and the end of the text. The claim in the
        // middle — that a Down retraces the Up that crossed the paragraph gap —
        // is the one this test is for, and it is asserted where it is made.
        let mut d = wysiwyg_doc("wys_updown", "abc\n\ndef\n");
        d.caret = 5; // start of "def"
        let start = d.caret_pos();
        d.move_up(false);
        assert_eq!(d.caret_pos().0, 0, "Up reaches the first paragraph");
        d.move_up(false);
        assert_eq!(d.caret, 0, "a second Up runs on to the document's start");
        d.move_down(false);
        assert_eq!(d.caret_pos(), start, "Down retraces Up exactly");
        d.move_down(false);
        assert_eq!(d.caret, 8, "a second Down runs on to the document's end");
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

    // ── IME composition ──────────────────────────────────────────────────────

    #[test]
    fn a_composition_run_undoes_as_one_step() {
        let mut d = doc_with("compose", "\n");
        d.caret = 0;
        // What an IME does: each step replaces the last one's provisional bytes.
        d.edit_composing(0, 0, "k");
        d.edit_composing(0, 1, "か");
        d.edit_composing(0, 3, "かん");
        d.edit_composing(0, 6, "感"); // the commit
        d.end_composition();
        assert_eq!(d.source, "感\n");
        d.undo(); // the whole composition, not its last keystroke
        assert_eq!(d.source, "\n");
        assert_eq!(d.status.as_deref(), None, "the run was a single step");
    }

    #[test]
    fn two_compositions_are_two_undo_steps() {
        let mut d = doc_with("compose_two", "\n");
        d.caret = 0;
        d.edit_composing(0, 0, "か");
        d.edit_composing(0, 3, "蚊");
        d.end_composition();
        d.edit_composing(3, 3, "き");
        d.edit_composing(3, 6, "木");
        d.end_composition();
        assert_eq!(d.source, "蚊木\n");
        d.undo();
        assert_eq!(d.source, "蚊\n", "only the second composition");
        d.undo();
        assert_eq!(d.source, "\n");
    }

    #[test]
    fn a_composition_does_not_fold_into_the_typing_around_it() {
        let mut d = doc_with("compose_typing", "\n");
        d.caret = 0;
        d.insert("a");
        d.insert("b");
        d.edit_composing(2, 2, "か");
        d.edit_composing(2, 5, "蚊");
        d.end_composition();
        d.insert("c");
        assert_eq!(d.source, "ab蚊c\n");
        d.undo();
        assert_eq!(d.source, "ab蚊\n");
        d.undo();
        assert_eq!(d.source, "ab\n");
        d.undo();
        assert_eq!(d.source, "\n");
    }

    #[test]
    fn ending_a_composition_that_never_began_leaves_a_typing_run_alone() {
        let mut d = doc_with("compose_spurious", "\n");
        d.caret = 0;
        d.insert("a");
        d.end_composition(); // an IME unmarking unprompted
        d.insert("b");
        assert_eq!(d.source, "ab\n");
        d.undo();
        assert_eq!(d.source, "\n", "still one typed run");
    }

    // ── the clipboard's rich flavor ──────────────────────────────────────────

    #[test]
    fn an_inline_selection_publishes_html_without_a_paragraph_wrapper() {
        let mut d = doc_with("sel_inline", "a **bold** c\n");
        d.anchor = Some(2);
        d.caret = 10; // `**bold**`, inside the paragraph
        assert_eq!(d.selection_html().as_deref(), Some("<strong>bold</strong>"));
    }

    #[test]
    fn a_whole_block_selection_keeps_its_paragraph() {
        let mut d = doc_with("sel_block", "a **bold** c\n");
        d.anchor = Some(0);
        d.caret = 12; // the entire paragraph
        assert_eq!(
            d.selection_html().as_deref(),
            Some("<p>a <strong>bold</strong> c</p>")
        );
    }

    #[test]
    fn a_multi_block_selection_keeps_its_structure() {
        let mut d = doc_with("sel_multi", "para\n\n- one\n- two\n");
        d.select_all();
        let html = d.selection_html().expect("renders");
        assert!(html.contains("<p>para</p>"), "{html:?}");
        assert!(html.contains("<li>one</li>"), "{html:?}");
    }

    #[test]
    fn a_word_inside_a_heading_publishes_as_text_not_a_heading() {
        // The fragment `Head` is a paragraph standalone; the *document* says it
        // sits inside one block, so the wrapper is an artifact either way.
        let mut d = doc_with("sel_heading", "# Head line\n");
        d.anchor = Some(2);
        d.caret = 6;
        assert_eq!(d.selection_html().as_deref(), Some("Head"));
    }

    #[test]
    fn no_selection_publishes_no_html() {
        let mut d = doc_with("sel_none", "a b\n");
        d.caret = 1;
        assert_eq!(d.selection_html(), None);
    }

    #[test]
    fn pasting_html_converts_it_and_is_one_undo_step() {
        let mut d = doc_with("paste_html", "x\n");
        d.caret = 1;
        assert!(d.paste_html("<p>a <strong>b</strong> c</p>"));
        assert_eq!(d.source, "xa **b** c\n");
        d.undo();
        assert_eq!(d.source, "x\n", "the whole paste, in one step");
    }

    #[test]
    fn pasting_html_replaces_the_selection() {
        let mut d = doc_with("paste_html_sel", "keep drop\n");
        d.anchor = Some(5);
        d.caret = 9;
        assert!(d.paste_html("<em>new</em>"));
        assert_eq!(d.source, "keep *new*\n");
    }

    #[test]
    fn html_that_would_paste_garbage_declines_so_the_caller_falls_back() {
        let mut d = doc_with("paste_html_bad", "x\n");
        d.caret = 1;
        // twig builds no table from HTML; raw `<table>` in prose is worse than
        // the plain flavor the caller still holds.
        assert!(!d.paste_html("<table><tr><td>a</td></tr></table>"));
        assert_eq!(d.source, "x\n", "declined edits nothing");
    }

    #[test]
    fn copy_then_paste_round_trips_through_the_html_flavor() {
        let mut d = doc_with("clip_round", "a **b** and [l](https://x.dev)\n");
        d.select_all();
        let html = d.selection_html().expect("renders");
        let mut into = doc_with("clip_round_dst", "\n");
        into.caret = 0;
        assert!(into.paste_html(&html));
        assert_eq!(into.source, "a **b** and [l](https://x.dev)\n");
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
            d.undo();
            assert_eq!(d.status.as_deref(), Some("nothing to undo"), "spends no undo step");
            assert_eq!(d.source, "hello\n");
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

    // ── the document's edges ─────────────────────────────────────────────────

    #[test]
    fn vertical_motion_at_the_document_edges_runs_to_them_in_both_views() {
        // The reproduction, and the disagreement: Down on the last line ran to
        // the end of the document in the source view — by accident, an
        // out-of-range row clamping to the end of the string — and did nothing
        // whatever in the view leaf opens in. One rule now, in both.
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("edge_{tag}"), "abc");
            d.caret = 1;
            d.move_down(false);
            assert_eq!(d.caret, 3, "{tag}: Down on the last line runs to the end");
            d.move_up(false);
            assert_eq!(d.caret, 0, "{tag}: Up on the first line runs to the start");
        }
    }

    #[test]
    fn vertical_motion_at_the_edges_carries_the_column_across_the_lines_between() {
        // Down off the bottom is a motion like any other, so it latches a goal
        // column — and Up comes back to the column the caret left, not to the
        // one the document's end happened to be in.
        for (view, tag) in VIEWS {
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let src = format!("abcdef{gap}ghijkl");
            let mut d = doc_in(view, &format!("edge_goal_{tag}"), &src);
            d.caret = 2; // row 0, col 2
            d.move_down(false);
            assert_eq!(d.caret_pos().1, 2, "{tag}: Down keeps the column");
            d.move_down(false);
            assert_eq!(d.caret, src.len(), "{tag}: Down off the bottom reaches the end");
            d.move_up(false);
            assert_eq!(d.caret_pos().1, 2, "{tag}: Up returns to the column Down left");
        }
    }

    #[test]
    fn vertical_motion_with_nowhere_to_go_latches_no_goal_column() {
        // `goal_col.get_or_insert` ran *before* the early return at row 0, so an
        // Up that did nothing still armed a goal column, and the next Down aimed
        // at a column the caret had never been in.
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("noop_goal_{tag}"), "abc\n\ndef");
            d.caret = 0;
            d.move_up(false);
            assert_eq!(d.caret, 0, "{tag}: already at the start");
            assert_eq!(d.goal_col, None, "{tag}: a no-op Up latched a goal column");

            d.caret = d.source.len();
            d.move_down(false);
            assert_eq!(d.caret, d.source.len(), "{tag}: already at the end");
            assert_eq!(d.goal_col, None, "{tag}: a no-op Down latched a goal column");
        }
    }

    // ── soft wrap ────────────────────────────────────────────────────────────
    // Every other test here builds the map at 80 columns, where no fixture is
    // long enough to fold. A wrap is where one offset belongs to two rows at
    // once, and it broke everything that asks the caret what row it is on.

    /// The wrapped fixture these cases share, folded at 12 columns into
    /// `one two ` / `three four ` / `five six ` / `seven eight`.
    fn wrapped_doc(name: &str) -> Doc {
        let mut d = wysiwyg_doc(name, "one two three four five six seven eight");
        d.build_visual(12);
        d
    }

    #[test]
    fn home_and_end_work_from_a_wrapped_row() {
        // The reproduction: offset 19 is the `f` of "five", the first character
        // of the third row — and also the offset the second row ends at. It
        // resolved to the *second* row, so End aimed at a place the caret was
        // already in and did nothing, while Home walked backwards onto a row the
        // caret had left.
        let mut d = wrapped_doc("wrap_home_end");
        d.caret = 19;
        assert_eq!(d.caret_pos(), (2, 0), "the wrap boundary opens the third row");
        d.move_end(false);
        assert_eq!(d.caret, 27, "End stalled at the wrap boundary");
        d.move_home(false);
        assert_eq!(d.caret, 19, "Home left the row the caret was on");
    }

    #[test]
    fn end_of_a_wrapped_row_stays_put_when_pressed_again() {
        // The row's end is the last offset that is only ever its own: the offset
        // past it opens the row below, and aiming there would send a second
        // press on to *that* row's end, and a third to the next — End walking
        // down the paragraph rather than sitting where it landed.
        let mut d = wrapped_doc("wrap_end_twice");
        d.caret = 12; // inside "three", on the second row
        d.move_end(false);
        assert_eq!(d.caret, 18, "the end of `three four`, before the space the wrap ate");
        assert_eq!(d.caret_pos(), (1, 10), "drawn on the row it is the end of");
        d.move_end(false);
        assert_eq!(d.caret, 18, "a second End moved the caret");
        d.move_home(false);
        assert_eq!(d.caret, 8, "Home takes the row's own start");
    }

    #[test]
    fn vertical_motion_crosses_a_soft_wrap() {
        // Down aimed at the row below's column 0, an offset that resolved *up*
        // to the row above's end — so it landed on the offset it already had and
        // the caret could never leave a paragraph's first row.
        let mut d = wrapped_doc("wrap_down");
        d.caret = 0;
        for (want, row) in [(8, 1), (19, 2), (28, 3), (39, 3)] {
            d.move_down(false);
            assert_eq!(d.caret, want, "Down stalled");
            assert_eq!(d.caret_pos().0, row, "Down landed on the wrong row");
        }
        d.move_down(false);
        assert_eq!(d.caret, 39, "the last row's Down runs to the end and stops");

        // ...and back up, one row per press. The goal column is the end of the
        // last row, past every other row's width, so each press clamps to the
        // row's own last offset rather than to the one that opens the next.
        let mut d = wrapped_doc("wrap_up");
        d.caret = 39;
        for (want, pos) in [(27, (2, 8)), (18, (1, 10)), (7, (0, 7)), (0, (0, 0))] {
            d.move_up(false);
            assert_eq!(d.caret, want, "Up stalled");
            assert_eq!(d.caret_pos(), pos, "Up landed on the wrong row");
        }
    }

    #[test]
    fn a_kill_on_a_wrapped_row_stops_at_the_row() {
        // The kills take the same line Home and End do, so in WYSIWYG they take
        // the visual row — and a soft wrap has no newline in it to delete, so
        // nothing is joined by reaching the end of one.
        let mut d = wrapped_doc("wrap_kill");
        d.caret = 19; // the `f` of "five", opening the third row
        d.delete_to_line_end();
        // The space the wrap ate goes with the row it was drawn on: sparing it
        // would leave "four  seven", two spaces where the row had been.
        assert_eq!(d.source, "one two three four seven eight");

        // Backwards from the row's last caret position — which is *before* that
        // space, so this one survives, being on the far side of the caret.
        let mut d = wrapped_doc("wrap_kill_back");
        d.caret = 27;
        d.delete_to_line_start();
        assert_eq!(d.source, "one two three four  seven eight");
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
        // At two widths: the wide one every other test builds at, where no
        // fixture folds, and one narrow enough that they all do. A soft wrap is
        // where an offset stops being on exactly one row, and testing only the
        // width that never wraps is how the caret came to be pinned at the first
        // one Down reached.
        let src = "# Title\n\na **bold** e\u{0301}mo👨‍👩‍👧ji `x` c\n\n\
                   - item one\n\n| A | B |\n|---|---|\n| x | y |\n";
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
        for width in [80, 12] {
            let mut d = wysiwyg_doc("stop_invariant", src);
            d.build_visual(width);
            let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
            assert!(stops.len() > 20, "fixture should have plenty of stops");
            for start in stops {
                for (name, motion) in &motions {
                    d.caret = start;
                    d.anchor = None;
                    motion(&mut d);
                    assert!(
                        d.vmap.is_stop(d.caret),
                        "{name} from {start} at width {width} landed at {} — not a caret stop",
                        d.caret
                    );
                }
            }
        }
    }

    #[test]
    fn no_wysiwyg_motion_is_a_dead_end() {
        // Down held to the bottom of a document reaches the bottom, and Up held
        // to the top reaches the top — from anywhere, at a width that wraps. The
        // invariant above says a motion lands somewhere legal; this one says it
        // gets somewhere at all, which is what a caret pinned at a wrap boundary
        // was quietly failing to do while every assertion around it held.
        let src = "# Title\n\none two three four five six seven eight nine ten\n\n\
                   - item one two three four five\n\nlast\n";
        for width in [80, 12] {
            let mut d = wysiwyg_doc("no_dead_end", src);
            d.build_visual(width);
            let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
            let (first, last) = (stops[0], stops[stops.len() - 1]);
            for &start in &stops {
                for (name, motion, want) in [
                    ("down", (|d: &mut Doc| d.move_down(false)) as fn(&mut Doc), last),
                    ("up", |d: &mut Doc| d.move_up(false), first),
                ] {
                    d.caret = start;
                    d.anchor = None;
                    d.goal_col = None;
                    // Every row, plus the presses the edges take, plus slack.
                    for _ in 0..d.vmap.num_rows() + 4 {
                        motion(&mut d);
                    }
                    assert_eq!(
                        d.caret, want,
                        "{name} held from {start} at width {width} never arrived"
                    );
                }
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

    // ── active inline marks ───────────────────────────────────────────────────

    /// The marks at a `|`-marked fixture's caret, in `InlineMarks::iter` order.
    fn marks(view: View, name: &str, marked: &str) -> Vec<InlineKind> {
        let (src, caret) = parse_caret(marked);
        let mut d = doc_in(view, name, &src);
        d.caret = caret;
        d.active_inline_marks().iter().collect()
    }

    /// The marks over the selection `[start, end)`.
    fn marks_over(view: View, name: &str, src: &str, start: usize, end: usize) -> Vec<InlineKind> {
        let mut d = doc_in(view, name, src);
        d.anchor = Some(start);
        d.caret = end;
        d.active_inline_marks().iter().collect()
    }

    #[test]
    fn a_caret_in_a_mark_reports_it() {
        for (view, tag) in VIEWS {
            let m = |marked| marks(view, &format!("marks_in_{tag}"), marked);
            assert_eq!(m("a **bo|ld** b"), [InlineKind::Strong], "{tag}");
            assert_eq!(m("a *it|alic* b"), [InlineKind::Emph], "{tag}");
            assert_eq!(m("a `co|de` b"), [InlineKind::Verbatim], "{tag}");
            // Plain text under no mark lights nothing — the toolbar's resting state.
            assert_eq!(m("a| **bold** b"), [], "{tag}");
            assert!(m("plain t|ext").is_empty(), "{tag}");
        }
    }

    #[test]
    fn nested_marks_all_report() {
        // Bold *and* italic: a toolbar lights both buttons, so the set has both —
        // the ancestor chain is a chain, and every mark on it is in force.
        for (view, tag) in VIEWS {
            assert_eq!(
                marks(view, &format!("marks_nested_{tag}"), "**bold and *bo|th*** end"),
                [InlineKind::Strong, InlineKind::Emph],
                "{tag}"
            );
        }
    }

    #[test]
    fn the_caret_at_a_marks_edge_reports_it_where_typing_would_extend_it() {
        // The offsets a WYSIWYG caret actually reaches at a bold run's edges are
        // the first byte of its text and the byte after its last — both inside
        // the mark's span, both places typing lands inside the bold. The offset
        // past the closing delimiter is the next text, and reports nothing.
        let src = "a **bold** b";
        let inner_start = src.find("bold").unwrap(); // 4
        let inner_end = inner_start + "bold".len(); // 8, on the closing `**`
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("marks_edge_{tag}"), src);
            for off in [2, 3, inner_start, inner_end, 9] {
                d.caret = off;
                assert!(
                    d.active_inline_marks().contains(InlineKind::Strong),
                    "{tag}: offset {off} is inside the strong span"
                );
            }
            for off in [0, 1, 10, 11, 12] {
                d.caret = off;
                assert!(
                    !d.active_inline_marks().contains(InlineKind::Strong),
                    "{tag}: offset {off} is outside the strong run"
                );
            }
        }
    }

    #[test]
    fn a_mark_ends_the_same_way_at_the_end_of_the_buffer_as_in_the_middle() {
        // Regression: twig resolves an offset that is one node's end and the
        // next one's start to the node that *starts* there, so `**bold**|\n`
        // isn't bold. With nothing following there's no tie to break and the
        // chain still ended at the mark, which made a trailing `\n` — not the
        // text — decide whether the caret after a bold word reported bold. It's
        // the offset past the mark either way, and typing there is plain either
        // way. A blank document typed into is exactly this shape.
        for (view, tag) in VIEWS {
            let m = |name: String, marked| marks(view, &name, marked);
            assert_eq!(m(format!("marks_eob_{tag}"), "**bold**|"), [], "{tag}: no trailing newline");
            assert_eq!(m(format!("marks_eol_{tag}"), "**bold**|\n"), [], "{tag}: with one");
            // And the last offset that *is* in the mark still is.
            assert_eq!(
                m(format!("marks_eob_in_{tag}"), "**bold*|*"),
                [InlineKind::Strong],
                "{tag}"
            );
        }
    }

    #[test]
    fn a_selection_reports_a_mark_only_when_it_covers_the_whole_thing() {
        let src = "a **bold** b";
        let (b, d_) = (src.find("bold").unwrap(), src.find("bold").unwrap() + 4);
        for (view, tag) in VIEWS {
            let m = |s, e| marks_over(view, &format!("marks_sel_{tag}"), src, s, e);
            // The whole bold word, and a slice of it.
            assert_eq!(m(b, d_), [InlineKind::Strong], "{tag}: the whole word");
            assert_eq!(m(b + 1, d_ - 1), [InlineKind::Strong], "{tag}: a slice");
            // Ending exactly at the closing delimiter's start is still all-bold:
            // an exclusive end sits *past* the last selected character, so the
            // question is asked of the character, not the boundary.
            assert_eq!(m(b, d_ + 2), [InlineKind::Strong], "{tag}: through the close");
            // Half in, half out: Bold lit here would claim a press turns it off.
            assert_eq!(m(0, d_), [], "{tag}: leading plain text");
            assert_eq!(m(b, src.len()), [], "{tag}: trailing plain text");
        }
    }

    #[test]
    fn a_selection_across_two_runs_of_the_same_mark_reports_nothing() {
        // Both ends are bold, but the space between them isn't — two runs are two
        // nodes, which is exactly what the node id catches and a kind-only
        // comparison would not.
        let src = "**one** **two**";
        for (view, tag) in VIEWS {
            let m = marks_over(view, &format!("marks_runs_{tag}"), src, 2, 13);
            assert_eq!(m, [], "{tag}: `one** **two` is not all bold");
        }
    }

    #[test]
    fn marks_read_the_document_as_it_is_edited() {
        // The point of asking twig every frame instead of caching: the answer has
        // to follow the toggle that changed it.
        let mut d = wysiwyg_doc("marks_live", "one two\n");
        d.anchor = Some(0);
        d.caret = 3;
        assert!(d.active_inline_marks().is_empty(), "plain to start");
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "**one** two\n");
        // `toggle` leaves the bolded text selected, so the button it lit stays lit.
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
        d.toggle(InlineKind::Strong);
        assert!(d.active_inline_marks().is_empty(), "and off again");
    }

    #[test]
    fn a_link_is_not_an_inline_mark() {
        // `link`/`str` are inline nodes, but nothing on the inline toolbar
        // toggles them — a set with a "link mark" in it would have no button.
        for (view, tag) in VIEWS {
            assert_eq!(marks(view, &format!("marks_link_{tag}"), "a [te|xt](u) b"), [], "{tag}");
        }
    }

    // ── blank documents ───────────────────────────────────────────────────────

    #[test]
    fn a_blank_document_is_untitled_empty_and_markdown() {
        let mut d = Doc::blank().unwrap();
        assert!(d.is_untitled());
        assert_eq!(d.path, PathBuf::new());
        assert_eq!(d.file_name(), "untitled", "the header has to show something");
        assert_eq!(d.format_name(), "markdown");
        assert_eq!(d.source, "");
        assert!(!d.dirty, "nothing typed yet is nothing to lose");
        assert_eq!(d.disk_state(), DiskState::Untitled);
        // And it's a document you can be in: the default view renders it.
        d.build_visual(80);
        assert_eq!(d.caret, 0);
    }

    #[test]
    fn saving_an_untitled_document_asks_for_a_name_instead_of_writing() {
        let mut d = Doc::blank().unwrap();
        d.insert("hello");
        assert!(d.dirty);
        d.save();
        assert_eq!(d.status.as_deref(), Some("untitled — save as…"));
        assert!(d.dirty, "it must not come away believing it saved");
        assert!(d.is_untitled(), "and it still has no file");
    }

    #[test]
    fn a_blank_document_becomes_a_real_one_at_the_first_save_as() {
        let p = temp_path("blank_save_as");
        let mut d = Doc::blank().unwrap();
        d.insert("# hi");
        d.save_as(p.clone());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "# hi");
        assert!(!d.is_untitled());
        assert!(!d.dirty);
        assert_eq!(d.file_name(), p.file_name().unwrap().to_string_lossy());
        assert_eq!(d.disk_state(), DiskState::Unchanged, "the watermark is stamped");
        // And ⌘S is a plain save from here on.
        d.insert("!");
        d.save();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "# hi!");
        let _ = std::fs::remove_file(&p);
    }

    // ── save as ───────────────────────────────────────────────────────────────

    /// A unique path in the temp dir that no fixture wrote — a Save As target.
    fn temp_path(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_test_target_{name}_{seq}.md"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn save_as_moves_the_document_and_leaves_the_old_file_alone() {
        let mut d = doc_with("save_as_move", "original\n");
        let old = d.path.clone();
        let new = temp_path("save_as_move");
        d.insert("edited: ");
        d.save_as(new.clone());

        assert_eq!(std::fs::read_to_string(&new).unwrap(), "edited: original\n");
        assert_eq!(
            std::fs::read_to_string(&old).unwrap(),
            "original\n",
            "Save As doesn't touch the file it came from"
        );
        assert_eq!(d.path, new, "the document moved");
        assert!(!d.dirty);
        assert_eq!(d.status.as_deref(), Some(&*format!("saved {}", d.file_name())));

        // Every later save follows it, which is the whole difference from a copy.
        d.caret = 0;
        d.insert("re-");
        d.save();
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "re-edited: original\n");
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "original\n");
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn save_as_overwrites_an_existing_target() {
        // The picker already asked; asking again down here is the same question
        // twice, and the second one has no way to be answered.
        let new = temp_path("save_as_over");
        std::fs::write(&new, "theirs\n").unwrap();
        let mut d = doc_with("save_as_over", "ours\n");
        d.save_as(new.clone());
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "ours\n");
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn a_save_as_that_fails_leaves_the_document_where_it_was() {
        let mut d = doc_with("save_as_fail", "body\n");
        let old = d.path.clone();
        d.insert("x");
        // A directory that doesn't exist: the write can't land.
        let bad = std::env::temp_dir().join("leaf_test_no_such_dir_9f2/doc.md");
        d.save_as(bad);

        assert_eq!(d.path, old, "the document must not move to a file that isn't there");
        assert!(d.dirty, "and must not believe it saved");
        assert!(
            d.status.as_deref().unwrap().starts_with("save failed:"),
            "the same failure a plain save reports, got {:?}",
            d.status
        );
        // The original is still the document's file, and still saveable.
        d.save();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "xbody\n");
        assert!(!d.dirty);
    }

    #[test]
    fn save_as_renames_without_reparsing_the_format() {
        // `.dj` on the name doesn't make the buffer djot: it was parsed as
        // Markdown and still is, and saying otherwise would be a conversion the
        // user never asked for (and an undo history thrown away to do it).
        let mut d = doc_with("save_as_format", "**b**\n");
        let mut new = temp_path("save_as_format");
        new.set_extension("dj");
        d.save_as(new.clone());
        assert_eq!(d.format_name(), "markdown");
        let _ = std::fs::remove_file(&new);
    }

    // ── external change / reload ──────────────────────────────────────────────

    #[test]
    fn an_untouched_file_reports_unchanged() {
        let mut d = doc_with("disk_clean", "body\n");
        assert_eq!(d.disk_state(), DiskState::Unchanged);
        // Editing the buffer is not editing the file.
        d.insert("x");
        assert_eq!(d.disk_state(), DiskState::Unchanged);
        assert!(d.dirty);
        // Saving re-stamps the watermark rather than reporting our own bytes back.
        d.save();
        assert_eq!(d.disk_state(), DiskState::Unchanged);
    }

    #[test]
    fn a_file_written_underneath_reports_changed() {
        let mut d = doc_with("disk_changed", "body\n");
        std::fs::write(&d.path, "someone else\n").unwrap();
        assert_eq!(d.disk_state(), DiskState::Changed);
        // Dirty *and* changed is the clobber: both halves are readable, and
        // leaf-core takes neither side.
        d.insert("x");
        assert!(d.dirty && d.disk_state() == DiskState::Changed);
        // Saving anyway is allowed — the frontend asked, or chose not to.
        d.save();
        assert_eq!(std::fs::read_to_string(&d.path).unwrap(), "xbody\n");
        assert_eq!(d.disk_state(), DiskState::Unchanged);
    }

    #[test]
    fn a_file_rewritten_with_the_same_bytes_is_unchanged() {
        // The hash is what makes this honest: the file was written (a fresh
        // mtime), and nothing about the document is stale.
        let d = doc_with("disk_same_bytes", "body\n");
        std::fs::write(&d.path, "body\n").unwrap();
        assert_eq!(d.disk_state(), DiskState::Unchanged);
    }

    #[test]
    fn a_deleted_file_reports_missing() {
        let mut d = doc_with("disk_missing", "body\n");
        std::fs::remove_file(&d.path).unwrap();
        assert_eq!(d.disk_state(), DiskState::Missing);
        // A save recreates it, and the document is whole again.
        d.save();
        assert_eq!(d.disk_state(), DiskState::Unchanged);
        assert_eq!(std::fs::read_to_string(&d.path).unwrap(), "body\n");
    }

    #[test]
    fn reload_replaces_the_document_with_the_file() {
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("reload_{tag}"), "one\n\ntwo\n");
            d.insert("edited ");
            assert!(d.dirty);
            std::fs::write(&d.path, "one\n\ntwo\n\nthree\n").unwrap();
            d.reload();

            assert_eq!(d.source, "one\n\ntwo\n\nthree\n", "{tag}");
            assert!(!d.dirty, "{tag}: the file is what we have");
            assert_eq!(d.disk_state(), DiskState::Unchanged, "{tag}");
            assert_eq!(d.status.as_deref(), Some(&*format!("reloaded {}", d.file_name())));
            // The reloaded tree is live, not the old parse.
            d.caret = d.source.find("three").unwrap();
            assert_eq!(d.breadcrumb(), "doc › para › str", "{tag}");
        }
    }

    #[test]
    fn reload_clamps_the_caret_and_drops_the_selection() {
        let mut d = doc_with("reload_caret", "a long first line\n");
        d.caret = 12;
        d.anchor = Some(4);
        std::fs::write(&d.path, "short\n").unwrap();
        d.reload();
        assert_eq!(d.caret, d.source.len(), "clamped into the shorter file");
        assert_eq!(d.anchor, None, "a selection over bytes that changed is a lie");
        assert!(d.selection().is_none());

        // A caret the file still has room for stays put.
        let mut d = doc_with("reload_caret_keep", "one\n\ntwo\n");
        d.caret = 2;
        std::fs::write(&d.path, "one\n\ntwo\n\nthree\n").unwrap();
        d.reload();
        assert_eq!(d.caret, 2);
    }

    #[test]
    fn reload_drops_the_undo_history() {
        // twig's stack belongs to the buffer, and these are different bytes:
        // replaying a step recorded against the old ones would corrupt the file.
        let mut d = doc_with("reload_undo", "body\n");
        d.insert("x");
        std::fs::write(&d.path, "replaced\n").unwrap();
        d.reload();
        d.undo();
        assert_eq!(d.source, "replaced\n", "an undo must not resurrect the old buffer");
        assert_eq!(d.status.as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn a_reload_that_cant_read_leaves_the_document_alone() {
        let mut d = doc_with("reload_gone", "body\n");
        d.insert("x");
        std::fs::remove_file(&d.path).unwrap();
        d.reload();
        assert_eq!(d.source, "xbody\n", "the unsaved work is still here");
        assert!(d.dirty);
        assert!(d.status.as_deref().unwrap().starts_with("reload failed:"), "{:?}", d.status);

        // And an untitled document has nothing to reload from.
        let mut d = Doc::blank().unwrap();
        d.insert("typed");
        d.reload();
        assert_eq!(d.source, "typed");
        assert_eq!(d.status.as_deref(), Some("no file to reload"));
    }
}

/// twig's node-kind name for an inline mark, back to the [`InlineKind`] a
/// frontend names when it calls [`Doc::toggle`] — the inverse of the mapping
/// twig applies writing the mark out, so the toolbar can light the same button
/// that made the node.
///
/// `None` for every other kind, including the inline nodes that aren't marks at
/// all (`str`, `link`, `image`, the math and break kinds): they're things a
/// caret stands in, not formatting a button toggles.
fn inline_kind(kind: &str) -> Option<InlineKind> {
    Some(match kind {
        "strong" => InlineKind::Strong,
        "emph" => InlineKind::Emph,
        "verbatim" => InlineKind::Verbatim,
        "mark" => InlineKind::Mark,
        "superscript" => InlineKind::Superscript,
        "subscript" => InlineKind::Subscript,
        "insert" => InlineKind::Insert,
        "delete" => InlineKind::Delete,
        _ => return None,
    })
}

/// A watermark for a file's contents (see `Doc::disk_hash`).
///
/// `DefaultHasher` is not stable across Rust releases, which doesn't matter: a
/// watermark is compared only against one taken by the same process moments
/// earlier, and never outlives it. 64 bits leaves a collision — an external edit
/// that hashes to exactly what leaf wrote — at odds no filesystem race gets near.
fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

#[cfg(feature = "fs")]
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



