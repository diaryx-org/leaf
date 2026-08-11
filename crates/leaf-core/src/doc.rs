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
use std::ops::Range;
use std::path::PathBuf;
#[cfg(feature = "fs")]
use std::path::Path;

use anyhow::{Result, anyhow};
#[cfg(feature = "fs")]
use anyhow::Context;
use twig::{
    Alignment, BlockContainerKind, BlockKind, Change, Editor, FlatNode, Format, InlineKind,
    MarkdownExtensions, NodeId, QueryMatch,
};
use unicode_segmentation::GraphemeCursor;

use crate::html;
use crate::wysiwyg::{self, MediaKind, MediaStop, VisualMap};

/// Which view the body shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// The raw document with a caret in source bytes.
    Source,
    /// Markup resolved to real styles, caret riding the rendered glyphs.
    Wysiwyg,
}

/// How much of the source markup the WYSIWYG view exposes — a per-editor
/// preference, orthogonal to [`View`]. Named for markup rather than for Markdown
/// because leaf is grammar-agnostic: twig hands it Djot, HTML and XML on the same
/// terms, and every rung below is about *delimiters*, whatever grammar spells
/// them. The examples are Markdown only because that is what most documents are.
///
/// A single ladder over two underlying axes, because only three of their four
/// combinations are coherent:
///
/// | | authoring off | authoring on |
/// |---|---|---|
/// | delimiters hidden | [`None`](Self::None) | [`Shortcuts`](Self::Shortcuts) |
/// | caret line revealed | *incoherent* | [`Full`](Self::Full) |
///
/// The empty quadrant would show delimiters on the caret's line and then escape
/// the ones you type — a surface that displays a syntax it refuses to accept.
/// Someone who wants to read raw markup without authoring it has
/// [`View::Source`], which is the better tool for it.
///
/// The two axes are read separately by the code that cares — see
/// [`reveals_caret_line`](Self::reveals_caret_line) and
/// [`authors`](Self::authors) — so neither behaviour has to know it's spelled
/// as a ladder.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MarkupMode {
    /// Delimiters stay hidden even on the caret's line, and typed syntax stays
    /// literal — twig escapes anything that would open markup, so formatting
    /// comes from commands (⌘b, the toolbar) instead of from spelling. The clean
    /// reading surface for people who don't write markup by hand; the default,
    /// and what Diaryx ships.
    #[default]
    None,
    /// Delimiters stay hidden, but typing them authors real markup: `*x*`
    /// becomes italic and the asterisks disappear into the styling
    /// (Typora/Bear-shaped). For someone who knows the syntax but wants the
    /// clean surface back once it has been applied.
    Shortcuts,
    /// The caret's line shows its raw markup while every other line renders
    /// resolved (Obsidian live-preview-shaped), and typed syntax authors markup
    /// — for people fluent in the document's grammar who want to see and edit
    /// the delimiters they type.
    Full,
}

impl MarkupMode {
    /// Whether the rich view shows raw delimiters on the line holding the caret.
    /// The rendering axis — read by [`Doc::reveal_line`] and threaded into the
    /// WYSIWYG builder.
    pub fn reveals_caret_line(self) -> bool {
        matches!(self, MarkupMode::Full)
    }

    /// Whether typed markup characters author real formatting. The editing axis
    /// — read by [`Doc::insert`], which escapes typed syntax when this is false.
    pub fn authors(self) -> bool {
        !matches!(self, MarkupMode::None)
    }
}

/// How the WYSIWYG view treats a *soft break* — a bare newline inside a
/// paragraph. An axis of its own, orthogonal to [`MarkupMode`] (which governs
/// inline-markup delimiters) and to [`View`]: any reveal preference pairs with
/// either flow. The renderer consults it when it lays a block's inline content
/// into visual rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LineFlow {
    /// A soft break folds into a space and the paragraph reflows to the
    /// viewport width — flowing prose, where the source's line wrapping is
    /// insignificant. The default, and what Diaryx ships.
    #[default]
    Fold,
    /// A soft break renders as a line break exactly where it was written, so
    /// the author's source line structure shows on screen unchanged — the mode
    /// for people who lay out their prose deliberately (one sentence or clause
    /// per line, semantic line breaks). The break is still a soft break in the
    /// source; only its rendering changes.
    Preserve,
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

    /// Flip `kind` in the set — the sticky-marks toggle at a collapsed caret.
    fn flip(&mut self, kind: InlineKind) {
        self.0 ^= Self::bit(kind);
    }

    /// The symmetric difference: which marks differ between the two sets. Used
    /// to resolve the marks already in force at the caret against the pending
    /// delta — a bit set in the delta flips the base mark for the next keystroke.
    fn xor(self, other: InlineMarks) -> InlineMarks {
        InlineMarks(self.0 ^ other.0)
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

/// Which side of the caret a delete looks for an in-cell `<br>` break to swallow
/// whole — see [`Doc::cell_break_at`]. `Backward` is Backspace (a break ending at
/// the caret), `Forward` is Delete (one starting at it).
#[derive(Clone, Copy)]
enum BreakEdge {
    Backward,
    Forward,
}

/// A re-spelling of one inline mark run, held ready in case the edit about to
/// happen breaks it — see [`Doc::mark_edge_fix`] and [`Doc::repair_mark_edges`].
/// Every offset in it is in the coordinates the document will have *after* the
/// plain edit, since that is when it may be applied.
struct MarkEdgeFix {
    /// The run's kind, and an offset inside what was its content: together they
    /// answer "did the plain edit actually break this mark?" — the question that
    /// decides whether any of this is applied at all.
    kind: InlineKind,
    probe: usize,
    /// The byte range to re-spell (the run's delimiters included) and its new
    /// spelling, with the edge whitespace moved outside the delimiters.
    start: usize,
    end: usize,
    text: String,
    /// Where the caret belongs afterwards — the same place on screen it would
    /// have had, which is now on the other side of a delimiter.
    caret: usize,
    /// The marks in force for text typed at that caret. The caret can land
    /// outside a run it was inside, and the marks have to survive the move or
    /// the toolbar goes dark mid-word.
    want: InlineMarks,
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
    /// How much of the source markup the rich view exposes — a frontend preference (see
    /// [`MarkupMode`]). Its two axes are read apart: the rendering one by
    /// [`reveal_line`](Self::reveal_line), the editing one by
    /// [`insert`](Self::insert).
    markup_mode: MarkupMode,
    /// Whether soft breaks fold into the reflowed paragraph or render where
    /// they were written (see [`LineFlow`]) — an independent frontend
    /// preference the WYSIWYG builder consults when it lays out a block.
    line_flow: LineFlow,
    /// The kind of the last edit, for coalescing: twig owns the undo *history*
    /// (see `undo`/`redo`), but "what counts as one undo step" is a frontend-UX
    /// call, so leaf decides when a run continues and tells twig to coalesce.
    last_edit_kind: Option<EditKind>,
    /// The inline marks the user has toggled *at a collapsed caret* with no
    /// selection — "start typing bold here". Held as the XOR delta from the marks
    /// already in force at [`pending_at`](Self::pending_at): a set bit means
    /// "flip this kind for the next typed text", so it both turns a mark on where
    /// none is (type into bold) and off where one already covers the caret (type
    /// past the bold you're standing in). [`Doc::insert`] realises it onto the
    /// freshly typed text and then clears it — a mark once realised is carried by
    /// the caret sitting inside the run, not by this delta.
    pending_marks: InlineMarks,
    /// The caret offset [`pending_marks`](Self::pending_marks) applies to. The
    /// delta is live only while the caret still stands here with no selection;
    /// any motion or edit ([`move_to`](Self::move_to), a splice, a click) drops
    /// it, so a toggled-but-never-typed format doesn't leak onto text elsewhere.
    pending_at: Option<usize>,
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
    /// a pure function of `(revision, wrap, reveal line)`, so when those haven't
    /// moved, rebuilding it produces the identical map — see
    /// [`Doc::build_visual`].
    ///
    /// The reveal line ([`Doc::reveal_line`]) is the caret's, and is `None` in
    /// every mode but [`MarkupMode::Full`] — so outside that mode the key is
    /// text and width alone, and a caret motion still rebuilds nothing.
    vmap_key: Option<(u64, Option<usize>, Option<Range<usize>>)>,
    /// Per-block row cache backing the incremental rebuild: when the text
    /// changes, only the top-level blocks whose bytes moved are re-rendered and
    /// the rest are reused shifted (see [`wysiwyg::BlockCache`]). Persists across
    /// builds; a pure accelerator, so it's never read for correctness.
    block_cache: wysiwyg::BlockCache,
    /// How many visual rows each block image reserves, keyed by its destination —
    /// set by the frontend through [`Doc::set_media_rows`] once it has decoded and
    /// measured the pictures. Core does no image I/O, so this is the only way it
    /// learns a picture's height; a destination not in the map reserves the bare
    /// one-row placeholder. Threaded into the builder so [`wysiwyg::build_cached`]
    /// sizes each placeholder, and folded into `vmap_key` so a height change
    /// rebuilds the map.
    media_rows: HashMap<String, usize>,

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

/// The Markdown extensions every leaf document is parsed with. `html_elements`
/// and `directives` depart from twig's defaults. `html_elements` promotes
/// embedded raw HTML (`<img>`, `<picture>`, `<source>`, …) into semantic AST
/// nodes, so a picture becomes a real `image` node the frontends can frame and
/// rasterize instead of opaque `raw_block` text. `directives` turns on generic
/// `:::name{.class}` fenced-div containers (`directive` nodes), which a host
/// app uses for its own semantics (diaryx's `:::vis{.audience}` visibility
/// blocks) — core renders any directive as a plain tinted container, agnostic
/// of `name`. Both flags are inert for non-Markdown formats, so it's safe to
/// pass them unconditionally. Threading this through every constructor (not
/// just `open`) keeps `from_source`, `blank`, and `reload` parsing the same
/// document the same way — twig reparses with these same flags after each edit.
fn parse_extensions() -> MarkdownExtensions {
    MarkdownExtensions { html_elements: true, directives: true, ..Default::default() }
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
            // `None` by default — the clean surface Diaryx ships, with typed
            // syntax kept literal; a markup-fluent frontend can climb the
            // ladder to `Shortcuts` or `Full`.
            markup_mode: MarkupMode::default(),
            // Fold by default — flowing prose that reflows to the viewport, the
            // behaviour every frontend had before this preference existed.
            line_flow: LineFlow::default(),
            last_edit_kind: None,
            pending_marks: InlineMarks::empty(),
            pending_at: None,
            goal_col: None,
            vmap: VisualMap::default(),
            revision: 0,
            // No map yet — the first `build_visual` always builds.
            vmap_key: None,
            block_cache: wysiwyg::BlockCache::default(),
            media_rows: HashMap::new(),
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

    /// The current markup-exposure preference (see [`MarkupMode`]).
    pub fn markup_mode(&self) -> MarkupMode {
        self.markup_mode
    }

    /// Set the markup-exposure preference. Both of its axes take effect at
    /// once: the editing one on the next [`insert`](Self::insert), and the
    /// rendering one on the next build — which is why this drops the cached
    /// visual map and the per-block render cache, exactly as
    /// [`set_line_flow`](Self::set_line_flow) does.
    pub fn set_markup_mode(&mut self, mode: MarkupMode) {
        if self.markup_mode == mode {
            return;
        }
        self.markup_mode = mode;
        // Neither cache is keyed on the mode, and moving between `Full` and the
        // hidden modes changes every row the caret's line renders to — so
        // invalidate both explicitly.
        self.vmap_key = None;
        self.block_cache = wysiwyg::BlockCache::default();
    }

    /// The source byte range of the line the caret sits on, when that line
    /// should render its raw delimiters — `None` in every mode and view that
    /// hides them, which is what the builder reads as "reveal nothing".
    ///
    /// A *source* line (newline to newline), not a visual row: a wrapped
    /// paragraph and a `LineFlow::Preserve` soft break both split one source
    /// line across several rows, and revealing half a delimiter pair because the
    /// other half wrapped would be worse than revealing neither. The range
    /// excludes the terminating newline and is empty-but-present on a blank
    /// line, which reveals nothing but still keys the caches correctly.
    ///
    /// Only in [`View::Wysiwyg`]: source view already shows every byte, so
    /// there is nothing there to reveal.
    pub(crate) fn reveal_line(&self) -> Option<Range<usize>> {
        if !self.markup_mode.reveals_caret_line() || self.view != View::Wysiwyg {
            return None;
        }
        Some(source_line_range(&self.source, self.caret))
    }

    /// The current soft-break flow preference (see [`LineFlow`]).
    pub fn line_flow(&self) -> LineFlow {
        self.line_flow
    }

    /// Set the soft-break flow preference. The mode changes how every block lays
    /// out, so a change drops the cached visual map and the per-block render
    /// cache, forcing the next [`build_visual`] to rebuild under the new flow.
    ///
    /// [`build_visual`]: Self::build_visual
    pub fn set_line_flow(&mut self, mode: LineFlow) {
        if self.line_flow == mode {
            return;
        }
        self.line_flow = mode;
        // Both caches are keyed on `(revision, wrap)`, neither of which moved —
        // so invalidate them explicitly, or the next build would reuse rows laid
        // out under the old flow.
        self.vmap_key = None;
        self.block_cache = wysiwyg::BlockCache::default();
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
    pub fn set_media_rows(&mut self, rows: HashMap<String, usize>) {
        if self.media_rows == rows {
            return;
        }
        self.media_rows = rows;
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
        // Under `MarkupMode::Full` the map is a function of the caret's *line*
        // as well as the text, so the line joins the key: moving within a line
        // still reuses the map, and crossing into another one rebuilds it. In
        // every other mode `reveal_line` is `None` and the key is what it was,
        // so caret motion goes on costing nothing.
        let reveal = self.reveal_line();
        let key = (self.revision, wrap, reveal.clone());
        if self.vmap_key.as_ref() != Some(&key) {
            // Enumerate the top-level blocks cheaply — no whole-arena marshal.
            // A subtree is pulled only for the block(s) that actually changed, so
            // the FFI marshal shrinks from O(document) to O(edited block).
            let top = self.top_blocks();

            // Fast path: when twig reports a dirty byte range, try to patch the
            // previous map in place — a single-block edit moves the prefix,
            // shifts the suffix, and re-renders only one block. `build_spliced`
            // returns `None` (and we fall back to the always-correct full rebuild)
            // whenever the edit reshaped the block structure, hit a table, or
            // there's no previous map to patch.
            // Preserve soft breaks as written when the flow preference asks for
            // it — the builder renders each as its own visual row instead of
            // folding it into the reflowed paragraph.
            let preserve_soft = self.line_flow == LineFlow::Preserve;
            let spliced = match self.editor.dirty_range() {
                Some(dirty) => {
                    let prev = std::mem::take(&mut self.vmap);
                    let source = &self.source;
                    let cache = &mut self.block_cache;
                    let media_rows = &self.media_rows;
                    let editor = &mut self.editor;
                    wysiwyg::build_spliced(prev, source, wrap, preserve_soft, &top, dirty, media_rows, reveal.clone(), cache, |id| {
                        editor.subtree(NodeId(id)).unwrap_or_default()
                    })
                }
                None => None,
            };
            self.vmap = spliced.unwrap_or_else(|| {
                let source = &self.source;
                let cache = &mut self.block_cache;
                let media_rows = &self.media_rows;
                let editor = &mut self.editor;
                wysiwyg::build_cached(&top, source, wrap, preserve_soft, media_rows, reveal, cache, |id| {
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

    /// The document's top-level blocks for the incremental render. See
    /// [`wysiwyg::top_blocks`] for why this isn't simply `child_spans(None)`.
    fn top_blocks(&mut self) -> Vec<QueryMatch> {
        let source = &self.source;
        wysiwyg::top_blocks(&mut self.editor, source)
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
        // Typing against a block picture would dissolve it — see
        // `open_paragraph_at_block_media`. Give the text a paragraph first, so
        // what the caret was standing beside stays a picture.
        self.open_paragraph_at_block_media(text);
        // Armed sticky marks (⌘b with no selection) turn the next typed text
        // bold/italic/… and then retire — see `insert_with_marks`. Whitespace is
        // the exception: it takes no mark of its own and keeps the delta armed
        // for the character behind it — see `insert_space_with_marks`.
        let pending = self.pending_here();
        if !pending.is_empty() && self.selection().is_none() && !text.is_empty() {
            if text.trim().is_empty() {
                self.insert_space_with_marks(self.caret, text, pending);
            } else {
                self.insert_with_marks(self.caret, text, pending);
            }
            return;
        }
        // `MarkupMode::None`: typed syntax stays literal — twig escapes
        // anything that would open markup, so a Diaryx user never mints
        // formatting by keyboard (it comes from commands instead). The other two
        // rungs of the ladder author markup from what you type, which is the
        // whole difference between them and this one. Only in the rendered view
        // (source view is for typing raw markup) and only for the authorable
        // lightweight formats (a parse-only format has no literal spelling).
        // Marks (⌘b) still format — that path returned above; and leaf's own
        // structural inserts go through `insert_raw`, never here, so a list
        // marker or quote gutter is written as the markup it is.
        if !self.markup_mode.authors()
            && self.view == View::Wysiwyg
            && !text.is_empty()
            && matches!(self.format, Format::Markdown | Format::Djot)
        {
            self.insert_literal_typed(text);
            return;
        }
        self.insert_raw(text);
    }

    /// Insert `text` verbatim at the caret (replacing any selection) — the plain
    /// path with no Hidden-mode literal escaping. leaf's own structural inserts
    /// (a list marker, a quote gutter, an in-cell `<br>`) call this: they ARE
    /// markup by design and must not be escaped.
    fn insert_raw(&mut self, text: &str) {
        let (s, e) = self.selection().unwrap_or((self.caret, self.caret));
        self.splice(s, e, text, typed_edit_kind(text));
    }

    /// Open a paragraph for text about to be inserted at one of a block media's
    /// two caret stops, and leave the caret standing in it.
    ///
    /// A block image is a paragraph whose entire content is the picture, and the
    /// caret's only homes on it are in front of it and just past it (see
    /// [`VisualMap::block_media_stop`]). Text inserted at either offset joins
    /// *that* paragraph — and a paragraph holding anything besides the image is
    /// no longer a block image but a line of text with an inline one in it. The
    /// frontend that was painting a photo there paints a text run instead; the
    /// picture is still in the file, and nothing said a word. Those two offsets
    /// are also exactly where a click on the picture lands, so the whole accident
    /// is one tap and one keystroke.
    ///
    /// So the break goes in first and the text lands in the new empty paragraph —
    /// what pressing Return before typing would have done, which is a habit no
    /// one should have to learn from losing a photo. A no-op everywhere else, and
    /// over a selection (which is replaced, not joined into).
    ///
    /// A picture inside a quote or a list leaves its container, because `\n\n`
    /// ends the block. The alternative is worse: the `\n> ` / next-item
    /// continuation [`newline`](Self::newline) writes stays in the same
    /// *paragraph*, which is the thing being prevented.
    ///
    /// Only in the rendered view. Source view is for typing raw markup, where
    /// putting a character against an image is exactly what it looks like.
    fn open_paragraph_at_block_media(&mut self, text: &str) {
        if self.view != View::Wysiwyg || text.is_empty() || text == "\n" {
            return;
        }
        if self.selection().is_some() {
            return;
        }
        // The map may be a revision behind (nothing has drawn since the last
        // edit), and this asks it about offsets — a stale answer would splice a
        // break into the wrong place. Free when it is already current, which it
        // is whenever a frontend drew a frame between keystrokes.
        self.rebuild_map();
        let at = self.caret;
        let Some((side, _)) = self.vmap.block_media_stop(at) else {
            return;
        };
        if !self.splice(at, at, "\n\n", EditKind::Other) {
            return;
        }
        // The break is part of the keystroke, not an edit of its own: leave the
        // run marked as typing so the character about to arrive folds into it and
        // one undo puts the document back the way it was found. (A paste, or a
        // multi-character insert, is `EditKind::Other` and stays its own step —
        // as it would have been anywhere else in the document.)
        self.last_edit_kind = Some(EditKind::Insert);
        if side == MediaStop::Before {
            // The break went in above the picture and the caret rode to the end
            // of it — which is still hard against the picture. Step back onto the
            // blank line it opened, so the text lands above rather than in front.
            self.caret = at;
        }
    }

    /// A delete key pressed at one of a block picture's two caret stops, handled
    /// as the picture being an *atom* rather than a run of bytes. Returns whether
    /// the key was consumed.
    ///
    /// The caret rests in front of a block image and just past it, never inside
    /// its markup — which the rendered view doesn't show. So the byte a delete
    /// key nominally takes there is one the writer cannot see, and taking it
    /// leaves the picture as broken markup rather than as anything anyone asked
    /// for: Backspace at the stop past `![](p.png)` removes the closing paren, and
    /// a photo becomes the literal text `![](p.png`. That is how a picture goes
    /// missing from a document with nobody having touched it — the same
    /// dissolution [`open_paragraph_at_block_media`](Self::open_paragraph_at_block_media)
    /// prevents from the typing side, and it cost this repository's own test vault
    /// a photo before it was found.
    ///
    /// So the key aimed *at* the picture deletes the picture, whole — Backspace
    /// when it is behind the caret, Delete when it is in front — which is what
    /// every editor does with an embed, and one undo away. The key aimed *away*
    /// from it would otherwise delete the paragraph break and merge a neighbour
    /// into the picture's own paragraph, which dissolves it just as surely; it
    /// steps the caret over the boundary instead and leaves the
    /// next press to delete in the block it has reached — the same "first press
    /// steps out of the atom, second press deletes" every delete key here gets,
    /// word-deletes included (⌥⌫ in front of a picture is aimed at the prose
    /// above, and reaches it on the second press rather than taking the break and
    /// the picture with it on the first).
    fn delete_around_block_media(&mut self, forward: bool) -> bool {
        // The map answers about offsets, so it has to be this revision's — see
        // the same call in `open_paragraph_at_block_media`.
        self.rebuild_map();
        let Some((side, span)) = self.vmap.block_media_stop(self.caret) else {
            return false;
        };
        let aimed_at_it = side
            == if forward {
                MediaStop::Before
            } else {
                MediaStop::After
            };
        if !aimed_at_it {
            let over = if forward {
                self.vmap.stop_after(self.caret)
            } else {
                self.vmap.stop_before(self.caret)
            };
            if let Some(off) = over.filter(|&o| o >= self.caret_floor()) {
                self.caret = off;
                self.anchor = None;
                self.goal_col = None;
            }
            return true;
        }
        // Take the break that held the picture apart from its neighbour with it,
        // so the delete doesn't leave a blank paragraph standing where the
        // picture was. The last arm is a picture that is the whole document.
        let (from, to) = if self.source[..span.start].ends_with("\n\n") {
            (span.start - 2, span.end)
        } else if self.source[span.end..].starts_with("\n\n") {
            (span.start, span.end + 2)
        } else {
            (span.start, span.end)
        };
        self.splice(from.max(self.caret_floor()), to, "", EditKind::Other);
        true
    }

    /// The Hidden-mode typing path: replace any selection, then insert `text`
    /// escaped so it stays literal. When it replaces a selection the two edits
    /// fold into one undo step, so an overwrite undoes atomically (and restores
    /// the selection) exactly as a plain one does.
    fn insert_literal_typed(&mut self, text: &str) {
        let kind = typed_edit_kind(text);
        match self.selection() {
            Some((s, e)) => {
                if !self.splice(s, e, "", EditKind::Other) {
                    return;
                }
                // Typing over a whole marked run takes its delimiters with it
                // (the empty content couldn't hold them — see
                // `repair_mark_edges`) and leaves its marks armed at the caret.
                // The text taking the run's place inherits them, exactly as it
                // would have by landing inside a run that survived.
                let pending = self.pending_here();
                if !pending.is_empty() && !text.trim().is_empty() {
                    self.insert_with_marks(self.caret, text, pending);
                    return;
                }
                self.insert_literal_at(self.caret, text, kind, true);
            }
            None => {
                self.insert_literal_at(self.caret, text, kind, false);
            }
        }
    }

    /// The sticky-mark delta that is live right now: the marks armed by [`toggle`]
    /// at a collapsed caret, but only while the caret still stands where they
    /// were armed and nothing is selected. Empty otherwise, so a stale delta
    /// never styles text it wasn't meant for.
    fn pending_here(&self) -> InlineMarks {
        if self.anchor.is_none() && self.pending_at == Some(self.caret) {
            self.pending_marks
        } else {
            InlineMarks::empty()
        }
    }

    /// Drop the armed sticky marks — any caret motion, selection, or edit does
    /// this, so "start bold here" only ever applies at the exact spot it was
    /// asked for.
    fn clear_pending(&mut self) {
        self.pending_marks = InlineMarks::empty();
        self.pending_at = None;
    }

    /// Insert `text` at `at` carrying the armed sticky `marks`: a mark not yet in
    /// force is wrapped around the freshly typed text; a mark the caret already
    /// stands inside is *shed* — the text is inserted past the run's end so it
    /// lands unmarked ("type normally again"). The caret comes to rest inside any
    /// added runs, so continued typing inherits the marks with no re-wrapping,
    /// and the delta is cleared: the marks now live in the document, not here.
    fn insert_with_marks(&mut self, at: usize, text: &str, marks: InlineMarks) {
        let base = self.mark_spans_at(at);
        let base_set: InlineMarks = base.iter().map(|(k, _)| *k).collect();
        // Nothing to shed, and a run of exactly these marks standing just behind
        // the caret: carry on writing *that* run rather than opening a second
        // one beside it.
        if base_set.is_empty() && self.rejoin_run(at, text, marks) {
            return;
        }
        // Shed the marks we're turning off: step the insertion point past the
        // end of each run the caret sits in, so the new text falls outside it.
        let mut ins_at = at;
        for (kind, span) in &base {
            if marks.contains(*kind) {
                ins_at = ins_at.max(span.end);
            }
        }
        if !self.splice_exact(ins_at, ins_at, text, EditKind::Other) {
            return;
        }
        // The plain splice inserted exactly `text` at `ins_at`; that byte range
        // is the content every added mark wraps.
        let (mut cs, mut ce) = (ins_at, ins_at + text.len());
        for kind in marks.iter() {
            if !base_set.contains(kind) {
                let (ncs, nce) = self.wrap_span(cs, ce, kind);
                cs = ncs;
                ce = nce;
            }
        }
        self.caret = ce.min(self.source.len());
        self.anchor = None;
        self.last_edit_kind = None;
        // Realised: the marks are in the document now, and the caret sits inside
        // them, so there is no delta left to carry. Arm nothing, but remember the
        // spot so a *further* toggle before typing starts a clean delta here.
        self.pending_marks = InlineMarks::empty();
        self.pending_at = Some(self.caret);
        self.clamp_caret();
        self.record_caret();
    }

    /// Carry on the marked run just behind `at` — moving its closing delimiters
    /// out past the new text — instead of opening a second run of the same marks
    /// beside it. Returns whether it did.
    ///
    /// This is the far half of the mark-edge rule (see [`splice`](Self::splice)).
    /// A space typed after a bold word steps the caret out of the run, because
    /// `**bold **` is not bold; the next character has to step back *in*, or the
    /// writer who typed one bold phrase is left with `**bold** **and**` — two
    /// runs that read the same to a reader but spell the file in a way nobody
    /// wrote. Only whitespace may stand in the gap (a run doesn't reach across
    /// words it isn't marking), and the marks behind it must be exactly the ones
    /// armed — a run of *some* other kind is a neighbour, not this phrase.
    fn rejoin_run(&mut self, at: usize, text: &str, marks: InlineMarks) -> bool {
        if text.is_empty() || text.trim() != text {
            return false;
        }
        let gap_at = self.source[..at].trim_end_matches(|c: char| c == ' ' || c == '\t').len();
        // Walk in through the delimiters stacked at that point, innermost last:
        // `***both*** ` closes two runs with one `***`, and rejoining means
        // getting behind all of them.
        let (mut cut, mut kinds) = (gap_at, InlineMarks::empty());
        loop {
            let Some((kind, content_end)) = self
                .editor
                .ancestors_at(prev_boundary(&self.source, cut))
                .unwrap_or_default()
                .into_iter()
                .filter(|m| m.span.end == cut)
                .find_map(|m| Some((inline_kind(&m.kind)?, m.content_span.clone()?.end)))
            else {
                break;
            };
            if content_end >= cut {
                break; // a mark with no closing delimiter to step behind
            }
            kinds.insert(kind);
            cut = content_end;
        }
        if cut == gap_at || kinds != marks {
            return false;
        }
        // Re-spell the tail: the gap, then the new text, then the delimiters that
        // used to close in front of them — read out of the document rather than
        // written from a table, so whatever twig spells them with is what moves.
        let tail = format!("{}{text}{}", &self.source[gap_at..at], &self.source[cut..gap_at]);
        if !self.splice_exact(cut, at, &tail, EditKind::Other) {
            return false;
        }
        self.caret = (cut + (at - gap_at) + text.len()).min(self.source.len());
        self.anchor = None;
        self.last_edit_kind = None;
        self.pending_marks = InlineMarks::empty();
        self.pending_at = Some(self.caret);
        self.clamp_caret();
        self.record_caret();
        true
    }

    /// Insert typed whitespace at a caret with sticky marks armed. Whitespace is
    /// never itself wrapped: a mark around a space draws nothing a reader can
    /// see, and in Markdown and Djot it draws its own delimiters instead
    /// (`** **`). So the space goes in unmarked — outside any run the armed
    /// marks are shedding — and the marks stay armed for the character after it,
    /// which rejoins the run (see [`rejoin_run`](Self::rejoin_run)).
    fn insert_space_with_marks(&mut self, at: usize, text: &str, marks: InlineMarks) {
        let base = self.mark_spans_at(at);
        // What the *next* character carries: the armed delta resolved against the
        // marks in force here, which the space must not quietly drop.
        let want = base.iter().map(|(k, _)| *k).collect::<InlineMarks>().xor(marks);
        let mut ins_at = at;
        for (kind, span) in &base {
            if marks.contains(*kind) {
                ins_at = ins_at.max(span.end);
            }
        }
        if !self.splice(ins_at, ins_at, text, typed_edit_kind(text)) {
            return;
        }
        self.rearm(want);
        self.record_caret();
    }

    /// Wrap `[s, e)` in `kind` via twig and return the byte span the *content*
    /// (not the delimiters) occupies afterwards. Markdown/Djot inline delimiters
    /// are symmetric (`**`…`**`, `_`…`_`, `` ` ``…`` ` ``), so the bytes twig
    /// added split evenly around the content — half the growth on each side.
    fn wrap_span(&mut self, s: usize, e: usize, kind: InlineKind) -> (usize, usize) {
        match self.editor.toggle_inline(s, e, kind) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                self.dirty = self.source != self.clean_source;
                let added = (change.new.end - change.new.start).saturating_sub(e - s);
                let half = added / 2;
                (change.new.start + half, change.new.end - half)
            }
            // Unsupported here (e.g. mark on Markdown): leave the text unwrapped
            // rather than lose the keystroke.
            Err(e2) => {
                self.status = Some(format!("{kind:?}: {e2}"));
                (s, e)
            }
        }
    }

    /// The safe offset to splice a block-level break at, given a caret that may
    /// sit exactly between an inline mark's content and its own closing
    /// delimiter (`content_span.end == off < span.end` for some enclosing mark
    /// — the WYSIWYG caret's natural resting place at the end of `**bold**`
    /// with nothing following it on the line: the closing `**` renders no
    /// glyph of its own, so the caret's "end of line" offset lands right
    /// before it). Splicing a paragraph/list/quote break at `off` itself would
    /// sever the delimiter from its content, stranding it alone on the new
    /// line. Walks out to the *outermost* such mark's `span.end` instead, so
    /// nested marks closing at the same point (`**_x_**`) all clear together.
    /// A no-op everywhere else — mid-run, or past real trailing content, no
    /// mark's `content_span` ends exactly at `off`.
    fn skip_trailing_close_delims(&mut self, off: usize) -> usize {
        let off = off.min(self.source.len());
        self.editor
            .ancestors_at(off)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| inline_kind(&m.kind).is_some())
            .filter(|m| off < m.span.end && m.content_span.as_ref().is_some_and(|c| c.end == off))
            .map(|m| m.span.end)
            .max()
            .unwrap_or(off)
    }

    /// The offset a *delete* aimed at the character before `off` should stop at,
    /// when `off` is the start of a run's text and the bytes behind it are that
    /// run's opening delimiter. The rich view draws no glyph for a `**`, so the
    /// byte behind the caret at the start of a bold word is not a character the
    /// writer can see, let alone one they aimed Backspace at: taking it leaves
    /// `a *bold** c` — the styling gone and a literal asterisk in its place. The
    /// delete steps over the whole delimiter to the visible character in front of
    /// it instead. Walks out to the *outermost* mark opening there, so
    /// `**_x_**` clears every delimiter at once, and is a no-op anywhere else.
    fn skip_leading_open_delims(&mut self, off: usize) -> usize {
        let off = off.min(self.source.len());
        self.editor
            .ancestors_at(off)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| inline_kind(&m.kind).is_some())
            .filter(|m| m.span.start < off && m.content_span.as_ref().is_some_and(|c| c.start == off))
            .map(|m| m.span.start)
            .min()
            .unwrap_or(off)
    }

    /// `off` moved *inside* the run whose closing delimiters end there — the
    /// other offset the rich view draws in the same place, since a `**` renders
    /// no glyph of its own. `**bold**` has a caret home on each side of its
    /// closing delimiter, one column apart on screen and eight bytes and a whole
    /// run apart in the file, and a plain ← lands on the outer one whenever a
    /// space follows the phrase. The inner one is what the writer is pointing at
    /// there: the end of their bold word. Walks in through every mark closing at
    /// that point, innermost last, so `***both***` lands inside both. A no-op
    /// anywhere else — mid-run, or in prose, no mark's span ends at `off`.
    fn step_inside_close_delims(&mut self, off: usize) -> usize {
        let mut off = off.min(self.source.len());
        loop {
            let inner = self
                .editor
                .ancestors_at(prev_boundary(&self.source, off))
                .unwrap_or_default()
                .into_iter()
                .filter(|m| inline_kind(&m.kind).is_some() && m.span.end == off)
                .filter_map(|m| m.content_span.clone().map(|c| c.end))
                .filter(|&end| end < off)
                .max();
            match inner {
                Some(end) => off = end,
                None => return off,
            }
        }
    }

    /// The mirror at the opening edge: `off` moved inside the run whose
    /// delimiters *start* there, onto the first character of its text. See
    /// [`step_inside_close_delims`](Self::step_inside_close_delims).
    fn step_inside_open_delims(&mut self, off: usize) -> usize {
        let mut off = off.min(self.source.len());
        loop {
            let inner = self
                .editor
                .ancestors_at(off)
                .unwrap_or_default()
                .into_iter()
                .filter(|m| inline_kind(&m.kind).is_some() && m.span.start == off)
                .filter_map(|m| m.content_span.clone().map(|c| c.start))
                .filter(|&start| start > off)
                .min();
            match inner {
                Some(start) => off = start,
                None => return off,
            }
        }
    }

    /// The inline mark kinds whose span covers `off`, each with that span — the
    /// span-carrying sibling of [`marks_at`](Self::marks_at), which reports node
    /// ids instead. Used to shed a mark by stepping past the end of its run.
    fn mark_spans_at(&mut self, off: usize) -> Vec<(InlineKind, std::ops::Range<usize>)> {
        let off = off.min(self.source.len());
        self.editor
            .ancestors_at(off)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| off < m.span.end)
            .filter_map(|m| inline_kind(&m.kind).map(|k| (k, m.span.clone())))
            .collect()
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
        // Pasting against a block picture dissolves it exactly as typing does,
        // and for the same reason — see `open_paragraph_at_block_media`.
        self.open_paragraph_at_block_media(text);
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
        // Nesting changes an ordered list's numbering (the nested item restarts,
        // its old siblings resume) — keep the source markers in step.
        self.renumber_here();
        // Nesting an empty `-` item under a text line reparses that text as a
        // setext heading; swap the dash for a `*` before it can (a no-op unless
        // the collapse actually happened).
        self.avoid_setext_collapse();
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
        self.renumber_here();
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
        let mut line_off = start;
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let delta = if add {
                if skip_blank && line.trim().is_empty() {
                    out.push_str(line);
                    0
                } else if list_marker_width(line).is_some() && self.first_item_of_list(line_off) {
                    // The first item of a list has no preceding sibling to nest
                    // under, so a Tab here can't spell a sub-list — twig would
                    // reparse the shoved-over marker as the same list, only
                    // indented, which Shift+Tab then can't cleanly undo. Leave the
                    // item where it is, the way every list editor refuses to
                    // over-indent a list's first line.
                    out.push_str(line);
                    0
                } else {
                    // Indent by the line's own *marker width* when it's a list
                    // item, so Tab nests it under its sibling: an ordered item's
                    // `1. ` marker is 3 wide, and only 3 spaces push the new
                    // marker to the parent's content column where twig reparses
                    // it as a sub-list. A fixed two-space step nests a bullet
                    // (`- ` is 2 wide) but leaves an ordered item flat. A plain
                    // line still indents by the ordinary step.
                    let unit = indent_unit(line);
                    for _ in 0..unit {
                        out.push(' ');
                    }
                    out.push_str(line);
                    unit as isize
                }
            } else {
                // Outdent one level: a nested item gives back its marker width,
                // an ordinary line the ordinary step (Shift+Tab unnests in one
                // press, the mirror of the indent above).
                let strip = outdent_width(line, indent_unit(line));
                out.push_str(&line[strip..]);
                -(strip as isize)
            };
            deltas.push(delta);
            line_off += line.len() + 1;
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
    ///   - paragraph / heading  → a new paragraph (blank line), or a single soft
    ///                            break under [`LineFlow::Preserve`], where that
    ///                            break renders as a visible line
    ///   - list item            → the next item (same bullet, next number);
    ///                            an *empty* item exits the list
    ///   - block quote          → a new quoted line
    ///   - code block           → a literal newline (stay in the block)
    pub fn newline(&mut self) {
        if self.view == View::Source {
            self.insert_raw("\n");
            return;
        }
        // Enter over a selection replaces it with a paragraph break.
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "\n\n", EditKind::Other);
            return;
        }
        // A caret resting exactly between an inline mark's content and its own
        // closing delimiter (`**bold**` with nothing after it on the line —
        // the WYSIWYG caret's natural end-of-line position) must not splice a
        // block break there: every path below eventually does via
        // `insert_raw`/`self.caret`, and splicing before the hidden closing
        // delimiter would strand it alone on the new line.
        self.caret = self.skip_trailing_close_delims(self.caret);
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
            self.insert_raw("\n");
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
            // A new item mid-list leaves the source markers stale (`next_list_marker`
            // only bumps the one it wrote); renumber the whole list to match.
            self.renumber_here();
            return;
        }
        if has("block_quote") {
            self.insert_raw("\n> ");
            return;
        }
        // On an *empty* paragraph line, a lone Enter should add a single blank line,
        // not another full paragraph break — so it moves down one line and one
        // Backspace undoes it, not two.
        let line_start = self.source[..self.caret].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.source[self.caret..]
            .find('\n')
            .map_or(self.source.len(), |i| self.caret + i);
        if self.source[line_start..line_end].trim().is_empty() {
            self.insert_raw("\n");
            return;
        }
        // A non-empty paragraph normally opens a fresh paragraph with `\n\n`
        // (splitting it there when the caret is mid-line). In `Preserve` flow a
        // soft break is a *visible* line the author means to make, so Enter writes
        // a single `\n` and typing continues the same paragraph on the next line —
        // the behaviour of an ordinary text editor. A second Enter then lands on
        // the blank line above and takes the empty-line branch, so double-Enter
        // still promotes to a full paragraph break; and Backspace, which deletes a
        // lone `\n` over a soft break, undoes a single Enter symmetrically. In
        // `Fold` flow a lone `\n` would render as an invisible space, so Enter
        // keeps making the paragraph break that actually shows.
        if self.line_flow == LineFlow::Preserve {
            self.insert_raw("\n");
            return;
        }
        self.insert_raw("\n\n");
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
            self.insert_raw(&format!("\n{}", next_list_marker(&marker)));
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

    /// Whether the list item on `line_start`'s line is the **first item** of its
    /// list — the one Tab must not nest, because nesting needs a preceding
    /// sibling to become the new parent and a first item has none. `false` for a
    /// line that isn't a list item, and for an item with a sibling above it (the
    /// one Tab *can* nest). Gated on the AST, not the marker bytes: `- ` reads
    /// the same in a setext underline that opens no list at all.
    fn first_item_of_list(&mut self, line_start: usize) -> bool {
        let Some((_, marker)) = self.list_marker_on_line(line_start) else {
            return false;
        };
        // Probe just inside the marker, where the item's own node is in reach —
        // the marker offset itself can resolve to the enclosing list, not the
        // `list_item`, whose span starts at the marker.
        let probe = (line_start + marker.len()).min(self.source.len());
        let Ok(nodes) = self.editor.nodes() else {
            return false;
        };
        // The innermost list item covering the probe (smallest span wins).
        let Some(item) = nodes
            .iter()
            .filter(|n| {
                (n.kind == "list_item" || n.kind == "task_list_item")
                    && n.span.start <= probe
                    && probe < n.span.end
            })
            .min_by_key(|n| n.span.end - n.span.start)
        else {
            return false;
        };
        match item.parent {
            // First when the parent list opens with this very item.
            Some(pid) => nodes
                .get(pid.0 as usize)
                .is_some_and(|p| p.first_child == Some(item.id)),
            // A parentless item is trivially the first (and only) one.
            None => true,
        }
    }

    pub fn backspace(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
            return;
        }
        // WYSIWYG: Backspace at the very start of a list item's content is a
        // structural key, not a character delete — it walks the "un-indent, then
        // un-list" ladder every list editor gives that keystroke (outdent a
        // nested item, strip a top-level one's marker to a paragraph). In source
        // view the `- ` is visible text the user is deleting a byte of, so it
        // keeps its literal meaning there, like Enter does.
        if self.view != View::Source && self.backspace_list_start() {
            return;
        }
        // WYSIWYG: and the same at the start of a heading's content — the `# `
        // there is markup the rich view hides, not text the user typed.
        if self.view != View::Source && self.backspace_heading_start() {
            return;
        }
        // WYSIWYG: at a block picture's stops, a byte-at-a-time delete would take
        // the markup apart under a caret that cannot see it — see
        // `delete_around_block_media`.
        if self.view != View::Source && self.delete_around_block_media(false) {
            return;
        }
        // WYSIWYG: Backspace on a *blank line* deletes back to the previous caret
        // stop, not a single newline. On a line with no text of its own, the byte
        // before the caret is a `\n` that spells part of a block boundary — the gap
        // between two blocks, drawn but never a caret home. Removing just it strands
        // the caret in that gap and leaves an odd blank line the eye reads as one
        // separator but the caret can't land on: the "extra newline" left behind
        // after leaving a list (Enter, Enter) or a paragraph and pressing Backspace.
        // Deleting to the previous stop instead collapses the whole break at once,
        // landing the caret at the end of the block above. Two blank lines in a row
        // are one stop apart, so this still removes exactly one — the lone-Enter /
        // lone-Backspace symmetry the empty-line case is built on is untouched.
        if self.view != View::Source
            && self.caret > self.caret_floor()
            && self.caret_on_blank_line()
            && let Some(stop) = self.vmap.stop_before(self.caret)
        {
            let stop = stop.max(self.caret_floor());
            if stop < self.caret {
                self.splice(stop, self.caret, "", EditKind::Delete);
                return;
            }
        }
        if self.caret > self.caret_floor() {
            // An in-cell `<br>` draws as one newline glyph, so Backspace over it
            // takes the whole tag — a single-byte step would leave a broken `<br`
            // showing in the cell. Rich view only (source view edits the literal).
            if self.view != View::Source
                && let Some((start, end)) = self.cell_break_at(BreakEdge::Backward)
            {
                let start = start.max(self.caret_floor());
                if start < end {
                    self.splice(start, end, "", EditKind::Delete);
                    return;
                }
            }
            // Aim the delete at the character the writer can *see* behind the
            // caret, never at a delimiter the rich view drew nothing for. Two
            // steps, and either can apply: from the far side of a run's closing
            // `**` step back into the run (the caret is drawn at the end of its
            // word), and at the start of a run's text step out past its opening
            // `**` to the character in front of it, leaving the run standing.
            // Without them a plain Backspace unspells the phrase it is editing
            // and leaves a literal asterisk on screen.
            let end = if self.view == View::Source {
                self.caret
            } else {
                let inside = self.step_inside_close_delims(self.caret);
                self.skip_leading_open_delims(inside).max(self.caret_floor())
            };
            // Never delete back across the floor — that would eat hidden
            // frontmatter the WYSIWYG caret can't even see.
            let mut prev = prev_boundary(&self.source, end).max(self.caret_floor());
            // Take a hidden escape backslash with the char it escapes: the rich
            // view draws `\*` as a single `*`, so Backspace over it must delete
            // both bytes, never strand the `\` as a lone visible backslash (the
            // mirror of the Hidden-mode typing that wrote the escape). Source view
            // shows the `\`, so there it is an ordinary character.
            if self.view != View::Source
                && prev > self.caret_floor()
                && self.is_hidden_escape(prev - 1)
            {
                prev -= 1;
            }
            if prev < end {
                self.splice(prev, end, "", EditKind::Delete);
            }
        }
    }

    /// Whether the caret's own source line holds nothing but whitespace — an
    /// empty paragraph, or the blank line a block boundary is spelled with. The
    /// test for [`backspace`](Self::backspace)'s stop-wise delete: such a line has
    /// no text of its own, so the newline before the caret belongs to the gap
    /// between blocks rather than to any word the caret is editing.
    fn caret_on_blank_line(&self) -> bool {
        let line_start = self.source[..self.caret].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.source[self.caret..]
            .find('\n')
            .map_or(self.source.len(), |i| self.caret + i);
        self.source[line_start..line_end].trim().is_empty()
    }

    /// The source span of an in-cell hard break (`<br>`) touching the caret on the
    /// `edge` side — the byte range to delete whole. A table row is one source
    /// line, so its break is spelled `<br>` yet drawn as a single newline glyph
    /// (see `wysiwyg.rs`); a delete over it must take every byte, or a one-byte
    /// step strands a broken `<br` in the cell. `Backward` matches a break ending
    /// at the caret (Backspace), `Forward` one starting at it (Delete). `None`
    /// when no such break is adjacent. Only the in-cell break is spelled `<br>`
    /// (an ordinary hard break is `  \n`), so the leading `<` alone tells them
    /// apart — no ancestor walk needed. Rich view only; source view shows the
    /// literal tag and deletes it a byte at a time.
    fn cell_break_at(&mut self, edge: BreakEdge) -> Option<(usize, usize)> {
        let caret = self.caret;
        let nodes = self.nodes();
        let src = self.source.as_bytes();
        nodes
            .iter()
            .find(|n| {
                n.kind == "hard_break"
                    && n.span.start < n.span.end
                    && src.get(n.span.start) == Some(&b'<')
                    && match edge {
                        BreakEdge::Backward => n.span.end == caret,
                        BreakEdge::Forward => n.span.start == caret,
                    }
            })
            .map(|n| (n.span.start, n.span.end))
    }

    /// Whether the source byte at `off` is a backslash twig consumed as an escape
    /// (hidden in the rich view), as against a literal backslash (drawn). A
    /// backslash escapes exactly an ASCII-punctuation character (the CommonMark /
    /// Djot rule twig follows), so `\` + punctuation is the whole test — no AST
    /// round-trip needed.
    fn is_hidden_escape(&self, off: usize) -> bool {
        let b = self.source.as_bytes();
        b.get(off) == Some(&b'\\') && b.get(off + 1).is_some_and(u8::is_ascii_punctuation)
    }

    /// Backspace's list behaviour: when the caret sits exactly at the start of a
    /// list item's content (right after its marker), outdent the item if it's
    /// nested, else strip the marker so it becomes a paragraph. Returns whether
    /// it acted — `false` leaves Backspace its ordinary character delete.
    fn backspace_list_start(&mut self) -> bool {
        let Some((line_start, marker)) = self.list_marker_on_line(self.caret) else {
            return false;
        };
        // Only right after the marker, and only in a real list (an AST-gated
        // test — `- ` bytes alone read the same in a setext underline).
        if self.caret != line_start + marker.len() || !self.is_inside_list(line_start) {
            return false;
        }
        if marker.len() > marker.trim_start().len() {
            // Nested (the marker carries leading indent): give back one level,
            // keeping the marker and carrying the caret with it.
            self.outdent();
        } else {
            // Top level: drop the marker, leaving a paragraph, then renumber the
            // siblings the removed item was counted among.
            self.splice(line_start, self.caret, "", EditKind::Other);
            self.renumber_here();
        }
        true
    }

    /// Backspace's heading behaviour: with the caret exactly at the start of an
    /// ATX heading's content — right after the `#` marker the rich view hides —
    /// strip the marker so the line becomes a paragraph. The peer of
    /// [`backspace_list_start`](Self::backspace_list_start)'s ladder, and the same
    /// reasoning: hidden block markup is structure, so the keystroke over it is
    /// structural.
    ///
    /// Without this the ordinary delete takes the space out of `# Title` and
    /// leaves `#Title`, which is no longer a heading at all — the hash the view
    /// had been hiding surfaces as literal text the user has to delete a second
    /// time, having never typed it. A closing sequence (`# Title #`, hidden at the
    /// other end) goes with the marker for the same reason.
    ///
    /// Returns whether it acted; `false` leaves Backspace its character delete.
    fn backspace_heading_start(&mut self) -> bool {
        let caret = self.caret;
        // The heading whose content opens exactly at the caret. A bare `#` has no
        // content span at all — its content starts (and ends) where the line does.
        let Some((span, content_end)) = self.nodes().iter().find_map(|n| {
            let (start, end) = match &n.content_span {
                Some(c) => (c.start, c.end),
                None => (n.span.end, n.span.end),
            };
            (n.kind == "heading" && start == caret).then(|| (n.span.clone(), end))
        }) else {
            return false;
        };
        // Walk back over the marker: the space between it and the text, then the
        // hashes. A setext heading has neither — its content opens the line — so
        // it falls through to the ordinary delete, as does anything else sitting
        // at a content start.
        let bytes = self.source.as_bytes();
        let mut start = caret;
        while start > span.start && matches!(bytes[start - 1], b' ' | b'\t') {
            start -= 1;
        }
        let spaces_end = start;
        while start > span.start && bytes[start - 1] == b'#' {
            start -= 1;
        }
        if start == spaces_end {
            return false;
        }
        // A closing `#` sequence is hidden too, so it can't be left behind. Only
        // when the tail really is one: trailing spaces alone are nothing to strip.
        let tail = &self.source[content_end..span.end];
        if tail.contains('#') && tail.chars().all(|c| c == '#' || c.is_whitespace()) {
            let kept = self.source[caret..content_end].to_string();
            self.splice(start, span.end, &kept, EditKind::Other);
            // The splice leaves the caret past the text it re-wrote; the caret
            // belongs where the content now starts, which is where it already was.
            self.caret = start;
            self.record_caret();
        } else {
            self.splice(start, caret, "", EditKind::Other);
        }
        true
    }

    pub fn delete_forward(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
        } else if self.caret < self.source.len() {
            // The mirror of Backspace's: forward-delete in front of a picture
            // would eat the `!` off its markup and leave a link where a photo was.
            if self.view != View::Source && self.delete_around_block_media(true) {
                return;
            }
            // Delete forward over an in-cell `<br>` takes the whole tag, the mirror
            // of Backspace's swallow (see `cell_break_at`) — else a byte-step
            // strands a broken `<br` in the cell.
            if self.view != View::Source
                && let Some((start, end)) = self.cell_break_at(BreakEdge::Forward)
            {
                self.splice(start, end, "", EditKind::Delete);
                return;
            }
            // The mirror of Backspace's two steps: from in front of a run's
            // opening `**` step into it, onto the first letter of its text, and
            // at the end of a run's text step out past its closing `**` to the
            // character beyond. Either way Delete takes the character it looks
            // like it is pointing at, and never a delimiter drawn as nothing.
            // The caret then settles back inside the run it was standing in —
            // see `settle_inside_close_delims`.
            let from = if self.view == View::Source {
                self.caret
            } else {
                let inside = self.step_inside_open_delims(self.caret);
                self.skip_trailing_close_delims(inside)
            };
            let next = next_boundary(&self.source, from);
            if from < next {
                self.splice(from, next, "", EditKind::Delete);
            }
        }
    }

    /// Delete from the caret back to the start of the previous word (⌥⌫ /
    /// Ctrl+⌫). Deletes the selection instead when one is active.
    pub fn delete_word_back(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "", EditKind::Other);
        } else {
            // A word back from just past a picture is a word *of its markup*, and
            // a word back from in front of one runs through the paragraph break
            // into the prose above — dissolving the picture either way. See
            // `delete_around_block_media`.
            if self.view != View::Source && self.delete_around_block_media(false) {
                return;
            }
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
            // The mirror: a word forward from in front of a picture is its markup.
            if self.view != View::Source && self.delete_around_block_media(true) {
                return;
            }
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
            for n in nodes.iter().filter(|n| wysiwyg::is_inline(n)) {
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

    /// One splice of document text, keeping the **mark-edge rule**: an inline
    /// mark's content never begins or ends with whitespace. In Markdown and Djot
    /// a delimiter standing against a space is not a delimiter at all — `**bold **`
    /// is four literal asterisks around a word, and a rich view drawing the
    /// document faithfully has no choice but to show them. That is correct
    /// rendering of what the file says, and nobody typing a space after a bold
    /// word meant to say it.
    ///
    /// So the space goes *outside* the run instead — `**bold** ` — which is the
    /// same document to a reader and a live one to a parser. The caret follows it
    /// out and keeps the marks armed (see [`rearm`](Self::rearm)), so the next
    /// character rejoins the run (see [`rejoin_run`](Self::rejoin_run)) and the
    /// writer sees one unbroken bold phrase, never a flash of raw syntax.
    ///
    /// Every ordinary edit — typing, deleting, pasting, an IME step — comes
    /// through here, so the rule holds however the whitespace arrives at the
    /// edge. The repair is decided *after* the plain edit, by asking whether the
    /// mark actually died: a code span's backticks aren't whitespace-sensitive
    /// (`` `code ` `` is still code), and nothing is re-spelled when nothing broke.
    fn splice(&mut self, start: usize, end: usize, text: &str, kind: EditKind) -> bool {
        let fix = self.mark_edge_fix(start, end, text);
        if !self.splice_exact(start, end, text, kind) {
            return false;
        }
        if let Some(fix) = fix {
            self.repair_mark_edges(fix);
        }
        if text.is_empty() && end > start {
            self.settle_inside_close_delims();
        }
        true
    }

    /// After a delete, take a caret left standing past a run's closing delimiters
    /// back inside the run.
    ///
    /// A delete leaves the caret where the deleted bytes began, and when those
    /// bytes were the last thing after a marked phrase — the space the mark-edge
    /// rule pushed out of `**bold** `, say — that spot is the far side of the
    /// closing `**`. The rich view has nothing to draw there: the delimiters are
    /// hidden, so the caret shows at the end of the word either way, and the two
    /// offsets are one place on screen with two different meanings. Typing at the
    /// outer one lands past the run, so the writer who backspaced a space out of
    /// their bold phrase watches the next character come out plain, and the
    /// toolbar button go dark, with the caret never appearing to move.
    ///
    /// The end of the run's text is the caret's home there — a delete that took
    /// away everything after a phrase leaves the caret at the end of that phrase,
    /// which is inside it — so it settles onto that
    /// ([`step_inside_close_delims`](Self::step_inside_close_delims) does the
    /// walk, through every mark closing at the point): the word stays bold, the
    /// button stays lit, and the next character carries on the phrase.
    ///
    /// Rich view only, and only where a mark really closes at the caret — mid-run
    /// or in plain prose no span ends there and the caret stays put. The opening
    /// edge is left alone on purpose: a caret in front of a run inherits from the
    /// text on its left, which is the plain text outside.
    fn settle_inside_close_delims(&mut self) {
        if self.view != View::Wysiwyg {
            return;
        }
        let at = self.step_inside_close_delims(self.caret);
        if at != self.caret {
            self.caret = at;
            self.clear_pending();
            self.record_caret();
        }
    }

    /// The splice exactly as asked, with no mark-edge repair — for the callers
    /// that are *writing* the delimiters themselves ([`insert_with_marks`](Self::insert_with_marks)
    /// and [`rejoin_run`](Self::rejoin_run)) and place their own offsets around
    /// the bytes they inserted.
    ///
    /// One `edit_range` through twig, then re-anchor the caret from the returned
    /// `Change` and refresh the cached source. A reparse-breaking edit (rare for
    /// Markdown/Djot) leaves the document untouched and reports.
    ///
    /// Returns whether the edit landed — for a caller that has offsets of its
    /// own to place afterwards, which a rolled-back splice would leave pointing
    /// into text that never came to exist.
    fn splice_exact(&mut self, start: usize, end: usize, text: &str, kind: EditKind) -> bool {
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
                self.clear_pending();
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

    /// The re-spelling that would keep the mark-edge rule for the edit
    /// `[start, end)` → `text`, or `None` when the edit leaves no whitespace
    /// against a delimiter and the plain splice is already right. Computed
    /// *before* the edit, while the run's spans and delimiters can still be read
    /// off the document; applied afterwards, and only if the mark really died —
    /// see [`repair_mark_edges`](Self::repair_mark_edges).
    ///
    /// Rich view only. Source view is for typing raw markup, where a space put
    /// against a `**` is exactly the character it looks like.
    fn mark_edge_fix(&mut self, start: usize, end: usize, text: &str) -> Option<MarkEdgeFix> {
        if self.view != View::Wysiwyg || start > end || end > self.source.len() {
            return None;
        }
        // Every inline mark standing over the edit, outermost first, with the
        // content span that says where its delimiters are.
        let chain: Vec<(InlineKind, std::ops::Range<usize>, std::ops::Range<usize>)> = self
            .editor
            .ancestors_at(start)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| {
                let kind = inline_kind(&m.kind)?;
                let content = m.content_span.clone()?;
                Some((kind, m.span.clone(), content))
            })
            .collect();
        // The innermost run whose *content* holds the whole edit: the one whose
        // text is being changed, rather than one the edit merely sits under.
        let (kind, span, content) = chain
            .iter()
            .rev()
            .find(|(_, _, c)| c.start <= start && end <= c.end)?
            .clone();
        // What that content becomes. Whitespace at either end of it is what
        // would put out the mark.
        let body = format!(
            "{}{text}{}",
            &self.source[content.start..start],
            &self.source[end..content.end]
        );
        let (lead, trail) = if body.trim().is_empty() {
            // Nothing but whitespace left: there is no content to mark at all,
            // and the delimiters go with it rather than closing on a space.
            (body.len(), 0)
        } else {
            (
                body.len() - body.trim_start().len(),
                body.len() - body.trim_end().len(),
            )
        };
        // Nothing against a delimiter, and something still between them: the
        // plain edit stands. An emptied run is broken just as surely (`**b**`
        // with the `b` deleted is the literal `****`) and is re-spelt as the
        // nothing it now says.
        if lead == 0 && trail == 0 && !body.is_empty() {
            return None;
        }
        // Marks that open or close exactly where this one does — `***both***` is
        // two runs sharing an edge — spell their delimiters as one run of bytes,
        // so the whitespace has to clear all of them together.
        let (mut open_at, mut close_at) = (span.start, span.end);
        for _ in 0..chain.len() {
            match chain.iter().find(|(_, _, c)| c.start == open_at) {
                Some((_, s, _)) => open_at = s.start,
                None => break,
            }
        }
        for _ in 0..chain.len() {
            match chain.iter().find(|(_, _, c)| c.end == close_at) {
                Some((_, s, _)) => close_at = s.end,
                None => break,
            }
        }
        let open = &self.source[open_at..content.start];
        let close = &self.source[content.end..close_at];
        let core = &body[lead..body.len() - trail];
        let respelt = if core.is_empty() {
            body.clone()
        } else {
            format!("{}{open}{core}{close}{}", &body[..lead], &body[body.len() - trail..])
        };
        // The caret sits just past the inserted text within the new content —
        // which, when that lands in the whitespace, is now outside the delimiters.
        let pos = (start - content.start) + text.len();
        let caret = if core.is_empty() || pos <= lead {
            open_at + pos
        } else if pos >= lead + core.len() {
            open_at + lead + open.len() + core.len() + close.len() + (pos - lead - core.len())
        } else {
            open_at + lead + open.len() + (pos - lead)
        };
        Some(MarkEdgeFix {
            kind,
            probe: content.start,
            start: open_at,
            end: close_at + text.len() - (end - start),
            text: respelt,
            caret,
            // The marks in force here, resolved against any armed sticky delta —
            // what the writer is typing in, and so what has to still be true on
            // the far side of the delimiter the caret just stepped over.
            want: chain
                .iter()
                .filter(|(_, s, _)| start < s.end)
                .map(|(k, _, _)| *k)
                .collect::<InlineMarks>()
                .xor(self.pending_here()),
        })
    }

    /// Apply a [`MarkEdgeFix`] — but only if the edit it was computed for really
    /// did break the mark. Whether whitespace at a delimiter is fatal is the
    /// format's business, not leaf's: `**bold **` is no longer strong, while
    /// `` `code ` `` is still perfectly good verbatim, and Djot's braced spellings
    /// don't care either. Asking the parser afterwards settles it for every kind
    /// and format at once, and costs a re-spelling only where one is due.
    ///
    /// The repair rides along with the edit that caused it — one undo step puts
    /// back what the writer typed, not a delimiter shuffle they never saw.
    fn repair_mark_edges(&mut self, fix: MarkEdgeFix) {
        if fix.end > self.source.len() {
            return;
        }
        if self.marks_at(fix.probe).iter().any(|(k, _)| *k == fix.kind) {
            return; // still a mark: these delimiters don't mind the whitespace
        }
        let resumed = self.last_edit_kind;
        if !self.splice_exact(fix.start, fix.end, &fix.text, EditKind::Other) {
            return;
        }
        let _ = self.editor.coalesce_last_undo();
        // The keystroke owns the undo step, so the run of typing it belongs to
        // keeps coalescing over the repair rather than breaking in two here.
        self.last_edit_kind = resumed;
        self.caret = fix.caret.min(self.source.len());
        self.anchor = None;
        self.goal_col = None;
        self.rearm(fix.want);
        self.clamp_caret();
        self.record_caret();
    }

    /// Arm whatever sticky delta reproduces `want` at the caret — the marks the
    /// writer is typing in, carried across an edit that moved the caret out of
    /// the run holding them. Arms nothing when the caret already stands in
    /// exactly those marks, but still remembers the spot, so a further ⌘b starts
    /// a clean delta here (see [`toggle`](Self::toggle)).
    fn rearm(&mut self, want: InlineMarks) {
        let here: InlineMarks = self.marks_at(self.caret).into_iter().map(|(k, _)| k).collect();
        self.pending_marks = want.xor(here);
        self.pending_at = Some(self.caret);
    }

    /// Insert `text` at `at` as a *literal* run via twig's `insert_literal`,
    /// which backslash-escapes any character that would otherwise open markup in
    /// this format and position (`*` → `\*`, a line-start `#` → `\#`). The mirror
    /// of [`splice`](Self::splice) for the Hidden reveal mode's typing path, with
    /// the same caret re-anchor, coalescing, and rollback contract. `at` must be
    /// a collapsed point — a selection is deleted by the caller first, since
    /// `insert_literal` inserts rather than replaces.
    fn insert_literal_at(&mut self, at: usize, text: &str, kind: EditKind, force_coalesce: bool) -> bool {
        // `force_coalesce` folds this into the immediately preceding edit (the
        // selection-delete of an overwrite) so the pair is one undo step; else it
        // coalesces only when it continues a run of the same-kind typing.
        let coalesce = force_coalesce || (kind != EditKind::Other && self.last_edit_kind == Some(kind));
        // The mark-edge rule holds for typed text however it is spelled — see
        // `splice`. Only an insert twig passed through unchanged can use it,
        // since a fix is measured in the bytes that actually land, and an escape
        // adds bytes this couldn't have counted.
        let fix = self.mark_edge_fix(at, at, text);
        self.record_caret();
        match self.editor.insert_literal(at, text) {
            Ok(change) => {
                if coalesce {
                    let _ = self.editor.coalesce_last_undo();
                }
                self.last_edit_kind = Some(kind);
                self.refresh();
                self.caret = change.new.end;
                self.anchor = None;
                self.goal_col = None;
                self.clear_pending();
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.record_caret();
                if let Some(fix) = fix.filter(|_| change.new.end - change.new.start == text.len()) {
                    self.repair_mark_edges(fix);
                }
                true
            }
            Err(e) => {
                self.status = Some(format!("edit: {e}"));
                false
            }
        }
    }

    /// After a structural list edit (a new item, a nest/unnest), renumber the
    /// ordered list the caret sits in so its source markers run `1, 2, 3, …`
    /// again — a raw splice leaves them stale (`1. 2. 2. 3.`). twig does the
    /// renumber as its own edit; fold it into the edit that triggered it so the
    /// two undo as one, and only when it actually changed the source (a no-op or
    /// a caret outside any ordered list must not coalesce the real edit into the
    /// step before it).
    fn renumber_here(&mut self) {
        let before = self.source.clone();
        if self.editor.renumber_ordered_lists(self.caret).is_err() {
            return; // not inside an ordered list — nothing to renumber
        }
        self.refresh();
        if self.source != before {
            let _ = self.editor.coalesce_last_undo();
            self.dirty = self.source != self.clean_source;
            self.clamp_caret();
            self.record_caret();
        }
    }

    /// Repair the one trap a list edit can spring on itself. An *empty* `-`
    /// sub-item written directly beneath a text line reparses that text as a
    /// setext heading — `- hello\n  - ` is `<h2>hello</h2>`, because a lone `-`
    /// is also a setext-H2 underline (twig is right; pandoc agrees). `*` and `+`
    /// bullets can't underline anything, so swap the dash for a `*`: the item
    /// stays an empty nested bullet, the parent stays prose, and the source
    /// round-trips instead of hiding a heading the user never asked for. Folded
    /// into the triggering edit's undo step, the way renumbering is.
    ///
    /// Gated on the collapse having actually happened (the swapped dash was
    /// swallowed into a `heading`), so a real setext heading the author wrote —
    /// or a `- x` with content, which can't underline anything — is never
    /// touched. This has to live in the *edit*, not the renderer: leaving the
    /// hazardous bytes on disk and only painting over them would ship a file
    /// every other CommonMark tool reads as a heading.
    fn avoid_setext_collapse(&mut self) {
        let Some((line_start, marker)) = self.list_marker_on_line(self.caret) else {
            return;
        };
        let indent_len = marker.len() - marker.trim_start().len();
        // A dash bullet is the only marker that doubles as a setext underline.
        if marker.as_bytes().get(indent_len) != Some(&b'-') {
            return;
        }
        // Only an *empty* item is a bare underline; `- x` carries content and
        // can't fold the line above into a heading.
        let content_start = line_start + marker.len();
        let line_end = self.source[content_start..]
            .find('\n')
            .map_or(self.source.len(), |i| content_start + i);
        if !self.source[content_start..line_end].trim().is_empty() {
            return;
        }
        // The tell: that dash was swallowed into a `heading`. A properly nested
        // empty item sits under a `list_item`, with no heading in reach. Probe
        // the dash byte itself (well inside the heading), not the caret, whose
        // end-of-line offset can fall on the half-open span boundary.
        let dash = line_start + indent_len;
        let collapsed = self
            .editor
            .ancestors_at(dash)
            .map(|c| c.into_iter().any(|m| m.kind == "heading"))
            .unwrap_or(false);
        if !collapsed {
            return;
        }
        let caret = self.caret;
        if self.splice(dash, dash + 1, "*", EditKind::Other) {
            // Same width, so the caret keeps its column; fold into the edit that
            // triggered this so Tab stays one undo step.
            let _ = self.editor.coalesce_last_undo();
            self.caret = caret.min(self.source.len());
            self.clamp_caret();
            self.record_caret();
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
            // No selection: arm the mark for the next text typed here, the way a
            // word processor does. `⌘b`, type, `⌘b` again toggles bold on and off
            // in the flow of typing without ever selecting anything — the delta
            // is realised onto the freshly typed text by `insert`. A fresh caret
            // position starts the delta over from the marks actually in force.
            if self.pending_at != Some(self.caret) {
                self.pending_marks = InlineMarks::empty();
                self.pending_at = Some(self.caret);
            }
            self.pending_marks.flip(kind);
            self.status = None;
            return;
        };
        // Whitespace at the edge of a selection is not part of what was chosen —
        // a double-click takes the space after the word with it — and a mark
        // cannot close against one anyway: `**word **` is four literal asterisks
        // (the mark-edge rule, see `splice`). Mark the words, leave the spaces.
        let picked = &self.source[s..e];
        let (s, e) = (
            s + (picked.len() - picked.trim_start().len()),
            e - (picked.len() - picked.trim_end().len()),
        );
        if s >= e {
            self.status = Some(format!("{kind:?}: nothing selected to mark"));
            return;
        }
        // Styling a selection is a one-shot act, not a sticky mode.
        self.clear_pending();
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
                .any(|m| !wysiwyg::is_inline_kind(&m.kind) && !is_block_container(&m.kind))
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
        self.insert_raw(&prefix);
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
            // The marks actually in force at the caret, flipped by any armed
            // sticky delta — so `⌘b` at a bare caret lights the Bold button
            // immediately, before a single character is typed.
            let base: InlineMarks = self.marks_at(self.caret).into_iter().map(|(k, _)| k).collect();
            return base.xor(self.pending_here());
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

    // ── Tables ───────────────────────────────────────────────────────────────
    // A table is a grid, and twig edits it as one — add/remove/move a row or
    // column, set a column's alignment — re-spelling the whole table in a single
    // splice. Every gesture is anchored at the caret's cell. leaf just names the
    // gesture and re-reads the result; the whole table's numbering, borders, and
    // delimiter are twig's to keep straight.

    /// Whether the caret is inside a table — what a frontend asks to enable or
    /// disable its table controls.
    pub fn caret_in_table(&mut self) -> bool {
        let caret = self.caret.min(self.source.len());
        self.editor
            .ancestors_at(caret)
            .map(|c| c.into_iter().any(|m| m.kind == "table"))
            .unwrap_or(false)
    }

    /// Insert an empty row below (`below`) or above the caret's row.
    pub fn table_insert_row(&mut self, below: bool) {
        self.record_caret();
        let r = self.editor.table_insert_row(self.caret, below);
        self.apply_table(r, "table row");
    }

    /// Delete the caret's row (not the header, not the last body row).
    pub fn table_delete_row(&mut self) {
        self.record_caret();
        let r = self.editor.table_delete_row(self.caret);
        self.apply_table(r, "table row");
    }

    /// Insert an empty column right (`right`) or left of the caret's column.
    pub fn table_insert_column(&mut self, right: bool) {
        self.record_caret();
        let r = self.editor.table_insert_column(self.caret, right);
        self.apply_table(r, "table column");
    }

    /// Delete the caret's column (unless it is the only one).
    pub fn table_delete_column(&mut self) {
        self.record_caret();
        let r = self.editor.table_delete_column(self.caret);
        self.apply_table(r, "table column");
    }

    /// Set the caret's column to `alignment`.
    pub fn table_set_alignment(&mut self, alignment: Alignment) {
        self.record_caret();
        let r = self.editor.table_set_alignment(self.caret, alignment);
        self.apply_table(r, "table alignment");
    }

    /// Move the caret's row one place down (`down`) or up, within the body rows.
    pub fn table_move_row(&mut self, down: bool) {
        self.record_caret();
        let r = self.editor.table_move_row(self.caret, down);
        self.apply_table(r, "table row");
    }

    /// Move the caret's column one place right (`right`) or left.
    pub fn table_move_column(&mut self, right: bool) {
        self.record_caret();
        let r = self.editor.table_move_column(self.caret, right);
        self.apply_table(r, "table column");
    }

    /// Settle the caret and document flags after a table op (or report its
    /// error). twig re-spells the whole table, so the caret rides its old byte
    /// offset and is clamped back into the rebuilt bytes — near enough to where
    /// it was, since the op preserves the cells' content and order around it.
    fn apply_table(&mut self, result: Result<(), twig::Error>, what: &str) {
        match result {
            Ok(()) => {
                self.last_edit_kind = None;
                self.refresh();
                self.anchor = None;
                self.clamp_caret();
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("{what}: {e}")),
        }
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

    /// Both halves go through twig (`insert_literal` for the alt text,
    /// `insert_image` for the image), so neither is spelled here. That used to be a
    /// `format!`, and it was wrong the first time an app inserted a real filename:
    /// Markdown ends a destination at the first space, so `![](my photo.png)` is
    /// not an image at all — and the fix is per-format, since moving into the
    /// `<…>` form is exactly wrong for Djot, where `<…>` becomes the URL itself.
    pub fn insert_image(&mut self, destination: &str, alt: &str) {
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        self.record_caret();
        // With no selection and an explicit `alt`, the alt text has to exist in the
        // document before it can be the image's — and it is raw caller input, so
        // it goes in through `insert_literal`, which escapes it for the format
        // rather than letting a `]` in someone's caption close the image early.
        let (start, end) = if start == end && !alt.is_empty() {
            match self.editor.insert_literal(start, alt) {
                Ok(change) => (change.new.start, change.new.end),
                Err(e) => {
                    self.status = Some(format!("image: {e}"));
                    return;
                }
            }
        } else {
            (start, end)
        };
        match self.editor.insert_image(start, end, destination) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                // Just past the image, nothing selected — where a caret belongs
                // after inserting one.
                self.anchor = None;
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("image: {e}")),
        }
    }

    /// Insert a block-level image, video, or audio at the caret. The image case
    /// is [`insert_image`](Self::insert_image); video and audio are spelled as
    /// HTML elements, which is the only spelling Markdown and Djot have for them:
    ///
    /// ```text
    /// <video src="clip.mp4" controls>alt</video>
    /// <audio src="take.mp3" controls>alt</audio>
    /// ```
    ///
    /// HTML rather than a `::video{…}` directive deliberately. A directive means
    /// something only to an app that knows the vocabulary, so the document would
    /// read as literal punctuation everywhere else; `<video>` is what every other
    /// renderer already understands, and what leaf's own reader picks back up
    /// through `html_elements` promotion (see [`parse_extensions`]).
    ///
    /// The one-line spelling needs twig ≥ 2.5.1, which widened CommonMark's
    /// HTML-block tag list to cover `<video>`/`<audio>`/`<picture>` under
    /// `html_elements`. Before that only the multi-line form parsed as a block at
    /// all, and this wrote three lines to work around it.
    ///
    /// `controls` is always written: a player with no transport is a still frame
    /// the reader can't do anything with. Any selection becomes the element's
    /// fallback text, exactly as it becomes an image's alt.
    ///
    /// The same verbatim-insertion caveat as [`insert_image`](Self::insert_image)
    /// applies, and bites harder here: a `"` in `destination` closes the
    /// attribute. A frontend taking these from a file picker is fine; one taking
    /// them from free text should keep them tame.
    ///
    /// [`MediaInfo`]: crate::MediaInfo
    pub fn insert_media(&mut self, kind: MediaKind, destination: &str, alt: &str) {
        if kind == MediaKind::Image {
            return self.insert_image(destination, alt);
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let alt_text = self
            .selected_text()
            .map(str::to_string)
            .unwrap_or_else(|| alt.to_string());
        let tag = match kind {
            MediaKind::Audio => "audio",
            _ => "video",
        };
        let markup = format!("<{tag} src=\"{destination}\" controls>{alt_text}</{tag}>");
        self.edit(start, end, &markup);
    }

    /// Insert a thematic break (`---`) at the caret — the toolbar's Horizontal
    /// Rule button. Twig has no primitive for minting a brand-new block (only
    /// for retagging or wrapping one that already exists), so this leans on the
    /// same block-context reasoning [`newline`](Self::newline) uses for Enter: a
    /// selection is replaced outright, a blank line gets the rule with no
    /// leading break, and non-blank text is split into a paragraph break first —
    /// mid-word, mid-list-item, or mid-quote, wherever the caret happens to be.
    ///
    /// The blank line on both sides is not cosmetic: a bare `---` with no blank
    /// line above it is a *setext* heading underline in Markdown, not a rule,
    /// and one with no blank line below would fuse with whatever follows. The
    /// leading break's un-indented `---` also does the work of leaving a list or
    /// quote — CommonMark and djot both end a container on unindented content,
    /// so the rule always lands at the top level rather than nested a level deep
    /// inside whatever the caret started in.
    ///
    /// Refuses inside a code block, where `---` would be three literal
    /// characters of code rather than a rule.
    pub fn insert_thematic_break(&mut self) {
        self.caret = self.skip_trailing_close_delims(self.caret);
        let off = self.block_offset_for_caret().unwrap_or(self.caret);
        let in_code_block = self
            .editor
            .ancestors_at(off)
            .map(|c| c.into_iter().any(|m| m.kind == "code_block"))
            .unwrap_or(false);
        if in_code_block {
            self.status = Some("thematic break: not available inside a code block".into());
            return;
        }
        if let Some((s, e)) = self.selection() {
            self.splice(s, e, "\n\n---\n\n", EditKind::Other);
            return;
        }
        let line_start = self.source[..self.caret].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.source[self.caret..]
            .find('\n')
            .map_or(self.source.len(), |i| self.caret + i);
        if self.source[line_start..line_end].trim().is_empty() {
            self.insert_raw("---\n\n");
        } else {
            self.insert_raw("\n\n---\n\n");
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
            // The source view reaches every byte, so there is no stop to snap
            // to — but "every byte" still means every *character* boundary. A
            // caret resting inside a multi-byte character draws nowhere real
            // and panics the next time anything slices there.
            View::Source => {
                let mut o = offset.min(self.source.len());
                while o > 0 && !self.source.is_char_boundary(o) {
                    o -= 1;
                }
                o
            }
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
                    .find(|m| !wysiwyg::is_inline_kind(&m.kind) && !is_block_container(&m.kind))
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

    /// Land in a table cell with its whole content selected — the anchor at the
    /// cell's start, the caret at its end — so a Tab/Return hop into a cell reads
    /// like tabbing into a form field: the text comes up selected, so typing
    /// replaces it and an arrow collapses to an edge. An empty cell (`start ==
    /// end`) collapses to a plain caret home (an empty selection is no selection).
    fn select_cell(&mut self, start: usize, end: usize) {
        let floor = self.caret_floor();
        self.anchor = Some(start.min(self.source.len()).max(floor));
        self.caret = end.min(self.source.len()).max(floor);
        self.goal_col = None;
        self.status = None;
        self.last_edit_kind = None;
        self.clear_pending();
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
        // Moving away disarms any sticky mark — "start bold" applies only where
        // it was asked for, not wherever the caret next lands.
        self.clear_pending();
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

    /// Hop to the next (Tab) or previous (Shift+Tab) table cell, landing with the
    /// cell's whole content selected (see [`Self::select_cell`]). Returns `false`
    /// when the caret isn't in a table, or is already in the last/first cell — the
    /// frontend then does whatever Tab normally does (indent), so Tab keeps its
    /// meaning everywhere else.
    pub fn cell_hop(&mut self, forward: bool) -> bool {
        let Some((grid, r, c)) = self.table_grid_at(self.caret) else {
            return false;
        };
        // Flatten to document (row-major) order and step one cell either way.
        let i: usize = grid[..r].iter().map(Vec::len).sum::<usize>() + c;
        let flat: Vec<(usize, usize)> = grid.into_iter().flatten().collect();
        let next = if forward { i.checked_add(1) } else { i.checked_sub(1) };
        let Some(&(start, end)) = next.and_then(|j| flat.get(j)) else {
            return false; // at the table's edge; leave Tab to the frontend
        };
        self.select_cell(start, end);
        true
    }

    /// Move the caret to the cell directly above (`down == false`) or below in
    /// the same column, landing with the cell's whole content selected (see
    /// [`Self::select_cell`]). Returns `false` at the grid's top/bottom edge (or
    /// when the caret isn't in a table), so the frontend can fall through — the
    /// vertical counterpart of [`cell_hop`].
    ///
    /// A ragged row that is short a column clamps to its last cell, so Down never
    /// falls out of the table over a gap the row above happened to have.
    pub fn cell_move_vertical(&mut self, down: bool) -> bool {
        let Some((grid, r, c)) = self.table_grid_at(self.caret) else {
            return false;
        };
        let target = match down {
            true => r + 1,
            false if r == 0 => return false,
            false => r - 1,
        };
        let Some(row) = grid.get(target) else {
            return false;
        };
        let Some(&(start, end)) = row.get(c).or_else(|| row.last()) else {
            return false;
        };
        self.select_cell(start, end);
        true
    }

    /// The table containing `off` as a row-major grid of `(start, end)` cell
    /// caret homes, plus the `(row, col)` the caret sits in — `None` when `off`
    /// isn't in a table. Read straight off the visual map's laid-out grid, so
    /// every cell (an empty one included, whose derived home twig gives no
    /// `content_span` for) is present and in the order Tab walks them.
    fn table_grid_at(&self, off: usize) -> Option<(Vec<Vec<(usize, usize)>>, usize, usize)> {
        for t in &self.vmap.tables {
            let mut pos = None;
            let grid: Vec<Vec<(usize, usize)>> = t
                .grid
                .iter()
                .enumerate()
                .map(|(r, row)| {
                    row.cells
                        .iter()
                        .enumerate()
                        .map(|(c, cell)| {
                            if pos.is_none() && off >= cell.start && off <= cell.end {
                                pos = Some((r, c));
                            }
                            (cell.start, cell.end)
                        })
                        .collect()
                })
                .collect();
            if let Some((r, c)) = pos {
                return Some((grid, r, c));
            }
        }
        None
    }

    // ── table key policy ──────────────────────────────────────────────────────
    // The three keys a table gives its own meaning — Tab, Return, Shift+Return —
    // as one policy every frontend shares, rather than each re-deriving it. Each
    // reports whether it acted *as a table key*; a `false` hands the key back to
    // the frontend's ordinary handling (indent, newline) so it keeps its meaning
    // everywhere else.

    /// Tab / Shift+Tab inside a table. Tab steps to the next cell, appending a
    /// fresh row and entering it when it runs off the last one; Shift+Tab steps
    /// back and simply stays put at the very first cell. `false` when the caret
    /// isn't in a table.
    pub fn cell_tab(&mut self, forward: bool) -> bool {
        if !self.caret_in_table() {
            return false;
        }
        if self.cell_hop(forward) {
            return true;
        }
        // Off the last cell: grow the table by a row and step into its first
        // cell. (Shift+Tab at the first cell has nowhere to go and just holds.)
        if forward {
            self.append_row_and_enter(0);
        }
        true
    }

    /// Return inside a table: drop to the cell below in the same column,
    /// appending a new row when the caret is already in the last one. `false`
    /// when the caret isn't in a table, so the frontend inserts a newline.
    pub fn cell_return(&mut self) -> bool {
        if !self.caret_in_table() {
            return false;
        }
        if self.cell_move_vertical(true) {
            return true;
        }
        // Already on the last row: grow one below and drop into the same column.
        let col = self.table_grid_at(self.caret).map_or(0, |(_, _, c)| c);
        self.append_row_and_enter(col);
        true
    }

    /// Append a row below the caret's (last) row and land in `col` of it. The
    /// caret is in the last row, so twig's "insert below" makes the fresh row the
    /// table's new last — but twig re-spells the whole table, moving every byte,
    /// so the destination is read back from the rebuilt grid by the table's
    /// position (stable across a row insert), not from the pre-edit caret.
    fn append_row_and_enter(&mut self, col: usize) {
        let table = self.caret_table_index();
        self.table_insert_row(true);
        self.rebuild_map();
        let Some((start, end)) = table
            .and_then(|ti| self.vmap.tables.get(ti))
            .and_then(|t| t.grid.last())
            .and_then(|row| row.cells.get(col.min(row.cells.len().saturating_sub(1))))
            .map(|cell| (cell.start, cell.end))
        else {
            return;
        };
        self.select_cell(start, end);
    }

    /// The index, among the document's tables, of the one the caret sits in —
    /// `None` when it's in none. Used to re-find a table after an edit re-spells
    /// it (a row insert leaves the table order unchanged).
    fn caret_table_index(&self) -> Option<usize> {
        let off = self.caret;
        self.vmap.tables.iter().position(|t| {
            t.grid
                .iter()
                .any(|row| row.cells.iter().any(|c| off >= c.start && off <= c.end))
        })
    }

    /// Shift+Return inside a table: insert a hard line break *within* the current
    /// cell, via twig's `insert_line_break`. `false` when the caret isn't in a
    /// table, so the frontend inserts an ordinary line break.
    ///
    /// A table row is a single source line, so the newline-spelled hard break
    /// can't live in a cell. twig spells the in-cell break the format's way
    /// (`<br>` for Markdown) and reparses it as a *semantic* `hard_break`, so the
    /// break round-trips as structure the renderer reads back as a line — not the
    /// opaque raw HTML the old raw-splice left behind.
    ///
    /// Djot has no idiomatic in-cell break, so twig refuses it
    /// (`UnsupportedFormat`) rather than emit a `<br>` that any other djot reader
    /// would render as the literal text `<br>`. The gesture is still *consumed*
    /// there — returning `false` would let the frontend insert a real newline,
    /// which splits the one-line row — it just leaves the cell unchanged and says
    /// so on the status line. A rollback (`EditConflict`) is swallowed the same.
    pub fn cell_line_break(&mut self) -> bool {
        if !self.caret_in_table() {
            return false;
        }
        self.record_caret();
        match self.editor.insert_line_break(self.caret) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                self.caret = change.new.end;
                self.anchor = None;
                self.goal_col = None;
                self.clamp_caret();
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.record_caret();
            }
            Err(twig::Error::UnsupportedFormat) => {
                self.status = Some("in-cell line breaks aren't supported in djot".into());
            }
            Err(_) => {}
        }
        true
    }

    /// Rebuild the visual map at the width the last build used. A structural edit
    /// bumps the revision and swaps the source in, but leaves the *map* stale;
    /// when a single gesture edits and then moves over the result (Tab appending
    /// a row, then stepping into it), the move needs the map to already show the
    /// edit rather than waiting for the frontend's next frame.
    fn rebuild_map(&mut self) {
        let wrap = self.vmap_key.as_ref().and_then(|(_, w, _)| *w);
        self.build_map(wrap);
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

/// How an insert of `text` groups for undo: a single typed character folds into
/// the run of typing around it, while a newline or a multi-character insert is a
/// step of its own.
fn typed_edit_kind(text: &str) -> EditKind {
    if text.chars().take(2).count() == 1 && text != "\n" {
        EditKind::Insert
    } else {
        EditKind::Other
    }
}

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
            | "directive"
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
fn outdent_width(line: &str, unit: usize) -> usize {
    if line.starts_with('\t') {
        return 1;
    }
    line.bytes()
        .take(unit)
        .take_while(|b| *b == b' ')
        .count()
}

/// The indentation step for `line`: one list-marker width when the line opens a
/// list item (so Tab nests it), otherwise the ordinary [`Doc::INDENT`] step. See
/// [`list_marker_width`].
fn indent_unit(line: &str) -> usize {
    list_marker_width(line).unwrap_or(Doc::INDENT.len())
}

/// The display width of the list marker `line` opens with — the bullet or number
/// through its trailing space, *excluding* any indentation before it — or `None`
/// when the line isn't a list item. `"- "` → 2, `"1. "` → 3, `"  10) "` → 4.
/// This is exactly how far the marker's content is inset, so indenting a sibling
/// by it lands the sibling's marker at this item's content column and nests it.
fn list_marker_width(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    let marker_start = i;
    if i < b.len() && matches!(b[i], b'-' | b'*' | b'+') {
        i += 1;
    } else {
        let digits_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == digits_start || !(i < b.len() && matches!(b[i], b'.' | b')')) {
            return None;
        }
        i += 1; // the . or )
    }
    let after_marker = i;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    if i == after_marker {
        return None; // a marker needs a trailing space
    }
    Some(i - marker_start)
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

    /// Every visual row's drawn text — what the reader actually sees, which is
    /// the only thing the reveal preference is supposed to change.
    fn drawn_rows(d: &Doc) -> Vec<String> {
        d.vmap
            .rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
            .collect()
    }

    /// Put the caret at the first byte of `needle` and rebuild, so the row under
    /// it becomes the revealed line.
    fn caret_at(d: &mut Doc, needle: &str) {
        d.caret = d.source.find(needle).expect("needle in source");
        d.build_visual(80);
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
    fn sticky_bold_with_no_selection_wraps_the_next_typed_text() {
        // ⌘b at a bare caret, then type: the text comes out bold with no
        // selection ever made — the word-processor "start bold here" gesture.
        let mut d = doc_with("sticky_wrap", "xy\n");
        d.caret = 1; // between x and y
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "xy\n", "arming a mark must not edit the document");
        d.insert("A");
        assert_eq!(d.source, "x**A**y\n");
    }

    #[test]
    fn sticky_bold_lights_the_toolbar_before_any_typing() {
        // The button must light the instant ⌘b is pressed, or the mode is
        // invisible until the first character lands.
        let mut d = doc_with("sticky_light", "xy\n");
        d.caret = 1;
        assert!(!d.active_inline_marks().contains(InlineKind::Strong));
        d.toggle(InlineKind::Strong);
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
    }

    #[test]
    fn sticky_bold_toggled_off_types_normally_again() {
        // ⌘b, type, ⌘b, type: the first run is bold, the second is not — all
        // in the flow of typing, the exact sequence the user described.
        let mut d = doc_with("sticky_off", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        d.insert("a");
        d.insert("b"); // continues inside the run, no re-arming
        assert_eq!(d.source, "**ab**\n");
        d.toggle(InlineKind::Strong); // ⌘b again — shed bold
        d.insert("c");
        assert_eq!(d.source, "**ab**c\n");
    }

    #[test]
    fn continued_typing_after_a_sticky_run_stays_in_the_run() {
        // Once a mark is realised the caret sits inside the run, so plain typing
        // extends it rather than starting a second, adjacent bold span.
        let mut d = doc_with("sticky_cont", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Emph);
        d.insert("h");
        d.insert("i");
        assert_eq!(d.source, "*hi*\n");
    }

    #[test]
    fn moving_the_caret_disarms_a_sticky_mark() {
        // Arming a mark and then moving away must not style text elsewhere.
        let mut d = doc_with("sticky_disarm", "xy\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        d.move_right(false); // caret 0 → 1, disarms
        assert!(!d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("A");
        assert_eq!(d.source, "xAy\n", "the mark must not follow the caret");
    }

    #[test]
    fn stacked_sticky_marks_apply_together() {
        // ⌘b then ⌘i before typing: the text comes out both bold and italic.
        let mut d = doc_with("sticky_stack", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        d.toggle(InlineKind::Emph);
        d.insert("x");
        // Land the caret on the styled character and confirm both marks are live.
        d.anchor = Some(d.source.find('x').unwrap());
        d.caret = d.anchor.unwrap() + 1;
        let marks = d.active_inline_marks();
        assert!(marks.contains(InlineKind::Strong), "bold: {}", d.source);
        assert!(marks.contains(InlineKind::Emph), "italic: {}", d.source);
    }

    // ── the mark-edge rule (see `Doc::splice`) ───────────────────────────────

    #[test]
    fn a_space_typed_in_a_bold_run_never_leaves_the_delimiters_showing() {
        // The reported bug, keystroke for keystroke: ⌘b, "bold", space, "hey".
        // The space inside the run made `**bold **`, which is *not* bold — four
        // literal asterisks — so the rich view drew them, correctly and
        // uselessly, until the next character happened to close the run again.
        let mut d = wysiwyg_doc("edge_typing", "a \n");
        d.caret = 2;
        d.toggle(InlineKind::Strong);
        for c in "bold".chars() {
            d.insert(&c.to_string());
        }
        assert_eq!(d.source, "a **bold**\n");
        d.insert(" ");
        assert_eq!(d.source, "a **bold** \n", "the space belongs outside the run");
        assert!(
            d.active_inline_marks().contains(InlineKind::Strong),
            "bold is still what's being typed, so the button stays lit"
        );
        // What the writer is looking at while all this happens: their words.
        d.build_visual(80);
        let drawn: String = d.vmap.rows[0].glyphs.iter().map(|g| g.ch).collect();
        assert_eq!(drawn, "a bold ", "no delimiter ever surfaces: {}", d.source);
        for c in "hey".chars() {
            d.insert(&c.to_string());
        }
        assert_eq!(d.source, "a **bold hey**\n", "one bold phrase, not two runs");
    }

    #[test]
    fn typing_past_a_space_can_still_leave_the_bold_behind() {
        // The other half: the marks stay armed across the space, so ⌘b turns
        // them off again there and the next word is plain — the run isn't
        // rejoined by a caret that was told not to.
        let mut d = wysiwyg_doc("edge_shed", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        for c in "bold ".chars() {
            d.insert(&c.to_string());
        }
        assert_eq!(d.source, "**bold** \n");
        d.toggle(InlineKind::Strong);
        assert!(!d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("x");
        assert_eq!(d.source, "**bold** x\n");
    }

    #[test]
    fn a_space_typed_first_of_all_still_leaves_the_mark_armed() {
        // ⌘b and then a space before any word: the space is not marked (nothing
        // is), and the word after it is.
        let mut d = wysiwyg_doc("edge_space_first", "a\n");
        d.caret = 1;
        d.toggle(InlineKind::Strong);
        d.insert(" ");
        assert_eq!(d.source, "a \n");
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("b");
        assert_eq!(d.source, "a **b**\n");
    }

    #[test]
    fn a_space_typed_at_either_edge_of_an_existing_mark_steps_outside_it() {
        let mut d = wysiwyg_doc("edge_tail", "x **bold**\n");
        d.caret = 8; // the caret's home at the end of the run's text
        d.insert(" ");
        assert_eq!(d.source, "x **bold** \n", "the space lands past the delimiters");
        assert_eq!(d.caret, 11, "and the caret stands past it, outside the run");

        let mut d = wysiwyg_doc("edge_head", "x **bold** y\n");
        d.caret = 4; // in front of the "b"
        d.insert(" ");
        assert_eq!(d.source, "x  **bold** y\n");
        assert_eq!(d.caret, 3, "in front of the run, where the space was typed");
    }

    #[test]
    fn a_delete_that_backs_a_space_onto_a_delimiter_moves_the_delimiter() {
        // Backspace over the last letter of a bold phrase.
        let mut d = wysiwyg_doc("edge_bksp", "a **bold h**\n");
        d.caret = 10; // past the "h"
        d.backspace();
        assert_eq!(d.source, "a **bold** \n");
        assert_eq!(d.caret, 11, "the caret keeps the place on screen it had");
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("x");
        assert_eq!(d.source, "a **bold x**\n", "and typing rejoins the run");
    }

    #[test]
    fn deleting_the_last_of_a_run_takes_its_delimiters_with_it() {
        // `**b**` with the `b` gone is `****`: two delimiters with nothing to
        // mark, which is only text. The marks live on in the caret instead.
        let mut d = wysiwyg_doc("edge_empty", "a **b** c\n");
        d.caret = 5;
        d.backspace();
        assert_eq!(d.source, "a  c\n");
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("x");
        assert_eq!(d.source, "a **x** c\n");
    }

    #[test]
    fn typing_over_a_whole_bold_word_keeps_it_bold() {
        let mut d = wysiwyg_doc("edge_replace", "a **bold** c\n");
        d.anchor = Some(4);
        d.caret = 8; // the word, not its delimiters
        d.insert("x");
        assert_eq!(d.source, "a **x** c\n");
    }

    #[test]
    fn a_code_span_keeps_the_space_it_is_given() {
        // Backticks are not whitespace-sensitive the way `**` is: `` `code ` ``
        // is still verbatim, so nothing is re-spelt. The repair asks the parser
        // rather than a table of kinds, and this is the answer it gets.
        let mut d = wysiwyg_doc("edge_code", "a `code` c\n");
        d.caret = 7;
        d.insert(" ");
        assert_eq!(d.source, "a `code ` c\n");
    }

    #[test]
    fn a_delete_from_a_runs_outer_edge_reaches_into_the_run() {
        // A run's closing delimiter has a caret home on each side of it, one
        // column apart on screen — and a plain ← off the space after a bold word
        // lands on the outer one. The character drawn behind the caret there is
        // still the last letter of the phrase, so that is what Backspace takes;
        // the byte behind it is a `*` nobody can see.
        let mut d = wysiwyg_doc("edge_outer_close", "**bold** x\n");
        d.caret = 9;
        d.move_left(false);
        assert_eq!(d.caret, 8, "← rests past the delimiters, not inside them");
        d.backspace();
        assert_eq!(d.source, "**bol** x\n", "a letter of the phrase, not its `*`");
        assert_eq!(d.caret, 5);

        // And the mirror in front of the opening delimiter, where Delete's
        // character is the first letter of the run.
        let mut d = wysiwyg_doc("edge_outer_open", "x**bold**\n");
        d.caret = 1;
        d.delete_forward();
        assert_eq!(d.source, "x**old**\n");
        assert_eq!(d.caret, 3, "inside the run, in front of what is left of it");
    }

    #[test]
    fn a_delete_at_a_run_edge_never_eats_a_delimiter() {
        // The byte beside the caret at either edge of a bold word is a `*` the
        // rich view draws nothing for. Taking it is not the character delete the
        // key was pressed for — it unspells the run and puts a literal asterisk
        // on screen (`a *bold** c`). The visible character is the one that goes.
        let mut d = wysiwyg_doc("edge_open_bksp", "a **bold** c\n");
        d.caret = 4; // in front of the "b"
        d.backspace();
        assert_eq!(d.source, "a**bold** c\n", "the space goes, the run stands");

        let mut d = wysiwyg_doc("edge_close_del", "a **bold** c\n");
        d.caret = 8; // past the "d"
        d.delete_forward();
        assert_eq!(d.source, "a **bold**c\n");
        assert_eq!(d.caret, 8, "and the caret stays inside the run");
        d.insert("x");
        assert_eq!(d.source, "a **boldx**c\n");

        // A code span's backticks are hidden the same way, so they are covered
        // by the same rule and not by a list of kinds.
        let mut d = wysiwyg_doc("edge_open_code", "a `code` c\n");
        d.caret = 3;
        d.backspace();
        assert_eq!(d.source, "a`code` c\n");
    }

    #[test]
    fn the_source_view_deletes_the_delimiter_byte_it_is_shown() {
        // The asterisks are on the screen there and the caret can stand between
        // them, so a delete takes exactly the byte it is aimed at.
        let mut d = doc_with("edge_open_src", "a **bold** c\n");
        d.caret = 4;
        d.backspace();
        assert_eq!(d.source, "a *bold** c\n");

        let mut d = doc_with("edge_close_src", "a **bold** c\n");
        d.caret = 8;
        d.delete_forward();
        assert_eq!(d.source, "a **bold* c\n");
    }

    #[test]
    fn backspacing_the_space_out_of_a_bold_phrase_leaves_the_caret_in_it() {
        // The reported bug, keystroke for keystroke: ⌘b, "bold", space, Backspace.
        // The space had stepped outside the run (the mark-edge rule), taking the
        // caret with it, so the delete put it back down on the far side of the
        // closing `**` — one place on screen, and the wrong side of it. Typing
        // came out plain and the toolbar went dark, with nothing to see.
        let mut d = wysiwyg_doc("edge_bksp_space", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        for c in "bold".chars() {
            d.insert(&c.to_string());
        }
        d.insert(" ");
        assert_eq!(d.source, "**bold** \n");
        d.backspace();
        assert_eq!(d.source, "**bold**\n", "the space goes, the delimiters stay");
        assert_eq!(d.caret, 6, "and the caret comes back inside the run");
        assert!(
            d.active_inline_marks().contains(InlineKind::Strong),
            "so the button is still lit"
        );
        d.insert("x");
        assert_eq!(d.source, "**boldx**\n", "and the next character is still bold");
    }

    #[test]
    fn a_second_backspace_there_deletes_a_letter_of_the_phrase() {
        // What the stranded caret did next: the byte behind it was the closing
        // `*`, so a second press took that instead of a letter — `**bold*`, the
        // styling gone and an asterisk on the screen where the word had been.
        let mut d = wysiwyg_doc("edge_bksp_twice", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        for c in "bold ".chars() {
            d.insert(&c.to_string());
        }
        assert_eq!(d.source, "**bold** \n");
        d.backspace();
        d.backspace();
        assert_eq!(d.source, "**bol**\n", "the delete lands inside the run");
        assert_eq!(d.caret, 5);
    }

    #[test]
    fn a_delete_that_ends_at_a_nested_run_settles_inside_every_delimiter() {
        // `***both***` closes two runs with one stack of asterisks: the caret has
        // to walk in through all of them, or it lands between the emph and the
        // strong and types half-marked.
        let mut d = wysiwyg_doc("edge_bksp_nested", "***both*** \n");
        d.caret = 11;
        d.backspace();
        assert_eq!(d.source, "***both***\n");
        assert_eq!(d.caret, 7, "past the last letter, inside both runs");
        d.insert("x");
        assert_eq!(d.source, "***bothx***\n");
    }

    #[test]
    fn a_delete_that_ends_mid_run_leaves_the_caret_where_it_fell() {
        // The settle only moves a caret a run actually closed over. Ordinary
        // deletes — inside a run, or in plain prose — are untouched.
        let mut d = wysiwyg_doc("edge_bksp_mid", "a **bold** c\n");
        d.caret = 8;
        d.backspace();
        assert_eq!(d.source, "a **bol** c\n");
        assert_eq!(d.caret, 7);

        let mut d = wysiwyg_doc("edge_bksp_plain", "plain\n");
        d.caret = 5;
        d.backspace();
        assert_eq!(d.source, "plai\n");
        assert_eq!(d.caret, 4);
    }

    #[test]
    fn the_source_view_leaves_a_delete_where_it_landed() {
        // The delimiters are on the screen there, so the offset past them is a
        // place the caret can be seen to be — nothing to settle.
        let mut d = doc_with("edge_bksp_src", "**bold** \n");
        d.caret = 9;
        d.backspace();
        assert_eq!(d.source, "**bold**\n");
        assert_eq!(d.caret, 8);
    }

    #[test]
    fn the_mark_edge_rule_clears_every_delimiter_of_a_nested_run() {
        // `***both***` closes two runs with one stack of asterisks; a space that
        // clears only the inner one lands against the outer's and breaks that
        // instead.
        let mut d = wysiwyg_doc("edge_nested", "a ***both***\n");
        d.caret = 9;
        d.insert(" ");
        assert_eq!(d.source, "a ***both*** \n");
        assert_eq!(d.caret, 13);
        d.insert("x");
        assert_eq!(d.source, "a ***both x***\n");
    }

    #[test]
    fn the_mark_edge_repair_undoes_with_the_keystroke_that_caused_it() {
        // The delimiter shuffle is not an edit the writer made, so it is not a
        // step they have to undo past.
        let mut d = wysiwyg_doc("edge_undo", "a **bold**\n");
        d.caret = 8;
        d.insert(" ");
        assert_eq!(d.source, "a **bold** \n");
        d.undo();
        assert_eq!(d.source, "a **bold**\n");
    }

    #[test]
    fn the_source_view_types_the_space_where_it_was_asked_to() {
        // The rule is a rich-view courtesy. In the source view the delimiters are
        // on the screen and the user is editing the bytes they can see.
        let mut d = doc_with("edge_src", "a **bold** c\n");
        d.caret = 8;
        d.insert(" ");
        assert_eq!(d.source, "a **bold ** c\n");
    }

    #[test]
    fn toggling_a_mark_over_a_selection_leaves_its_edge_whitespace_out() {
        // Double-clicking a word takes the space after it; bolding that must not
        // spell `**word **`, which is not bold at all.
        let mut d = wysiwyg_doc("edge_sel", "a word b\n");
        d.anchor = Some(2);
        d.caret = 7; // "word "
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a **word** b\n");
        // And a selection of nothing but whitespace has no word to mark.
        let mut d = wysiwyg_doc("edge_sel_ws", "a word b\n");
        d.anchor = Some(6);
        d.caret = 7;
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a word b\n");
        assert!(d.status.is_some());
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
    fn preserve_enter_at_a_line_end_lands_the_caret_on_the_new_blank_line() {
        // Regression: Enter at the end of a soft-break line (mid-paragraph) opened
        // the blank line but the caret rendered on the *next* line, because the
        // separator was a non-navigable decoration row. In Preserve flow that
        // blank line is a real caret home — the caret must resolve onto it, and
        // typing there makes the soft break that continues the paragraph.
        let src = "line one:\nsecond line\n";
        let mut d = wysiwyg_doc("pre_enter_lineend", src);
        d.set_line_flow(LineFlow::Preserve);
        d.build_visual_unwrapped(); // the GUI path (pixel-wrapped)
        d.caret = 9; // the visual end of row 0, at the soft-break '\n'
        d.newline();
        d.build_visual_unwrapped();
        assert_eq!(d.source, "line one:\n\nsecond line\n");
        assert_eq!(d.caret, 10, "caret sits on the new blank line, not the next line");
        // The blank line is row 1, and the caret resolves onto it — not row 2.
        assert_eq!(d.vmap.pos_of_offset(10), (1, 0), "caret renders on the blank row");
        assert!(!d.vmap.rows[1].decoration, "the blank line is navigable in Preserve");
        // Typing there makes a soft break: one paragraph, three lines.
        d.insert("new clause,");
        assert_eq!(d.source, "line one:\nnew clause,\nsecond line\n");
    }

    #[test]
    fn preserve_enter_makes_a_soft_break_not_a_paragraph() {
        // Mid-paragraph: Enter splits the line with a single `\n`, a soft break
        // that keeps it one paragraph — where Fold would open a second paragraph.
        let mut d = wysiwyg_doc("pre_enter_mid", "abcdef\n");
        d.set_line_flow(LineFlow::Preserve);
        d.caret = 3;
        d.newline();
        assert_eq!(d.source, "abc\ndef\n", "mid-line Enter is a soft break");

        // End-of-paragraph: Enter then typing continues the same paragraph on a
        // new line (a soft break), not a fresh paragraph.
        let mut d = wysiwyg_doc("pre_enter_end", "abc\n");
        d.set_line_flow(LineFlow::Preserve);
        d.caret = 3;
        d.newline();
        d.insert("def");
        assert_eq!(d.source, "abc\ndef\n", "end-of-line Enter + typing is a soft break");
    }

    #[test]
    fn preserve_double_enter_still_makes_a_paragraph() {
        // Two Enters in a row promote to a real paragraph break: the second lands
        // on the blank line the first opened and takes the empty-line branch.
        let mut d = wysiwyg_doc("pre_enter_dbl", "abc\n");
        d.set_line_flow(LineFlow::Preserve);
        d.caret = 3;
        d.newline();
        d.newline();
        d.insert("def");
        assert_eq!(d.source, "abc\n\ndef\n", "double Enter is a paragraph break");
    }

    #[test]
    fn preserve_backspace_joins_across_a_soft_break() {
        // Backspace is the symmetric undo of a Preserve Enter: over the `\n` of a
        // soft break it deletes the single newline and joins the two lines.
        let mut d = wysiwyg_doc("pre_bs", "abc\ndef\n");
        d.set_line_flow(LineFlow::Preserve);
        d.build_visual(80);
        d.caret = 4; // start of "def", just past the soft break
        d.backspace();
        assert_eq!(d.source, "abcdef\n", "Backspace joins across the soft break");
        assert_eq!(d.caret, 3, "caret lands where the lines meet");
    }

    #[test]
    fn fold_enter_still_starts_a_new_paragraph() {
        // The default flow is unchanged: a lone `\n` would render as an invisible
        // space, so Enter keeps opening the paragraph break that actually shows.
        let mut d = wysiwyg_doc("fold_enter", "abcdef\n");
        d.caret = 3;
        d.newline();
        assert_eq!(d.source, "abc\n\ndef\n", "Fold mid-line Enter is a paragraph break");
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
    fn enter_at_the_end_of_a_bold_run_keeps_its_closing_delimiter_attached() {
        // Regression: Enter at the caret's natural End-of-line resting place
        // after a bold run with nothing following it (on screen: right after
        // "bold", before the hidden closing "**") spliced the paragraph break
        // at that very byte offset — which sits *before* the closing "**" in
        // the source, since the delimiter is hidden and emits no glyph of its
        // own for `push_row`'s "end of row" fallback to count. That severed the
        // mark: "**bold**\n" became "**bold\n\n**\n", stranding the closing
        // "**" alone on the new line instead of leaving "**bold**" intact with
        // a fresh empty paragraph after it.
        let mut d = wysiwyg_doc("bold_eol_enter", "**bold**\n");
        d.move_end(false); // the WYSIWYG End key, from caret 0
        assert_eq!(d.caret, 6, "caret rests right after \"bold\", before the hidden \"**\"");
        d.newline();
        assert!(
            d.source.starts_with("**bold**"),
            "the closing ** must stay attached to \"bold\": got {:?}",
            d.source
        );
        assert_eq!(
            d.source, "**bold**\n\n\n",
            "a fresh empty paragraph follows the still-intact bold run"
        );
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
    fn a_heading_typed_on_a_blank_line_keeps_the_caret_on_its_own_row() {
        // The reported bug, end to end: click a blank line with another one under
        // it, press H1, type. The text landed in the heading and the caret's
        // offset was right (the source view drew it there), but the rich view
        // drew it two rows lower, on the trailing blank line — the empty `# `
        // heading had left every row below it short by the marker's two bytes,
        // and the blank line ended up claiming the heading's own end offset.
        let mut d = wysiwyg_doc("head_blank", "one\n\ntwo\n\n\n\n");
        d.build_visual_unwrapped();
        d.caret = d.vmap.offset_of_pos(4, 0); // the first of the two blank lines
        d.toggle_heading(1);
        for c in "title".chars() {
            d.insert(&c.to_string());
            d.build_visual_unwrapped(); // as a frontend does, one frame per key
        }
        assert_eq!(d.source, "one\n\ntwo\n\n# title\n\n");
        assert_eq!(d.caret_pos(), (4, 5), "the caret draws at the end of the heading");
    }

    #[test]
    fn clicking_an_empty_heading_types_after_its_marker() {
        // The same anchor from the other side: the empty heading's row is its own
        // caret home, so a click on it must land past the hidden `# `. Landing in
        // front of the hashes made the first keystroke un-heading the line.
        let mut d = wysiwyg_doc("head_click", "# \n");
        d.build_visual_unwrapped();
        d.caret = d.vmap.offset_of_pos(0, 0);
        d.insert("x");
        assert_eq!(d.source, "# x\n");
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
    fn wysiwyg_backspace_after_leaving_a_list_collapses_the_gap_cleanly() {
        // Regression for the "extra newline" left between a list and the paragraph
        // below it. Enter, Enter leaves the list on a fresh empty paragraph
        // (`- item\n\n\n\nnext`, a navigable blank between the two blocks); one
        // Backspace should then take the caret cleanly back to the end of the list
        // item, `- item\n\nnext`, not delete a single newline and strand it on the
        // odd `- item\n\n\nnext` — a blank line the eye reads as one separator but
        // no caret can land on. The map is rebuilt between keystrokes exactly as a
        // frontend does, since Backspace reads the stop table to place the delete.
        let mut d = wysiwyg_doc("wys_exit_bksp", "- item\n\nnext\n");
        d.caret = 6; // end of "item"
        d.newline();
        d.build_visual(80);
        d.newline(); // leave the list onto a fresh empty paragraph
        d.build_visual(80);
        assert_eq!(d.source, "- item\n\n\n\nnext\n", "double-Enter opens the empty paragraph");
        d.backspace();
        assert_eq!(d.source, "- item\n\nnext\n", "one Backspace collapses the whole gap");
        assert_eq!(d.caret, 6, "and lands the caret back at the end of the list item");
    }

    #[test]
    fn wysiwyg_backspace_on_stacked_blank_lines_still_removes_just_one() {
        // The stop-wise delete must not over-reach when there is no block boundary
        // to cross: two blank lines in a row are one caret stop apart, so pressing
        // Enter on an empty line and then Backspace removes exactly the one newline
        // it added — the lone-Enter / lone-Backspace symmetry, preserved.
        let mut d = wysiwyg_doc("wys_stack", "abc\n\n\n");
        d.caret = 5; // the empty paragraph the first Enter already opened
        d.build_visual(80);
        d.newline();
        d.build_visual(80);
        assert_eq!(d.source, "abc\n\n\n\n", "Enter on the blank line adds one newline");
        d.backspace();
        assert_eq!(d.source, "abc\n\n\n", "Backspace takes back exactly that one newline");
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

    /// The bug a real vault hit: a filename with spaces in it. Markdown ends a
    /// destination at the first space, so the `format!` this used to be wrote
    /// something that was not an image at all — and the reader saw the markup as
    /// text. twig owns the spelling now, and moves it into the angle form.
    #[test]
    fn insert_image_spells_a_destination_with_spaces_so_it_stays_an_image() {
        let mut d = doc_with("img_space", "x\n");
        d.caret = 0;
        d.insert_image("Jesus Commands the Apostles to Rest.jpg", "");
        assert_eq!(
            d.source,
            "![](<Jesus Commands the Apostles to Rest.jpg>)x\n"
        );
        // And it reads back as an image pointing at the unescaped path — the angle
        // brackets are spelling, not part of the destination.
        d.caret = 2;
        assert_eq!(
            d.image_destination_at_caret(),
            Some("Jesus Commands the Apostles to Rest.jpg".to_string())
        );
    }

    /// A `)` in a caption or a filename must not close the image early.
    #[test]
    fn insert_image_escapes_a_paren_in_either_half() {
        let mut d = doc_with("img_paren", "x\n");
        d.caret = 0;
        d.insert_image("a)b.png", "");
        assert_eq!(d.source, "![](a\\)b.png)x\n");
        d.caret = 2;
        assert_eq!(d.image_destination_at_caret(), Some("a)b.png".to_string()));
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
    fn insert_media_spells_a_video_as_html_and_reads_it_back_as_a_block() {
        // The round trip is the point: it's no use writing markup the reader
        // can't pick up again. This is the pair that only holds from twig 2.5.1
        // on — before it, the one-line form went in fine and came back as a
        // paragraph of raw tags, publishing no media at all.
        let mut d = doc_with("vid_rt", "\n");
        d.caret = 0;
        d.insert_media(MediaKind::Video, "clip.mp4", "a clip");
        assert_eq!(d.source, "<video src=\"clip.mp4\" controls>a clip</video>\n");

        d.build_visual(80);
        assert_eq!(d.vmap.media.len(), 1, "reads back as one block media");
        assert_eq!(d.vmap.media[0].kind, MediaKind::Video);
        assert_eq!(d.vmap.media[0].destination, "clip.mp4");
        assert_eq!(d.vmap.media[0].alt, "a clip");
    }

    #[test]
    fn insert_media_spells_audio_with_its_own_tag() {
        let mut d = doc_with("aud_rt", "\n");
        d.caret = 0;
        d.insert_media(MediaKind::Audio, "take.mp3", "");
        assert_eq!(d.source, "<audio src=\"take.mp3\" controls></audio>\n");
        d.build_visual(80);
        assert_eq!(d.vmap.media[0].kind, MediaKind::Audio);
    }

    #[test]
    fn insert_media_uses_the_selection_as_fallback_text() {
        // The same courtesy `insert_image` does with alt: select a caption,
        // insert, and the caption labels the thing rather than being replaced.
        let mut d = doc_with("vid_sel", "the talk here\n");
        d.anchor = Some(0);
        d.caret = 8; // "the talk"
        d.insert_media(MediaKind::Video, "talk.mp4", "ignored fallback");
        assert_eq!(d.source, "<video src=\"talk.mp4\" controls>the talk</video> here\n");
    }

    #[test]
    fn insert_media_with_an_image_kind_is_just_insert_image() {
        let mut d = doc_with("img_via_media", "\n");
        d.caret = 0;
        d.insert_media(MediaKind::Image, "logo.svg", "x");
        assert_eq!(d.source, "![x](logo.svg)\n");
    }

    // ── thematic breaks ─────────────────────────────────────────────────────

    /// The node the source parses as at `caret` — what confirms an inserted
    /// `---` actually reads back as a rule, not stray text or a setext heading.
    fn kind_at(d: &mut Doc, caret: usize) -> Option<String> {
        d.nodes()
            .into_iter()
            .find(|n| n.span.start <= caret && caret < n.span.end)
            .map(|n| n.kind)
    }

    #[test]
    fn insert_thematic_break_splits_a_paragraph_and_lands_past_it() {
        let mut d = doc_with("hr_mid", "before after\n");
        d.caret = 7; // between "before " and "after"
        d.insert_thematic_break();
        assert_eq!(d.source, "before \n\n---\n\nafter\n");
        assert_eq!(d.selection(), None);
        assert_eq!(d.caret, "before \n\n---\n\n".len());
        assert_eq!(kind_at(&mut d, "before \n\n".len()), Some("thematic_break".to_string()));
    }

    #[test]
    fn insert_thematic_break_on_a_blank_line_needs_no_leading_break() {
        let mut d = doc_with("hr_blank", "abc\n\n\ndef\n");
        d.caret = "abc\n\n".len(); // the blank line between the two paragraphs
        d.insert_thematic_break();
        assert_eq!(d.source, "abc\n\n---\n\n\ndef\n");
    }

    #[test]
    fn insert_thematic_break_replaces_the_selection() {
        let mut d = doc_with("hr_sel", "one two three\n");
        d.anchor = Some(4);
        d.caret = 7; // "two"
        d.insert_thematic_break();
        assert_eq!(d.source, "one \n\n---\n\n three\n");
    }

    #[test]
    fn insert_thematic_break_refuses_inside_a_code_block() {
        let mut d = doc_with("hr_code", "```\nfn x() {}\n```\n");
        d.caret = 5; // inside the fenced code
        d.insert_thematic_break();
        assert_eq!(d.source, "```\nfn x() {}\n```\n", "refused, so nothing changed");
        assert!(d.status.is_some(), "the refusal should reach the status line");
    }

    #[test]
    fn insert_thematic_break_in_a_list_item_ends_the_list() {
        // The un-indented `---` cannot continue the list, so it closes the list
        // and lands the rule at the top level rather than nested inside it.
        let mut d = doc_with("hr_list", "- one\n- two\n");
        d.caret = "- one\n- tw".len(); // mid "two"
        d.insert_thematic_break();
        d.build_visual(80);
        let rule_at = d.source.find("---\n\n").unwrap();
        assert_eq!(kind_at(&mut d, rule_at), Some("thematic_break".to_string()));
        assert!(
            !d.nodes().iter().any(|n| n.kind == "bullet_list"
                && n.span.start <= rule_at
                && rule_at < n.span.end),
            "the rule must not be nested inside the list"
        );
    }

    #[test]
    fn insert_thematic_break_in_a_blockquote_ends_the_quote() {
        let mut d = doc_with("hr_quote", "> hello\n");
        d.caret = 4; // inside the quoted text
        d.insert_thematic_break();
        d.build_visual(80);
        let rule_at = d.source.find("---\n\n").unwrap();
        assert_eq!(kind_at(&mut d, rule_at), Some("thematic_break".to_string()));
        assert!(
            !d.nodes().iter().any(|n| n.kind == "block_quote"
                && n.span.start <= rule_at
                && rule_at < n.span.end),
            "the rule must not be nested inside the quote"
        );
    }

    // ── typing against a block picture ────────────────────────────────────────

    /// A rendered-view document with the caret parked on one of the picture's two
    /// stops, and the map already built — the state a frontend is in between
    /// drawing a frame and the next keystroke.
    fn doc_at_picture(name: &str, src: &str, side: MediaStop) -> Doc {
        let mut d = doc_in(View::Wysiwyg, name, src);
        d.build_visual_unwrapped();
        let start = src.find("![").unwrap();
        d.caret = match side {
            MediaStop::Before => start,
            MediaStop::After => start + "![](p.png)".len(),
        };
        d
    }

    /// The block media the map publishes, after rebuilding it — "is this still a
    /// picture, or has it become a line of text with an image in it?"
    fn media_count(d: &mut Doc) -> usize {
        d.build_visual_unwrapped();
        d.vmap.media.len()
    }

    #[test]
    fn typing_past_a_block_picture_opens_a_paragraph_under_it() {
        // The accident this prevents: tap the blank page under a photo (which
        // lands on the picture's trailing stop), type, and `![](p.png)xy` is a
        // paragraph with an *inline* image — the photo stops being drawn.
        let mut d = doc_at_picture("pic_after", "hi\n\n![](p.png)\n", MediaStop::After);
        d.insert("xy");
        assert_eq!(d.source, "hi\n\n![](p.png)\n\nxy\n");
        assert_eq!(media_count(&mut d), 1, "still a picture");
    }

    #[test]
    fn typing_in_front_of_a_block_picture_opens_a_paragraph_above_it() {
        let mut d = doc_at_picture("pic_before", "hi\n\n![](p.png)\n", MediaStop::Before);
        d.insert("xy");
        assert_eq!(d.source, "hi\n\nxy\n\n![](p.png)\n");
        assert_eq!(media_count(&mut d), 1);
    }

    #[test]
    fn a_picture_that_opens_the_document_still_takes_a_paragraph_above_it() {
        let mut d = doc_at_picture("pic_first", "![](p.png)\n", MediaStop::Before);
        d.insert("x");
        assert_eq!(d.source, "x\n\n![](p.png)\n");
        assert_eq!(media_count(&mut d), 1);
    }

    #[test]
    fn one_undo_puts_the_picture_back_the_way_it_was_found() {
        // The opened paragraph is part of the keystroke, not an edit the writer
        // made — so it undoes with the character, not a step later.
        let mut d = doc_at_picture("pic_undo", "hi\n\n![](p.png)\n", MediaStop::After);
        d.insert("x");
        assert_eq!(d.source, "hi\n\n![](p.png)\n\nx\n");
        d.undo();
        assert_eq!(d.source, "hi\n\n![](p.png)\n");
    }

    #[test]
    fn pasting_against_a_block_picture_opens_a_paragraph_too() {
        // ⌘V dissolves the picture exactly as a keystroke does.
        let mut d = doc_at_picture("pic_paste", "hi\n\n![](p.png)\n", MediaStop::After);
        d.paste("pasted");
        assert_eq!(d.source, "hi\n\n![](p.png)\n\npasted\n");
        assert_eq!(media_count(&mut d), 1);
    }

    #[test]
    fn typing_beside_an_inline_image_is_ordinary_editing() {
        // An inline image has no placeholder row and no stops of its own. Opening
        // a paragraph mid-sentence would be the bug, not the fix.
        let mut d = doc_in(View::Wysiwyg, "pic_inline", "see ![](p.png) here\n");
        d.build_visual_unwrapped();
        d.caret = "see ![](p.png)".len();
        d.insert("!");
        assert_eq!(d.source, "see ![](p.png)! here\n");
    }

    #[test]
    fn source_view_types_raw_markup_against_an_image_untouched() {
        // Source view is for writing the markup itself; a break inserted behind
        // the writer's back there would be the editor arguing with them.
        let mut d = doc_in(View::Source, "pic_src", "![](p.png)\n");
        d.caret = "![](p.png)".len();
        d.insert("x");
        assert_eq!(d.source, "![](p.png)x\n");
    }

    #[test]
    fn typing_over_a_selection_that_starts_at_a_picture_stop_replaces_it() {
        // A selection is replaced, not joined into, so there is nothing to
        // protect: the range takes the picture with it.
        let mut d = doc_at_picture("pic_sel", "hi\n\n![](p.png)\n", MediaStop::Before);
        d.anchor = Some(d.caret);
        d.caret = d.source.find("![").unwrap() + "![](p.png)".len();
        d.insert("x");
        assert_eq!(d.source, "hi\n\nx\n");
    }

    #[test]
    fn backspace_past_a_block_picture_deletes_the_picture_not_its_last_byte() {
        // What this actually cost: a real vault's photo, to one stray Backspace.
        // The caret past `![](p.png)` was deleting the closing paren — invisible
        // in the rendered view — and the photo became the text `![](p.png`.
        let mut d = doc_at_picture("pic_bs", "hi\n\n![](p.png)\n", MediaStop::After);
        d.backspace();
        assert_eq!(d.source, "hi\n");
        assert_eq!(media_count(&mut d), 0, "the picture went, in one piece");
        d.undo();
        assert_eq!(d.source, "hi\n\n![](p.png)\n", "and comes back in one piece");
    }

    #[test]
    fn backspace_in_front_of_a_block_picture_steps_out_instead_of_merging_it() {
        // Deleting the break here would join the picture to the paragraph above,
        // where it is an *inline* image and stops being drawn. Step over the
        // boundary; the next press deletes in the paragraph the caret reached.
        let mut d = doc_at_picture("pic_bs_before", "hi\n\n![](p.png)\n", MediaStop::Before);
        d.backspace();
        assert_eq!(d.source, "hi\n\n![](p.png)\n", "nothing deleted");
        assert_eq!(d.caret, 2, "the caret stepped up to the end of `hi`");
        d.backspace();
        assert_eq!(d.source, "h\n\n![](p.png)\n", "and now it deletes there");
        assert_eq!(media_count(&mut d), 1, "the picture was never at risk");
    }

    #[test]
    fn forward_delete_in_front_of_a_block_picture_deletes_the_picture() {
        // The mirror. A byte-step here eats the `!` and leaves a link.
        let mut d = doc_at_picture("pic_del", "hi\n\n![](p.png)\n\nbye\n", MediaStop::Before);
        d.delete_forward();
        assert_eq!(d.source, "hi\n\nbye\n");
        assert_eq!(media_count(&mut d), 0);
    }

    #[test]
    fn forward_delete_past_a_block_picture_steps_over_the_boundary() {
        let mut d = doc_at_picture("pic_del_after", "hi\n\n![](p.png)\n\nbye\n", MediaStop::After);
        d.delete_forward();
        assert_eq!(d.source, "hi\n\n![](p.png)\n\nbye\n", "nothing deleted");
        assert_eq!(d.caret, d.source.find("bye").unwrap(), "the caret stepped down to `bye`");
    }

    #[test]
    fn a_picture_that_is_the_whole_document_still_deletes_cleanly() {
        let mut d = doc_at_picture("pic_only", "![](p.png)\n", MediaStop::After);
        d.backspace();
        assert_eq!(d.source, "\n");
        assert_eq!(media_count(&mut d), 0);
    }

    #[test]
    fn a_word_delete_takes_the_picture_whole_or_steps_out_of_it() {
        // ⌥⌫ past a picture would otherwise eat a "word" of its markup.
        let mut d = doc_at_picture("pic_wordbs", "hi there\n\n![](p.png)\n", MediaStop::After);
        d.delete_word_back();
        assert_eq!(d.source, "hi there\n");

        // And in front of one it runs *through* the paragraph break into the
        // prose above, which merges the picture inline — so it steps out first,
        // and the second press deletes the word it was aimed at.
        let mut d = doc_at_picture("pic_wordbs2", "hi there\n\n![](p.png)\n", MediaStop::Before);
        d.delete_word_back();
        assert_eq!(d.source, "hi there\n\n![](p.png)\n");
        d.delete_word_back();
        assert_eq!(d.source, "hi \n\n![](p.png)\n", "the word above went, the picture stayed");
        assert_eq!(media_count(&mut d), 1);
    }

    #[test]
    fn source_view_deletes_raw_markup_against_an_image_untouched() {
        let mut d = doc_in(View::Source, "pic_src_del", "![](p.png)\n");
        d.caret = "![](p.png)".len();
        d.backspace();
        assert_eq!(d.source, "![](p.png\n", "raw editing, byte by byte");
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
    fn set_media_rows_reserves_blank_filler_rows_the_frontend_paints_over() {
        // The image is one placeholder row by default, and `set_media_rows` grows
        // it to the height the frontend measured: the label row plus blank
        // `decoration` fillers that hold the vertical space a raster is drawn into.
        let mut d = wysiwyg_doc("img_rows", "intro\n\n![a cat](cat.png)\n\nend\n");
        assert_eq!(d.vmap.media.len(), 1);
        let img_row = d.vmap.media[0].rows_span.start;
        assert_eq!(d.vmap.media[0].rows_span, img_row..img_row + 1, "default is one row");

        d.set_media_rows(HashMap::from([("cat.png".to_string(), 4)]));
        d.build_visual(80);
        assert_eq!(d.vmap.media.len(), 1, "still one image, now taller");
        let span = d.vmap.media[0].rows_span.clone();
        assert_eq!(span.end - span.start, 4, "reserves the four rows asked for");
        // The label row carries the mark and its glyphs; the three below are blank
        // decoration — drawn, but no caret and no text.
        assert!(d.vmap.rows[span.start].media.is_some(), "mark rides the first row");
        for r in (span.start + 1)..span.end {
            assert!(d.vmap.rows[r].decoration, "filler row {r} is decoration");
            assert!(d.vmap.rows[r].glyphs.is_empty(), "filler row {r} is blank");
            assert!(d.vmap.rows[r].media.is_none(), "only the first row is marked");
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
                d.set_media_rows(HashMap::from([("p.png".to_string(), rows)]));
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
        reference_map_revealing(source, None)
    }

    /// [`reference_map`] with a reveal line — the ground truth for the
    /// `MarkupMode::Full` builds, where the map is a function of the caret's
    /// line as well as the text.
    fn reference_map_revealing(
        source: &str,
        reveal: Option<Range<usize>>,
    ) -> crate::wysiwyg::VisualMap {
        // The same parse `Doc` uses. With twig's plain defaults instead, the two
        // sides disagree on what the *document* is before the renderer is even
        // reached — a bare `:word` is a text directive to one and prose to the
        // other — and the mismatch reads as a splice bug that isn't one.
        let mut ed =
            twig::Editor::new_ext(source.as_bytes(), Format::Markdown, parse_extensions()).unwrap();
        let nodes = ed.nodes().unwrap();
        crate::wysiwyg::build(&nodes, source, None, false, &std::collections::HashMap::new(), reveal)
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
            // A footnote definition is a root beside `doc`, merged back into the
            // top-level list by `wysiwyg::top_blocks`. The random edits below
            // make and unmake definitions as they go (a deleted `:` turns one
            // back into a paragraph, and vice versa), which is exactly the
            // structural churn the splice path has to notice and bail out of.
            "text[^1] here\n\n[^1]: the note\n\nmore text[^b]\n\n[^b]: second\n",
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
    fn incremental_build_matches_a_fresh_build_under_full_reveal() {
        // The same correctness net as `incremental_build_matches_a_fresh_build_
        // across_edits`, under `MarkupMode::Full` — where the map depends on
        // the caret's *line* as well as the text, so the two caches have a new
        // way to be wrong. Both are exercised: the block cache can hand back
        // rows built for a line that is no longer the revealed one, and the
        // splice path can reuse a suffix that still has yesterday's line raw.
        //
        // Caret motion is interleaved with the edits deliberately, because a
        // caret that only ever moved with the edit would never cross a line
        // without also dirtying it — the case where a stale reveal survives.
        let docs = [
            "# Title\n\n*one* and **two**\n\n[lk](http://x) and `code`\n\n- a *b*\n",
            "para *em* one\n\n> quote **bold** text\n\ntail ~~del~~ paragraph\n",
        ];
        let inserts = ["x", "*", "\n\n", "#", "`", " ", "_"];
        for src in docs {
            let mut d = wysiwyg_doc("reveal_diff", src);
            d.set_markup_mode(MarkupMode::Full);

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
                // Walk the caret somewhere else in the document, independently
                // of where the edit landed.
                let want = (step * 29 + 11) % (d.source.len() + 1);
                d.caret = (want..=d.source.len())
                    .find(|&i| d.source.is_char_boundary(i))
                    .unwrap();
                d.build_visual_unwrapped();

                let want = reference_map_revealing(&d.source, d.reveal_line());
                if maps_differ(&d.vmap, &want) {
                    panic!(
                        "FIRST MISMATCH at step {step}: {action}, caret {}\n  pre  = {pre:?}\n  post = {:?}",
                        d.caret, d.source
                    );
                }
            }
        }
    }

    #[test]
    fn caret_motion_across_lines_rebuilds_only_under_full() {
        // The cache-key change has to earn its keep in both directions: `Full`
        // must rebuild when the caret changes line (or the reveal would never
        // move), and the hidden modes must *not* (or every arrow key would pay
        // for a feature they don't use). The existing `cache_motion` test pins
        // the second for the default mode; this pins the pair against a mode
        // change alone.
        let body = "*one* here\n\n*two* there\n";

        let mut full = doc_in(View::Wysiwyg, "motion_full", body);
        full.set_markup_mode(MarkupMode::Full);
        caret_at(&mut full, "one");
        let before = full.revision();
        caret_at(&mut full, "two");
        assert_eq!(full.revision(), before, "motion is not an edit");
        assert!(
            drawn_rows(&full).iter().any(|r| r == "*two* there"),
            "the map followed the caret: {:?}",
            drawn_rows(&full)
        );

        let mut hidden = doc_in(View::Wysiwyg, "motion_hidden", body);
        caret_at(&mut hidden, "one");
        let key = hidden.vmap_key.clone();
        caret_at(&mut hidden, "two");
        assert_eq!(hidden.vmap_key, key, "a hidden mode rebuilds nothing on motion");
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
    fn wysiwyg_edits_inside_a_vis_directive_block_without_disturbing_its_fences() {
        // diaryx's `:::vis{.audience}` visibility block — any `:::name{.class}`
        // fenced div, really, since core parses these on for every document
        // now (`parse_extensions`). The container is a `directive` node, an
        // `is_block_container` kind like `block_quote`, so the caret works
        // inside its child paragraph exactly as it would inside a quote: typing
        // edits the paragraph, and the `:::vis{...}` / `:::` fences round-trip
        // untouched.
        let body = ":::vis{.public .family}\nhello\n:::\nafter\n";
        let mut d = wysiwyg_doc("wys_vis", body);
        d.caret = body.find("hello").unwrap() + "hello".len();
        d.insert("!");
        assert_eq!(
            d.source,
            ":::vis{.public .family}\nhello!\n:::\nafter\n",
            "typing inside the block edits its content in place"
        );
        assert!(d.source.contains(":::vis{.public .family}"), "opening fence survives");
        assert!(d.source.contains(":::\nafter"), "closing fence survives");
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
        // A hop lands with the destination cell's whole content selected, the
        // caret at its end — so typing replaces the cell like a form field.
        assert!(d.cell_hop(true));
        assert_eq!(d.selected_text(), Some("Qty"), "the target cell comes up selected");
        assert_eq!(d.caret, TABLE.find("Qty").unwrap() + "Qty".len());
        assert!(d.cell_hop(true), "Tab wraps onto the next row's first cell");
        assert_eq!(d.selected_text(), Some("Pear"));
        assert!(d.cell_hop(false));
        assert_eq!(d.selected_text(), Some("Qty"));
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
    fn wysiwyg_vertical_cell_motion_holds_the_column() {
        // Down/Up step to the cell above/below in the *same column*, not back to
        // the top-left the way a naive row/col motion over the picture would.
        let mut d = wysiwyg_doc("tbl_vert", TABLE);
        d.caret = TABLE.find("Qty").unwrap();
        // Each vertical hop selects the destination cell, holding the column.
        assert!(d.cell_move_vertical(true));
        assert_eq!(d.selected_text(), Some("3"), "Down holds column 1");
        assert!(d.cell_move_vertical(true));
        assert_eq!(d.selected_text(), Some("12"), "Down again, still column 1");
        assert!(!d.cell_move_vertical(true), "no row below the last");
        assert!(d.cell_move_vertical(false));
        assert_eq!(d.selected_text(), Some("3"), "Up holds column 1");
        assert!(d.cell_move_vertical(false));
        assert_eq!(d.selected_text(), Some("Qty"), "Up onto the header");
        assert!(!d.cell_move_vertical(false), "no row above the header");
    }

    #[test]
    fn tab_off_the_last_cell_grows_a_row_and_enters_it() {
        let mut d = wysiwyg_doc("tbl_grow", TABLE);
        d.caret = TABLE.rfind("12").unwrap();
        let rows_before = d.source.matches('\n').count();
        assert!(d.cell_tab(true), "acts as a table key");
        assert_eq!(
            d.source.matches('\n').count(),
            rows_before + 1,
            "a fresh row was appended"
        );
        assert!(d.caret_in_table(), "the caret entered the new row");
        // The caret sits in the new row's first cell — past the old last cell.
        assert!(d.caret > TABLE.rfind("12").unwrap());
    }

    #[test]
    fn return_in_a_table_drops_a_cell_and_grows_a_row_at_the_bottom() {
        let mut d = wysiwyg_doc("tbl_ret", TABLE);
        d.caret = TABLE.find("Name").unwrap();
        assert!(d.cell_return(), "acts as a table key");
        assert_eq!(d.selected_text(), Some("Pear"), "Return drops one cell, selecting it");
        // From the last row, Return appends a row and enters it.
        d.caret = TABLE.rfind("Fig").unwrap();
        let rows_before = d.source.matches('\n').count();
        assert!(d.cell_return());
        assert_eq!(d.source.matches('\n').count(), rows_before + 1);
        assert!(d.caret_in_table());
    }

    #[test]
    fn return_and_tab_outside_a_table_decline() {
        let mut d = wysiwyg_doc("tbl_decline", "just a paragraph\n");
        d.caret = 4;
        assert!(!d.cell_return(), "no table: the frontend inserts a newline");
        assert!(!d.cell_tab(true), "no table: the frontend indents");
        assert!(!d.cell_line_break(), "no table: the frontend breaks the line");
    }

    #[test]
    fn shift_return_inserts_an_in_cell_break_the_renderer_reads_as_a_line() {
        let mut d = wysiwyg_doc("tbl_break", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4; // just after "Pear"
        assert!(d.cell_line_break(), "acts as a table key");
        assert!(d.source.contains("Pear<br>"), "spelled as an inline <br>: {}", d.source);
        assert!(d.caret_in_table(), "still in the cell, past the break");
        // The break renders as a real line: the "Pear" cell now draws two lines,
        // so the table's picture is one row taller than a single-line table.
        d.build_visual(80);
        let table = &d.vmap.tables[0];
        let cell = &table.grid[1].cells[0]; // first body row, first column
        assert!(
            cell.glyphs.iter().any(|g| g.ch == '\n'),
            "the cell carries the break as a newline glyph for the frontend to split"
        );
    }

    #[test]
    fn shift_return_in_a_markdown_cell_leaves_a_semantic_hard_break_not_raw_html() {
        // twig promotes the in-cell `<br>` to a `hard_break`, so the break reads
        // back as structure — the whole point of routing through insert_line_break
        // instead of splicing raw `<br>` bytes.
        let mut d = wysiwyg_doc("tbl_break_semantic", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4;
        assert!(d.cell_line_break());
        let kinds: Vec<String> = d.editor.nodes().unwrap().iter().map(|n| n.kind.clone()).collect();
        assert!(kinds.iter().any(|k| k == "hard_break"), "got {kinds:?}");
        assert!(!kinds.iter().any(|k| k == "raw_inline"), "still raw HTML: {kinds:?}");
    }

    #[test]
    fn backspace_over_an_in_cell_break_deletes_the_whole_br_not_a_byte() {
        // The `<br>` draws as one newline glyph, so Backspace over it must take
        // all four bytes — a one-byte delete would strand a visible `<br` in the
        // cell (the reported bug).
        let mut d = wysiwyg_doc("tbl_break_bs", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4;
        assert!(d.cell_line_break());
        assert!(d.source.contains("Pear<br>"), "precondition: {}", d.source);
        d.backspace(); // caret sits just past the break
        assert!(!d.source.contains("<br"), "no half-deleted <br left: {}", d.source);
        assert!(d.source.contains("| Pear |"), "the cell is back to one line: {}", d.source);
    }

    #[test]
    fn delete_forward_over_an_in_cell_break_deletes_the_whole_br() {
        let mut d = wysiwyg_doc("tbl_break_del", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4;
        assert!(d.cell_line_break());
        d.caret = TABLE.find("Pear").unwrap() + 4; // back onto the break's start
        d.delete_forward();
        assert!(!d.source.contains("<br"), "no half-deleted <br: {}", d.source);
        assert!(d.source.contains("| Pear |"), "cell back to one line: {}", d.source);
    }

    #[test]
    fn shift_return_in_a_djot_cell_is_swallowed_and_leaves_the_row_intact() {
        // Djot has no idiomatic in-cell break, so twig refuses it. The gesture is
        // still consumed (a real newline would split the one-line row), but the
        // cell must be left exactly as it was — no non-idiomatic `<br>` spliced in.
        let src = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n";
        let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
        d.caret = src.find("Pear").unwrap() + 4;
        assert!(d.caret_in_table(), "caret should be inside the djot table");
        assert!(d.cell_line_break(), "the key is consumed, not passed to the frontend");
        assert_eq!(d.source, src, "the djot cell is left untouched");
        assert!(!d.source.contains("<br>"), "no non-idiomatic <br> spliced into djot");
        assert!(d.status.is_some(), "the refusal is surfaced on the status line");
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
        // Tab indents a list item by its own marker width, landing its marker at
        // the parent's content column so twig reparses it as a nested list.
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
    fn indent_nests_an_ordered_item_at_its_marker_width() {
        // An ordered marker `1. ` is three columns wide, so a two-space step
        // (which nests a bullet) leaves it flat. Regression: Tab must use the
        // marker width, three, so the item actually nests — and the source
        // renumbers so the sub-list restarts at 1 and the outer list resumes.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_ord", "1. a\n2. b\n3. c\n");
            d.caret = d.source.find('b').unwrap();
            d.indent();
            assert_eq!(d.source, "1. a\n   1. b\n2. c\n");
            let lists = d.nodes().iter().filter(|n| n.kind == "ordered_list").count();
            assert_eq!(lists, 2, "the indented item is a nested ordered list");
        }
    }

    #[test]
    fn indent_leaves_a_lists_first_item_put() {
        // The first item of a list has no sibling above it to nest under, so Tab
        // is a no-op there — the marker stays at column zero rather than being
        // shoved into indentation twig can't read as a sub-list.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_first", "- a\n- b\n");
            d.caret = 1; // on the FIRST item
            d.indent();
            assert_eq!(d.source, "- a\n- b\n", "the first item doesn't nest");
            // The sibling below still nests, proving the guard is per-item.
            d.caret = d.source.find('b').unwrap();
            d.indent();
            assert_eq!(d.source, "- a\n  - b\n");
        }
    }

    #[test]
    fn hidden_mode_keeps_typed_markup_literal() {
        // The Diaryx default: typing `*hi*` gives the characters, not emphasis —
        // twig escapes what would open markup, so the source is `\*hi\*` and the
        // AST is a plain string. Formatting is the commands' job in this mode.
        let mut d = doc_in(View::Wysiwyg, "hidden_literal", "");
        d.insert("*hi*");
        assert_eq!(d.source, "\\*hi\\*");
        assert!(d.nodes().iter().all(|n| n.kind != "emph" && n.kind != "strong"));
    }

    #[test]
    fn hidden_mode_escapes_a_line_start_block_marker() {
        // A `#`/`-`/`>` at a line start would open a block, so Hidden mode keeps
        // it literal too — a Diaryx user's "# 1 idea" stays prose, not a heading.
        let mut d = doc_in(View::Wysiwyg, "hidden_block", "");
        d.insert("# hi");
        assert_eq!(d.source, "\\# hi");
        assert!(d.nodes().iter().all(|n| n.kind != "heading"));
    }

    #[test]
    fn authoring_modes_keep_typed_markup_live() {
        // Both authoring rungs of the ladder: typing `*hi*` really is emphasis
        // (no escape), the same as source view — escaping is `None`'s alone, and
        // it's the axis, not the reveal, that decides.
        for (view, mode) in [
            (View::Wysiwyg, MarkupMode::Shortcuts),
            (View::Wysiwyg, MarkupMode::Full),
            (View::Source, MarkupMode::None),
        ] {
            let mut d = doc_in(view, "live_markup", "");
            d.set_markup_mode(mode);
            d.insert("*hi*");
            assert_eq!(d.source, "*hi*", "{mode:?} in {view:?} types raw markup");
        }
    }

    #[test]
    fn hidden_mode_overwrite_undoes_in_one_step() {
        // Typing over a selection escapes the replacement *and* stays a single
        // undo — the selection-delete and the literal insert fold together, so
        // one undo brings the whole selection back, like a plain overwrite.
        let mut d = doc_in(View::Wysiwyg, "hidden_overwrite", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6; // "word"
        d.insert("*");
        assert_eq!(d.source, "a \\* b\n", "the replacement is escaped");
        d.undo();
        assert_eq!(d.source, "a word b\n");
        assert_eq!(d.selection(), Some((2, 6)), "one undo, selection restored");
    }

    #[test]
    fn backspace_over_an_escaped_char_takes_the_hidden_backslash_too() {
        // Type `*` in Hidden mode → `\*` (drawn as one `*`); one Backspace clears
        // the whole visual character, never stranding the hidden `\`.
        let mut d = doc_in(View::Wysiwyg, "bsp_escape", "");
        d.insert("*");
        assert_eq!(d.source, "\\*");
        d.backspace();
        assert_eq!(d.source, "", "the escape backslash went with the *");
        // A *literal* backslash (source view, no escape) is an ordinary char.
        let mut s = doc_in(View::Source, "bsp_lit", "a\\b\n");
        s.caret = 3; // after `b`
        s.backspace();
        assert_eq!(s.source, "a\\\n", "only the b is deleted, the \\ stays");
    }

    #[test]
    fn hidden_mode_leaves_structural_markup_alone() {
        // Enter continues a bullet list by writing a real `- ` marker (an
        // `insert_raw`, not the typing path), so Hidden mode's escaping never
        // touches it — the list keeps working.
        let mut d = doc_in(View::Wysiwyg, "hidden_struct", "- item\n");
        d.caret = 6;
        d.newline();
        d.insert("two");
        assert_eq!(d.source, "- item\n- two\n");
    }

    #[test]
    fn markup_mode_defaults_to_none_and_round_trips() {
        // Diaryx's default is the clean `None` surface; a markup-fluent
        // frontend can climb the ladder, and the choice sticks.
        let mut d = doc_in(View::Wysiwyg, "markup_mode", "hi\n");
        assert_eq!(d.markup_mode(), MarkupMode::None, "None by default");
        for mode in [MarkupMode::Shortcuts, MarkupMode::Full, MarkupMode::None] {
            d.set_markup_mode(mode);
            assert_eq!(d.markup_mode(), mode);
        }
    }

    #[test]
    fn full_mode_reveals_only_the_caret_line() {
        // The mode's whole claim: the caret's line shows its raw delimiters and
        // every other line stays resolved. Two paragraphs with identical markup
        // so the only difference between the rows is where the caret is.
        let mut d = doc_in(View::Wysiwyg, "reveal_caret_line", "*one* here\n\n*two* there\n");
        d.set_markup_mode(MarkupMode::Full);

        caret_at(&mut d, "one");
        let rows = drawn_rows(&d);
        assert!(rows.iter().any(|r| r == "*one* here"), "caret's line raw: {rows:?}");
        assert!(rows.iter().any(|r| r == "two there"), "other line resolved: {rows:?}");

        // Move to the other paragraph: the reveal follows, and the line just
        // left goes back to being resolved.
        caret_at(&mut d, "two");
        let rows = drawn_rows(&d);
        assert!(rows.iter().any(|r| r == "*two* there"), "caret's line raw: {rows:?}");
        assert!(rows.iter().any(|r| r == "one here"), "left line resolved: {rows:?}");
    }

    #[test]
    fn hidden_modes_never_reveal_wherever_the_caret_is() {
        // The two rungs below `Full` share a rendering: delimiters stay hidden
        // even under the caret. `Shortcuts` differing from `None` only in what
        // typing does is exactly the point of splitting the axes.
        for mode in [MarkupMode::None, MarkupMode::Shortcuts] {
            let mut d = doc_in(View::Wysiwyg, "reveal_hidden", "*one* here\n");
            d.set_markup_mode(mode);
            caret_at(&mut d, "one");
            let rows = drawn_rows(&d);
            assert!(rows.iter().any(|r| r == "one here"), "{mode:?} hides: {rows:?}");
            assert!(!rows.iter().any(|r| r.contains('*')), "{mode:?} shows no `*`: {rows:?}");
        }
    }

    #[test]
    fn revealed_delimiters_are_the_authors_own_spelling() {
        // Delimiters are re-read from the source rather than synthesized per
        // kind, so a line comes back spelled the way it was written: `_em_` does
        // not turn into `*em*`, and a two-backtick fence keeps both backticks.
        let body = "_em_ and __st__ and ``lit ` tick`` and [lk](http://x) and ~~del~~\n";
        let mut d = doc_in(View::Wysiwyg, "reveal_spelling", body);
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "em");
        let rows = drawn_rows(&d);
        assert!(
            rows.iter().any(|r| r == body.trim_end()),
            "the revealed line is its own source: {rows:?}"
        );
    }

    #[test]
    fn revealed_heading_shows_its_hashes() {
        // The `# ` marker is a block-level prefix, not an inline delimiter, so
        // it takes its own path — but it reveals on the same rule.
        let mut d = doc_in(View::Wysiwyg, "reveal_heading", "# Title\n\nbody\n");
        d.set_markup_mode(MarkupMode::Full);

        caret_at(&mut d, "Title");
        assert!(drawn_rows(&d).iter().any(|r| r == "# Title"), "{:?}", drawn_rows(&d));

        caret_at(&mut d, "body");
        let rows = drawn_rows(&d);
        assert!(rows.iter().any(|r| r == "Title"), "hashes hidden again: {rows:?}");
    }

    #[test]
    fn revealed_delimiters_are_caret_stops() {
        // A delimiter that is drawn but can't be reached is worse than one
        // that's hidden: the mode exists so the markup can be *edited*. Every
        // revealed byte must be somewhere the caret can stand.
        let mut d = doc_in(View::Wysiwyg, "reveal_stops", "*em* x\n");
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "em");
        let opener = d.source.find('*').unwrap();
        assert!(d.vmap.is_stop(opener), "the opening `*` is a caret stop");
        assert!(d.vmap.is_stop(opener + 3), "the closing `*` is a caret stop");
    }

    #[test]
    fn setext_heading_reveals_nothing_across_its_newline() {
        // A setext heading's underline is on another line, so it is not the
        // caret line's to reveal — and emitting it would inject a `\n` glyph
        // that splits the row where the author wrote no break.
        let mut d = doc_in(View::Wysiwyg, "reveal_setext", "Title\n=====\n\nbody\n");
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "Title");
        let rows = drawn_rows(&d);
        assert!(rows.iter().any(|r| r == "Title"), "title renders alone: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains('=')), "no underline leaks in: {rows:?}");
    }

    #[test]
    fn markup_mode_axes_split_the_ladder() {
        // The two behaviours the ladder spells: `Shortcuts` is the middle rung
        // that authors markup but still hides it, and it's the only rung where
        // the two axes disagree.
        assert!(!MarkupMode::None.authors());
        assert!(!MarkupMode::None.reveals_caret_line());
        assert!(MarkupMode::Shortcuts.authors());
        assert!(!MarkupMode::Shortcuts.reveals_caret_line());
        assert!(MarkupMode::Full.authors());
        assert!(MarkupMode::Full.reveals_caret_line());
    }

    #[test]
    fn indenting_an_empty_dash_item_under_text_dodges_the_setext_collapse() {
        // Tabbing an empty `- ` under a text line would spell `- hello\n  - `,
        // which twig (correctly, per CommonMark — pandoc agrees) reparses as a
        // setext H2. leaf swaps the dash for a `*` so the item stays an empty
        // nested bullet and `hello` stays prose: the file round-trips instead of
        // hiding a heading the user never asked for.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "setext_guard", "- hello\n- \n");
            d.caret = d.source.find("- \n").unwrap() + 2; // after the empty marker
            d.indent();
            assert_eq!(d.source, "- hello\n  * \n");
            assert!(d.nodes().iter().all(|n| n.kind != "heading"), "no heading");
            // And it's genuinely a nested list, not a flat one.
            assert_eq!(d.nodes().iter().filter(|n| n.kind == "bullet_list").count(), 2);
        }
    }

    #[test]
    fn indenting_a_dash_item_with_content_keeps_its_dash() {
        // With content, `- x` can't be a setext underline, so there's nothing to
        // dodge: the marker stays a dash and nests as an ordinary sub-bullet.
        let mut d = doc_in(View::Wysiwyg, "setext_ok", "- hello\n- x\n");
        d.caret = d.source.find('x').unwrap();
        d.indent();
        assert_eq!(d.source, "- hello\n  - x\n");
    }

    #[test]
    fn the_setext_swap_undoes_as_one_step_with_the_indent() {
        // The dash→`*` repair coalesces into the Tab, so a single undo restores
        // the whole pre-Tab state rather than stranding a half-collapsed doc.
        let mut d = doc_in(View::Wysiwyg, "setext_undo", "- hello\n- \n");
        d.caret = d.source.find("- \n").unwrap() + 2;
        d.indent();
        assert_eq!(d.source, "- hello\n  * \n");
        d.undo();
        assert_eq!(d.source, "- hello\n- \n", "one undo, not two");
    }

    #[test]
    fn indent_leaves_a_nested_lists_first_item_put_too() {
        // The guard is about siblings, not depth: the first item of an *inner*
        // list (already nested under `a`) still has nothing before it at its own
        // level, so Tab can't take it deeper.
        let mut d = doc_in(View::Wysiwyg, "indent_first_nested", "- a\n  - b\n  - c\n");
        d.caret = d.source.find('b').unwrap();
        d.indent();
        assert_eq!(d.source, "- a\n  - b\n  - c\n", "inner first item holds");
        // But `c` (a sibling of `b`) nests under `b`.
        d.caret = d.source.find('c').unwrap();
        d.indent();
        assert_eq!(d.source, "- a\n  - b\n    - c\n");
    }

    #[test]
    fn backspace_at_a_nested_item_start_outdents_it() {
        // Backspace with the caret right after a nested item's marker gives back
        // one level of nesting, the mirror of Tab — and renumbers the flattened
        // ordered list back to a clean run.
        let mut d = doc_in(View::Wysiwyg, "bsp_outdent", "1. a\n   1. b\n2. c\n");
        d.caret = d.source.find('b').unwrap(); // start of the nested item's content
        d.backspace();
        assert_eq!(d.source, "1. a\n2. b\n3. c\n");
    }

    #[test]
    fn backspace_at_a_top_level_item_start_strips_the_marker() {
        // At the outermost level there's no nesting left to give back, so the same
        // keystroke drops the bullet and leaves a plain paragraph.
        let mut d = doc_in(View::Wysiwyg, "bsp_strip", "- a\n- b\n");
        d.caret = d.source.find('b').unwrap(); // right after `- `
        d.backspace();
        assert_eq!(d.source, "- a\nb\n", "the marker is gone, the text stays");
    }

    #[test]
    fn backspace_mid_item_still_deletes_a_character() {
        // The list behaviour is armed only at the item's content start; anywhere
        // else Backspace is the ordinary character delete.
        let mut d = doc_in(View::Wysiwyg, "bsp_mid", "- ab\n");
        d.caret = d.source.find('b').unwrap(); // between `a` and `b`
        d.backspace();
        assert_eq!(d.source, "- b\n");
    }

    #[test]
    fn backspace_at_a_heading_start_strips_the_marker() {
        // The `# ` is markup the rich view hides, so Backspace over it takes the
        // whole marker and leaves a paragraph. Deleting a byte of it instead left
        // `#Title` — no longer a heading, with the hash now literal text the user
        // never typed and has to delete again.
        let mut d = doc_in(View::Wysiwyg, "bsp_head", "## Title\n");
        d.caret = d.source.find('T').unwrap(); // right after `## `
        d.backspace();
        assert_eq!(d.source, "Title\n");
        assert_eq!(d.caret, 0, "the caret stays with the text it was in front of");
    }

    #[test]
    fn backspace_at_a_heading_start_keeps_the_block_around_it() {
        // Only the heading's own marker goes — the quote (or list) it sits in is
        // untouched, exactly as un-heading it should be.
        let mut d = doc_in(View::Wysiwyg, "bsp_head_quote", "> # Title\n");
        d.caret = d.source.find('T').unwrap();
        d.backspace();
        assert_eq!(d.source, "> Title\n");
    }

    #[test]
    fn backspace_at_a_heading_start_takes_its_closing_sequence_too() {
        // `# Title #`'s trailing hashes are hidden at the other end; leaving them
        // behind would surface the same stray hash the marker delete just avoided.
        let mut d = doc_in(View::Wysiwyg, "bsp_head_closed", "# Title #\n");
        d.caret = d.source.find('T').unwrap();
        d.backspace();
        assert_eq!(d.source, "Title\n");
        // And it's one edit: a single undo puts the whole heading back.
        d.undo();
        assert_eq!(d.source, "# Title #\n");
    }

    #[test]
    fn backspace_mid_heading_still_deletes_a_character() {
        // The heading behaviour is armed only at the content's start; anywhere
        // else Backspace is the ordinary character delete.
        let mut d = doc_in(View::Wysiwyg, "bsp_head_mid", "# ab\n");
        d.caret = d.source.find('b').unwrap();
        d.backspace();
        assert_eq!(d.source, "# b\n");
    }

    #[test]
    fn source_view_backspace_still_edits_the_heading_marker_literally() {
        // In source view the `# ` is text on the screen the user is deleting a
        // byte of, so it keeps its literal meaning — the same split the list
        // ladder and Enter draw between the two views.
        let mut d = doc_with("bsp_head_src", "# Title\n");
        d.caret = d.source.find('T').unwrap();
        d.backspace();
        assert_eq!(d.source, "#Title\n");
    }

    #[test]
    fn outdent_unnests_an_ordered_item_in_one_press() {
        // Shift+Tab gives back exactly the marker width the indent added, so a
        // nested ordered item unnests in a single press, and the flattened list
        // renumbers back to a clean 1, 2, 3.
        let mut d = doc_with("outdent_ord", "1. a\n   2. b\n3. c\n");
        d.caret = d.source.find('b').unwrap();
        d.outdent();
        assert_eq!(d.source, "1. a\n2. b\n3. c\n");
        let lists = d.nodes().iter().filter(|n| n.kind == "ordered_list").count();
        assert_eq!(lists, 1, "back to one flat list");
    }

    #[test]
    fn table_insert_row_adds_a_row_below_the_caret() {
        let mut d = doc_with("tbl_ins_row", "| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        d.caret = d.source.find('1').unwrap(); // in the body row
        d.table_insert_row(true);
        assert_eq!(
            d.source,
            "| a | b |\n| --- | --- |\n| 1 | 2 |\n|  |  |\n"
        );
    }

    #[test]
    fn table_insert_and_delete_column_at_the_caret() {
        let mut d = doc_with("tbl_col", "| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        d.caret = d.source.find('a').unwrap(); // column 0
        d.table_insert_column(true); // add a column to the right of `a`
        assert_eq!(
            d.source,
            "| a |  | b |\n| --- | --- | --- |\n| 1 |  | 2 |\n"
        );
        d.caret = d.source.find('b').unwrap(); // now the third column
        d.table_delete_column();
        assert_eq!(d.source, "| a |  |\n| --- | --- |\n| 1 |  |\n");
    }

    #[test]
    fn table_set_alignment_respells_the_delimiter() {
        let mut d = doc_with("tbl_align", "| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        d.caret = d.source.find('b').unwrap();
        d.table_set_alignment(Alignment::Right);
        assert_eq!(d.source, "| a | b |\n| --- | ---: |\n| 1 | 2 |\n");
    }

    #[test]
    fn each_empty_table_cell_has_its_own_editable_home() {
        // Regression: an empty cell has no twig content_span, so both cells of a
        // `|  |  |` row collapsed onto the row's start (before the first `│`).
        // Typing there inserted *before* the table (`hello|  |  |`); nav couldn't
        // tell the cells apart. Each empty cell must now have a distinct home
        // inside it.
        let mut d = wysiwyg_doc("tbl_empty", "| a | b |\n| --- | --- |\n|  |  |\n");
        let (c0, c1) = {
            let cells = &d.vmap.tables[0].grid[1].cells;
            (cells[0].start, cells[1].start)
        };
        assert!(c0 < c1, "the two empty cells have distinct homes: {c0} < {c1}");
        d.caret = c0;
        d.insert("x");
        assert_eq!(d.source, "| a | b |\n| --- | --- |\n| x |  |\n", "typed inside the cell");
    }

    #[test]
    fn arrows_step_into_each_empty_table_cell() {
        let mut d = wysiwyg_doc("tbl_empty_nav", "| a | b |\n| --- | --- |\n|  |  |\n");
        let (c0, c1) = {
            let cells = &d.vmap.tables[0].grid[1].cells;
            (cells[0].start, cells[1].start)
        };
        d.caret = d.source.find('b').unwrap(); // in the header's second cell
        let mut seen = std::collections::HashSet::new();
        for _ in 0..6 {
            d.move_right(false);
            seen.insert(d.caret);
        }
        assert!(seen.contains(&c0), "right arrow reaches the first empty cell");
        assert!(seen.contains(&c1), "right arrow reaches the second empty cell");
    }

    #[test]
    fn table_op_off_a_table_is_a_no_op_with_a_status() {
        let mut d = doc_with("tbl_none", "just text\n");
        d.caret = 3;
        d.table_insert_row(true);
        assert_eq!(d.source, "just text\n", "nothing changed");
        assert!(d.status.is_some(), "a status explains why");
        assert!(!d.caret_in_table());
    }

    #[test]
    fn enter_in_an_ordered_list_renumbers_the_following_items() {
        // Inserting an item mid-list left the source markers stale (`1. 2. 2. 3.`);
        // the renumber pass keeps them sequential, matching what the view draws.
        let mut d = wysiwyg_doc("enter_renumber", "1. a\n2. b\n3. c\n");
        d.caret = d.source.find('a').unwrap() + 1; // end of item a
        d.newline();
        d.insert("x");
        assert_eq!(d.source, "1. a\n2. x\n3. b\n4. c\n");
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
    fn line_flow_preserve_resplits_the_map_and_defaults_to_fold() {
        // The paragraph holds one soft break. Folded (the default) it lays out as
        // a single reflowed row; Preserve re-lays it as a row per source line.
        // The setter must invalidate the cached map for the change to show, and
        // again on the way back — so a round trip returns to the folded layout.
        let mut d = wysiwyg_doc("line_flow", "one two\nthree four\n");
        assert_eq!(d.line_flow(), LineFlow::Fold, "fold is the default");
        d.build_visual(80);
        assert_eq!(d.vmap.num_rows(), 1, "fold: one flowing row");

        d.set_line_flow(LineFlow::Preserve);
        d.build_visual(80);
        assert_eq!(d.vmap.num_rows(), 2, "preserve: a row per source line");

        d.set_line_flow(LineFlow::Fold);
        d.build_visual(80);
        assert_eq!(d.vmap.num_rows(), 1, "fold again: back to one row");
    }

    #[test]
    fn the_caret_still_crosses_a_preserved_soft_break() {
        // Preserve renders the soft break as a row boundary rather than a space,
        // but the caret must still reach every offset — the break's own offset is
        // the first row's end stop, so Right walks clean off the end of line one
        // onto line two, exactly as it does when the break is folded.
        let mut d = wysiwyg_doc("preserve_walk", "one two\nthree four\n");
        d.set_line_flow(LineFlow::Preserve);
        d.build_visual(80);
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
        // has a job to do — over the word that's left, with the space the delete
        // pushed against the opening delimiter moved out in front of it, or the
        // run would be no run at all (`** words**` is literal asterisks — see
        // the mark-edge rule on `splice`).
        let src = "a **two words** c\n";
        let mut d = wysiwyg_doc("wys_word_del_partial", src);
        d.caret = src.find(" words").unwrap();
        d.delete_word_back();
        assert_eq!(d.source, "a  **words** c\n");
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
        // Plain text — a blank doc opens in Hidden mode, where a typed `#` would
        // be kept literal (`\#`); this test is about save-as, not escaping (which
        // has its own test), so it types nothing that escaping would touch.
        d.insert("hi");
        d.save_as(p.clone());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hi");
        assert!(!d.is_untitled());
        assert!(!d.dirty);
        assert_eq!(d.file_name(), p.file_name().unwrap().to_string_lossy());
        assert_eq!(d.disk_state(), DiskState::Unchanged, "the watermark is stamped");
        // And ⌘S is a plain save from here on.
        d.insert("!");
        d.save();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hi!");
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



