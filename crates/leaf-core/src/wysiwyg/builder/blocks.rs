//! Walking the top-level blocks: the blank rows and separators between them,
//! and the dispatch on each block's kind.

use super::*;

impl Builder<'_> {
    /// Render an explicit, ordered list of top-level blocks — [`Builder::blocks`]
    /// for a walk that isn't "the children of one node". The document's top level
    /// no longer is: a footnote definition is a root beside `doc`, not under it,
    /// and [`top_level`] merges it into this list by source position.
    ///
    /// The separator between blocks is spelled by the same
    /// [`Builder::emit_separators_before`] the incremental top-level walk in
    /// [`build_cached`] uses, so the two paths can't drift on how a boundary
    /// looks.
    ///
    /// Returns the class of the last block that drew anything — what the
    /// trailing blank lines close — or `None` when nothing did.
    pub(in crate::wysiwyg) fn top_blocks(
        &mut self,
        ids: &[usize],
        hidden_end: usize,
    ) -> Option<BlockClass> {
        let mut above: Option<BlockClass> = None;
        for &child in ids {
            let below = BlockClass::from_node_kind(&self.nodes[child].kind);
            let before_sep = self.rows.len();
            if let Some(above) = above {
                self.emit_separators_before(
                    self.nodes[child].span.start,
                    &[],
                    true,
                    Boundary { above, below },
                );
            } else {
                let from = self.leading_from(hidden_end);
                self.emit_leading_blank_lines(from, self.nodes[child].span.start, below);
            }
            if self.block_or_hidden(child, before_sep, &[], &[]) {
                above = Some(below);
            }
        }
        above
    }

    /// Emit the blank separator row(s) that sit between a block ending at the
    /// current `last_off` and the next block starting at `next_start`, wearing
    /// the continuation prefix `pc`. Shared by [`Builder::blocks`] and the
    /// incremental top-level walk so the two can't drift on how a boundary is
    /// spelled.
    ///
    /// The blank line(s) between two blocks are real caret stops, each needing
    /// its *own* source offset — one strictly past the previous block's content,
    /// else it collides with that block's last row and `pos_of_offset`
    /// (first-match-wins) would resolve the caret onto the wrong row, pinning
    /// downward motion there.
    ///
    /// One row *per* blank source line, not a single collapsed separator: an
    /// empty paragraph opened between two blocks (Enter in the gap,
    /// `…\n\n\n\n…`) must be a navigable empty row, not vanish — else the caret
    /// in it snaps onto the *next* block's start and Enter looks like it did
    /// nothing.
    pub(in crate::wysiwyg) fn emit_separators_before(
        &mut self,
        next_start: usize,
        pc: &[Glyph],
        synthetic: bool,
        boundary: Boundary,
    ) {
        let mut offs = self.blank_rows_between(self.last_off, next_start);
        // Under a `</div>` the first blank line is the one Markdown needs to
        // end the div, not a line the author opened. Preserve flow drew it as
        // one, and Backspace there took it and glued the block below onto the
        // closing tag, as raw HTML.
        if self.preserve_soft && offs.first().is_some_and(|&o| self.closes_div_above(o)) {
            offs.remove(0);
            if offs.is_empty() {
                return;
            }
        }
        if offs.is_empty() {
            if !synthetic {
                // A tight list item's own text sits directly above the sub-list
                // nested in it — no fabricated gap. The "breathe" row belongs
                // between free-standing blocks, not between an item and its
                // child list, which the source writes on the very next line. A
                // real blank source line (a loose list) still lands a gap below,
                // because `blank_rows_between` found it and we never reach here.
                return;
            }
            // A tight gap with no blank line (e.g. a heading directly above its
            // text): keep the one conventional separator row so blocks still
            // breathe, as they always have.
            offs.push(self.blank_line_offset(self.last_off, next_start));
        }
        let last = offs.len() - 1;
        for (k, end_src) in offs.into_iter().enumerate() {
            // Only the drawn-only rows carry the boundary: the navigable blank
            // lines between them (and every blank line under preserve-soft flow)
            // are somewhere text can go, not a gap between blocks, and a frontend
            // that shrank one would be shrinking a line the author is typing on.
            let drawn = !self.preserve_soft && (k == 0 || k == last);
            // The blank line a boundary is *drawn* with isn't a place text can
            // go. The first one closes the block above and the last one opens the
            // block below — with a single blank line, the usual case, doing both
            // at once. Typing on either just continues the paragraph it abuts,
            // since the blank line it would need to be a paragraph of its own is
            // the very line being typed on. So they're a gap, like a table's
            // border: drawn, clickable, never a caret's home.
            //
            // The lines *between* them are the real ones. That's what Enter
            // opens: it inserts a paragraph break (`\n\n`), which leaves a blank
            // line spare on each side and the caret on the navigable line
            // between them.
            //
            // Preserve flow is the exception: there a bare `\n` is a visible line
            // break the author edits directly, so a lone blank line *is* a caret
            // home — typing on it makes the soft break the mode exists to show,
            // and Enter at a line's end lands the caret on exactly this row. So no
            // separator is drawn-only; every blank line is navigable.
            self.rows.push(VRow {
                glyphs: pc.to_vec(),
                end_src,
                decoration: drawn,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                // A line the author can type on is a line of the block
                // around it, and keeps its spacing and alignment.
                align: (!drawn).then_some(self.presentation.align).flatten(),
                line_height: (!drawn).then_some(self.presentation.line_height).flatten(),
                hang: None,
                // With blank lines between them, the two drawn rows don't
                // meet: each borders an empty paragraph the caret can rest
                // in, and is labelled so. Spaced by the block beyond, the
                // gap over an empty line above a heading was a heading's
                // wide one, and shrank under the line when its first
                // character made it the paragraph it had been all along.
                boundary: drawn.then_some(Boundary {
                    above: if k > 0 && last > 1 {
                        BlockClass::Paragraph
                    } else {
                        boundary.above
                    },
                    below: if k < last && last > 1 {
                        BlockClass::Paragraph
                    } else {
                        boundary.below
                    },
                }),
                mark_ends: Vec::new(),
                math: Vec::new(),
                region: None,
                inline_regions: Vec::new(),
            });
        }
    }

    /// The blank lines above the first block that draws anything, from `from`
    /// — the first line past any hidden frontmatter or comment — to the line
    /// holding `next_start`. [`Builder::emit_trailing_blank_lines`] turned
    /// upside down: nothing above needs a gap, so every line is an empty
    /// paragraph but the last, which is the gap that opens the block below.
    ///
    /// One blank line is only that gap, and draws nothing, which keeps the
    /// usual line under frontmatter out of sight. Two are the empty paragraph
    /// Enter opens at the first block's start (`\n\nHello`), which drew
    /// nothing either: the caret stayed with the text and the text stayed put.
    /// Preserve flow draws every line, as it does between blocks, but the one
    /// under frontmatter, which is the frontmatter's and not the author's.
    pub(in crate::wysiwyg) fn emit_leading_blank_lines(
        &mut self,
        from: usize,
        next_start: usize,
        below: BlockClass,
    ) {
        let next_line_start = self.source[..next_start].rfind('\n').map_or(0, |p| p + 1);
        let mut offs = Vec::new();
        let mut start = from;
        while start < next_line_start {
            offs.push(start);
            match self.source[start..next_line_start].find('\n') {
                Some(k) => start += k + 1,
                None => break,
            }
        }
        if self.preserve_soft {
            if from > 0 && !offs.is_empty() {
                offs.remove(0);
            }
        } else if offs.len() < 2 {
            return;
        }
        let last = offs.len().saturating_sub(1);
        for (k, end_src) in offs.into_iter().enumerate() {
            let drawn = !self.preserve_soft && k == last;
            self.rows.push(VRow {
                glyphs: Vec::new(),
                end_src,
                decoration: drawn,
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
                boundary: drawn.then_some(Boundary {
                    above: BlockClass::Paragraph,
                    below,
                }),
                mark_ends: Vec::new(),
                math: Vec::new(),
                region: None,
                inline_regions: Vec::new(),
            });
        }
    }

    /// Whether the line above the one starting at `line` is a `</div>`.
    fn closes_div_above(&self, line: usize) -> bool {
        let Some(above) = self.source[..line].strip_suffix('\n') else {
            return false;
        };
        let above = above.strip_suffix('\r').unwrap_or(above);
        let start = above.rfind('\n').map_or(0, |p| p + 1);
        above[start..].trim() == "</div>"
    }

    /// The blank lines between a `<div>`'s last block and its closing tag,
    /// drawn inside the div, as the lines between two of its blocks are.
    ///
    /// One blank line is the one Markdown needs before `</div>`, and two are
    /// that and the gap, so neither draws (in Preserve flow, which has no
    /// gap, the second does). A third is the empty paragraph
    /// Enter opens at the end of the div's last block — the end of a line
    /// with a line height or an alignment on it — which drew nothing: the
    /// key looked dead, and each press left another line nobody could see.
    fn emit_div_trailing_lines(&mut self, id: usize, pc: &[Glyph]) {
        let Some(&last) = self.children(id).last() else {
            return;
        };
        if self.rows.is_empty() {
            return;
        }
        let end = self.nodes[id].span.end.min(self.source.len());
        let from = block_line(self.source, self.last_off).1;
        if from >= end {
            return;
        }
        let Some(close) = self.source[from..end].rfind("</div>") else {
            return;
        };
        let close_line = self.source[..from + close].rfind('\n').map_or(0, |p| p + 1);
        let mut offs = Vec::new();
        let mut start = from;
        while start < close_line {
            if !self.source[start..close_line]
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                return;
            }
            offs.push(start);
            match self.source[start..close_line].find('\n') {
                Some(k) => start += k + 1,
                None => break,
            }
        }
        // Preserve flow draws every blank line but the one `</div>` needs.
        if offs.len() < if self.preserve_soft { 2 } else { 3 } {
            return;
        }
        offs.pop();
        let above = BlockClass::from_node_kind(&self.nodes[last].kind);
        for (k, end_src) in offs.into_iter().enumerate() {
            let drawn = !self.preserve_soft && k == 0;
            self.rows.push(VRow {
                glyphs: pc.to_vec(),
                end_src,
                decoration: drawn,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                align: (!drawn).then_some(self.presentation.align).flatten(),
                line_height: (!drawn).then_some(self.presentation.line_height).flatten(),
                hang: None,
                boundary: drawn.then_some(Boundary {
                    above,
                    below: BlockClass::Paragraph,
                }),
                mark_ends: Vec::new(),
                math: Vec::new(),
                region: None,
                inline_regions: Vec::new(),
            });
        }
    }

    /// Where the lines above the first drawn block begin: past the hidden
    /// frontmatter, or past the line of the last hidden block the walk
    /// stepped over, whichever is later.
    pub(in crate::wysiwyg) fn leading_from(&self, hidden_end: usize) -> usize {
        if self.last_off == 0 {
            return hidden_end;
        }
        block_line(self.source, self.last_off).1.max(hidden_end)
    }

    /// One block, drawn under whatever presentation the containers around it
    /// impose.
    ///
    /// A container named `div` with `Element` origin is transparent already —
    /// its children draw as themselves — and it now also *contributes* its
    /// vocabulary keys to every block it holds. That is the reading side of
    /// twig's own rule for where a Markdown block's attributes live: there is
    /// no attribute syntax to put on the paragraph, so `set_block_attrs` writes
    /// a `<div …>` around it, and reading one back has to look through the div.
    /// `<div class="center">` around three paragraphs centres all three, which
    /// is what the author of that HTML meant, and around one is the sole-child
    /// shape twig writes.
    ///
    /// Saved and restored rather than pushed onto a stack, so a nested div
    /// reads its own keys over its parent's and the block *after* the div is
    /// unaffected.
    pub(in crate::wysiwyg) fn block(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        if element_tag(&self.nodes[id]) == Some("div") {
            let saved = self.presentation;
            self.presentation = saved.under(&self.nodes[id].attrs, &self.faces);
            self.block_kind(id, pf, pc);
            self.emit_div_trailing_lines(id, pc);
            self.presentation = saved;
            // Step the walk past the closing `</div>`, as the fenced-div arm
            // below anchors past its `:::`. The tag sits on a line of its own
            // after the last child and the blank line under it, and the rich
            // view draws nothing for it — so left where the last child ended,
            // the separator logic read the tag's line as a blank line between
            // the div and the block below, and drew a navigable empty row there
            // that the author never opened and Backspace could not close; and
            // at the end of the document the trailing count read it as an empty
            // paragraph the author had left. It is hidden markup the walk steps
            // over, which is what `stepped_over` records, so both counts start
            // past it.
            let end = self.nodes[id].span.end;
            self.last_off = self.last_off.max(end);
            self.stepped_over = self.stepped_over.max(end);
            return;
        }
        self.block_kind(id, pf, pc);
    }

    fn block_kind(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        let node = &self.nodes[id];
        match node.kind.as_str() {
            "doc" | "section" => self.blocks(id, pf, pc, false),
            "heading" => {
                // A heading whose only visible content is a single image — a
                // banner set in an `<h1>` (`<h1><picture><img></picture></h1>`),
                // or `# ![](banner.png)` — is a block picture, not text. Render
                // it as one; anything with real heading text falls through.
                if let Some((m, kind)) = self.media_only(id) {
                    self.block_media(m, kind, id, pf);
                    return;
                }
                let level = node.level.unwrap_or(1);
                // A `data-size` on a heading scales the *heading's* ramp, not
                // the body's — the role and the step compose rather than
                // compete, which is the same thing a colour does to a link.
                let pres = self.presentation.under(&node.attrs, &self.faces);
                let style = pres.over(heading_style(level));
                let mut glyphs = Vec::new();
                // On the revealed line the `# ` comes back as real, editable
                // text in front of the heading. Only the opening marker: a
                // closing `#`-run (`## title ##`) is covered by the same
                // `delims` pair, and a setext underline is excluded there for
                // being on another line entirely.
                if let Some((open, close)) =
                    self.revealed(&node.span).then(|| self.delims(id)).flatten()
                {
                    self.push_delim(&mut glyphs, &open, style);
                    glyphs.extend(self.inline_children_with_trailing(id, style));
                    self.push_delim(&mut glyphs, &close, style);
                } else {
                    glyphs = self.inline_children_with_trailing(id, style);
                }
                // An *empty* heading — `# ` with nothing typed after it, which is
                // what the toolbar's H1 leaves on a blank line — has no glyph for
                // its row to end on, so the fallback below is the row's whole
                // extent: its only caret stop, and the offset every row after it
                // is measured from. The block's start is the wrong answer for
                // both, because it sits *in front of* the `# ` the rich view
                // hides: the caret drew (and typed) before the hashes, and the
                // rows below inherited an offset short by the marker's length,
                // which put the caret on one of them the moment the heading grew
                // text. Its content's start is where the caret belongs.
                let home = heading_content_start(self.source, &node.span);
                let first = self.rows.len();
                self.emit_wrapped(glyphs, home, pf, pc);
                // Stamp the level on every row the heading just emitted — a
                // wrapped heading's continuation rows as much as its first, and
                // an empty one's single glyphless row, which is the whole point
                // (see [`VRow::heading`]).
                for row in &mut self.rows[first..] {
                    row.heading = Some(level.min(255) as u8);
                    row.align = pres.align;
                    row.line_height = pres.line_height;
                }
            }
            "block_quote" => {
                let (start, end) = (node.span.start, node.span.end);
                let gutter = synth("│ ", Role::QuoteGutter, start);
                let f = concat(pf, &gutter);
                let c = concat(pc, &gutter);
                // A childless quote — a bare `> ` on an otherwise blank line,
                // which is what the toolbar's Quote button leaves there — has no
                // inner block to carry the gutter or a caret home, so `blocks`
                // emitted *nothing at all*: the quote didn't merely draw
                // unstyled, it disappeared, and a document that was only `> `
                // rendered zero rows with the caret nowhere to stand. Emit the
                // gutter row itself, ending just past the marker, exactly as an
                // empty `list_item` emits its bare bullet.
                if self.children(id).is_empty() {
                    self.push_row_at(f, end.min(self.source.len()));
                } else {
                    self.blocks(id, &f, &c, false);
                    self.emit_quote_trailing_lines(&c, end);
                }
            }
            // A generic `:::name{.class}` fenced-div container (twig's
            // `directive`, container form). Core is agnostic of `name` — it's
            // the host app's vocabulary (diaryx's `vis` for audience
            // visibility, say) and isn't available here regardless: twig only
            // threads an `element`'s tag name through `FlatNode::name`, not a
            // directive's own identifier. Every row gets marked `directive` (a
            // frontend draws a tinted panel around each maximal run, the
            // `code`/`code_block` recipe) and the first row carries a label —
            // the way a code fence's language rides only its first row.
            //
            // The label reads BOTH attribute conventions diaryx content
            // actually uses: twig's own dot-prefixed classes (`{.public
            // .family}`, one combined `class` attr) and bare pandoc-style
            // words with no leading dot (`{public family}` — the syntax
            // `diaryx_core::visibility`'s hand-rolled publish-time filter and
            // apps/web's directive serializer both write; twig parses each
            // bare word as its own attribute with an empty value, per
            // twig-markdown's `attributes.rs`). Reading only `.class`
            // would leave every *existing* diaryx `:::vis{...}` block
            // unlabeled.
            // Only the *container* form is the panel below. A `text` directive
            // is inline and never reaches the block walker (see `is_inline`); a
            // `leaf` one is a standalone block with no body, drawn as a
            // placeholder the way an image is.
            "container"
                if container_is_directive(node)
                    && node.directive_form == Some(DirectiveForm::Leaf) =>
            {
                self.block_directive(id, pf);
            }
            // HTML and AsciiDoc spell `insert_directive`'s page break in ways
            // of their own — `<page-break></page-break>`, an *element* with
            // no form at all, and `<<<`, a directive with none — so neither
            // meets the arm above. Both are named `page-break` and empty, and
            // both draw the same placeholder, for the same reason djot's
            // fence below does: a frontend that paginates on the mark must
            // not be able to tell which format the file is in. Narrow to the
            // one name: an arbitrary empty custom element is not a leaf
            // directive.
            "container"
                if node.name.as_deref() == Some(crate::doc::PAGE_BREAK)
                    && node.directive_form.is_none()
                    && node.origin.is_some()
                    && self.children(id).is_empty() =>
            {
                self.block_directive(id, pf);
            }
            // djot has no *leaf* directive form. `insert_directive` spells the
            // same document as an empty `::: page-break` fence — a container
            // with nothing in it — and the name comes back as the fence's one
            // class rather than as the node's name, because djot's div is
            // anonymous. Draw it as the placeholder Markdown's `::page-break`
            // gets, so a frontend that paginates on a `page-break`
            // [`DirectiveMark`] cannot tell which format the file is in.
            //
            // Narrow on purpose: only an *anonymous* empty fence. A Markdown
            // `:::note` with nothing in it keeps the reading it has, because
            // its name is its own and nothing about it says "a block with no
            // body" the way djot's spelling of a leaf directive does.
            "container"
                if container_is_directive(node)
                    && node.directive_form == Some(DirectiveForm::Container)
                    && node.name.as_deref().unwrap_or_default().is_empty()
                    && self.children(id).is_empty()
                    && !leaf_directive_identity(node, self.source).0.is_empty() =>
            {
                self.block_directive(id, pf);
            }
            "container" if container_is_directive(node) => {
                let label = directive_attr_label(&node.attrs);
                let start_row = self.rows.len();
                self.blocks(id, pf, pc, false);
                for (i, row) in self.rows[start_row..].iter_mut().enumerate() {
                    row.directive = true;
                    if i == 0 {
                        row.directive_label = label.clone();
                    }
                }
                // Anchor the block's end past its closing `:::` fence, exactly as
                // the code-block arm anchors past its ```` ``` ````. A container's
                // last content row ends at its last *child*, before the fence and
                // the blank line under it, so the separator logic counted the
                // fence line as a blank row of its own and drew a second boundary
                // — one gap's worth of margin twice, under every fenced div.
                self.last_off = node.span.end;
            }
            "bullet_list" | "ordered_list" | "task_list" => {
                let ordered = node.kind == Kind::OrderedList;
                // The marker's *style* follows the list's nesting depth, the
                // word-processor / HTML convention: `•` `◦` `▪` and `1.` `a.`
                // `i.`, then round again. Display only — the source keeps its
                // `-` and `1.`.
                let depth = self.list_depth(id);
                let kids = self.children(id);
                // Numbering starts where the source's first item says it does
                // (`5.` counts on from 5), which twig keeps only as the
                // marker's own text.
                let mut item_no = if ordered {
                    self.list_start(&kids).saturating_sub(1)
                } else {
                    0
                };
                for (i, child) in kids.iter().copied().enumerate() {
                    let kind = &self.nodes[child].kind;
                    if *kind == Kind::ListItem || *kind == Kind::TaskListItem {
                        let start = self.nodes[child].span.start;
                        item_no += 1;
                        // A task item's box replaces the bullet rather than
                        // joining it. The `[ ] ` that spells it is markup twig
                        // has already consumed — the item's paragraph *content*
                        // starts past it — so without a drawn box a task item
                        // was indistinguishable from a plain bullet, ticked or
                        // not. `☐`/`☑` is the marker for the same reason `•` is:
                        // it stands where the source's own marker stands. Which
                        // way it faces is `checked`, straight off the node.
                        let checked = self.nodes[child].checked;
                        let marker = match (checked, ordered) {
                            (Some(true), _) => "☑ ".to_string(),
                            (Some(false), _) => "☐ ".to_string(),
                            (None, true) => format!("{}. ", ordinal(item_no, depth)),
                            (None, false) => format!("{} ", bullet(depth)),
                        };
                        let bullet = synth(&marker, Role::ListMarker, start);
                        // The item's later rows wear the marker's own
                        // characters, drawn blank: the width a proportional
                        // face gives the marker, whatever the marker is.
                        let indent = synth(&marker, Role::ListIndent, start);
                        let first_row = self.rows.len();
                        self.block(child, &concat(pc, &bullet), &concat(pc, &indent));
                        // On the item's first row, the way `code_lang` rides the
                        // first row of its block.
                        if let (Some(c), Some(row)) = (checked, self.rows.get_mut(first_row)) {
                            row.task = Some(c);
                        }
                    } else {
                        // twig can nest a *following* top-level block as a direct
                        // child of the list rather than a sibling of it — e.g.
                        // `- item\n\n> quote` parses the block quote under the
                        // `bullet_list`. It isn't a list item, so render it de-nested:
                        // no bullet, at the list's own prefix, with the usual block
                        // separator — never `• │ quote`.
                        if i > 0 {
                            self.emit_separators_before(
                                self.nodes[child].span.start,
                                pc,
                                true,
                                Boundary {
                                    above: BlockClass::from_node_kind(
                                        &self.nodes[kids[i - 1]].kind,
                                    ),
                                    below: BlockClass::from_node_kind(&self.nodes[child].kind),
                                },
                            );
                        }
                        self.block(child, pc, pc);
                    }
                }
            }
            "list_item" | "task_list_item" => {
                // A childless item — the empty bullet you get the instant you
                // press Enter to open a new one — has no inner block to carry the
                // marker prefix or a caret home, so `blocks` would emit nothing
                // and the new bullet simply wouldn't appear until something was
                // typed into it. Emit the prefixed row itself, ending at a caret
                // stop just past the marker (the item's `span.end`), the way an
                // empty paragraph emits its one prefixed row via `emit_wrapped`.
                if self.children(id).is_empty() {
                    let home = self.nodes[id].span.end.min(self.source.len());
                    self.push_row_at(pf.to_vec(), home);
                } else {
                    // Tight: an item's text and the list nested under it butt
                    // together (`• a` / `  • b`), no fabricated blank row between —
                    // a loose item's real blank line still parts them.
                    self.blocks(id, pf, pc, true);
                }
            }
            // A footnote *definition* (`[^1]: the note`). It reaches this walker
            // only because [`top_level`] merges it back in — twig hangs it off no
            // parent at all, so a walk from `doc` never sees one and every byte
            // of its body used to render as nothing.
            //
            // Drawn as a hanging-indent item, the way a list item is: the marker
            // reads `[1] `, matching the `[1]` its references render as, so the
            // two can be paired by eye, and the body wraps under it. The marker
            // is synthetic decoration (one shared offset, never a caret stop) —
            // the `[^1]: ` that spells it in the source is markup, hidden like a
            // heading's `# `.
            //
            // On a surface that raises a superscript
            // ([`Surface::raised_superscripts`]) the marker is the label alone,
            // raised as its reference is, and a space: `¹ the note`, pairing
            // with the bare raised `¹` in the prose. The continuation indent
            // is raised the same, glyph for glyph — the indent is spelled with
            // the marker's characters so it is exactly the marker's width, and
            // a raised label is set smaller, so an indent set at body size
            // would hang the body further right than the marker it hangs under.
            "footnote" => {
                let (start, end) = (node.span.start, node.span.end);
                let source = self.source;
                let label = footnote_label(source, start).unwrap_or("");
                let (f, c) = if self.surface.raised_superscripts {
                    let marker = |role: Role| {
                        let mut g = synth(label, role, start);
                        for g in &mut g {
                            g.style = g.style.baseline(Baseline::Super);
                        }
                        g.extend(synth(" ", role, start));
                        g
                    };
                    (
                        concat(pf, &marker(Role::ListMarker)),
                        concat(pc, &marker(Role::ListIndent)),
                    )
                } else {
                    let marker = format!("[{label}] ");
                    (
                        concat(pf, &synth(&marker, Role::ListMarker, start)),
                        concat(pc, &synth(&marker, Role::ListIndent, start)),
                    )
                };
                if self.children(id).is_empty() {
                    // A definition with no body yet — the instant `[^1]: ` has
                    // been typed and nothing after it. `blocks` would emit
                    // nothing and the definition simply wouldn't appear, so emit
                    // the marker row itself with a caret home just past it,
                    // exactly as an empty list item does.
                    self.push_row_at(f, end.min(source.len()));
                } else {
                    self.blocks(id, &f, &c, false);
                }
            }
            // A link reference definition (`[foo]: /url`): resolved by label
            // into the links that use it, and drawn nowhere — the rich view has
            // no more use for its line than for a comment's. It is walked at all
            // (see [`top_level`]) so [`Builder::block_or_hidden`] can step the
            // walk past its bytes rather than count them as blank lines.
            "reference" => {}
            "table" => self.table(id, pf, pc),
            "line_block" => self.verse(id, pf, pc),
            "code_block" => {
                // A fence the frontend edits itself is one placeholder row.
                if let Some(mark) = self.owned_fence(id) {
                    self.block_region(id, mark, pf);
                    return;
                }
                let style = Style::default().role(Role::Code);
                let text = node.text.clone().unwrap_or_default();
                // Cut the block's *terminator*, not every trailing newline. A
                // block whose last line is empty spells that as a second `\n`,
                // and `trim_end_matches` ate it along with the terminator: the
                // Return that made the line got no row, so the caret placed on
                // it fell through to the paragraph below and typing landed
                // outside the block. twig's `content_span` is `text` less
                // exactly this one newline, so cutting one and no more is also
                // what keeps `code_line_offsets` lined up.
                let lines: Vec<&str> = text
                    .strip_suffix('\n')
                    .unwrap_or(text.as_str())
                    .split('\n')
                    .collect();
                // Each line at its own source offset, so the caret can walk the
                // code a character at a time like any other text. Where the
                // lines can't be lined up with the source there's no honest
                // offset to give, so the block maps coarsely to its start (and
                // stays a source-view job, as all of it once was).
                let offs = node
                    .content_span
                    .as_ref()
                    .and_then(|c| self.code_line_offsets(c, &lines));
                // The fence's info string, carried on the block's first row as
                // its language label (`None` for an indented block or a bare
                // fence). Kept on the row so it rides the block cache.
                let lang = code_language(self.source, node.span.start);
                // The block's syntax highlighting, a token per byte range of
                // each line — `None` unless the fence names a language the
                // grammars know (and unless the `syntax` feature is on). Done
                // here, once per build of the block, because the rows it
                // colours ride the block cache: an edit elsewhere in the
                // document reuses them, tokens and all.
                let tokens = lang.as_deref().and_then(|l| code_tokens(l, &lines));
                for (i, raw) in lines.iter().enumerate() {
                    let at = offs.as_ref().map_or(node.span.start, |o| o[i]);
                    // No gutter glyph: the block is set apart by the border and
                    // tint a frontend draws around the whole run of `code` rows,
                    // not by a per-line mark. Just the block prefix and the
                    // code text: the first-row prefix on the first line (a list
                    // item's marker) and the continuation on the rest (its
                    // indent), as a wrapped paragraph takes them.
                    let mut glyphs: Vec<Glyph> = if i == 0 { pf } else { pc }.to_vec();
                    match tokens.as_ref().and_then(|t| t.get(i)) {
                        Some(spans) => push_code_text(&mut glyphs, raw, at, style, spans),
                        None => push_text(&mut glyphs, raw, at, style),
                    }
                    // Explicitly past the line's *text*: a blank code line has no
                    // glyph, and any prefix's offset would put the row's end
                    // inside the next line.
                    self.push_row_at(glyphs, at + raw.len());
                    if let Some(row) = self.rows.last_mut() {
                        row.code = true;
                        if i == 0 {
                            row.code_lang = lang.clone();
                        }
                    }
                }
                // Anchor the block's end past its closing fence. Its last content
                // row ends at the last code line, before the ``` and the blank
                // line under it; without this the separator logic would count the
                // closing-fence line as its own blank row and open a phantom
                // second gap below the block.
                self.last_off = node.span.end;
            }
            "thematic_break" => {
                let full = self.wrap.unwrap_or(UNWRAPPED_RULE_WIDTH);
                let w = full.saturating_sub(prefix_width(pf)).max(4);
                let mut glyphs = pf.to_vec();
                for _ in 0..w {
                    glyphs.push(Glyph {
                        ch: '─',
                        style: Style::default().role(Role::Rule),
                        src: node.span.start,
                        // A rule is a block the caret can sit on, as it always
                        // has; it maps coarsely to the block's start.
                        stop: true,
                    });
                }
                // The dashes share one caret home in front of the atomic block,
                // while the row's end is the second home, past the newline that
                // ends the rule's line. Without that trailing stop a final rule
                // made the document end unreachable: Right could not cross it
                // and a click in the empty space below it snapped back before
                // the rule.
                let (text_end, after_line) = block_line(self.source, node.span.end);
                self.push_row_at(glyphs, after_line);
                // The walk stands at the rule itself, though, as it stands at
                // the end of any other block's last line: the lines under a
                // rule are counted from the newline that ends it. Counted from
                // the home past that newline, every gap under a rule came up a
                // line short, and the empty paragraph Enter opens beneath one
                // drew as nothing at all.
                self.last_off = text_end;
            }
            // A block-level image node with no wrapping paragraph — a promoted
            // top-level HTML `<img>` lands as a direct `doc` child like this
            // (a Markdown `![](…)` comes wrapped in a `para`, handled below).
            "image" => self.block_media(id, MediaKind::Image, id, pf),
            // The same case for a promoted top-level `<video>`/`<audio>`, which
            // arrives as a generic `container` rather than a node kind of its
            // own. It can't be found by the `media_only` scan below the way a
            // wrapped one is: that scan looks at a wrapper's *children*, and here
            // the media element is itself the block.
            "container" if matches!(element_tag(node), Some("video") | Some("audio")) => {
                let kind = match element_tag(node) {
                    Some("audio") => MediaKind::Audio,
                    _ => MediaKind::Video,
                };
                self.block_media(id, kind, id, pf);
            }
            _ => {
                // A container of blocks, or an inline-bearing paragraph.
                let kids = self.children(id);
                // A block-level image: a paragraph (or other wrapper — a
                // `<picture>`, an `<h1>` banner) whose only visible content is a
                // single `image` node. Render it as a placeholder row + record an
                // [`MediaInfo`] a capable frontend replaces. An image mixed with
                // real text or other images on the line isn't block-level and
                // falls through to the inline path below, still as its alt text.
                if let Some((m, kind)) = self.media_only(id) {
                    self.block_media(m, kind, id, pf);
                    return;
                }
                // A display formula on lines of its own — a paragraph whose
                // only visible content is one `display_math` — promotes the
                // same way, to a placeholder row and a [`MathMark`].
                if let Some(m) = self.math_only(id) {
                    self.block_math(m, pf, pc);
                    return;
                }
                let inline = !kids.is_empty() && kids.iter().all(|&c| is_inline(&self.nodes[c]));
                if inline || kids.is_empty() {
                    // The block's own attributes over its containers' — the
                    // three run-level keys become the style its glyphs start
                    // from, and the two line-level ones ride every row it
                    // emits, a wrapped paragraph's continuations included.
                    let pres = self.presentation.under(&node.attrs, &self.faces);
                    let glyphs =
                        self.inline_children_with_trailing(id, pres.over(Style::default()));
                    if !glyphs.is_empty() {
                        let first = self.rows.len();
                        self.emit_wrapped(glyphs, node.span.start, pf, pc);
                        for row in &mut self.rows[first..] {
                            row.align = pres.align;
                            row.line_height = pres.line_height;
                        }
                    }
                } else {
                    self.blocks(id, pf, pc, false);
                }
            }
        }
    }
}
