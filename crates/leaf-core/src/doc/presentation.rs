//! The presentation vocabulary: alignment, spacing, font size and face, text
//! color, and the attributes that carry them.

use super::*;

impl Doc {
    // ── the presentation vocabulary ─────────────────────────────────────────
    //
    // Six gestures and five queries over twig's two attribute ops. Each gesture
    // edits **one key and keeps the rest**: it reads the node's attributes,
    // removes its own key (and, for alignment, its own tokens out of `class`),
    // adds the new value or nothing, and passes the list back whole — twig's
    // contract is replace-not-merge, so the read is the caller's job. A
    // paragraph that came in as `class="lead center" id="intro"
    // data-line-height="1.5"` and is right-aligned goes out as `class="lead
    // right" id="intro" data-line-height="1.5"`. Nothing leaf did not write is
    // touched, which is what lets a document from elsewhere pass through the
    // editor unharmed.
    //
    // Clearing is the same gesture with `None`: the key goes, and an empty list
    // at the end unwraps the span or the Markdown div, which twig does.

    /// Set — or with `None` clear — the alignment of the block the caret is in.
    ///
    /// A block property, so the gesture is `set_block_attrs` on the caret's
    /// block **whatever is selected**: a line is a block's, and "centre this"
    /// with three words selected means the paragraph, not the words. The
    /// vocabulary is [`Align`], written as `class` tokens; other tokens on the
    /// same `class` are kept.
    ///
    /// In Markdown the attributes live on a `<div>` around the block — twig has
    /// no paragraph attribute syntax to write — and this reads them back off
    /// that div when the block is its sole child, so a second press rewrites
    /// the div rather than nesting a second one.
    pub fn set_alignment(&mut self, align: Option<Align>) {
        let attrs = self.block_attrs_at_caret();
        if align.is_none()
            && self.refuse_clear_from_div("alignment", &attrs, |a| Align::from_attrs(a).is_some())
        {
            return;
        }
        let attrs = with_class_token(
            &attrs,
            |t| Align::from_token(t).is_some(),
            align.map(Align::name),
        );
        self.write_block_attrs("alignment", attrs);
    }

    /// Set — or with `None` clear — the line spacing of the block the caret is
    /// in. [`set_alignment`](Self::set_alignment)'s peer in every respect but
    /// the key: [`LineHeight`] under `data-line-height`, one of the menu's
    /// three names or an exact ratio, written in its canonical spelling.
    pub fn set_line_spacing(&mut self, spacing: Option<LineHeight>) {
        let attrs = self.block_attrs_at_caret();
        if spacing.is_none()
            && self.refuse_clear_from_div("line spacing", &attrs, |a| {
                LineHeight::from_attrs(a).is_some()
            })
        {
            return;
        }
        let spelling = spacing.map(LineHeight::name);
        let attrs = with_attr(&attrs, "data-line-height", spelling.as_deref());
        self.write_block_attrs("line spacing", attrs);
    }

    /// Set — or with `None` clear — the size of the selected run, or of the
    /// caret's whole block when nothing is selected.
    ///
    /// Size, face and colour are the *run's*, and the block's when no run is
    /// chosen. With a selection the gesture is `wrap_range_attrs`, which wraps
    /// the range in an attributed span or re-styles the span it already lies in
    /// (never nesting a second, and unwrapping it when the last key goes). With
    /// no selection it is `set_block_attrs` on the caret's block, so that "make
    /// this paragraph larger" is a click with the caret in it rather than a
    /// select-all first.
    ///
    /// The walker reads the key at both levels with the nearer winning, so a
    /// span's `data-size` inside a block carrying its own applies to the span.
    ///
    /// The vocabulary is [`FontSize`]: one of CSS's seven keywords, which is
    /// what a menu offers first because a step reads as a step up under every
    /// theme, or the point size an author asked for, which is exact and is all
    /// it is. Either is written in its canonical spelling, so a size set twice
    /// from the same field writes the same bytes both times.
    pub fn set_font_size(&mut self, size: Option<FontSize>) {
        let spelling = size.map(FontSize::name);
        self.set_run_attr("size", "data-size", spelling.as_deref());
    }

    /// Set — or with `None` clear — the face of the selected run, or of the
    /// caret's whole block. [`set_font_size`](Self::set_font_size)'s peer, with
    /// [`FontFace`] under `data-font` — one of the four generics, or the family
    /// the author named, which the frontends resolve through the platform's
    /// font registry and fall back to the body face without.
    pub fn set_font_family(&mut self, font: Option<FontFace>) {
        let spelling = font.as_ref().map(FontFace::name);
        self.set_run_attr("font", "data-font", spelling.as_deref());
    }

    /// Set — or with `None` clear — the *text* colour of the selected run, or of
    /// the caret's whole block. [`set_font_size`](Self::set_font_size)'s peer,
    /// with [`TextColor`] under `data-color` — one of the seven names, whose
    /// two inks the theme owns, or the triple the author picked, which is
    /// painted as written in both appearances.
    ///
    /// The same key and the same seven names [`set_mark_color`](Self::set_mark_color)
    /// writes, and a different thing: that one colours a highlight's
    /// *background* and rides the `mark` node twig owns the spelling of, this
    /// one colours the letters and rides an attributed span. The two never
    /// collide, because a `mark` is a `mark` and a span is a span — and they
    /// share a vocabulary on purpose, so that a frontend with a red for a
    /// highlight has a red for text and both are *that* red.
    pub fn set_text_color(&mut self, color: Option<TextColor>) {
        let spelling = color.map(TextColor::name);
        self.set_run_attr("text colour", "data-color", spelling.as_deref());
    }

    /// Insert a page break at the caret — `::page-break`, a leaf directive with
    /// no label and no attributes, which twig spells in every format that names
    /// a leaf container (Markdown under the `directives` extension
    /// [`parse_extensions`] turns on, and djot, where it is an empty `:::
    /// page-break` fence), and HTML and AsciiDoc in spellings of their own that
    /// the walker reads back for this one name.
    ///
    /// [`insert_directive`](Self::insert_directive) with [`PAGE_BREAK`], gated
    /// on [`Capabilities::page_break`] rather than
    /// [`Capabilities::directives`], since the walker reads HTML's and
    /// AsciiDoc's page break back and no other name of theirs.
    ///
    /// The frontends that paginate read the row's
    /// [`DirectiveMark`](crate::wysiwyg::DirectiveMark) and open a page there;
    /// the ones that do not draw the `⧉ page-break` placeholder every leaf
    /// directive gets.
    pub fn insert_page_break(&mut self) {
        let supported = self.capabilities().page_break;
        self.write_directive("page break", supported, PAGE_BREAK, None, &[]);
    }

    /// Insert the leaf directive `name` at the caret — `::name[label]{attrs}`
    /// in Markdown, an empty `::: name` fence in djot — for a host whose
    /// vocabulary it is. leaf draws it as the `⧉` placeholder row and publishes
    /// it in [`VisualMap::directives`](crate::wysiwyg::VisualMap::directives),
    /// where a host that knows the name paints the real thing.
    ///
    /// Placed exactly as [`insert_thematic_break`](Self::insert_thematic_break)
    /// places a rule, and for the same reason: a directive is a block, so twig
    /// alone has nowhere to put one mid-paragraph and lands it after the
    /// caret's whole block. A bare paragraph is therefore parted at the caret
    /// first and the directive aimed at the *first* half, and the caret lands
    /// on a line under it. A selection is replaced by it. See that method for
    /// the whole of the rule, including why a code block, a list item, a table
    /// and a setext heading are left unsplit.
    ///
    /// `attrs` are `(key, value)` pairs in the order they are written, a `None`
    /// value a bare attribute — the shape
    /// [`DirectiveMark::attrs`](crate::wysiwyg::DirectiveMark::attrs) reads
    /// back.
    ///
    /// Refused, with a status and nothing written, where
    /// [`Capabilities::directives`] is `false`, and for a `name` or `label`
    /// twig will not write: a name is an ASCII letter followed by letters,
    /// digits, `-` and `_`, and a label may hold no line end and no square
    /// bracket. twig is the judge of both, asked before anything is parted.
    ///
    /// djot needs two things of its own, since its spelling is a fence under
    /// an attribute line. A bare attribute is written `key=""`: twig writes
    /// `{wide}`, which djot does not read as an attribute at all, and
    /// Markdown reads its own `{wide}` back as `wide=""` regardless. And a
    /// label is refused, because djot has nowhere to put one: twig writes it
    /// as the fence's body, which reads back as a container holding a
    /// paragraph and not as a directive.
    pub fn insert_directive(
        &mut self,
        name: &str,
        label: Option<&str>,
        attrs: &[(String, Option<String>)],
    ) {
        let supported = self.capabilities().directives;
        if self.format != Format::Djot {
            self.write_directive("directive", supported, name, label, attrs);
            return;
        }
        if label.is_some_and(|l| !l.is_empty()) {
            if !self.read_only {
                self.status = Some("directive: djot has no label for a leaf directive".into());
            }
            return;
        }
        let attrs: Attrs = attrs
            .iter()
            .map(|(k, v)| (k.clone(), Some(v.clone().unwrap_or_default())))
            .collect();
        self.write_directive("directive", supported, name, None, &attrs);
    }

    /// The gesture behind [`insert_directive`](Self::insert_directive) and
    /// [`insert_page_break`](Self::insert_page_break), which differ in the
    /// word a refusal uses and in the capability that gates them.
    fn write_directive(
        &mut self,
        what: &str,
        supported: bool,
        name: &str,
        label: Option<&str>,
        attrs: &[(String, Option<String>)],
    ) {
        if self.read_only || self.refuse_unless(what, supported) {
            return;
        }
        let attrs: Vec<(&str, Option<&str>)> = attrs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_deref()))
            .collect();
        // Ask twig first, on an empty document of the same format, so that a
        // name or label it refuses is refused before the selection is cut or
        // the paragraph parted: the refusal writes nothing at all. An empty
        // document is a fine place for any directive the format spells, so
        // the only thing this can refuse is the name, the label or the
        // attributes.
        if let Err(e) = new_editor(b"", self.format)
            .map_err(|e| e.to_string())
            .and_then(|mut scratch| {
                scratch
                    .insert_directive(0, name, label, &attrs)
                    .map_err(|e| e.to_string())
            })
        {
            self.status = Some(format!("{what}: {e}"));
            return;
        }
        self.caret = self.skip_trailing_close_delims(self.caret);
        // A selection is replaced by the directive, as a rule replaces one.
        if let Some((s, e)) = self.selection() {
            self.splice_selection(s, e, "", EditKind::Other);
        }
        self.anchor = None;
        self.record_caret();
        let at = self.caret;
        // A failure here is not fatal: the directive still lands after the
        // block, which is what this call was trying to improve on.
        let parted = self.part_for_block(at);
        match self.editor.insert_directive(at, name, label, &attrs) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                if parted {
                    self.coalesce_last_undo();
                }
                self.anchor = None;
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
                self.caret_under_written_block(change.new, parted);
            }
            Err(e) => self.status = Some(format!("{what}: {e}")),
        }
    }

    /// The alignment in force at the caret, or `None` for the theme's default —
    /// which swatch of an alignment control is lit.
    ///
    /// Read off the nearest node that names one: the block the caret is in, and
    /// the `div`s around it after that. [`mark_color_at_caret`](Self::mark_color_at_caret)'s
    /// shape, one property along.
    pub fn alignment_at_caret(&mut self) -> Option<Align> {
        self.presentation_chain()
            .iter()
            .find_map(|attrs| Align::from_attrs(attrs))
    }

    /// The line spacing in force at the caret, or `None` for the theme's own.
    /// [`alignment_at_caret`](Self::alignment_at_caret)'s peer.
    pub fn line_spacing_at_caret(&mut self) -> Option<LineHeight> {
        self.presentation_chain()
            .iter()
            .find_map(|attrs| LineHeight::from_attrs(attrs))
    }

    /// The size in force at the caret, or `None` for the theme's own — the
    /// entry a size menu shows ticked.
    ///
    /// Run-level, so the chain starts one node deeper: the attributed span the
    /// caret stands in, then its block, then the `div`s around it. The nearest
    /// wins, which is the rule the walker draws by.
    ///
    /// A name or a value, whichever the nearest node wrote. A `data-size` the
    /// grammar does not cover — a `huge` from elsewhere — is not a size this
    /// can answer, so the answer is `None` and the menu ticks *Default*, the
    /// same thing it did before the vocabulary opened.
    pub fn font_size_at_caret(&mut self) -> Option<FontSize> {
        self.presentation_chain()
            .iter()
            .find_map(|attrs| FontSize::from_attrs(attrs))
    }

    /// The face in force at the caret, or `None` for the theme's body face.
    /// [`font_size_at_caret`](Self::font_size_at_caret)'s peer.
    pub fn font_family_at_caret(&mut self) -> Option<FontFace> {
        self.presentation_chain()
            .iter()
            .find_map(|attrs| FontFace::from_attrs(attrs))
    }

    /// The *text* colour in force at the caret, or `None` for the theme's.
    /// [`font_size_at_caret`](Self::font_size_at_caret)'s peer, and not
    /// [`mark_color_at_caret`](Self::mark_color_at_caret) — that one reads a
    /// highlight's background off a `mark`, and a `mark` is never in this chain.
    pub fn text_color_at_caret(&mut self) -> Option<TextColor> {
        self.presentation_chain()
            .iter()
            .find_map(|attrs| TextColor::from_attrs(attrs))
    }

    /// The selection-or-caret half of the three run-level gestures: a span over
    /// a real selection, the caret's block over none.
    fn set_run_attr(&mut self, what: &str, key: &str, value: Option<&str>) {
        match self.selection() {
            Some((start, end)) => {
                let attrs = with_attr(&self.run_attrs_over(start, end), key, value);
                self.write_run_attrs(what, start, end, attrs);
            }
            None => {
                let own = self.block_attrs_at_caret();
                if value.is_none()
                    && self.refuse_clear_from_div(what, &own, |a| a.iter().any(|(k, _)| k == key))
                {
                    return;
                }
                let attrs = with_attr(&own, key, value);
                self.write_block_attrs(what, attrs);
            }
        }
    }

    /// A clear this gesture cannot carry out, said out loud instead of written:
    /// the node it rewrites — the caret's block, or the `<div>` around it that
    /// [`block_attrs_at_caret`](Self::block_attrs_at_caret) folds to in Markdown
    /// — does not name the property at all, and a `div` further out does.
    ///
    /// Handing twig the block's attributes with the key already absent changes
    /// no byte, and the query goes on answering `Some` off the div: the menu
    /// entry the author pressed stays unticked, and nothing says why. Twig's
    /// `set_block_attrs` reaches one node, so leaf cannot clear a key it did not
    /// write on a node it is not rewriting — the honest answer is the status
    /// line, in the voice the other refusals use.
    ///
    /// `names` is the property's own reading of an attribute list, because
    /// alignment lives in a `class` token rather than a key of its own. Spans
    /// are skipped: one inside the block is not what a *block* gesture writes
    /// either, but neither is it "the div around the block", and the run-level
    /// gestures reach it through a selection.
    fn refuse_clear_from_div(
        &mut self,
        what: &str,
        own: &Attrs,
        names: impl Fn(&Attrs) -> bool,
    ) -> bool {
        if names(own) {
            return false;
        }
        let caret = self.caret.min(self.source.len());
        if !self
            .attr_chain_at(caret)
            .iter()
            .any(|(span, attrs)| !span && names(attrs))
        {
            return false;
        }
        self.status = Some(format!("{what}: set on the div around the block"));
        true
    }

    /// Hand `attrs` to twig as the caret's block's whole attribute set, with the
    /// status, undo and caret plumbing [`set_mark_color`](Self::set_mark_color)
    /// has.
    ///
    /// **The caret keeps its place in the text, not its byte offset.** How a
    /// format spells a block's attributes is markup written *around* the block
    /// — djot's `{…}` line above it, a `<div …>` and two blank lines in front of
    /// it in Markdown, a longer opening tag in HTML — and every one of those
    /// grows or shrinks above the author's own bytes. Where twig's change
    /// rewrites the block whole (Markdown's div is spliced as one region, block
    /// included) the plain arithmetic of [`reanchor`] has nothing to shift by
    /// and parks the caret at the end of the splice, past the closing `</div>`:
    /// the caret is then in no block at all, so a second press of the same menu
    /// answers "no block at the caret" and the toolbar's queries read nothing.
    /// [`reanchor_in_block`] is what carries it across instead — the block's
    /// content span before and after, which is the one thing the respelling
    /// leaves alone.
    ///
    /// Read *before* the splice and applied *after* `refresh`, because both
    /// halves of that mapping are facts about a tree twig is between: the
    /// block's old bytes are gone once the edit lands, and its new ones are not
    /// in `self.source` until the refresh puts them there.
    pub(super) fn write_block_attrs(&mut self, what: &str, attrs: Attrs) {
        if self.read_only || self.refuse_unsupported(what, Gesture::SetBlockAttrs) {
            return;
        }
        // A blank line has no block to carry an attribute, and twig answers
        // `NotFound` there — say so in leaf's own words instead.
        let Some(at) = self.block_offset_for_caret() else {
            self.status = Some(format!("{what}: no block at the caret"));
            return;
        };
        self.record_caret();
        let pairs = attr_pairs(&attrs);
        let was = self.block_content_at(at);
        let text = was.clone().map(|s| self.source[s].to_string());
        match self.editor.set_block_attrs(at, &pairs) {
            Ok(change) => {
                let (caret, anchor) = (self.caret, self.anchor);
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                let now = self.block_content_in(&change.new, text.as_deref());
                // A block the two halves cannot both name — a code block, a
                // caret in a list's marker — takes the plain arithmetic, which
                // is what it had before.
                let block = was.as_ref().zip(now.as_ref());
                self.caret = reanchor_in_block(caret, &change, block);
                self.anchor = anchor.map(|a| reanchor_in_block(a, &change, block));
                self.dirty = self.source != self.clean_source;
                self.status = None;
                // The clamp reads the caret floor off the map, and this edit
                // can move the floor: taking the `{…}` line off a djot
                // document's first block moves the first rendered offset to 0,
                // and a floor read from the old map stood the caret past the
                // block's text. So the map is this revision's before the clamp
                // — see `open_paragraph_at_block_edge`.
                self.rebuild_map();
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("{what}: {e}")),
        }
    }

    /// The content span of the innermost paragraph or heading covering `off` —
    /// the author's own bytes, without the `# ` or the `<p>` that spells the
    /// block around them.
    ///
    /// The same two kinds [`block_attrs_at_caret`](Self::block_attrs_at_caret)
    /// reads, so that what a gesture re-anchors by is the block it wrote to.
    fn block_content_at(&mut self, off: usize) -> Option<Range<usize>> {
        self.nodes()
            .into_iter()
            .filter(|n| matches!(n.kind, Kind::Para | Kind::Heading))
            .filter(|n| n.span.start <= off && off <= n.span.end)
            .min_by_key(|n| n.span.end - n.span.start)
            .map(|n| n.content_span.unwrap_or(n.span))
    }

    /// [`block_content_at`](Self::block_content_at)'s other half: the content
    /// span of the block `region` holds now, found by the bytes it held before.
    ///
    /// Matched on the text rather than taken as the first block in the region,
    /// because a rewritten region is markup and all — `<div class="center">`
    /// carries words of its own — and because the block this gesture moved is
    /// the one whose content the respelling did not touch. `None` where the
    /// region holds no block at all, which is djot's every case: the `{…}` line
    /// is spliced above the block and the block itself never moves through the
    /// change at all, only past it.
    fn block_content_in(
        &mut self,
        region: &Range<usize>,
        text: Option<&str>,
    ) -> Option<Range<usize>> {
        let text = text?;
        let spans: Vec<Range<usize>> = self
            .nodes()
            .into_iter()
            .filter(|n| matches!(n.kind, Kind::Para | Kind::Heading))
            .filter(|n| region.start <= n.span.start && n.span.end <= region.end)
            .map(|n| n.content_span.unwrap_or(n.span))
            .collect();
        spans
            .into_iter()
            .find(|s| self.source.get(s.clone()) == Some(text))
    }

    /// Hand `attrs` to twig as the attribute set of the span over `[start,
    /// end)` — wrapping one, or re-styling the one the range already lies in,
    /// or unwrapping it when `attrs` is empty.
    ///
    /// What the splice leaves selected is the span's **content** — the author's
    /// words — and not the whole of `change.new`, which is markup and all:
    /// `[big]{data-size="large"}` in djot, `<span …>big</span>` in Markdown. A
    /// selection reaching past the node's own span lies in no span at all, so a
    /// second press of the menu would nest a fresh one instead of re-styling
    /// the one just written.
    fn write_run_attrs(&mut self, what: &str, start: usize, end: usize, attrs: Attrs) {
        if self.read_only || self.refuse_unsupported(what, Gesture::WrapRangeAttrs) {
            return;
        }
        self.record_caret();
        let pairs = attr_pairs(&attrs);
        match self.editor.wrap_range_attrs(start, end, &pairs) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                let content = self.span_content_in(&change.new);
                self.anchor = Some(content.start);
                self.caret = content.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("{what}: {e}")),
        }
    }

    /// The content range of the attributed span `spliced` now holds — the
    /// outermost one inside it, since that is the one just written — or
    /// `spliced` itself where the splice left no span, which is what an unwrap
    /// leaves behind.
    fn span_content_in(&mut self, spliced: &Range<usize>) -> Range<usize> {
        self.nodes()
            .into_iter()
            .filter(wysiwyg::is_run_span)
            .filter(|n| spliced.start <= n.span.start && n.span.end <= spliced.end)
            .max_by_key(|n| n.span.end - n.span.start)
            .and_then(|n| n.content_span)
            .unwrap_or_else(|| spliced.clone())
    }

    /// The attribute set `set_block_attrs` is about to **replace** at the caret
    /// — which is the block's own, except in Markdown, where twig writes a
    /// block's attributes onto a `<div>` around it and rewrites that div when
    /// the block is its sole child. Reading the paragraph there would hand back
    /// an empty list and quietly drop everything the div said.
    ///
    /// Empty when the caret is in no block at all, which is the same list a
    /// block carrying no attributes gives — and the right one either way, since
    /// the gesture then refuses on its own.
    fn block_attrs_at_caret(&mut self) -> Attrs {
        let Some(off) = self.block_offset_for_caret() else {
            return Vec::new();
        };
        let nodes = self.nodes();
        let Some(block) = nodes
            .iter()
            .filter(|n| matches!(n.kind, Kind::Para | Kind::Heading))
            .filter(|n| n.span.start <= off && off <= n.span.end)
            .min_by_key(|n| n.span.end - n.span.start)
        else {
            return Vec::new();
        };
        if self.format == Format::Markdown
            && let Some(parent) = block.parent.and_then(|p| nodes.iter().find(|n| n.id == p))
            && wysiwyg::element_tag(parent) == Some("div")
            && nodes.iter().filter(|n| n.parent == Some(parent.id)).count() == 1
        {
            return parent.attrs.clone();
        }
        block.attrs.clone()
    }

    /// The attribute set `wrap_range_attrs` is about to **replace** over
    /// `[start, end)` — the innermost attributed span the range lies inside,
    /// which twig re-styles rather than nesting a second one in. Empty when the
    /// range lies in no span, where the gesture mints a fresh one.
    fn run_attrs_over(&mut self, start: usize, end: usize) -> Attrs {
        self.nodes()
            .into_iter()
            .filter(wysiwyg::is_run_span)
            .filter(|n| n.span.start <= start && end <= n.span.end)
            .min_by_key(|n| n.span.end - n.span.start)
            .map(|n| n.attrs)
            .unwrap_or_default()
    }

    /// The attribute lists that bear on a presentation query, **nearest first**:
    /// the attributed spans the caret stands in (innermost first), then its
    /// block, then the `div`s around it. A `find_map` down this is the whole of
    /// each query, and the order is the rule the walker draws by.
    ///
    /// Read at the caret, and at the selection's *start* when the caret stands
    /// in no span there. [`write_run_attrs`](Self::write_run_attrs) leaves the
    /// caret one past the span it just wrote — `toggle`'s convention — so
    /// asking the menu which entry that press just ticked must not answer
    /// `None`. Exactly the reason [`mark_offset`](Self::mark_offset) tries both.
    fn presentation_chain(&mut self) -> Vec<Attrs> {
        let caret = self.caret.min(self.source.len());
        let mut chain = self.attr_chain_at(caret);
        if !chain.iter().any(|(span, _)| *span)
            && let Some((start, _)) = self.selection()
        {
            let alt = self.attr_chain_at(start);
            if alt.iter().any(|(span, _)| *span) {
                chain = alt;
            }
        }
        chain.into_iter().map(|(_, attrs)| attrs).collect()
    }

    /// [`presentation_chain`](Self::presentation_chain) at one offset — every
    /// node bearing the vocabulary that covers it, innermost first, each paired
    /// with whether it is an attributed span (which is what tells the caller
    /// its run-level answer came from a run).
    ///
    /// Sorted by span length, which *is* the nesting order: a span lies inside
    /// its block and a block inside its div, so shortest-first is
    /// nearest-first without a second tree walk.
    fn attr_chain_at(&mut self, off: usize) -> Vec<(bool, Attrs)> {
        let off = off.min(self.source.len());
        let mut hits: Vec<(usize, bool, Attrs)> = Vec::new();
        for n in self.nodes() {
            let span = wysiwyg::is_run_span(&n);
            let block = matches!(n.kind, Kind::Para | Kind::Heading);
            let div = wysiwyg::element_tag(&n) == Some("div");
            if !(span || block || div) {
                continue;
            }
            // A span is half-open, the way a mark is: the offset one past it is
            // the text after it. A block and a div claim their end too, so a
            // caret resting at the end of a line still reads its paragraph.
            let inside = if span {
                n.span.start <= off && off < n.span.end
            } else {
                n.span.start <= off && off <= n.span.end
            };
            if !inside {
                continue;
            }
            hits.push((n.span.end - n.span.start, span, n.attrs));
        }
        hits.sort_by_key(|(len, _, _)| *len);
        hits.into_iter()
            .map(|(_, span, attrs)| (span, attrs))
            .collect()
    }
}
