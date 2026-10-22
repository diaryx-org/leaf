//! Return: splitting a block, continuing or leaving a list or quote, and the
//! prefix a new line carries.

use super::*;

impl Doc {
    /// The Enter key.
    ///
    /// In source view it's a literal newline. In WYSIWYG it's **AST-aware**: a
    /// bare `\n` is only a markdown soft break (same paragraph), so the block the
    /// caret is in decides what actually gets written.
    ///
    ///   - paragraph            → twig's [`Editor::split_block`], which parts the
    ///                            block at the caret and reopens its container
    ///   - list item            → likewise: the next item, its indent, quote
    ///                            prefix and `[ ]` box all reproduced by twig —
    ///                            except an *empty* item, which exits the list
    ///   - block quote          → likewise: a new paragraph inside the quote
    ///   - heading              → a new *paragraph*, not another heading
    ///   - code block           → a literal newline (stay in the block)
    ///   - thematic break       → the line under the rule, opened if it has
    ///                            none ([`stand_under_block`](Self::stand_under_block))
    ///   - blank line           → a literal newline (one Backspace undoes it)
    ///   - [`LineFlow::Preserve`] → a single soft break, which renders as a
    ///                            visible line
    ///
    /// Where `split_block` is used it replaces markup leaf used to spell by hand,
    /// and it is better at it: it drops the whitespace the caret was sitting in
    /// front of instead of stranding it at the head of the second half, and it
    /// knows continuations leaf's marker scan never covered — a checklist item
    /// continues as an *unchecked* checklist item rather than a plain bullet.
    ///
    /// The exceptions above are exceptions because `split_block` is either wrong
    /// there or refuses: parting a fence yields two fences with the code split
    /// between them, parting a heading yields a second heading where every editor
    /// gives a paragraph, and a blank line, an empty item, a setext heading and a
    /// table all report an error rather than a split.
    pub fn newline(&mut self) {
        if self.view == View::Source {
            self.insert_raw("\n");
            return;
        }
        // Enter over a selection deletes it and splits the block where it
        // was, as Enter at a caret does: a `"\n\n"` spliced in its place would
        // put the closing delimiters of a run it cut in the paragraph below.
        if let Some((s, e)) = self.selection()
            && !self.splice_selection(s, e, "", EditKind::Other)
        {
            return;
        }
        // On a blank page there is no block to push down, and a blank line
        // with nothing under it is not something Fold flow draws. Enter wrote
        // one anyway: an edit, and an undo step, that showed nothing.
        if self.line_flow == LineFlow::Fold && self.source[self.caret_floor()..].trim().is_empty() {
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
        let kinds: Vec<Kind> = self
            .editor
            .ancestors_at(off)
            .map(|c| c.into_iter().map(|m| m.kind).collect())
            .unwrap_or_default();
        let has = |k: Kind| kinds.contains(&k);

        if has(Kind::CodeBlock) {
            self.insert_raw("\n");
            return;
        }
        // In a verse a line break is the content, so Enter makes a line — the
        // text editor's Enter, in every flow. See `verse_newline`.
        if has(Kind::LineBlock) {
            self.verse_newline();
            return;
        }
        // HTML keeps neither a blank line nor a newline in running text, so
        // the whitespace the branches below write is whitespace its page does
        // not have. In prose Enter writes elements instead; a list, a quote
        // and a table keep the branches below. See `html_newline`.
        if self.format == Format::Html
            && ![
                Kind::ListItem,
                Kind::TaskListItem,
                Kind::BlockQuote,
                Kind::Table,
            ]
            .into_iter()
            .any(has)
        {
            self.html_newline();
            return;
        }
        // An *empty* list item exits the list — the standard double-Enter — which
        // `split_block` reports as an error rather than a split (there is no
        // content to part), so it stays leaf's. `list_marker_on_line` is itself
        // the AST gate — it answers from the tree, so a `- ` that reads as a
        // marker byte-for-byte but opens no item (a setext underline, a Djot
        // continuation line) never reaches here.
        if let Some(marker) = self.list_marker_on_line(self.caret)
            && self.item_is_empty(&marker)
        {
            self.exit_list(&marker);
            return;
        }
        // The same double-Enter leaves a quote: the first Enter at its end
        // wrote a separator and an empty quoted line, and a second one there
        // is the writer asking out, which `split_block` would answer with a
        // third quoted line. Outside any list — an item's own empty line took
        // the branch above.
        if !has(Kind::ListItem) && !has(Kind::TaskListItem) && self.exit_quote() {
            return;
        }
        // Beside a rule, at the home past it, the caret's source line is the
        // blank one under the rule, which the branch below would take for an
        // empty paragraph. Mid-document its lone newline left the caret on the
        // next block's first line, and what was typed there joined that block.
        if let Some(end) = self.rule_above_caret() {
            self.stand_under_block(end);
            return;
        }
        // On an *empty* paragraph line, a lone Enter should add a single blank line,
        // not another full paragraph break — so it moves down one line and one
        // Backspace undoes it, not two. (`split_block` errors here too.)
        let line_start = self.source[..self.caret].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.source[self.caret..]
            .find('\n')
            .map_or(self.source.len(), |i| self.caret + i);
        if self.source[line_start..line_end].trim().is_empty() {
            self.insert_raw("\n");
            return;
        }
        // At the start of a paragraph with a block above it, twig's split
        // writes a lone newline ahead of the paragraph, which Fold draws as one
        // more gap: the first Enter showed nothing. A heading's branch below
        // parted it after its `#`, leaving an empty heading over a paragraph
        // that had been the heading's text. Both open an empty paragraph above
        // instead, the caret staying with the text. In Preserve flow too: its
        // lone newline went inside a div, where nothing drew it, and between
        // a mark's delimiters, where it cut the mark.
        if let Some((at, text)) = self.paragraph_opening_at_caret() {
            let caret = self.caret;
            if self.splice(at, at, text, EditKind::Other) {
                self.caret = caret + text.len();
                self.record_caret();
            }
            return;
        }
        // In `Preserve` flow a soft break is a *visible* line the author means to
        // make, so Enter writes a single `\n` and typing continues the same
        // paragraph on the next line — the behaviour of an ordinary text editor.
        // A second Enter then lands on the blank line above and takes the
        // empty-line branch, so double-Enter still promotes to a full paragraph
        // break; and Backspace, which deletes a lone `\n` over a soft break,
        // undoes a single Enter symmetrically. In `Fold` flow a lone `\n` would
        // render as an invisible space, so Enter keeps making the paragraph break
        // that actually shows.
        //
        // Only in running prose. A list or a quote has a continuation of its own
        // to write, and a `\n` there is not a soft line but a lost container.
        // Nor in a heading, which a newline ends in Markdown and runs on
        // unseen in djot: it takes the heading's own branch below.
        let in_container = has(Kind::ListItem) || has(Kind::TaskListItem) || has(Kind::BlockQuote);
        if self.line_flow == LineFlow::Preserve && !in_container && !has(Kind::Heading) {
            self.insert_raw("\n");
            return;
        }
        // A heading gets a *paragraph*, never a second heading: Enter at the end
        // of a title is how every editor is asked for the body under it, and
        // `split_block` would repeat the `#` instead. Whitespace at the split
        // point goes with the break rather than opening the new paragraph, which
        // is what `split_block` does everywhere else.
        if has(Kind::Heading) {
            let mut end = self.caret;
            while self.source.as_bytes().get(end) == Some(&b' ') {
                end += 1;
            }
            self.splice(self.caret, end, "\n\n", EditKind::Other);
            return;
        }
        self.split_block_here();
    }

    /// Enter in an HTML document's prose, which writes elements where
    /// Markdown writes whitespace.
    ///
    ///   - at the end of a paragraph or heading, or in an empty one → an
    ///     empty `<p></p>` after it, the caret inside
    ///   - at the start of one → an empty `<p></p>` before it, the caret
    ///     staying with the text
    ///   - between two blocks, on the whitespace that separates them → an
    ///     empty `<p></p>` there
    ///   - inside a paragraph in [`LineFlow::Preserve`] → a `<br>`, the soft
    ///     line that flow's Enter makes
    ///   - inside one otherwise → two elements, parted at the caret
    ///     ([`split_html_block`](Self::split_html_block))
    ///
    /// At a paragraph's end Preserve flow opens a paragraph too: a `<br>`
    /// with nothing after it in its block draws no line in a browser.
    ///
    /// The paragraph break Markdown writes at a block's edge went inside the
    /// element, `<p>I cry\n\n</p>`, which the view drew as an empty paragraph
    /// and a browser does not draw at all; Preserve flow's lone `\n` drew
    /// nothing anywhere.
    fn html_newline(&mut self) {
        let caret = self.caret.min(self.source.len());
        let block = self.editor.nodes().ok().and_then(|nodes| {
            nodes
                .iter()
                .filter(|n| matches!(n.kind, Kind::Para | Kind::Heading))
                .filter(|n| {
                    let content = n.content_span.clone().unwrap_or_else(|| n.span.clone());
                    content.start <= caret && caret <= content.end
                })
                .min_by_key(|n| n.span.len())
                .cloned()
        });
        let Some(block) = block else {
            let at_line_start = caret == 0 || self.source.as_bytes()[caret - 1] == b'\n';
            let (text, inside) = if at_line_start {
                ("<p></p>\n", "<p>".len())
            } else {
                ("\n<p></p>", "\n<p>".len())
            };
            self.open_html_paragraph(caret, text, caret + inside);
            return;
        };
        let drawn: Vec<Range<usize>> = self
            .editor
            .subtree(block.id)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter(|n| {
                        matches!(n.kind, Kind::Image | Kind::HardBreak)
                            || n.text.as_deref().is_some_and(|t| !t.trim().is_empty())
                    })
                    .map(|n| n.span.clone())
                    .collect()
            })
            .unwrap_or_default();
        let before = drawn.iter().any(|s| s.start < caret);
        let after = drawn.iter().any(|s| s.end > caret);
        if !after {
            let at = block.span.end;
            self.open_html_paragraph(at, "\n<p></p>", at + "\n<p>".len());
        } else if !before {
            let text = "<p></p>\n";
            self.open_html_paragraph(block.span.start, text, caret + text.len());
        } else if self.line_flow == LineFlow::Preserve && block.kind == Kind::Para {
            self.splice(caret, caret, "<br>", EditKind::Other);
        } else {
            self.split_html_block(&block);
        }
    }

    /// Part the HTML paragraph or heading `block` at the caret, which has text
    /// on both sides of it: each element open there is closed and opened
    /// again, so `<p>a <b>bo|ld</b></p>` becomes `<p>a <b>bo</b></p>` over
    /// `<p><b>ld</b></p>`, each paragraph keeping the tag it was written
    /// with. A heading's second half is a paragraph, as in every format.
    ///
    /// twig's split wrote a Markdown paragraph break, `\n\n`, inside the
    /// element, which HTML reads as one space.
    fn split_html_block(&mut self, block: &FlatNode) {
        let caret = self.caret.min(self.source.len());
        let Some(content) = block.content_span.clone() else {
            return;
        };
        // The inline elements around the caret, outermost first. At one's
        // edge the caret is outside it, and the split goes outside it too.
        let mut inline: Vec<FlatNode> = self
            .editor
            .subtree(block.id)
            .map(|nodes| {
                nodes
                    .into_iter()
                    .filter(|n| n.span != block.span)
                    .filter(|n| {
                        n.content_span
                            .as_ref()
                            .is_some_and(|c| c.start <= caret && caret <= c.end && *c != n.span)
                    })
                    .collect()
            })
            .unwrap_or_default();
        inline.sort_by_key(|n| (n.span.start, std::cmp::Reverse(n.span.end)));
        let mut at = caret;
        let mut open = Vec::new();
        for n in inline {
            let c = n.content_span.clone().unwrap_or_else(|| n.span.clone());
            if caret == c.start {
                at = n.span.start;
                break;
            }
            if caret == c.end {
                at = n.span.end;
                break;
            }
            open.push(n);
        }
        let tag = |r: Range<usize>| self.source.get(r).unwrap_or("").to_string();
        let opening =
            |n: &FlatNode| n.span.start..n.content_span.as_ref().map_or(n.span.start, |c| c.start);
        let closing =
            |n: &FlatNode| n.content_span.as_ref().map_or(n.span.end, |c| c.end)..n.span.end;
        let (second_open, second_close) = if block.kind == Kind::Heading {
            ("<p>".to_string(), "</p>".to_string())
        } else {
            (tag(opening(block)), tag(closing(block)))
        };
        let mut text: String = open.iter().rev().map(|n| tag(closing(n))).collect();
        text += &tag(closing(block));
        text.push('\n');
        text += &second_open;
        text.extend(open.iter().map(|n| tag(opening(n))));
        let caret = at + text.len();
        text += &self.source[at..content.end];
        text += &second_close;
        if self.splice(at, block.span.end, &text, EditKind::Other) {
            self.caret = caret.min(self.source.len());
            self.record_caret();
        }
    }

    /// Write `text` at `at` and stand the caret at `caret`, an offset in the
    /// edited source.
    fn open_html_paragraph(&mut self, at: usize, text: &str, caret: usize) {
        if self.splice(at, at, text, EditKind::Other) {
            self.caret = caret.min(self.source.len());
            self.record_caret();
        }
    }

    /// Where Enter writes to open an empty paragraph above the paragraph or
    /// heading the caret starts, and what — `None` when the caret does not
    /// start one.
    ///
    /// The caret starts a paragraph that is in no list item, quote, fence or
    /// table when no text of the paragraph stands before it: at its first
    /// byte, or past only the opening delimiters of marks that begin with it
    /// (`**`, a colour's `<span …>`), which is where a tap before the first
    /// letter lands. A heading's `#` marker is such a delimiter. Parted there, twig's split cut the mark in two. The break
    /// goes in front of those delimiters, and in front of any `<div>` or
    /// fenced div the paragraph opens — the view draws no row for a blank line
    /// above a container's first block, so a centred paragraph's empty line
    /// has to open outside its div.
    ///
    /// Inside a list twig's split already writes a marked line, which draws,
    /// and so it does in a quote, except above the quote's first line.
    ///
    /// What is written is what it takes to draw one more empty row. Between
    /// two blocks the first and last blank lines are the gap, so a paragraph
    /// break goes where there are fewer than two and a newline where the gap
    /// is already there. Above the first block only the last blank line is
    /// the gap, so a newline goes where there is one already. Preserve flow
    /// draws every blank line, so a newline is always enough there, but under
    /// frontmatter, whose own blank line draws nothing.
    fn paragraph_opening_at_caret(&mut self) -> Option<(usize, &'static str)> {
        let caret = self.caret.min(self.source.len());
        let chain = self.editor.ancestors_at(caret).ok()?;
        if chain.iter().any(|m| {
            matches!(
                m.kind,
                Kind::ListItem | Kind::TaskListItem | Kind::CodeBlock | Kind::Table
            )
        }) {
            return None;
        }
        let para = chain
            .iter()
            .find(|m| matches!(m.kind, Kind::Para | Kind::Heading))?;
        // An empty heading has nothing to push down: Enter there is still
        // the body under it.
        let end = para.span.end.min(self.source.len());
        if self.source[caret.min(end)..end].trim().is_empty() {
            return None;
        }
        let mut at = para.span.start;
        if at != caret {
            let text_before = self
                .editor
                .subtree(twig::NodeId(para.node_id))
                .ok()?
                .iter()
                .any(|n| {
                    n.span.start < caret
                        && (n.kind == Kind::Image
                            || n.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
                });
            if text_before {
                return None;
            }
        }
        // Out past each container this paragraph opens, innermost first: one
        // whose source before it is its opening line and blank lines. A quote
        // opens on its first paragraph's own line, and draws no line above
        // that paragraph: the break goes above the quote. Further down a
        // quote twig's split writes a quoted line, which draws.
        let mut outer: Vec<_> = chain
            .iter()
            .filter(|m| matches!(m.kind, Kind::Container | Kind::BlockQuote) && m.span.start < at)
            .collect();
        outer.sort_by_key(|m| std::cmp::Reverse(m.span.start));
        for c in outer {
            let before = &self.source[c.span.start..at];
            let opens = match c.kind {
                Kind::BlockQuote => !before.contains('\n'),
                _ => before.lines().skip(1).all(|l| l.trim().is_empty()),
            };
            if !opens {
                break;
            }
            at = c.span.start;
        }
        if chain
            .iter()
            .any(|m| m.kind == Kind::BlockQuote && m.span.start < at)
        {
            return None;
        }
        let content = self.source[..at].trim_end().len();
        let newlines = self.source[content..at].matches('\n').count();
        // Every newline starts a blank line but one that ends the line of
        // what stands above, a block or hidden frontmatter.
        let blank = if content == 0 {
            newlines
        } else {
            newlines.saturating_sub(1)
        };
        let first = self
            .top_blocks()
            .iter()
            .all(|m| m.kind == Kind::Metadata || m.span.start >= at);
        // Preserve flow draws every blank line, but the one under frontmatter.
        let gap = match (self.line_flow, first) {
            (LineFlow::Preserve, true) => usize::from(content > 0),
            (LineFlow::Preserve, false) => 0,
            (LineFlow::Fold, true) => 1,
            (LineFlow::Fold, false) => 2,
        };
        Some((at, if blank >= gap { "\n" } else { "\n\n" }))
    }

    /// Part the block at the caret with twig's [`Editor::split_block`], leaving
    /// the caret in the second half.
    ///
    /// twig reopens whatever the first half was inside of — the bullet with its
    /// indent, the quote's `>`, a checklist item's `[ ]` — which is the whole
    /// reason this replaced the markup leaf used to spell from the line's bytes.
    /// It renumbers nothing, though: a new item mid-list is written with its
    /// neighbour's number, so [`renumber_here`](Self::renumber_here) still runs
    /// behind it, folded into the same undo step.
    ///
    /// Falls back to a plain paragraph break if twig declines, so an unhandled
    /// shape still moves the caret down rather than swallowing the keystroke.
    fn split_block_here(&mut self) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        // The pre-edit caret, as `splice_exact` hands it over: twig files it
        // with the step, and without it Undo put the caret back wherever the
        // edit before this one had left it.
        self.record_caret();
        match self.editor.split_block(self.caret) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                self.anchor = None;
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
                // Aimed at the new block's *start*: the caret twig leaves is one
                // past the marker it wrote, where there is no list in reach.
                self.renumber_at(change.new.start);
            }
            Err(_) => self.insert_raw("\n\n"),
        }
    }

    /// Whether the item on the marker's line carries no content — the shape
    /// double-Enter reads as "I'm done with this list."
    fn item_is_empty(&self, line: &ListMarker) -> bool {
        let content_start = line.content_start().min(self.source.len());
        let line_end = self.source[self.caret..]
            .find('\n')
            .map(|i| self.caret + i)
            .unwrap_or(self.source.len());
        self.source[content_start..line_end.max(content_start)]
            .trim()
            .is_empty()
    }

    /// Leave the list: replace the empty item's marker with a blank line, so the
    /// caret lands in a fresh paragraph below it.
    ///
    /// Inside a quote the blank line has to stay quoted (a bare one would end the
    /// quote), and the caret's new line keeps the `> ` it was already behind —
    /// leaving the list without also leaving the quote.
    fn exit_list(&mut self, line: &ListMarker) {
        let prefix = self.quote_prefix_at(line.marker_start);
        let blank = prefix.trim_end();
        self.splice(
            line.line_start,
            self.caret,
            &format!("{blank}\n{prefix}"),
            EditKind::Other,
        );
    }

    /// What a line continuing the containers at `off` has to open with — the
    /// quote markers reproduced, each enclosing item's marker as its width in
    /// spaces. Also the column a nested item's marker stands in, which is what
    /// makes it Tab's answer.
    pub(super) fn continuation_prefix_at(&mut self, off: usize) -> String {
        self.editor
            .document()
            .and_then(|mut d| d.continuation_prefix(off))
            .map(|p| p.text)
            .unwrap_or_default()
    }

    /// The column a *nested list* may open at inside the item at `off` — which
    /// is not always where the item's own text continues.
    ///
    /// twig counts a task item's `[ ] ` box as part of its marker, correctly:
    /// it is markup a rich view hides, and the item's own wrapped text does
    /// stand past it. But a nested list may only open at the *list* marker's
    /// column, and four columns further in is an indented continuation of the
    /// paragraph instead — `- [ ] a` + `      - [ ] b` is one item, not two.
    /// So the box's own width goes back.
    ///
    /// The one place leaf still reads a checkbox's spelling. It goes when twig
    /// reports the list marker's column apart from the box; `checked` is what
    /// says a box is there at all, so only its width is being measured here.
    pub(super) fn nesting_prefix_at(&mut self, off: usize) -> String {
        let cont = self.continuation_prefix_at(off);
        let Some(item) = self.innermost_list_item(off) else {
            return cont;
        };
        if item.checked.is_none() {
            return cont;
        }
        let box_width = item
            .marker_span
            .and_then(|m| self.source.get(m))
            .and_then(|marker| marker.rfind('[').map(|i| marker.len() - i))
            .unwrap_or(0);
        // The trailing columns are the ones the item's own marker contributed,
        // so trimming from the end leaves any quote prefix standing.
        cont[..cont.len().saturating_sub(box_width)].to_string()
    }

    /// Where the line of the item *containing* the item at `off` begins — the
    /// prefix Shift+Tab moves back to, which gives up exactly the level the
    /// parent contributed. The quote prefix alone for a top-level item, which
    /// has no level left to give.
    pub(super) fn outdent_prefix_at(&mut self, off: usize) -> String {
        let items: Vec<usize> = self
            .editor
            .document()
            .and_then(|mut d| d.ancestors_at_caret(off))
            .map(|c| {
                c.into_iter()
                    .filter(|m| m.kind == Kind::ListItem || m.kind == Kind::TaskListItem)
                    .map(|m| m.span.start)
                    .collect()
            })
            .unwrap_or_default();
        // The second-innermost item is the parent; its own line's indent is the
        // target. `list_marker_on_line` gives that line's prefix directly.
        let parent = items.len().checked_sub(2).map(|i| items[i]);
        match parent.and_then(|p| self.list_marker_on_line(p)) {
            Some(m) => self.source[m.line_start..m.marker_start].to_string(),
            None => self.quote_prefix_at(off),
        }
    }

    /// Leave the innermost quote from an empty line at its end — one holding
    /// only quote markers — and say whether that was where the caret was.
    ///
    /// The empty line goes, and so does a blank quoted line above it, the
    /// separator the first Enter wrote: left behind, the quote would end in
    /// an empty paragraph. In their place goes what [`exit_list`] writes, a
    /// blank line and the caret's own line, each keeping the markers of any
    /// quote still around them.
    ///
    /// [`exit_list`]: Self::exit_list
    fn exit_quote(&mut self) -> bool {
        // Only the markers, and how many: a line whose `>`s are all there is
        // to it, at the depth of the quote it closes.
        fn depth(line: &str) -> Option<usize> {
            line.chars()
                .all(|c| c == '>' || c.is_whitespace())
                .then(|| line.matches('>').count())
        }
        fn opening_depth(line: &str) -> usize {
            line.chars()
                .take_while(|c| *c == '>' || c.is_whitespace())
                .filter(|c| *c == '>')
                .count()
        }
        let caret = self.caret.min(self.source.len());
        let line_start = self.source[..caret].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.source[caret..]
            .find('\n')
            .map_or(self.source.len(), |i| caret + i);
        let line = &self.source[line_start..line_end];
        let Some(level) = depth(line).filter(|&n| n > 0) else {
            return false;
        };
        // A blank quoted line with more of the quote under it is a gap between
        // two of its paragraphs, not its end.
        let next = self.source.get(line_end + 1..).unwrap_or("");
        let next = next.split('\n').next().unwrap_or("");
        if opening_depth(next) >= level {
            return false;
        }
        let mut start = line_start;
        if line_start > 0 {
            let above_start = self.source[..line_start - 1]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            if depth(&self.source[above_start..line_start - 1]) == Some(level) {
                start = above_start;
            }
        }
        let outer = &line[..line.rfind('>').unwrap_or(0)];
        let blank = outer.trim_end();
        let text = format!("{blank}\n{outer}");
        self.splice(start, line_end, &text, EditKind::Other)
    }

    /// The block-quote prefix in force at `off` — `""` outside a quote, `"> "`
    /// inside one, `"> > "` inside two.
    ///
    /// Assembled from each enclosing quote's own [`FlatNode::marker_span`], so
    /// the `>` and the space after it are twig's spelling rather than leaf's.
    /// The whole line prefix can't answer this: it also carries the indent of
    /// whatever the quote holds, which a blank separator line must *not* repeat.
    pub(super) fn quote_prefix_at(&mut self, off: usize) -> String {
        let Ok(chain) = self
            .editor
            .document()
            .and_then(|mut d| d.ancestors_at_caret(off))
        else {
            return String::new();
        };
        let quotes: Vec<usize> = chain
            .iter()
            .filter(|m| m.kind == Kind::BlockQuote)
            .map(|m| m.node_id as usize)
            .collect();
        let Ok(nodes) = self.editor.nodes() else {
            return String::new();
        };
        quotes
            .iter()
            .filter_map(|id| nodes.get(*id)?.marker_span.clone())
            .filter_map(|s| self.source.get(s))
            .collect()
    }

    /// Whether the item at `off` sits inside another one — the test Backspace
    /// uses to choose between outdenting and dropping the marker.
    ///
    /// Counted from the AST rather than from the line's leading whitespace,
    /// which is indentation in Markdown and, in Djot, may be nothing at all.
    pub(super) fn item_is_nested(&mut self, off: usize) -> bool {
        self.editor
            .document()
            .and_then(|mut d| d.ancestors_at_caret(off))
            .map(|c| {
                c.into_iter()
                    .filter(|m| m.kind == Kind::ListItem || m.kind == Kind::TaskListItem)
                    .count()
                    > 1
            })
            .unwrap_or(false)
    }

    /// The innermost list item containing `probe`, under twig's **caret**
    /// containment rule — a block's end is inside it.
    ///
    /// Half-open containment can't answer this. An empty item's span is exactly
    /// its marker, so the caret sitting after `- ` is one past the end and the
    /// item it is plainly in tests as out of reach; that is the shape
    /// double-Enter has to recognise to leave the list.
    pub(super) fn innermost_list_item(&mut self, probe: usize) -> Option<FlatNode> {
        let chain = self
            .editor
            .document()
            .and_then(|mut d| d.ancestors_at_caret(probe))
            .ok()?;
        let id = chain
            .iter()
            .rev()
            .find(|m| m.kind == Kind::ListItem || m.kind == Kind::TaskListItem)?
            .node_id as usize;
        self.editor.nodes().ok()?.get(id).cloned()
    }

    /// The list marker opening `off`'s line, per twig — `None` when that line
    /// opens no list item.
    ///
    /// [`Document::line_prefix`] is the whole hidden run from the line start:
    /// `>   1. ` is a quote's marker, an indent, and an item's marker together,
    /// and it is `None` on a *continuation* line, which opens nothing. That last
    /// case is the one leaf could never get right by reading bytes. `- a\n  - b`
    /// is two items in Markdown and one in Djot, where a marker cannot interrupt
    /// a paragraph and `  - b` is literal text — identical bytes, and only the
    /// parser knows which document it is looking at.
    ///
    /// The item's own marker is separated out via its
    /// [`FlatNode::marker_span`], so `marker_start` splits the prefix into what
    /// the containers around it contribute and what the item does.
    pub(super) fn list_marker_on_line(&mut self, off: usize) -> Option<ListMarker> {
        let off = off.min(self.source.len());
        let prefix = self.editor.document().ok()?.line_prefix(off).ok()??;
        // The prefix belongs to a list only when an item's marker closes it —
        // a heading's `# ` or a bare quote's `> ` is a prefix too.
        let item = self.innermost_list_item(prefix.end.min(self.source.len()))?;
        let marker = item.marker_span.clone()?;
        if marker.end != prefix.end {
            return None;
        }
        Some(ListMarker {
            line_start: prefix.start,
            marker_start: marker.start,
            text: self.source.get(prefix)?.to_string(),
        })
    }

    /// Whether the list item on `line_start`'s line is the **first item** of its
    /// list — the one Tab must not nest, because nesting needs a preceding
    /// sibling to become the new parent and a first item has none. `false` for a
    /// line that isn't a list item, and for an item with a sibling above it (the
    /// one Tab *can* nest). Gated on the AST, not the marker bytes: `- ` reads
    /// the same in a setext underline that opens no list at all.
    pub(super) fn first_item_of_list(&mut self, line_start: usize) -> bool {
        let Some(marker) = self.list_marker_on_line(line_start) else {
            return false;
        };
        // Probe just inside the marker, where the item's own node is in reach —
        // the marker offset itself can resolve to the enclosing list, not the
        // `list_item`, whose span starts at the marker.
        let probe = marker.content_start().min(self.source.len());
        let Some(item) = self.innermost_list_item(probe) else {
            return false;
        };
        let Ok(nodes) = self.editor.nodes() else {
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
}
