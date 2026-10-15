//! Verse: a block whose every line is its own row, hung and inset.

use super::*;

impl Builder<'_> {
    /// A verse — twig's `line_block`, whichever spelling it came from:
    /// Markdown's `<div class="verse">`, djot's `::: verse`, AsciiDoc's
    /// `[verse]`. Every line is a row of its own, the way a source line is in
    /// [`LineFlow::Preserve`](crate::LineFlow::Preserve), and the blank lines
    /// between them are rows too, navigable, so editing a poem is editing
    /// lines: a stanza break is the blank line it is spelled with, and Enter
    /// and Backspace make and unmake lines the way a text editor does.
    ///
    /// A line's indent is the em spaces it starts with in the source, drawn as
    /// the em spaces they are — real glyphs with real offsets, so the caret
    /// walks across them and Backspace takes one. A line too long for the
    /// measure wraps with a *hanging* indent, [`VERSE_HANG`] ems deeper than
    /// the line itself, which is how a reader tells a turnover from a new
    /// line: core writes it into the continuation prefix where it wraps, and
    /// stamps [`VRow::hang`] for a frontend that wraps rows itself.
    ///
    /// The blank line Markdown needs between `<div class="verse">` and the
    /// first line, and between the last line and `</div>`, is syntax and
    /// draws nothing; any more than that is a line the author opened (Enter
    /// at the end of the poem) and draws. djot and AsciiDoc need none.
    ///
    /// Verse is never justified: the right edge of a poem is the poet's.
    pub(super) fn verse(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        let node = &self.nodes[id];
        let span = node.span.clone();
        let saved = self.presentation;
        let mut pres = self.presentation.under(&node.attrs, &self.faces);
        if pres.align == Some(Align::Justify) {
            pres.align = None;
        }
        self.presentation = pres;
        let style = pres.over(Style::default());
        // The whole block stands [`VERSE_INSET`] ems in from the prose around
        // it, the way a book sets off verse quoted in a page of prose — the
        // one sign, before any line runs long, that these breaks are the
        // poet's and not the page's. Prefix, so every frontend draws it.
        let inset = synth(
            &"\u{2003}".repeat(VERSE_INSET),
            Role::ListIndent,
            span.start,
        );
        let pf = &concat(pf, &inset);
        let pc = &concat(pc, &inset);
        // Markdown's tag lines take a blank line each side; djot's fence and
        // AsciiDoc's delimiter do not.
        let tagged = self
            .source
            .get(span.start..)
            .is_some_and(|s| s.starts_with('<'));
        let lines: Vec<usize> = self
            .children(id)
            .into_iter()
            .filter(|&l| self.nodes[l].first_child.is_some())
            .collect();

        let open_end = self.source[span.start.min(self.source.len())..]
            .find('\n')
            .map_or(span.start, |k| span.start + k);
        let mut prev_end = open_end;
        // The block's opening prefix (a list bullet, say) goes on the first row
        // it draws, whatever that row is; every later one takes `pc`.
        let mut lead_used = false;
        for (k, &line) in lines.iter().enumerate() {
            let start = self.nodes[line].span.start;
            let mut blanks = self.blank_rows_between(prev_end, start);
            if k == 0 && tagged && !blanks.is_empty() {
                blanks.remove(0);
            }
            self.verse_blank_rows(&blanks, pf, pc, &mut lead_used);

            let indent = self.nodes[line].level.unwrap_or_default();
            let mut glyphs = self.verse_indent_glyphs(line, style);
            glyphs.extend(self.inline_children(line, style));
            glyphs.extend(self.verse_trailing_space(line, style));
            let hang = (indent as usize + VERSE_HANG).min(u8::MAX as usize);
            let turnover = concat(
                pc,
                &synth(&"\u{2003}".repeat(hang), Role::ListIndent, start),
            );
            let lead = if lead_used { pc } else { pf };
            lead_used = true;
            let first = self.rows.len();
            self.emit_wrapped(glyphs, start, lead, &turnover);
            for row in &mut self.rows[first..] {
                row.align = pres.align;
                row.line_height = pres.line_height;
                row.hang = Some(hang as u8);
            }
            prev_end = self.nodes[line].span.end;
        }

        if lines.is_empty() {
            // A verse with no line yet — nothing else would give the caret a
            // home inside it.
            let home = self.nodes[id]
                .content_span
                .clone()
                .map_or(span.start, |c| c.start);
            self.push_row_at(pf.to_vec(), home.min(self.source.len()));
        } else {
            // The lines opened under the last one, before the closing tag.
            let end = span.end.min(self.source.len());
            let close_line = self.source[..end]
                .trim_end_matches(['\n', '\r'])
                .rfind('\n')
                .map_or(0, |p| p + 1);
            if close_line > prev_end {
                let mut blanks = self.blank_rows_between(prev_end, close_line.max(prev_end + 1));
                // `blank_rows_between` stops before the line holding its end,
                // which here is the closing line itself, so every blank line
                // above it is counted; Markdown's last one is syntax.
                if tagged && !blanks.is_empty() {
                    blanks.pop();
                }
                self.verse_blank_rows(&blanks, pf, pc, &mut lead_used);
            }
        }

        self.presentation = saved;
        // Step past the closing tag or fence, as the div arm in
        // [`Builder::block`] does: the rich view draws nothing for it, and left
        // behind it reads as a blank line under the poem.
        self.last_off = self.last_off.max(span.end);
        self.stepped_over = self.stepped_over.max(span.end);
    }

    /// One navigable empty row per blank line of a verse, at `offs` — the
    /// first row the verse draws wearing its opening prefix `pf`.
    fn verse_blank_rows(
        &mut self,
        offs: &[usize],
        pf: &[Glyph],
        pc: &[Glyph],
        lead_used: &mut bool,
    ) {
        for &end_src in offs {
            let prefix = if *lead_used { pc } else { pf };
            *lead_used = true;
            self.rows.push(VRow {
                glyphs: prefix.to_vec(),
                end_src,
                decoration: false,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                align: self.presentation.align,
                line_height: self.presentation.line_height,
                // Every row a verse draws carries a hang, its blank lines too,
                // so a frontend can find the block's rows by it — see
                // [`VRow::hang`].
                hang: Some(VERSE_HANG as u8),
                boundary: None,
                mark_ends: Vec::new(),
                math: Vec::new(),
                region: None,
                inline_regions: Vec::new(),
            });
            self.last_off = end_src;
        }
    }

    /// The em spaces a verse line starts with, as glyphs at their own source
    /// offsets — one per step of the line's indent, whether the source spells
    /// it raw or as `&emsp;`, `&#8195;` or `&#x2003;`. twig has already taken
    /// them out of the line's text; they are between the line's start and its
    /// first child's.
    fn verse_indent_glyphs(&self, line: usize, style: Style) -> Vec<Glyph> {
        let start = self.nodes[line].span.start;
        let text_start = self.nodes[line]
            .first_child
            .map_or(start, |c| self.nodes[c.0 as usize].span.start);
        let mut out = Vec::new();
        let mut at = start;
        while at < text_start {
            let Some(len) = em_space_at(self.source, at) else {
                break;
            };
            out.push(Glyph {
                ch: '\u{2003}',
                style,
                src: at,
                stop: true,
            });
            at += len;
        }
        out
    }

    /// Glyphs for the spaces a verse line's source carries past its last
    /// inline node — the space just typed at the end of a line, which twig
    /// drops as insignificant — so the caret is drawn past it. The paragraph
    /// path reads these off `content_span`; a line has none.
    fn verse_trailing_space(&self, line: usize, style: Style) -> Vec<Glyph> {
        let mut at = self.nodes[line].span.end;
        let mut out = Vec::new();
        while let Some(&b) = self.source.as_bytes().get(at) {
            if b != b' ' && b != b'\t' {
                break;
            }
            out.push(Glyph {
                ch: ' ',
                style,
                src: at,
                stop: true,
            });
            at += 1;
        }
        // Two spaces before the newline are Markdown's hard break, not text the
        // caret stands past.
        if self.source.as_bytes().get(at) == Some(&b'\n') && out.len() >= 2 {
            out.clear();
        }
        out
    }
}
