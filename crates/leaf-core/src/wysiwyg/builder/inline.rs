//! Inline content: text, emphasis, links, code spans and the rest, with their
//! delimiters hidden or revealed.

use super::*;

impl Builder<'_> {
    /// An image's alt text: the flattened text of its inline descendants (an
    /// image's children *are* its alt content), empty when it has none. Also a
    /// leaf directive's `[label]`, which is the same shape — inline children
    /// standing for the block.
    pub(super) fn image_alt(&self, id: usize) -> String {
        let mut out = String::new();
        self.collect_text(id, &mut out);
        out
    }

    /// Append every descendant's `text` to `out`, in document order. Inline text
    /// (`str`) nodes are leaves, so a node never contributes both its own text and
    /// a child's — no double counting.
    fn collect_text(&self, id: usize, out: &mut String) {
        for c in self.children(id) {
            if let Some(t) = &self.nodes[c].text {
                out.push_str(t);
            }
            self.collect_text(c, out);
        }
    }

    pub(super) fn inline_children(&self, id: usize, base: Style) -> Vec<Glyph> {
        let mut out = Vec::new();
        for c in self.children(id) {
            self.inline(c, base, &mut out);
        }
        out
    }

    /// [`inline_children`](Self::inline_children) plus any trailing whitespace the
    /// block carries past its inline content (see [`trailing_ws_glyphs`]). Used
    /// for the leaf inline blocks — paragraphs and headings — whose own `span`
    /// bounds exactly one line of text, so the trailing gap is theirs. *Not* for
    /// a table cell, whose `span` is the whole row and would swallow the
    /// delimiters and neighbours between it and the row's end.
    ///
    /// [`trailing_ws_glyphs`]: Self::trailing_ws_glyphs
    pub(super) fn inline_children_with_trailing(&self, id: usize, base: Style) -> Vec<Glyph> {
        let mut out = self.inline_children(id, base);
        out.extend(self.trailing_ws_glyphs(id, base));
        out
    }

    /// Glyphs for whatever trailing whitespace a block's source carries past its
    /// last inline node — the space(s) at the end of `hello ` that Markdown and
    /// Djot drop from the `str` node as insignificant. twig still records them:
    /// a block's `content_span` ends at its last meaningful character while its
    /// `span` runs to the end of the line's text (before the terminating
    /// newline), so the gap between the two *is* that trailing whitespace.
    ///
    /// Emitting it as real caret-stop glyphs is what lets the caret be drawn
    /// past the last visible character. Without it, typing a space at the end of
    /// a paragraph moved the caret in the source but not on screen — the caret
    /// stuck on the last glyph until the next visible character reparsed the
    /// space into an interior `str` node that finally carried it.
    ///
    /// Restricted to spaces: only they are safe to synthesize one-cell-per-byte,
    /// and only they are what the parser silently strips. Anything else in the
    /// gap means the span accounting isn't what this assumes, so it's left alone.
    fn trailing_ws_glyphs(&self, id: usize, style: Style) -> Vec<Glyph> {
        let node = &self.nodes[id];
        let Some(content) = &node.content_span else {
            return Vec::new();
        };
        let (from, to) = (content.end, node.span.end);
        let Some(slice) = (from < to).then(|| self.source.get(from..to)).flatten() else {
            return Vec::new();
        };
        if slice.is_empty() || slice.bytes().any(|b| b != b' ') {
            return Vec::new();
        }
        slice
            .bytes()
            .enumerate()
            .map(|(i, _)| Glyph {
                ch: ' ',
                style,
                src: from + i,
                stop: true,
            })
            .collect()
    }

    fn inline(&self, id: usize, base: Style, out: &mut Vec<Glyph>) {
        let node = &self.nodes[id];
        match node.kind.as_str() {
            "str" | "smart_punctuation" => push_escaped_text(
                out,
                node.text.as_deref().unwrap_or(""),
                node.span.clone(),
                self.source,
                base,
            ),
            "soft_break" | "hard_break" | "non_breaking_space" => {
                // A break renders as a real, caret-navigable glyph — but twig
                // gives it no span of its own (`0..0`), so the offset comes from
                // the text in front of it: one *past* the last glyph, which is
                // the newline the break stands for. Past, not on: sharing the
                // previous glyph's offset would put two stops on one byte, and a
                // caret that can't change offset can't move.
                let src = if node.span.start != 0 {
                    node.span.start
                } else {
                    out.last().map(|g| g.src + g.ch.len_utf8()).unwrap_or(0)
                };
                // A *hard* break is a line break wherever it is written — a
                // `\` or two trailing spaces in prose, a `<br>` in a cell — so
                // it renders as `'\n'`, which ends the row. It drew as a space
                // in prose, folding a poem's lines into one paragraph. A soft
                // break normally folds into a space; under `LineFlow::Preserve`
                // it renders as a `'\n'` too, so the author's line break shows
                // where it was written. Never inside a cell (`break_glyph` is
                // `'\n'` there): a cell is one line and folds its own soft
                // breaks regardless.
                let preserved_soft = node.kind == Kind::SoftBreak
                    && self.preserve_soft
                    && self.break_glyph.get() == ' ';
                let ch = if node.kind == Kind::HardBreak || preserved_soft {
                    '\n'
                } else {
                    ' '
                };
                out.push(Glyph {
                    ch,
                    style: base,
                    src,
                    stop: true,
                });
            }
            // A cell's only spelling for an in-line break is a raw `<br>`; read it
            // back as one (outside a cell it stays the literal text it falls to
            // below). The tag's bytes carry no stop of their own — the line it
            // ends stops just before it, the next just after.
            "raw_inline" if self.break_glyph.get() == '\n' && is_br(node.text.as_deref()) => {
                out.push(Glyph {
                    ch: '\n',
                    style: base,
                    src: node.span.start,
                    stop: true,
                });
            }
            "emph" => self.inline_delimited(id, base.italic(), out),
            "strong" => self.inline_delimited(id, base.bold(), out),
            // A coloured highlight's emoji is spelling, not content: twig strips
            // it and records the colour on the node, so the glyphs are the
            // author's words and the colour rides the role. Revealed markup
            // still shows the emoji, because `delims` reads the source bytes
            // between the span and the content span — which is exactly the
            // `==🔴 ` the author typed.
            "mark" => {
                let color = MarkColor::from_attrs(&node.attrs);
                self.inline_delimited(id, base.role(Role::Mark(color)), out)
            }
            "insert" => self.inline_delimited(id, base.underline(), out),
            "delete" => self.inline_delimited(id, base.strikethrough(), out),
            // The one pair whose whole meaning is *where the glyphs sit*. Drawn
            // in the surrounding style otherwise, so `^**2**^` stays bold and a
            // superscript inside a heading keeps the heading's role — which is
            // exactly why this is a `Baseline` and not a `Role`.
            "superscript" => self.inline_delimited(id, base.baseline(Baseline::Super), out),
            "subscript" => self.inline_delimited(id, base.baseline(Baseline::Sub), out),
            "verbatim" => self.inline_verbatim(id, base, out, self.revealed(&node.span)),
            // A formula in a line. Three renderings, in order of preference:
            //
            // - On the caret's line it is its TeX in the code style with its
            //   delimiters shown — in *every* markup mode, because the
            //   content is not the picture and hiding the `$` alone would
            //   leave nothing to edit. `math_revealed` is `revealed` without
            //   the mode gate.
            // - On a surface that paints pictures in a line it is one atom
            //   glyph, `stop: true` at the formula's start, standing for the
            //   whole thing; the [`MathMark`] drained onto the row by
            //   [`take_math`](Self::take_math) says what the frontend draws
            //   there. The caret has the stop on the atom and the next glyph's
            //   past it, and nothing inside the markup.
            // - Elsewhere — a terminal — it is the code-styled TeX with the
            //   delimiters hidden, exactly the verbatim treatment, and what
            //   `inline_math` rendered as before there was anything else.
            //   `display_math` had no arm at all and fell to the default one,
            //   which pushed its text at the *node's* start, three bytes short
            //   of where the text sits.
            "inline_math" | "display_math" => {
                self.saw_math.set(true);
                if self.math_revealed(&node.span) {
                    self.inline_verbatim(id, base, out, true);
                } else if self.surface.inline_pictures {
                    let tex = node.text.clone().unwrap_or_default();
                    self.pending_math.borrow_mut().push((
                        node.span.start,
                        MathMark {
                            len: node.span.len(),
                            tex_at: tex_offset(self.source, node.span.clone(), &tex),
                            tex,
                            owned: self.surface.owns(MATH_REGION),
                            display: node.kind == Kind::DisplayMath,
                            glyph: None,
                            rows: 1,
                        },
                    ));
                    out.push(Glyph {
                        ch: MATH_ATOM,
                        style: base.role(Role::Math),
                        src: node.span.start,
                        stop: true,
                    });
                } else {
                    self.inline_verbatim(id, base, out, false);
                }
            }
            // An attributed span — the run-level half of the presentation
            // vocabulary. djot's `[text]{…}`, AsciiDoc's `[.a]#text#`, HTML's
            // and Markdown's `<span …>`: one node with a name twig hands back
            // for two of the four (see [`is_run_span`]), all four carrying the
            // author's `data-size`, `data-font` and `data-color` on the run
            // they cover.
            //
            // The keys are written over the surrounding style rather than
            // replacing it, so a span inside a block that names its own size
            // wins on size and keeps the block's face — the nearest-wins rule
            // the block walker applies through a `div`. A key the span does not
            // name is one the block still says.
            //
            // A `data-color` here is the text's *foreground*, where the same key
            // on a `mark` is a highlight's background: same vocabulary, same
            // enum, and no collision, because a `mark` is a `mark` and a span is
            // a span.
            //
            // Otherwise this is the plain `recurse` an anonymous container has
            // always had — no delimiters, because the `{…}` is markup and the
            // span's text is the author's words.
            "container" if is_run_span(node) && !self.children(id).is_empty() => {
                self.recurse(id, run_style(node, base, &self.faces), out)
            }
            // A text directive of a name the frontend draws and edits itself
            // (`directive:<name>` in its owned regions), on a surface that paints
            // pictures in a line: one atom glyph standing for the whole
            // directive, `stop: true` at its start — an inline formula's
            // recipe — and an [`InlineRegionMark`] drained onto the row, so the
            // frontend finds it in [`VisualMap::regions`]. Under
            // [`MarkupMode::Full`](crate::MarkupMode) the caret's line shows it
            // as its source, as a fence on it is. Every other frontend gets the
            // arms below, unchanged.
            "container"
                if container_is_directive(node)
                    && node.directive_form == Some(DirectiveForm::Text)
                    && self.surface.inline_pictures
                    && !self.revealed(&node.span)
                    && self.surface.owns(&format!(
                        "{DIRECTIVE_REGION}{}",
                        node.name.as_deref().unwrap_or_default()
                    )) =>
            {
                let name = node.name.clone().unwrap_or_default();
                let label = self.image_alt(id);
                let attrs = node.attrs.clone();
                self.pending_regions.borrow_mut().push((
                    node.span.start,
                    InlineRegionMark {
                        kind: format!("{DIRECTIVE_REGION}{name}"),
                        glyph: 0,
                        len: node.span.len(),
                        directive: DirectiveMark {
                            name,
                            attrs,
                            label,
                            rows: 1,
                        },
                    },
                ));
                out.push(Glyph {
                    ch: REGION_ATOM,
                    style: base.role(Role::Image),
                    src: node.span.start,
                    stop: true,
                });
            }
            // A text directive (`:name[label]{…}`) — the inline form of a generic
            // directive. Its `[label]` children are the visible text; the name and
            // the `{…}` attributes are the host app's vocabulary (diaryx's
            // `:vis[…]`) and stay hidden markup, exactly as a link's `](dest)` is.
            // Drawn in the surrounding style: a role of its own would need one
            // every frontend maps, and the bug this fixes is that the text was
            // invisible, not that it was unstyled.
            "container" if container_is_directive(node) && !self.children(id).is_empty() => {
                self.recurse(id, base, out)
            }
            // No `[label]`, so there are no children to render and recursing
            // emitted *nothing*: the directive's bytes vanished from the document
            // and left no caret stop behind. What to draw instead turns on
            // whether the syntax looks deliberate.
            //
            // Bare `:word` almost never is. twig matches a colon followed by any
            // letter-led word (`scanTextDirective`, deliberately matching remark),
            // so ordinary prose is full of them — `:see below`, a `:smile:`
            // shortcode, a stray colon before a word. Those are prose, and prose
            // renders as itself: every byte visible, every byte a caret stop, so a
            // colon typed by accident can be seen and deleted. Hiding them behind
            // a placeholder would be the invisible-and-unreachable failure this
            // arm exists to fix, just wearing a nicer glyph.
            "container" if container_is_directive(node) && node.attrs.is_empty() => {
                let span = node.span.clone();
                push_text(
                    out,
                    self.source.get(span.clone()).unwrap_or(""),
                    span.start,
                    base,
                );
            }
            // `{…}` attributes, though, are unmistakably deliberate — nobody
            // types `:vis{.family}` by accident, and diaryx writes exactly that
            // inline. So an attribute-bearing directive with no label draws as a
            // chip on `block_directive`'s recipe (`⧉ name attrs`, `Role::Image`),
            // the inline peer of the leaf form's placeholder row.
            //
            // Only the first glyph is a caret stop, and the whole chip shares the
            // directive's start offset: the caret treats it as one atomic thing
            // rather than walking hidden markup a byte at a time, and a paragraph
            // holding nothing but a chip still has a stop to be navigated to.
            "container" if container_is_directive(node) => {
                let start = node.span.start;
                let name = node.name.clone().unwrap_or_default();
                let shown = match directive_attr_label(&node.attrs) {
                    Some(attrs) if !name.is_empty() => format!("⧉ {name} {attrs}"),
                    Some(attrs) => format!("⧉ {attrs}"),
                    None => format!("⧉ {name}"),
                };
                let style = base.role(Role::Image);
                for (i, ch) in shown.chars().enumerate() {
                    out.push(Glyph {
                        ch,
                        style,
                        src: start,
                        stop: i == 0,
                    });
                }
            }
            // A footnote reference (`[^1]`). The label bracketed is what a reader
            // needs — bare, `note1` reads as a typo rather than a reference — so
            // the `^` is hidden as the spelling artefact it is (a link's
            // `](dest)` goes the same way) and the brackets are kept as
            // decoration: one shared offset, never a caret stop, like a table's
            // borders, so the caret walks the label alone.
            //
            // Styled `Role::Link`: a reference *is* a link to its definition, and
            // every frontend already paints that role. A role of its own would
            // need one in each of them, and what a frontend needs to tell the two
            // apart is not a paint colour but an answer to "what does clicking
            // here do" — which is [`Doc::footnote_at_caret`]'s job, not a glyph's.
            //
            // Raised, though, because that a reference is *set* differently from
            // the prose it interrupts is exactly what makes it read as a
            // reference. `[1]` at body size reads as bracketed text.
            //
            // And on a surface that does raise it
            // ([`Surface::raised_superscripts`]), the brackets go: the raise
            // is already what sets the label apart from the prose, which was
            // their whole job, and a bracketed superscript is an editor's
            // tell, not a book's. Where every glyph sits on the one baseline —
            // a terminal — they stay, since there `claim1` is a typo.
            "footnote_reference" => {
                let style = base.role(Role::Link);
                // Revealed, the reference is just its source bytes: the `^` that
                // is normally elided comes back and every byte becomes a real
                // stop, so the brackets stop being decoration and start being
                // text. That's the whole point of the mode, and it replaces the
                // hand-built chip below rather than decorating it — including the
                // raised baseline, since what's on screen there is source, and
                // source is set as prose.
                if self.revealed(&node.span) {
                    self.push_delim(out, &node.span, style);
                    return;
                }
                let style = style.baseline(Baseline::Super);
                // The label's own span, so its glyphs map to their true bytes.
                // Absent one, it starts past the `[^` that opens the reference.
                let (label, at) = match &node.content_span {
                    Some(c) => (self.source.get(c.clone()).unwrap_or(""), c.start),
                    None => (node.text.as_deref().unwrap_or(""), node.span.start + 2),
                };
                // A label has to be unique in its document, so a chapter whose
                // every verse has an `a` note spells them `20a`, `21a`. Inside
                // verse 20 the `20` is what the line already opens with, and a
                // reader is told it twice; drawn, the reference is the letter.
                // The number stays in the source and in the note's own marker,
                // where nothing else says which verse the note is for.
                let shown = self
                    .opening_number(id)
                    .and_then(|n| label.strip_prefix(n))
                    .filter(|rest| !rest.is_empty() && rest.chars().all(char::is_alphabetic))
                    .unwrap_or(label);
                let bracketed = !self.surface.raised_superscripts;
                if bracketed {
                    out.push(Glyph {
                        ch: '[',
                        style,
                        src: node.span.start,
                        stop: false,
                    });
                }
                push_text(out, shown, at + (label.len() - shown.len()), style);
                if bracketed {
                    out.push(Glyph {
                        ch: ']',
                        style,
                        src: node.span.end.saturating_sub(1),
                        stop: false,
                    });
                }
            }
            "link" | "url" | "email" => {
                let style = base.role(Role::Link);
                if self.children(id).is_empty() {
                    // A bare autolink (`<a@b.c>`, a naked URL): the destination
                    // *is* the visible text, so there is nothing elided to
                    // reveal and both modes draw the same thing.
                    push_text(
                        out,
                        node.destination
                            .as_deref()
                            .or(node.text.as_deref())
                            .unwrap_or("link"),
                        node.span.start,
                        style,
                    );
                } else {
                    // An inline link reveals asymmetrically — `[` before the
                    // label, `](dest)` after it — which the generic
                    // span-minus-content derivation already produces.
                    self.inline_delimited(id, style, out);
                }
            }
            _ => {
                if self.children(id).is_empty() {
                    if let Some(t) = &node.text {
                        push_text(out, t, node.span.start, base);
                    }
                } else {
                    self.recurse(id, base, out);
                }
            }
        }
    }

    pub(super) fn recurse(&self, id: usize, style: Style, out: &mut Vec<Glyph>) {
        for c in self.children(id) {
            self.inline(c, style, out);
        }
    }
}
