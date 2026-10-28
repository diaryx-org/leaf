//! Building a `Doc` and the state every other part reads: the view and its
//! markup mode, the visual and source maps, the selection, counts, and highlights.

use super::*;

impl Doc {
    #[cfg(feature = "fs")]
    pub fn open(path: PathBuf) -> Result<Self> {
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_disk_bytes(path, bytes)
    }

    /// An empty document *named* `path`, for a file that isn't there yet — what
    /// every other terminal editor gives you when you name a file that doesn't
    /// exist. It is a real named document, not a [`Doc::blank`]: `is_untitled`
    /// is false, so ⌘S writes straight to `path` with no Save As detour, and
    /// the header shows the name the user asked for.
    ///
    /// The format comes from the extension, exactly as [`Doc::open`] reads it —
    /// so `leaf notes.dj` starts a djot buffer rather than the Markdown
    /// [`Doc::blank`] has to assume for want of a name. An extension leaf can't
    /// parse is still an error: a mistyped flag or a stray argument should say
    /// so, not open a buffer promising to save somewhere.
    ///
    /// The watermark is the hash of *no bytes*, not `None`, and that is the
    /// whole trick: `None` means untitled, and would leave [`Doc::disk_state`]
    /// answering [`DiskState::Untitled`] for a document that has a path and
    /// intends to write to it. Hashing `""` instead makes the answers the true
    /// ones — [`DiskState::Missing`] while the file still isn't there (a save
    /// recreates it, which is exactly what this is for), and
    /// [`DiskState::Changed`] if somebody creates it underneath us between
    /// launch and save, so the frontend's overwrite prompt guards a new file as
    /// it guards an opened one.
    ///
    /// Nothing is written here. A buffer that is never typed into never touches
    /// the filesystem, and a `path` whose directory doesn't exist is allowed to
    /// open — the write is where that fails, and it says so then.
    #[cfg(feature = "fs")]
    pub fn create(path: PathBuf) -> Result<Self> {
        Self::from_disk_bytes(path, Vec::new())
    }

    /// [`Doc::open`] when the file is there, [`Doc::create`] when it isn't —
    /// the call a CLI frontend wants for its path argument.
    ///
    /// The decision is made from the failed read itself rather than a `exists()`
    /// check first, so there is no window between the two for the file to appear
    /// or vanish in. Only `NotFound` opens a new buffer: a permissions error or
    /// a directory in the way is still an error, because pretending those are
    /// "no file yet" would offer to save over something leaf couldn't read.
    #[cfg(feature = "fs")]
    pub fn open_or_create(path: PathBuf) -> Result<Self> {
        match std::fs::read(&path) {
            Ok(bytes) => Self::from_disk_bytes(path, bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::create(path),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// The shared body of [`Doc::open`] and [`Doc::create`]: bytes that are (or
    /// stand in for) the file at `path`, parsed as the format its extension
    /// names. Keeping the two on one path is what makes a new file's document
    /// identical in every respect to an opened one but its contents.
    #[cfg(feature = "fs")]
    fn from_disk_bytes(path: PathBuf, bytes: Vec<u8>) -> Result<Self> {
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
        Ok(Doc::from_parts(
            editor,
            format,
            PathBuf::new(),
            source,
            None,
        ))
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
        Ok(Doc::from_parts(
            editor,
            format,
            PathBuf::new(),
            String::new(),
            None,
        ))
    }

    /// The fields every constructor agrees on, so `open` and `blank` can't drift
    /// apart in the ones neither of them has an opinion about.
    // `identity` is taken from a counter rather than from the `Doc`'s address,
    // which moves — a session that holds one is moved into and out of
    // containers freely, and an identity that changed with it would defeat the
    // one comparison it exists for.
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
            ops_base: source.clone(),
            source,
            caret: 0,
            anchor: None,
            dirty: false,
            status: None,
            read_only: false,
            highlights: Vec::new(),
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
            typeset: false,
            last_edit_kind: None,
            pending_marks: InlineMarks::empty(),
            pending_at: None,
            goal_col: None,
            vmap: VisualMap::default(),
            smap: SourceMap::default(),
            // No map yet — the first `build_source` always builds.
            smap_key: None,
            revision: 0,
            undo_steps: 0,
            redo_steps: 0,
            ops: Vec::new(),
            ops_revision: 0,
            undo_group: None,
            #[cfg(test)]
            refuse_next_literal: false,
            // No map yet — the first `build_visual` always builds.
            vmap_key: None,
            screen: None,
            identity: NEXT_IDENTITY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            block_cache: wysiwyg::BlockCache::default(),
            surface: wysiwyg::Surface::default(),
            scroll: 0,
            body_origin: (0, 0),
            body_width: 0,
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

    /// The line the caret sits on, when that line should render something
    /// raw — `None` when nothing on it would, which is what the builder reads
    /// as "reveal nothing" and what keeps caret motion from costing a build.
    ///
    /// Two things ask for it. Under [`MarkupMode::Full`] every delimiter on
    /// the caret's line shows ([`Reveal::full`]). In the two hidden modes a
    /// *formula* on it still shows its TeX ([`Reveal::math`]), because a
    /// formula's content is not its picture and hiding the `$` alone would
    /// leave nothing to edit; there the line is threaded through only when it
    /// meets a block that holds one, which the last build's layout knows
    /// ([`wysiwyg::BlockCache::math_meets`]) — so a document with no math
    /// keeps the `None` it always had, and one with math pays a rebuild only
    /// while the caret is in the formula's block.
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
    pub(crate) fn reveal_line(&self) -> Option<Reveal> {
        // A page has no caret, so no line of it is the caret's.
        if self.view != View::Wysiwyg || self.screen.is_some() {
            return None;
        }
        let line = source_line_range(&self.source, self.caret);
        if self.markup_mode.reveals_caret_line() {
            return Some(Reveal::full(line));
        }
        // A frontend with a formula editor of its own opens the picture,
        // not the source.
        if self.surface.owns(wysiwyg::MATH_REGION) {
            return None;
        }
        self.block_cache
            .math_meets(&line)
            .then_some(Reveal::math(line))
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
    /// Tell the document its rich view is drawn by a typesetter (Leaf's
    /// reflowed and paged views), where an empty paragraph sets as nothing:
    /// Enter in one does nothing, and Enter at a paragraph's start puts the
    /// caret in the paragraph it opens above, the one empty paragraph a
    /// typesetter's view can show, as the caret's line.
    pub fn set_typeset(&mut self, typeset: bool) {
        self.typeset = typeset;
    }

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

    /// Lay the document out as paper shows it, with no line revealed — or,
    /// with `false`, go back to the screen's map.
    ///
    /// The reveal is core's, folded into the rows before a frontend sees them:
    /// under [`MarkupMode::Full`] the caret's line shows its delimiters, and in
    /// every mode a formula on it is its TeX rather than its picture. A page
    /// has no caret, so a PDF or a printout laid out from the screen's map
    /// printed the one line the reader happened to be standing on as source.
    /// While this is on, [`build_visual`](Self::build_visual) and its twin
    /// build as though nothing were revealed.
    ///
    /// Kept apart from the screen's build rather than replacing it: turning
    /// this on sets the screen's map aside — and the caret, which a build
    /// clamps against the map it builds — and turning it off puts both back
    /// as they were, so the screen's next build costs nothing it would not
    /// have cost anyway. Meant to bracket one read of the map, on and then
    /// off; an edit in between is not the paper's to make.
    pub fn set_unrevealed(&mut self, on: bool) {
        match (on, self.screen.take()) {
            (true, None) => {
                self.screen = Some(Box::new(ScreenMap {
                    vmap: std::mem::take(&mut self.vmap),
                    key: self.vmap_key.take(),
                    caret: self.caret,
                    anchor: self.anchor,
                    goal_col: self.goal_col,
                }));
            }
            (false, Some(screen)) => {
                let ScreenMap {
                    vmap,
                    key,
                    caret,
                    anchor,
                    goal_col,
                } = *screen;
                self.vmap = vmap;
                self.vmap_key = key;
                self.caret = caret;
                self.anchor = anchor;
                self.goal_col = goal_col;
            }
            // Already in the state asked for.
            (_, screen) => self.screen = screen,
        }
    }

    /// Build the source view's syntax map ([`Doc::smap`]) — the styling for
    /// [`View::Source`], the way [`build_visual`](Self::build_visual) is the
    /// styling for [`View::Wysiwyg`].
    ///
    /// A frontend calls this before painting raw source. One that doesn't gets
    /// an empty map and paints unstyled text, so this is additive: nothing
    /// breaks by not calling it.
    ///
    /// Built at most once per revision, and the revision is the whole key — the
    /// map has no width and no caret in it, so it survives every resize, every
    /// motion, and every selection change.
    ///
    /// The builds it does do cost a whole-arena marshal, which is precisely what
    /// the WYSIWYG path works to avoid, so this has no incremental path where
    /// that one has two. From `cargo run --release -p leaf-core --example
    /// bench`, per keystroke, against the WYSIWYG build the source view is
    /// *not* doing:
    ///
    /// |  size |  nodes | marshal | `source::build` | (`wysiwyg::build`) |
    /// |------:|-------:|--------:|----------------:|-------------------:|
    /// |  10 KB|    613 |  0.16 ms|         0.07 ms |            0.28 ms |
    /// | 100 KB|  6 097 |  0.84 ms|         0.38 ms |            2.43 ms |
    /// |   1 MB| 60 601 |  5.67 ms|         3.12 ms |           23.39 ms |
    ///
    /// Linear, two thirds of it the marshal, and the build itself five to seven
    /// times cheaper than the one it stands in for at every size. Comfortable
    /// well past any document a person edits in a terminal — a megabyte is where
    /// it would want [`Editor::dirty_range`] and the same splice treatment
    /// `build_spliced` gives the other map. The door is open; nothing has needed
    /// it yet.
    pub fn build_source(&mut self) {
        if self.smap_key == Some(self.revision) {
            return;
        }
        let nodes = self.nodes();
        self.smap = source::build(&nodes, &self.source);
        self.smap_key = Some(self.revision);
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
        if self.surface.media_rows == rows {
            return;
        }
        self.surface.media_rows = rows;
        self.surface_changed();
    }

    /// Tell the model how many visual rows each leaf directive should reserve,
    /// keyed by its [`DirectiveKey`](wysiwyg::DirectiveKey) — its name, label
    /// and attributes, as [`DirectiveInfo::key`](wysiwyg::DirectiveInfo::key)
    /// hands it over. The peer of [`set_media_rows`](Self::set_media_rows) for
    /// a terminal host that draws a directive as lines of its own: it asks its
    /// renderer for the lines, counts them, and reports back, and the next
    /// build lays the placeholder out that tall — the label row plus blank
    /// filler rows the host's lines are drawn over, holding no caret. A
    /// directive left out of the map is the one-row placeholder, which is what
    /// a host with no drawing for it, or a frontend laying directives out in
    /// its own units, gets by never calling this.
    ///
    /// Keyed by what the directive says rather than where it stands, as a
    /// picture is keyed by its destination, so the height holds across an
    /// edit above it and two directives spelled alike share one entry. Cheap
    /// to call every frame with the same map: only a change drops the caches.
    pub fn set_directive_rows(&mut self, rows: HashMap<wysiwyg::DirectiveKey, usize>) {
        if self.surface.directive_rows == rows {
            return;
        }
        self.surface.directive_rows = rows;
        self.surface_changed();
    }

    /// Tell the model how many visual rows each display formula should
    /// reserve, keyed by the formula's TeX exactly as the map's
    /// [`MathInfo::tex`](wysiwyg::MathInfo::tex) handed it over. The peer of
    /// [`set_media_rows`](Self::set_media_rows) for the terminal, which
    /// typesets the picture, measures it in cells, and reports back; a
    /// frontend that lays formulas out in pixels never calls this and gets
    /// the one-row placeholder to paint over.
    pub fn set_math_rows(&mut self, rows: HashMap<String, usize>) {
        if self.surface.math_rows == rows {
            return;
        }
        self.surface.math_rows = rows;
        self.surface_changed();
    }

    /// Tell the model whether the frontend can paint a picture *inside* a line
    /// of text. When it can, an inline formula renders to one atom glyph the
    /// frontend draws its typeset picture over — see
    /// [`MathInfo`](wysiwyg::MathInfo) — and when it cannot (a terminal), to
    /// the code-styled TeX it always showed. Off until a frontend says
    /// otherwise, so a host that has not caught up sees what it saw.
    pub fn set_inline_pictures(&mut self, on: bool) {
        if self.surface.inline_pictures == on {
            return;
        }
        self.surface.inline_pictures = on;
        self.surface_changed();
    }

    /// Tell the model whether the frontend sets a raised glyph raised — a
    /// superscript drawn above the line and smaller, as a pixel-laid-out
    /// frontend does, rather than inline at body size, as a terminal must.
    ///
    /// What turns on it is a footnote's brackets. They are there to tell a
    /// reference from the prose it interrupts, and a raised label already
    /// says that by where it sits: on a surface that raises, `[^1]` draws as a
    /// bare raised `1`, and its note's marker as the same raised `1` rather
    /// than `[1]`. On one that cannot, the brackets are all that keeps
    /// `claim[1]` from reading as `claim1`, so they stay. Off until a frontend
    /// says otherwise, so a host that has not caught up sees what it saw.
    pub fn set_raised_superscripts(&mut self, on: bool) {
        if self.surface.raised_superscripts == on {
            return;
        }
        self.surface.raised_superscripts = on;
        self.surface_changed();
    }

    /// Name the regions the frontend edits itself rather than as their
    /// source: [`MATH_REGION`](wysiwyg::MATH_REGION) for formulas,
    /// [`FENCE_REGION`](wysiwyg::FENCE_REGION) with a language for that
    /// language's fenced code blocks (`fence:mermaid`), and
    /// [`DIRECTIVE_REGION`](wysiwyg::DIRECTIVE_REGION) with a name for that
    /// name's leaf directives (`directive:embed`). Each owned region is
    /// published in [`VisualMap::regions`](wysiwyg::VisualMap::regions), and
    /// its edits come back through [`Doc::edit`] on its content range.
    ///
    /// An owned formula on the caret's line stays a picture in the two hidden
    /// markup modes instead of revealing its TeX, and an owned fence is one
    /// placeholder row the frontend draws its own view over.
    /// [`MarkupMode::Full`] still reveals the caret's line, regions included.
    /// A leaf directive is a placeholder row whether it is owned or not, so
    /// owning one changes no rows: it only publishes the region, whose content
    /// is the directive's whole source. `directive:page-break` is never one.
    /// Replaces the whole set; an empty list owns nothing, the default.
    pub fn set_owned_regions<S: AsRef<str>>(&mut self, kinds: &[S]) {
        let owned: std::collections::BTreeSet<String> =
            kinds.iter().map(|k| k.as_ref().to_string()).collect();
        if self.surface.owned_regions == owned {
            return;
        }
        self.surface.owned_regions = owned;
        self.surface_changed();
    }

    /// A height or a capability lives outside a block's source bytes, so the
    /// content-keyed block cache would hand back the old rows on a hit. Drop
    /// it (and the splice layout it carries) so the next build re-renders
    /// every block against the new surface, and force that build by clearing
    /// the map key.
    fn surface_changed(&mut self) {
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

    /// The identity of the map presently in [`vmap`](Self::vmap) — what the last
    /// [`build_visual`](Self::build_visual) built it from, or the identity of an
    /// unbuilt map before the first one.
    ///
    /// This is *not* [`revision`](Self::revision). The revision says where the
    /// text is; this says where the map is, and the two part company the moment
    /// an edit lands, until something rebuilds. A frontend that keeps its own
    /// copy of the map — leaf-ratatui stashes core's before splicing filler rows
    /// under an oversized heading — compares this against the value it held when
    /// it took the copy, and learns whether `vmap` is still the map it stashed
    /// or one somebody else has since rebuilt. Restoring a copy over a newer
    /// map would paint a stale document; restoring nothing hands core's
    /// incremental rebuild a map it never built.
    ///
    /// "Somebody else" includes another document. The key names the `Doc`
    /// as well as the build, so a frontend that draws two documents through
    /// one stash — a host with several buffers, or one that opens the next
    /// document where the last one stood — never has the copy it took of one
    /// accepted by the other, however alike their builds are.
    pub fn visual_key(&self) -> VisualKey {
        VisualKey(self.identity, self.vmap_key.clone())
    }

    /// The map, built at most once per `(revision, wrap)`. `clamp_caret` still
    /// runs on every call: the caret moves without the document changing, and
    /// keeping it on a legal stop is this function's job either way.
    pub(super) fn build_map(&mut self, wrap: Option<usize>) {
        // Under `MarkupMode::Full` the map is a function of the caret's *line*
        // as well as the text, so the line joins the key: moving within a line
        // still reuses the map, and crossing into another one rebuilds it. In
        // every other mode `reveal_line` is `None` and the key is what it was,
        // so caret motion goes on costing nothing.
        let reveal = self.reveal_line();
        let key = (self.revision, wrap, reveal.clone());
        if self.vmap_key.as_ref() != Some(&key) {
            self.build_map_with(wrap, reveal);
            self.vmap_key = Some(key);
            // In a hidden mode the reveal line was decided from the *previous*
            // build's layout, whose spans are stale across an edit: the
            // keystroke that closes a new `$…$` on the caret's line asked "is
            // there math here?" of a layout that had none, and the formula
            // would snap to its picture under the caret until the next
            // motion. Ask again of the layout just built, and go once more if
            // the answer moved. Between edits the first answer is exact and
            // this is one comparison.
            let again = self.reveal_line();
            if again != self.vmap_key.as_ref().and_then(|k| k.2.clone()) {
                self.build_map_with(wrap, again.clone());
                self.vmap_key = Some((self.revision, wrap, again));
            }
        }
        self.clamp_caret();
    }

    /// One build of the map at `wrap` under `reveal`, incremental where it can
    /// be — the body of [`build_map`](Self::build_map), which decides whether
    /// to call it.
    fn build_map_with(&mut self, wrap: Option<usize>, reveal: Option<Reveal>) {
        {
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
                    let surface = &self.surface;
                    let editor = &mut self.editor;
                    wysiwyg::build_spliced(
                        prev,
                        source,
                        wrap,
                        preserve_soft,
                        &top,
                        dirty,
                        surface,
                        reveal.clone(),
                        cache,
                        |id| editor.subtree(NodeId(id)).unwrap_or_default(),
                    )
                }
                None => None,
            };
            self.vmap = spliced.unwrap_or_else(|| {
                let source = &self.source;
                let cache = &mut self.block_cache;
                let surface = &self.surface;
                let editor = &mut self.editor;
                wysiwyg::build_cached(
                    &top,
                    source,
                    wrap,
                    preserve_soft,
                    surface,
                    reveal,
                    cache,
                    |id| editor.subtree(NodeId(id)).unwrap_or_default(),
                )
            });
            // Acknowledge the dirty range so the next edit's range starts fresh.
            self.editor.clear_dirty();
        }
    }

    pub(super) fn nodes(&mut self) -> Vec<FlatNode> {
        self.editor.nodes().unwrap_or_default()
    }

    /// The document's top-level blocks for the incremental render. See
    /// [`wysiwyg::top_blocks`] for why this isn't simply `child_spans(None)`.
    pub(super) fn top_blocks(&mut self) -> Vec<QueryMatch> {
        wysiwyg::top_blocks(&mut self.editor)
    }

    pub fn format_name(&self) -> &'static str {
        // The wildcard is `Asciidoc`, which twig parses but leaf never opens a
        // document in (see `Doc::open`) — and any language `Format` learns
        // later, before this learns its name.
        match self.format {
            Format::Djot => "djot",
            Format::Markdown => "markdown",
            Format::Xml => "xml",
            Format::Html => "html",
            _ => "unknown",
        }
    }

    /// Whether this document's format offers *any* door in — `false` only for a
    /// wholly parse-only format (XML, AsciiDoc), where every gesture refuses and
    /// a frontend may as well open the file read-only.
    ///
    /// This is a much weaker claim than the name suggests, and driving per-button
    /// state from it is exactly the mistake to avoid: HTML answers `true` because
    /// it spells the inline marks with a tag pair (`<strong>`, `<em>`, `<code>`)
    /// while a heading, a quote, a list, a task box, a link and a code fence all
    /// remain unspellable there. Ask [`capabilities`](Self::capabilities) — or
    /// [`supports`](Self::supports) — per control.
    pub fn authorable(&self) -> bool {
        self.format.is_authorable()
    }

    /// Whether this document can spell `gesture`, which is twig's own answer
    /// rather than a copy of it: `Format::supports_with` reads the same
    /// `Syntax` table the `Editor` method consults before refusing, chosen by
    /// the very [`parse_extensions`] this document's editor reparses with — so
    /// what the toolbar offers and what the splice will accept are one table.
    ///
    /// It is a fact about the *document*, not about the caret. `true` does not
    /// promise the gesture succeeds where it is standing — a link over a table
    /// border still fails — only that it will not fail with
    /// `UnsupportedFormat`. Gray out on `false`; don't read `true` as "this
    /// will work here".
    pub fn supports(&self, gesture: Gesture) -> bool {
        self.format
            .supports_with(parse_extensions(self.format), gesture)
    }

    /// Every control's enabled state in one read — what a toolbar builds itself
    /// from when a document opens or its format changes. See [`Capabilities`].
    pub fn capabilities(&self) -> Capabilities {
        Capabilities::of(self.format)
    }

    /// Refuse a gesture this document's format cannot spell, saying so in the
    /// status line. `true` means the caller must return without calling twig.
    ///
    /// Most of these refusals duplicate one twig would make anyway, and they are
    /// made here regardless because a message naming the *document's* format
    /// reads better than one naming twig's internals. Two of them are not
    /// duplicates and are the reason this is a guard rather than an error
    /// translation:
    ///
    /// - The table family (see [`table_op`](Self::table_op)) consults no
    ///   `Syntax` table, so twig does not refuse it at all.
    /// - [`toggle`](Self::toggle) at a collapsed caret never reaches twig — it
    ///   arms a sticky mark for text not yet typed, which is a promise `insert`
    ///   could not keep.
    pub(super) fn refuse_unsupported(&mut self, what: &str, gesture: Gesture) -> bool {
        self.refuse_unless(what, self.supports(gesture))
    }

    /// [`refuse_unsupported`](Self::refuse_unsupported) against a capability leaf
    /// answers itself — today only [`spells_pipe_tables`].
    pub(super) fn refuse_unless(&mut self, what: &str, supported: bool) -> bool {
        if supported {
            return false;
        }
        self.status = Some(format!("{what}: not supported in {}", self.format_name()));
        true
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

    /// The selection as a quote with a little of what surrounds it — the shape
    /// a host that cites, annotates, or searches for a passage wants, cut from
    /// the **source** rather than from anything rendered, so the quote is
    /// findable in the document again by plain string search.
    ///
    /// `context` is a count of characters (not bytes) on each side, clipped at
    /// the document's edges; the slices land on char boundaries by
    /// construction. `None` when nothing is selected.
    pub fn selection_quote(&self, context: usize) -> Option<Quote> {
        let (start, end) = self.selection()?;
        let mut before = start;
        for _ in 0..context {
            match self.source[..before].chars().next_back() {
                Some(c) => before -= c.len_utf8(),
                None => break,
            }
        }
        let mut after = end;
        for _ in 0..context {
            match self.source[after..].chars().next() {
                Some(c) => after += c.len_utf8(),
                None => break,
            }
        }
        Some(Quote {
            exact: self.source[start..end].to_string(),
            prefix: self.source[before..start].to_string(),
            suffix: self.source[end..after].to_string(),
            start,
            end,
        })
    }

    /// Words, characters, and paragraphs over the whole document — the numbers
    /// a status bar or an inspector puts next to a piece of writing.
    ///
    /// Counted over the text a **reader** sees, not the markup that spells it:
    /// `**bold**` is one word and four characters, a link is its label and not
    /// its destination, a block picture's `🖼 alt` placeholder is a picture and
    /// counts nothing, and leading frontmatter — which the WYSIWYG view does
    /// not render at all — is not writing. [`crate::counts`] states the rules
    /// in full; [`TextCounts`] states them per field.
    ///
    /// The same numbers in both views. They have to be: a word count that fell
    /// when you pressed ⌘E would be telling you the view had changed, which
    /// you knew already. So this reads neither [`Doc::view`] nor the map the
    /// frontend last built — it renders the source afresh, unwrapped, with
    /// soft breaks folded and no line revealed, and counts that. A narrower
    /// window, a different [`MarkupMode`], a different [`LineFlow`], and the
    /// source view all give the identical answer, because none of them is an
    /// input.
    ///
    /// That costs a reparse and an unwrapped layout — O(document), about 4 ms
    /// on a 45 KB file in release and 36 ms on half a megabyte. Fine on a
    /// settle and wrong in a paint loop, so a frontend should ask when the
    /// typing stops rather than once a keystroke. Caching it against
    /// [`revision`](Self::revision) is the obvious next move if that is ever
    /// not enough; nothing has needed it yet.
    pub fn counts(&self) -> TextCounts {
        self.count_over(None)
    }

    /// The same statistics over the selection alone — `None` when nothing is
    /// selected, since an empty selection is no selection.
    ///
    /// Same rules, over the same rendering, narrowed to the glyphs whose
    /// source byte falls inside [`selection`](Self::selection)'s range. A
    /// block the selection only clips still counts as one paragraph, and one
    /// it enters without catching a visible character counts as none — a
    /// selection that starts on a hidden `**` gains no paragraph from it.
    pub fn selection_counts(&self) -> Option<TextCounts> {
        let (start, end) = self.selection()?;
        Some(self.count_over(Some(start..end)))
    }

    /// The rendering both counters tally, and the tally itself.
    ///
    /// A fresh parse rather than `self.editor`, because these take `&self` and
    /// twig's arena is reached through `&mut`. A document that will not
    /// reparse is a "cannot happen" — the source came out of an editor that
    /// had already accepted it — and answers zero rather than panicking in
    /// what is very likely a paint path.
    fn count_over(&self, range: Option<Range<usize>>) -> TextCounts {
        let Ok(mut editor) = new_editor(self.source.as_bytes(), self.format) else {
            return TextCounts::default();
        };
        let Ok(nodes) = editor.nodes() else {
            return TextCounts::default();
        };
        // A surface that paints pictures in a line, so an inline formula is
        // an atom here and never its TeX: a formula is a picture to a reader
        // whichever way it is written, and the count says so consistently.
        let surface = wysiwyg::Surface {
            inline_pictures: true,
            ..Default::default()
        };
        let map = wysiwyg::build(&nodes, &self.source, None, false, &surface, None);
        counts::tally(&map, range)
    }

    /// Whether the document refuses to change — see the field.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Turn the read-only gate on or off. A frontend preference like
    /// [`set_markup_mode`](Self::set_markup_mode): nothing about the document
    /// itself changes, only what may be done to it from here on.
    pub fn set_read_only(&mut self, on: bool) {
        self.read_only = on;
    }

    /// The host-painted ranges, sorted by start — see [`Highlight`].
    pub fn highlights(&self) -> &[Highlight] {
        &self.highlights
    }

    /// Replace the host-painted ranges wholesale. The whole set each time,
    /// rather than add/remove verbs: the host owns the list (it derives it
    /// from its own state — annotations, search hits), and a replace can
    /// never leave the two disagreeing about what should be on screen.
    pub fn set_highlights(&mut self, mut highlights: Vec<Highlight>) {
        highlights.retain(|h| h.start < h.end);
        highlights.sort_by_key(|h| (h.start, h.end));
        self.highlights = highlights;
    }

    /// The highlight covering source `offset`, if one does — first by start
    /// when several overlap, which makes overlapping washes resolvable rather
    /// than undefined. What a frontend asks when the reader activates a spot.
    ///
    /// [`Highlight::covering`] is the whole of it: the frontends paint by
    /// asking the same question per glyph, against a slice they were handed
    /// rather than against a `Doc`, and one answer for both is what keeps a
    /// wash and an activation agreeing about which range a spot is in.
    pub fn highlight_at(&self, offset: usize) -> Option<&Highlight> {
        Highlight::covering(&self.highlights, offset)
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
}
