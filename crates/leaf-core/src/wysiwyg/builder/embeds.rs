//! Blocks that draw something other than text: media, display math, owned
//! regions and leaf directives.

use super::*;

impl Builder<'_> {
    /// Render a block-level image, video, or audio as one placeholder row: the
    /// `🖼 alt` / `🎬 alt` / `🔊 alt` label styled [`Role::Image`], every glyph
    /// mapped to the media's start offset and a caret stop there (they share the
    /// offset, so the stop table dedups them to a single home in front of it, as
    /// a rule's dashes do), and the row's end stop set past it so the caret can
    /// also rest after it. The row carries a [`MediaMark`] so [`media_spans`]
    /// publishes it as a [`MediaInfo`] a capable frontend replaces with the real
    /// picture or player; a plain surface paints the label as-is. `pf` is the
    /// block prefix (a list indent, a quote gutter) the row opens with, exactly
    /// as every other block honours it.
    pub(super) fn block_media(
        &mut self,
        img: usize,
        kind: MediaKind,
        wrapper: usize,
        pf: &[Glyph],
    ) {
        let node = &self.nodes[img];
        let start = node.span.start;
        let end = node.span.end;
        // An `image`'s URL is twig's `destination`; a `<video>`/`<audio>` is a
        // generic element, so its URL is the `src` attribute — and may be absent
        // entirely, the element naming its candidates in child `<source>`s.
        let destination = match kind {
            MediaKind::Image => node.destination.clone().unwrap_or_default(),
            MediaKind::Video | MediaKind::Audio => attr_of(node, "src").unwrap_or_default(),
        };
        let poster = match kind {
            MediaKind::Video => attr_of(node, "poster").unwrap_or_default(),
            MediaKind::Image | MediaKind::Audio => String::new(),
        };
        // The `<source>`s under the media element itself, not under `wrapper`: a
        // `<video>` is its own container, unlike an `<img>`, whose `<picture>`
        // alternatives are its *siblings* and so only reachable from the wrapper.
        let sources = match kind {
            MediaKind::Image => self.media_sources(wrapper),
            MediaKind::Video | MediaKind::Audio => self.media_sources(img),
        };
        let alt = self.image_alt(img);
        let sigil = kind.sigil();
        let label = if alt.is_empty() {
            // With no alt, name the file — but a `<video>` with neither `src` nor
            // alt has only its `<source>`s to be named by, so fall back to the
            // first candidate rather than labelling the row a bare sigil.
            let named = if destination.is_empty() {
                sources
                    .first()
                    .map(|s| s.srcset.as_str())
                    .unwrap_or_default()
            } else {
                &destination
            };
            format!("{sigil} {}", media_label(named))
        } else {
            format!("{sigil} {alt}")
        };
        let style = Style::default().role(Role::Image);
        let mut glyphs = pf.to_vec();
        for ch in label.chars() {
            glyphs.push(Glyph {
                ch,
                style,
                src: start,
                stop: true,
            });
        }
        // How many rows the frontend wants for this picture: the label row plus
        // the blank fillers below it. Absent (a GUI that lays images out in
        // pixels, an image that didn't resolve, or a plain surface) means the
        // bare one-row placeholder.
        let rows = self
            .surface
            .media_rows
            .get(&destination)
            .copied()
            .unwrap_or(1)
            .max(1);
        // End past the image so the caret has a stop after it: the last glyph's
        // offset is the image *start*, not its extent, so `push_row`'s
        // last-glyph rule would strand the end stop inside the markup.
        self.push_row_at(glyphs, end);
        if let Some(row) = self.rows.last_mut() {
            row.media = Some(MediaMark {
                kind,
                destination,
                sources,
                alt,
                poster,
                rows,
            });
        }
        // Reserve the picture's remaining height as blank `decoration` rows: drawn
        // (so the frontend has the vertical room to paint the raster over them),
        // but holding no caret and contributing no stops — vertical motion steps
        // over them and the caret's only homes stay the stop in front of the image
        // and the one just past it, both on the label row above. They anchor at the
        // image's end offset so a click on the picture's lower half lands after it,
        // the nearest caret home. Mirrors how a table's box-rule rows reserve space
        // without ever holding the caret.
        for _ in 1..rows {
            self.rows.push(VRow {
                glyphs: Vec::new(),
                end_src: end,
                decoration: true,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                align: None,
                line_height: None,
                hang: None,
                boundary: None,
                mark_ends: Vec::new(),
                math: Vec::new(),
                region: None,
                inline_regions: Vec::new(),
            });
        }
        self.last_off = end;
    }

    /// The `<picture>` alternatives inside block-image `wrapper`, in document
    /// order — every `<source>` element in its subtree. Empty when there's no
    /// `<picture>`. Each is a `<source>`'s `media` + `srcset`; core keeps them
    /// verbatim and picks none (see [`MediaSource`]). A `<source>` with no
    /// `srcset` is dropped (nothing to load); its `media` may be empty (an
    /// unconditional override), which a frontend treats as always-matching.
    ///
    /// It scans the wrapper's whole subtree (via the forward `first_child` /
    /// `next_sibling` links, the reliable ones) rather than the `<img>`'s parent,
    /// for two reasons. A `<picture>` reaches core in two shapes: twig promotes a
    /// block `<picture>` to an `element(picture)` wrapping `[source, img]`, but
    /// leaves an inline one's tags as raw siblings — `[raw "<picture>", source,
    /// img, raw "</picture>"]` — so the `<source>`s sit at different depths in
    /// the two. And the editor's flat arena leaves a promoted inline node's
    /// `parent` back-pointer dangling on a phantom root, so only the wrapper
    /// (known at the call site) is a trustworthy anchor. A block image is the
    /// sole visible content of its wrapper, so every `<source>` under it is its
    /// picture's.
    fn media_sources(&self, wrapper: usize) -> Vec<MediaSource> {
        let mut out = Vec::new();
        self.collect_sources(wrapper, &mut out);
        out
    }

    fn collect_sources(&self, id: usize, out: &mut Vec<MediaSource>) {
        for c in self.children(id) {
            let node = &self.nodes[c];
            if node.name.as_deref() == Some("source") {
                // `<picture>` spells its candidate `srcset`, `<video>`/`<audio>`
                // spell it `src`. Both mean "the URL to load", so they normalise
                // onto one field; `srcset` wins where (illegally) both appear.
                let url = attr_of(node, "srcset").or_else(|| attr_of(node, "src"));
                if let Some(srcset) = url {
                    out.push(MediaSource {
                        media: attr_of(node, "media").unwrap_or_default(),
                        srcset,
                        mime: attr_of(node, "type").unwrap_or_default(),
                    });
                }
            }
            self.collect_sources(c, out);
        }
    }

    /// The single block-level media `id`'s subtree resolves to, or `None`.
    ///
    /// A wrapper is a block picture when the only *visible* thing under it is one
    /// image: whitespace-only text and structure-only elements (a `<picture>`'s
    /// `<source>`, which declares an alternate but paints nothing) don't count,
    /// and the search descends through wrapping elements (`<picture>`, a linking
    /// `<a>`). This is what makes `<p><img></p>`, a bare `<img>`, and
    /// `<h1><picture>…<img></picture></h1>` all render as one framed picture.
    /// Any real text, or a second image, means it isn't image-only — it falls
    /// back to inline rendering, where the image still shows as its alt text.
    ///
    /// [`FlatNode`]'s snapshot doesn't carry an element's tag name, so a
    /// `<source>` can't be skipped by name — but it needs no special case:
    /// contributing no image and no text, it's simply invisible to the scan.
    pub(super) fn media_only(&self, id: usize) -> Option<(usize, MediaKind)> {
        let mut found = None;
        let mut count = 0usize;
        let mut has_text = false;
        self.scan_visual(id, &mut found, &mut count, &mut has_text);
        (count == 1 && !has_text).then(|| found.unwrap())
    }

    /// Walk `id`'s subtree tallying visible leaves for [`media_only`]: each
    /// image, `<video>`, or `<audio>` (remembering the last, counting the total)
    /// and whether any non-whitespace text appears. Media isn't descended into —
    /// an image's inline children are alt text, and a `<video>`'s are its
    /// no-support fallback and its `<source>` declarations, none of which is
    /// document content.
    ///
    /// [`media_only`]: Self::media_only
    fn scan_visual(
        &self,
        id: usize,
        found: &mut Option<(usize, MediaKind)>,
        count: &mut usize,
        has_text: &mut bool,
    ) {
        for c in self.children(id) {
            let node = &self.nodes[c];
            match node.kind.as_str() {
                "image" => {
                    *found = Some((c, MediaKind::Image));
                    *count += 1;
                }
                // A `<video>`/`<audio>` reaches core as a generic `container`
                // (twig gives neither a semantic node, so `html_elements`
                // promotion leaves the tag name on `name`). Counted as media and
                // *not* descended into, so its `<source>` children and its
                // "your browser does not support…" fallback text neither add a
                // second count nor make the block look like text.
                "container" if matches!(element_tag(node), Some("video") | Some("audio")) => {
                    let kind = match element_tag(node) {
                        Some("audio") => MediaKind::Audio,
                        _ => MediaKind::Video,
                    };
                    *found = Some((c, kind));
                    *count += 1;
                }
                // Text leaves: only non-whitespace counts as visible content.
                // (Twig keeps the whitespace `str`s between HTML tags — the
                // newlines and indentation inside a `<picture>` — as real nodes.)
                "str" | "smart_punctuation" | "verbatim" | "inline_math" | "display_math" => {
                    if node.text.as_deref().is_some_and(|t| !t.trim().is_empty()) {
                        *has_text = true;
                    }
                }
                // Structural breaks carry no visible glyph of their own.
                "soft_break" | "hard_break" | "non_breaking_space" => {}
                // Any other wrapper (emphasis, a link, a `<picture>`) is
                // transparent to the scan — descend into it.
                _ => self.scan_visual(c, found, count, has_text),
            }
        }
    }

    /// The single `display_math` that is all of `id`'s visible content, or
    /// `None` — [`media_only`](Self::media_only) for a formula. Whitespace-only
    /// text around it does not count (twig keeps the newlines either side of a
    /// `$$` on its own lines as `str`s); any other text, or a second formula,
    /// means the paragraph is prose with math in it, and falls through to the
    /// inline path.
    pub(super) fn math_only(&self, id: usize) -> Option<usize> {
        let mut found = None;
        let mut count = 0usize;
        for c in self.children(id) {
            let node = &self.nodes[c];
            match node.kind.as_str() {
                "display_math" => {
                    found = Some(c);
                    count += 1;
                }
                "str" | "smart_punctuation" => {
                    if node.text.as_deref().is_some_and(|t| !t.trim().is_empty()) {
                        return None;
                    }
                }
                "soft_break" | "hard_break" | "non_breaking_space" => {}
                _ => return None,
            }
        }
        (count == 1).then(|| found.unwrap())
    }

    /// A display formula on lines of its own, drawn on
    /// [`block_media`](Self::block_media)'s recipe: one placeholder row —
    /// `∑` and the TeX on one line, every glyph [`Role::Math`] at the
    /// formula's start and a caret stop there, the row ending past the
    /// formula so the caret can rest after it too — carrying a [`MathMark`]
    /// for [`math_spans`] to publish, and under it as many blank decoration
    /// rows as the frontend said the picture is tall
    /// ([`Surface::math_rows`]).
    ///
    /// On the caret's line the block is its source instead: the `$$`
    /// delimiters in [`Role::Delimiter`] and the TeX between them in
    /// [`Role::Code`], line for line, the way a fence draws — so a formula is
    /// edited where it stands and folds back to its picture when the caret
    /// leaves. That is the same rule an inline formula follows, and it is
    /// what makes a block-level formula need no equivalent of
    /// [`VisualMap::block_media_stop`]: both of the placeholder's caret homes
    /// are on the formula's own lines, and standing on either reveals it.
    pub(super) fn block_math(&mut self, math: usize, pf: &[Glyph], pc: &[Glyph]) {
        self.saw_math.set(true);
        let node = &self.nodes[math];
        let (start, end) = (node.span.start, node.span.end);
        let tex = node.text.clone().unwrap_or_default();
        if self.math_revealed(&node.span) {
            let mut glyphs = Vec::new();
            self.inline_verbatim(math, Style::default(), &mut glyphs, true);
            self.emit_wrapped(glyphs, start, pf, pc);
            self.last_off = end;
            return;
        }
        let style = Style::default().role(Role::Math);
        let mut glyphs = pf.to_vec();
        // One line of label: the TeX with its newlines folded, so the row
        // reads as one thing however the source laid it out.
        let label: String = tex.split_whitespace().collect::<Vec<_>>().join(" ");
        for ch in format!("{MATH_ATOM} {label}").chars() {
            glyphs.push(Glyph {
                ch,
                style,
                src: start,
                stop: true,
            });
        }
        let rows = self
            .surface
            .math_rows
            .get(&tex)
            .copied()
            .unwrap_or(1)
            .max(1);
        self.push_row_at(glyphs, end);
        if let Some(row) = self.rows.last_mut() {
            row.math = vec![MathMark {
                len: end - start,
                tex_at: tex_offset(self.source, start..end, &tex),
                tex,
                owned: self.surface.owns(MATH_REGION),
                display: true,
                glyph: None,
                rows,
            }];
        }
        for _ in 1..rows {
            self.rows.push(VRow {
                glyphs: Vec::new(),
                end_src: end,
                decoration: true,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                align: None,
                line_height: None,
                hang: None,
                boundary: None,
                mark_ends: Vec::new(),
                math: Vec::new(),
                region: None,
                inline_regions: Vec::new(),
            });
        }
        self.last_off = end;
    }

    /// The [`RegionMark`] for code block `id` when the frontend edits fences
    /// of its language itself (`fence:<lang>` in [`Surface::owned_regions`],
    /// the first word of the info string), or `None` to draw it as code.
    ///
    /// Also `None` where the content's bytes are not the code's text — a
    /// fence in a blockquote or a list item, whose lines carry the
    /// container's prefix — since a frontend that splices the content would
    /// write over the prefixes. Under [`MarkupMode::Full`](crate::MarkupMode)
    /// the fence on the caret's line is its source, as a formula is.
    pub(super) fn owned_fence(&self, id: usize) -> Option<RegionMark> {
        if self.surface.owned_regions.is_empty() {
            return None;
        }
        let node = &self.nodes[id];
        let lang = code_language(self.source, node.span.start)?;
        let kind = format!("{FENCE_REGION}{}", lang.split_whitespace().next()?);
        if !self.surface.owns(&kind)
            || self
                .reveal
                .as_ref()
                .is_some_and(|r| r.markup && r.meets(&node.span))
        {
            return None;
        }
        let content = fence_content(self.source, node.span.clone())?;
        if self.source.get(content.clone())? != node.text.as_deref().unwrap_or("") {
            return None;
        }
        Some(RegionMark {
            kind,
            len: node.span.len(),
            content_at: content.start - node.span.start,
            content_len: content.len(),
        })
    }

    /// A block the frontend edits itself as one placeholder row, on
    /// [`block_directive`](Self::block_directive)'s recipe: a `▤ kind` label,
    /// every glyph at the block's start with a caret stop there, and the row
    /// ending past the block so the caret can rest after it too. The frontend
    /// draws its own view of the block over the row.
    pub(super) fn block_region(&mut self, id: usize, mark: RegionMark, pf: &[Glyph]) {
        let node = &self.nodes[id];
        let (start, end) = (node.span.start, node.span.end);
        let shown = mark.kind.strip_prefix(FENCE_REGION).unwrap_or(&mark.kind);
        let style = Style::default().role(Role::Image);
        let mut glyphs = pf.to_vec();
        for ch in format!("{REGION_ATOM} {shown}").chars() {
            glyphs.push(Glyph {
                ch,
                style,
                src: start,
                stop: true,
            });
        }
        self.push_row_at(glyphs, end);
        if let Some(row) = self.rows.last_mut() {
            row.region = Some(mark);
        }
        self.last_off = end;
    }

    /// A leaf directive (`::name{…}`) as one placeholder row — the
    /// [`block_media`](Self::block_media) recipe, for the same reason: it is a
    /// block that renders as *a thing*, not as text, and the frontend paints
    /// whatever the host app's vocabulary makes of it.
    ///
    /// The row's glyphs are a `⧉ label` (or `⧉ name`) stand-in a plain surface
    /// paints as-is, every glyph anchored at the directive's start with a caret
    /// stop there, and the row ending past it so the caret can also rest after
    /// it. It carries a [`DirectiveMark`] for [`directive_spans`], and is marked
    /// [`directive`](VRow::directive) so a frontend already drawing the
    /// container form's panel frames this one identically for free.
    ///
    /// Before this, a leaf directive emitted no rows at all: it was invisible,
    /// held no caret, and vertical motion crossed a void where it stood.
    pub(super) fn block_directive(&mut self, id: usize, pf: &[Glyph]) {
        let node = &self.nodes[id];
        let (start, end) = (node.span.start, node.span.end);
        let (name, attrs) = leaf_directive_identity(node, self.source);
        let label = self.image_alt(id); // its `[label]` children, flattened
        let shown = if label.is_empty() { &name } else { &label };
        // How many rows the frontend drew the directive in: the label row plus
        // the blank fillers below it. Absent means the bare placeholder.
        let rows = self
            .surface
            .directive_rows
            .get(&DirectiveKey::new(&name, &label, &attrs))
            .copied()
            .unwrap_or(1)
            .max(1);
        let style = Style::default().role(Role::Image);
        let mut glyphs = pf.to_vec();
        for ch in format!("{DIRECTIVE_ATOM} {shown}").chars() {
            glyphs.push(Glyph {
                ch,
                style,
                src: start,
                stop: true,
            });
        }
        // End past the directive so the caret has a stop after it — the same
        // reason `block_media` anchors its row at the image's end.
        self.push_row_at(glyphs, end);
        // A directive of a name the frontend draws itself is a region too,
        // its whole source its content — what its editor reads and splices.
        let kind = format!("{DIRECTIVE_REGION}{name}");
        let region =
            (name != crate::doc::PAGE_BREAK && self.surface.owns(&kind)).then(|| RegionMark {
                kind,
                len: end - start,
                content_at: 0,
                content_len: end - start,
            });
        if let Some(row) = self.rows.last_mut() {
            row.directive = true;
            row.region = region;
            row.leaf_directive = Some(DirectiveMark {
                name,
                attrs,
                label,
                rows,
            });
        }
        // The rest of the host's drawing as blank `decoration` rows, holding no
        // caret — `block_media`'s fillers, anchored at the directive's end for
        // the same reason: a click on the drawing's lower part lands after it.
        for _ in 1..rows {
            self.rows.push(VRow {
                glyphs: Vec::new(),
                end_src: end,
                decoration: true,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                align: None,
                line_height: None,
                hang: None,
                boundary: None,
                mark_ends: Vec::new(),
                math: Vec::new(),
                region: None,
                inline_regions: Vec::new(),
            });
        }
        self.last_off = end;
    }
}
