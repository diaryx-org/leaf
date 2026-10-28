//! What sits under the caret: a link's destination, the heading, a footnote,
//! an image, a code block's language.

use super::*;

impl Doc {
    /// The destination of the link under the caret — what a Link prompt shows so
    /// ⌘K on an existing link edits its URL instead of asking for it again.
    /// `None` when the caret stands in no link.
    ///
    /// An autolink carries no separate destination: its text *is* the URL, so
    /// that's what comes back for one.
    pub fn link_destination_at_caret(&mut self) -> Option<String> {
        self.link_destination_at(self.caret)
    }

    /// The destination of the link at `off`.
    /// [`link_destination_at_caret`](Self::link_destination_at_caret) for a place
    /// the caret isn't.
    ///
    /// The offset form exists for the same reason
    /// [`footnote_at`](Self::footnote_at)'s does: a frontend drawing a *piece* of
    /// the document somewhere else — a footnote's text in a popover, say — has
    /// rows and runs but no caret in them, and still needs to know which of those
    /// runs a reader can follow.
    pub fn link_destination_at(&mut self, off: usize) -> Option<String> {
        let link = self
            .nodes_at(off)
            .into_iter()
            .filter(|n| matches!(n.kind.as_str(), "link" | "url" | "email"))
            .filter(|n| n.span.start <= off && off < n.span.end)
            .max_by_key(|n| n.span.start)?;
        let n = self.node(link.node_id)?;
        n.destination.or(n.text)
    }

    /// The nodes whose spans reach `off`, root first: twig's walk from the
    /// root down to `off` and to the byte before it, which reads no more of
    /// the tree than those paths, where a marshal of every node read the
    /// whole document for each question a toolbar asks after a keystroke.
    /// The byte before is for a span that ends at `off`: the caret at the end
    /// of a heading's or a quote's last line. A footnote's definition lies
    /// outside the root's tree, where the walk finds only the document:
    /// there, every node whose span reaches `off`.
    pub(super) fn nodes_at(&mut self, off: usize) -> Vec<QueryMatch> {
        let mut out = self.editor.ancestors_at(off).unwrap_or_default();
        if off > 0 {
            for m in self.editor.ancestors_at(off - 1).unwrap_or_default() {
                if !out.iter().any(|o| o.node_id == m.node_id) {
                    out.push(m);
                }
            }
        }
        if out.iter().all(|m| m.kind == Kind::Doc) {
            return self
                .nodes()
                .into_iter()
                .filter(|n| n.span.start <= off && off <= n.span.end)
                .map(|n| QueryMatch {
                    node_id: n.id.0,
                    span: n.span,
                    content_span: n.content_span,
                    kind: n.kind,
                })
                .collect();
        }
        out
    }

    /// The node `id` names, with its payload (a heading's level, a link's
    /// destination), read alone.
    pub(super) fn node(&mut self, id: u32) -> Option<FlatNode> {
        self.editor.subtree(NodeId(id)).ok()?.into_iter().next()
    }

    /// The heading the caret is under — the nearest heading at or above it,
    /// whatever its level. See [`heading_at`](Self::heading_at).
    pub fn heading_at_caret(&mut self) -> Option<Heading> {
        self.heading_at(self.caret)
    }

    /// The heading `off` is under: the last heading that starts at or before
    /// it, or the one `off` stands in. `None` above the first heading.
    ///
    /// The question between [`link_destination_at`](Self::link_destination_at)
    /// ("which link is this") and [`locate`](Self::locate) ("where does this
    /// fragment land"): a host writing a link *to* a place needs the heading
    /// that place is under, to name it by `#slug`. Nearest, not enclosing —
    /// a level-3 heading under a level-2 is the answer for the text below the
    /// level-3, because it is the finer place to land.
    pub fn heading_at(&mut self, off: usize) -> Option<Heading> {
        let nodes = self.nodes();
        let h = nodes
            .iter()
            .filter(|n| n.kind == Kind::Heading && n.span.start <= off)
            .max_by_key(|n| n.span.start)?;
        let mut text = String::new();
        inline_text(&nodes, h.first_child, &mut text);
        Some(Heading {
            text: text.trim().to_string(),
            level: h.level.unwrap_or(1),
            span: h.span.clone(),
        })
    }

    /// Where the locator `id` lands in this document — the `#v2` half of a
    /// `chapter.dj#v2`, resolved to the block it names. `None` when nothing here
    /// answers to it.
    ///
    /// The other end of a link, and the reason this exists: without it a
    /// destination has only file granularity, so following a citation into a
    /// chapter drops the reader at the top of it to hunt for the verse. Which is
    /// also why it is a *document* query rather than a caret one — the document
    /// being asked is usually not the one the reader is in.
    ///
    /// Three readings, tried in order, because the same `#some-heading` is
    /// written three ways across the formats leaf opens:
    ///
    /// 1. **A declared id**, exactly as written: djot's `{#v1}` on a block, and
    ///    the auto-ids djot mints for its headings. The only exact answer, so it
    ///    goes first — a document that says `{#v1}` has settled the question.
    /// 2. **A declared id, slugged.** djot spells a heading's auto-id
    ///    `Some-Heading-Here`; nearly every tool that *writes* a link to one
    ///    spells it `#some-heading-here`. Comparing slugs is what lets a link
    ///    authored anywhere land on a djot heading.
    /// 3. **A heading's text, slugged.** Markdown has no ids at all — twig mints
    ///    none and `{#custom}` is literal text in a Markdown heading — so for
    ///    the format most vaults are written in, the heading's own words are the
    ///    only thing a fragment can name. This is the rule every Markdown
    ///    renderer already follows, which is what makes `#a-heading` mean in
    ///    diaryx what it means on the web.
    ///
    /// Ties go to the earliest match, then to the widest: a duplicated id is the
    /// document's mistake and the first one is the answer every anchor
    /// implementation gives, while preferring the wider span picks the section
    /// over the heading that opens it — more for a peek to show, same place to
    /// land.
    pub fn locate(&mut self, id: &str) -> Option<Landing> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        let nodes = self.nodes();

        // Earliest wins, then widest. `Reverse` on the end because `min_by_key`
        // is picking, among nodes that start together, the one that ends last.
        let pick = |matches: &mut dyn Iterator<Item = &FlatNode>| {
            matches
                .min_by_key(|n| (n.span.start, std::cmp::Reverse(n.span.end)))
                .map(|n| Landing {
                    start: n.span.start,
                    end: n.span.end,
                })
        };

        if let Some(landing) = pick(&mut nodes.iter().filter(|n| declared_id(n) == Some(id))) {
            return Some(landing);
        }
        let want = slug(id);
        if want.is_empty() {
            return None;
        }
        if let Some(landing) = pick(
            &mut nodes
                .iter()
                .filter(|n| declared_id(n).map(slug).as_deref() == Some(&*want)),
        ) {
            return Some(landing);
        }

        // A heading by its words. Its span is one line, so the end comes from
        // where the *section* it opens gives out — the next heading that is not
        // under it, or the end of the document. A Markdown heading has no
        // section node to ask (twig only builds those for djot), and a peek that
        // showed the heading alone would answer "what does that say" with the
        // title of the thing it says.
        let heading = nodes
            .iter()
            .filter(|n| n.kind == Kind::Heading)
            .filter(|n| {
                n.content_span
                    .clone()
                    .and_then(|s| self.source.get(s))
                    .is_some_and(|text| slug(text) == want)
            })
            .min_by_key(|n| n.span.start)?;
        let level = heading.level.unwrap_or(u32::MAX);
        let end = nodes
            .iter()
            .filter(|n| n.kind == Kind::Heading)
            .filter(|n| n.span.start > heading.span.start)
            .filter(|n| n.level.unwrap_or(u32::MAX) <= level)
            .map(|n| n.span.start)
            .min()
            .unwrap_or(self.source.len());
        Some(Landing {
            start: heading.span.start,
            end,
        })
    }

    /// Write a footnote at the caret — the toolbar's Footnote button, and the
    /// one gesture in the footnote story that *authors* rather than follows.
    ///
    /// Both halves go in as one twig edit: the `[^1]` where the caret is, and
    /// the `[^1]:` definition at the end of the document. Half a footnote is not
    /// a footnote — a bare reference with nothing defining it renders as literal
    /// brackets — so a single button that wrote only the reference would leave
    /// the author to hand-spell the other half in a document that had just
    /// stopped showing them what the first half meant. One edit also means one
    /// undo takes both back.
    ///
    /// The definition's body is left empty and **the caret lands in it**, which
    /// is the whole point of pressing the button: nobody wants a reference to a
    /// note they have not written yet. Getting back to where they were writing
    /// is [`footnote_definition_at_caret`](Self::footnote_definition_at_caret) —
    /// the same return leg a reader following a reference already uses, so the
    /// author is left standing on the near end of a round trip that works.
    ///
    /// A selection collapses to its *end* rather than being replaced: a
    /// reference annotates the words before it, so "select the claim, add a
    /// footnote" should mark that claim, not consume it.
    pub fn insert_footnote(&mut self) {
        if self.read_only || self.refuse_unsupported("footnote", Gesture::InsertFootnote) {
            return;
        }
        let at = self.selection().map_or(self.caret, |(_, end)| end);
        self.anchor = None;
        self.caret = at;
        self.record_caret();
        let label = self.next_footnote_label();
        match self.editor.insert_footnote(at, &label) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                self.anchor = None;
                // `change.new` runs from the reference to the end of the
                // document, so its start is the `[^1]` just written and
                // `footnote_at` resolves it to the note the same way a reader's
                // tap does — and to the note's *body*, which is already a caret
                // stop even when it is empty (the `[^1]:` marker draws as `[1] `
                // and has none), so this needs no snap on top. The fallback is
                // the reference's own offset: a format that spelled the pair some
                // way leaf can't read back should still leave the caret on the
                // edit rather than at the far end of a document it just grew.
                self.caret = self
                    .footnote_at(change.new.start)
                    .and_then(|note| note.offset)
                    .unwrap_or(change.new.start);
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("footnote: {e}")),
        }
    }

    /// The label to give a footnote the author has not named: the lowest counting
    /// number no footnote in the document is already wearing.
    ///
    /// twig takes the label rather than minting one, because it holds no opinion
    /// about what a document's footnotes should be called — and it is right not
    /// to. Numbering them is what every author of a numbered note expects, and
    /// re-using a taken number would silently point the new reference at somebody
    /// else's note (twig reuses an existing definition rather than appending a
    /// second one, which is the right rule for citing a note twice on purpose and
    /// exactly the wrong accident to have by default).
    ///
    /// *References* are counted alongside definitions, not just definitions: a
    /// document carrying a dangling `[^2]` has a 2 that means something to
    /// whoever wrote it, and minting a definition for it here would answer a
    /// question nobody asked. Non-numeric labels (`[^why]`) are left out of the
    /// count entirely — they take no number, so they block none.
    fn next_footnote_label(&mut self) -> String {
        let mut taken: Vec<u32> = wysiwyg::footnote_definitions(&mut self.editor)
            .into_iter()
            .filter_map(|note| wysiwyg::footnote_label(&self.source, note.span.start))
            .filter_map(|label| label.parse().ok())
            .collect();
        taken.extend(
            self.nodes()
                .into_iter()
                .filter(|n| n.kind == Kind::FootnoteReference)
                .filter_map(|n| wysiwyg::footnote_reference_label(&self.source, n.span))
                .filter_map(|label| label.parse::<u32>().ok()),
        );
        (1..).find(|n| !taken.contains(n)).unwrap_or(1).to_string()
    }

    /// The footnote reference under the caret, resolved to the note it names.
    /// [`footnote_at`](Self::footnote_at) at the caret's offset.
    pub fn footnote_at_caret(&mut self) -> Option<FootnoteRef> {
        self.footnote_at(self.caret)
    }

    /// The footnote reference at `off`, resolved to the note it names — what a
    /// frontend shows when a reader activates a `[^1]`.
    ///
    /// A reference is not a link node, so
    /// [`link_destination_at_caret`](Self::link_destination_at_caret) does not
    /// (and should not) answer for one: a link names a destination to leave for,
    /// a reference names a note that is already in this document. Following one
    /// is a move within the page, which is why this hands back an `offset`
    /// rather than something to open.
    ///
    /// Offset-based rather than caret-only because the gesture that wants this
    /// most is the one that must not move the caret: a pointer hovering a `[1]`
    /// asks what note it names without disturbing where the reader was typing.
    /// The caret is just the offset a click already placed —
    /// [`footnote_at_caret`](Self::footnote_at_caret) passes it.
    ///
    /// `None` when `off` stands in no reference. A reference whose note the
    /// document never defines is *not* `None` — it answers with the label it
    /// looked for and no text, which is what lets a frontend say so instead of
    /// silently doing nothing.
    pub fn footnote_at(&mut self, off: usize) -> Option<FootnoteRef> {
        // Innermost-wins by latest start, the rule its link sibling uses.
        let span = self
            .nodes()
            .into_iter()
            .filter(|n| n.kind == Kind::FootnoteReference)
            .filter(|n| n.span.start <= off && off < n.span.end)
            .max_by_key(|n| n.span.start)?
            .span;
        let label = wysiwyg::footnote_reference_label(&self.source, span)?.to_string();

        // The note itself. Definitions are roots beside `doc` rather than
        // children of it, so they're asked for directly — see
        // `wysiwyg::footnote_definitions`.
        let note = wysiwyg::footnote_definitions(&mut self.editor)
            .into_iter()
            .find(|m| wysiwyg::footnote_label(&self.source, m.span.start) == Some(&label));
        let Some(note) = note else {
            return Some(FootnoteRef {
                label,
                text: None,
                offset: None,
                end: None,
            });
        };
        let body = wysiwyg::footnote_body_span(&self.source, note.span.clone());
        Some(FootnoteRef {
            label,
            text: body
                .clone()
                .and_then(|b| self.source.get(b))
                .map(str::to_string),
            // The body's start, not the definition's — see `FootnoteRef::offset`.
            offset: body.clone().map(|b| b.start),
            end: body.map(|b| b.end),
        })
    }

    /// The footnote *definition* the caret stands in, and where the reference
    /// that names it is. [`footnote_definition_at`](Self::footnote_definition_at)
    /// at the caret's offset.
    pub fn footnote_definition_at_caret(&mut self) -> Option<FootnoteDef> {
        self.footnote_definition_at(self.caret)
    }

    /// The footnote definition spanning `off`, and where the reference that
    /// names it is — the return leg of [`footnote_at`](Self::footnote_at).
    ///
    /// The mirror image, deliberately: the same gesture that takes a reader from
    /// `[1]` down to the note takes them from the note back up to `[1]`, so
    /// following a footnote is a round trip rather than a fall. It needs no
    /// memory of how the reader arrived — the document says where the reference
    /// is — which is what makes it work for a reader who scrolled to the notes
    /// themselves, and what keeps it right after an edit moves either end.
    ///
    /// `None` when `off` stands in no definition. A definition nothing cites is
    /// *not* `None`, for [`FootnoteRef`]'s reason in reverse: it answers with
    /// its label and no offset, so a frontend can say "nothing refers to this"
    /// rather than offer a jump that goes nowhere.
    pub fn footnote_definition_at(&mut self, off: usize) -> Option<FootnoteDef> {
        // Definitions are roots beside `doc`, so `nodes()` — which walks the
        // document body — never reports one. They're asked for directly, the way
        // `footnote_at` asks for the note it resolves to.
        //
        // Closed at the end, unlike the half-open test its neighbours use. A
        // definition's span stops at its last content byte — the newline ending
        // the line is outside it — so `span.end` is the caret stop at the end of
        // the note's own row, not the first byte of anything after. Excluding it
        // meant the one caret an author is guaranteed to have, the one left
        // sitting at the end of the note they just typed, was in no definition at
        // all: writing a note and then asking to go back to its reference
        // answered nothing. Two definitions in a row still can't both match —
        // there is a blank line between them — and `max_by_key` decides anyway.
        let note = wysiwyg::footnote_definitions(&mut self.editor)
            .into_iter()
            .filter(|m| m.span.start <= off && off <= m.span.end)
            .max_by_key(|m| m.span.start)?;
        let label = wysiwyg::footnote_label(&self.source, note.span.start)?.to_string();

        // The earliest reference carrying this label. `min` rather than a `find`,
        // because `nodes()` reports a flattened walk whose order is twig's
        // business, not document order. Bound first: the walk needs `&mut self`
        // and reading the labels back out needs `&self.source`.
        let nodes = self.nodes();
        let offset = nodes
            .into_iter()
            .filter(|n| n.kind == Kind::FootnoteReference)
            .filter(|n| {
                wysiwyg::footnote_reference_label(&self.source, n.span.clone()) == Some(&*label)
            })
            // Past the `[^`, onto the label — see `FootnoteDef::offset`.
            .map(|n| n.span.start + 2)
            .min();
        Some(FootnoteDef { label, offset })
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
            .filter(|n| n.kind == Kind::Image)
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
    /// is in — the prompt's confirm. A no-op when the caret is in no fenced
    /// block, and a reported error for a language the format's fence cannot
    /// carry.
    ///
    /// twig rewrites the info string, so the fence's own width — measured
    /// against a body neither side touches — is kept, and a language holding a
    /// space, a line end or the fence character is refused rather than written
    /// out to reparse as something else. Leaf used to splice over the info span
    /// itself and `trim()` the input, which handled the one bad case it had
    /// thought of.
    pub fn set_code_language(&mut self, lang: &str) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        if self.refuse_unsupported("code language", Gesture::SetCodeLanguage) {
            return;
        }
        if self.code_block_start_at_caret().is_none() {
            return;
        }
        let lang = lang.trim();
        // `None` clears the info string; `Some("")` asks for an empty one. Both
        // write a bare fence, and the prompt's empty value means "clear".
        let want = (!lang.is_empty()).then_some(lang);
        self.record_caret();
        match self.editor.set_code_language(self.caret, want) {
            Ok(_) => {
                self.last_edit_kind = None;
                self.refresh();
                self.anchor = None;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("code language: {e}")),
        }
    }

    /// The `span.start` of the code block covering the caret — the anchor
    /// [`wysiwyg::code_info_span`] reads the fence from. `None` when the caret is
    /// in none.
    pub(super) fn code_block_start_at_caret(&mut self) -> Option<usize> {
        let off = self.caret;
        self.nodes_at(off)
            .into_iter()
            .filter(|n| n.kind == Kind::CodeBlock && n.span.start <= off && off <= n.span.end)
            .max_by_key(|n| n.span.start)
            .map(|n| n.span.start)
    }

    /// The source range of the text inside the link covering `off` — what sits
    /// between its `[` and `]`. `None` when twig reports no link there.
    pub(super) fn link_text_span(&mut self, off: usize) -> Option<std::ops::Range<usize>> {
        self.nodes_at(off)
            .into_iter()
            // Two links can touch (`[a](x)[b](y)`), and then one's `span.end` is
            // the other's `span.start`; the link that starts latest at or before
            // `off` is the one `off` is actually in.
            .filter(|n| n.kind == Kind::Link && n.span.start <= off && off < n.span.end)
            .max_by_key(|n| n.span.start)
            .and_then(|n| n.content_span)
    }
}
