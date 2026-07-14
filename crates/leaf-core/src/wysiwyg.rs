//! The WYSIWYG view: render the document with its markup *resolved*, not shown —
//! headings coloured, `**bold**` as real bold, `# ` / `**` / `` ` `` delimiters
//! hidden — while keeping every visible glyph tied back to the source byte it
//! came from.
//!
//! That back-reference (`Glyph::src`) is what lets a caret still work: the caret
//! stays a source offset (shared with the source view), but the [`VisualMap`]
//! converts between an offset and a screen `(row, col)`, so cursor drawing,
//! mouse clicks, and — crucially — arrow motion all operate in *visible* space
//! and step right over the hidden delimiters.
//!
//! Text is walked from the AST (`str` nodes carry exact spans, and their text is
//! the verbatim source slice), so a Markdown and a Djot file that parse alike
//! render — and map — identically.

use twig::FlatNode;

use crate::style::{Color, Style};

/// One rendered character plus the source byte offset it originates from.
/// Synthetic glyphs (a list bullet, a quote gutter) point at their block's
/// start, so clicking one lands the caret at the start of that block.
#[derive(Clone)]
pub struct Glyph {
    pub ch: char,
    pub style: Style,
    pub src: usize,
}

/// One visual line. `end_src` is the source offset a caret sits at when placed
/// at the line's end (past its last glyph) — the anchor for end-of-line and
/// click-past-content.
pub struct VRow {
    pub glyphs: Vec<Glyph>,
    pub end_src: usize,
}

/// The rendered document plus the offset⇄position mapping the caret rides on.
#[derive(Default)]
pub struct VisualMap {
    pub rows: Vec<VRow>,
}

impl VisualMap {
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn row_len(&self, row: usize) -> usize {
        self.rows.get(row).map(|r| r.glyphs.len()).unwrap_or(0)
    }

    /// The screen `(row, col)` for a source offset — where to draw the caret.
    /// Snaps a hidden offset (inside a delimiter) to the next visible glyph.
    pub fn pos_of_offset(&self, off: usize) -> (usize, usize) {
        for (r, row) in self.rows.iter().enumerate() {
            for (c, g) in row.glyphs.iter().enumerate() {
                if g.src >= off {
                    return (r, c);
                }
            }
            if row.end_src >= off {
                return (r, row.glyphs.len());
            }
        }
        let r = self.rows.len().saturating_sub(1);
        (r, self.row_len(r))
    }

    /// The source offset for a screen `(row, col)` — where a click or a
    /// visual-space move lands the caret.
    pub fn offset_of_pos(&self, row: usize, col: usize) -> usize {
        let Some(r) = self.rows.get(row) else {
            return 0;
        };
        match r.glyphs.get(col) {
            Some(g) => g.src,
            None => r.end_src,
        }
    }
}

/// Render the document to a [`VisualMap`], wrapping to `width` columns. Text and
/// offsets come from the AST (`str` nodes carry the verbatim source slice and an
/// exact span), so the original source string isn't needed here.
pub fn build(nodes: &[FlatNode], source: &str, width: usize) -> VisualMap {
    let Some(doc) = nodes.iter().position(|n| n.kind == "doc") else {
        return VisualMap::default();
    };
    let mut b = Builder {
        nodes,
        source,
        width: width.max(8),
        rows: Vec::new(),
        last_off: 0,
    };
    b.blocks(doc, &[], &[]);
    b.emit_trailing_blank_lines();
    VisualMap { rows: b.rows }
}

struct Builder<'a> {
    nodes: &'a [FlatNode],
    /// The document source, consulted to place blank-line rows at the source
    /// offsets the caret should occupy on them (the AST drops blank lines).
    source: &'a str,
    width: usize,
    rows: Vec<VRow>,
    /// The end offset of the last content emitted — the anchor for blank
    /// separator rows so the caret never snaps onto one.
    last_off: usize,
}

impl Builder<'_> {
    fn children(&self, id: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut c = self.nodes[id].first_child;
        while let Some(cid) = c {
            out.push(cid.0 as usize);
            c = self.nodes[cid.0 as usize].next_sibling;
        }
        out
    }

    /// Render a node's block children, a blank separator between each.
    fn blocks(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        for (i, child) in self.children(id).into_iter().enumerate() {
            if i > 0 {
                // The blank line between two blocks is a real caret stop, so it
                // needs its *own* source offset — one strictly past the previous
                // block's content, else it collides with that block's last row
                // and `pos_of_offset` (first-match-wins) would resolve the
                // caret onto the wrong row, pinning downward motion there.
                let next_start = self.nodes[child].span.start;
                let end_src = self.blank_line_offset(self.last_off, next_start);
                self.rows.push(VRow {
                    glyphs: pc.to_vec(),
                    end_src,
                });
            }
            let first = if i == 0 { pf } else { pc };
            self.block(child, first, pc);
        }
    }

    fn block(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        let node = &self.nodes[id];
        match node.kind.as_str() {
            "doc" | "section" => self.blocks(id, pf, pc),
            "heading" => {
                let style = heading_style(node.level.unwrap_or(1));
                let glyphs = self.inline_children(id, style);
                self.emit_wrapped(glyphs, node.span.start, pf, pc);
            }
            "block_quote" => {
                let gutter = synth("│ ", Color::Green, node.span.start);
                let f = concat(pf, &gutter);
                let c = concat(pc, &gutter);
                self.blocks(id, &f, &c);
            }
            "bullet_list" | "ordered_list" | "task_list" => {
                let ordered = node.kind == "ordered_list";
                for (i, item) in self.children(id).into_iter().enumerate() {
                    let start = self.nodes[item].span.start;
                    let marker = if ordered {
                        format!("{}. ", i + 1)
                    } else {
                        "• ".to_string()
                    };
                    let bullet = synth(&marker, Color::Yellow, start);
                    let indent = synth(&" ".repeat(marker.chars().count()), Color::Default, start);
                    self.block(item, &concat(pc, &bullet), &concat(pc, &indent));
                }
            }
            "list_item" | "task_list_item" => self.blocks(id, pf, pc),
            "code_block" => {
                let style = Style::default().fg(Color::Green);
                let text = node.text.clone().unwrap_or_default();
                let base = node.span.start; // coarse: code editing is a source-view job
                for raw in text.trim_end_matches('\n').split('\n') {
                    let gutter = synth("▏ ", Color::DarkGray, base);
                    let mut glyphs: Vec<Glyph> = concat(pf, &gutter);
                    for ch in raw.chars() {
                        glyphs.push(Glyph { ch, style, src: base });
                    }
                    self.push_row(glyphs, base);
                }
            }
            "thematic_break" => {
                let w = self.width.saturating_sub(prefix_width(pf)).max(4);
                let mut glyphs = pf.to_vec();
                for _ in 0..w {
                    glyphs.push(Glyph {
                        ch: '─',
                        style: Style::default().fg(Color::DarkGray),
                        src: node.span.start,
                    });
                }
                self.push_row(glyphs, node.span.start);
            }
            _ => {
                // A container of blocks, or an inline-bearing paragraph.
                let kids = self.children(id);
                let inline = !kids.is_empty() && kids.iter().all(|&c| is_inline(&self.nodes[c].kind));
                if inline || kids.is_empty() {
                    let glyphs = self.inline_children(id, Style::default());
                    if !glyphs.is_empty() {
                        self.emit_wrapped(glyphs, node.span.start, pf, pc);
                    }
                } else {
                    self.blocks(id, pf, pc);
                }
            }
        }
    }

    fn inline_children(&self, id: usize, base: Style) -> Vec<Glyph> {
        let mut out = Vec::new();
        for c in self.children(id) {
            self.inline(c, base, &mut out);
        }
        out
    }

    fn inline(&self, id: usize, base: Style, out: &mut Vec<Glyph>) {
        let node = &self.nodes[id];
        match node.kind.as_str() {
            "str" | "smart_punctuation" => push_text(out, node.text.as_deref().unwrap_or(""), node.span.start, base),
            "soft_break" | "hard_break" | "non_breaking_space" => {
                let src = if node.span.start != 0 { node.span.start } else { out.last().map(|g| g.src).unwrap_or(0) };
                out.push(Glyph { ch: ' ', style: base, src });
            }
            "emph" => self.recurse(id, base.italic(), out),
            "strong" => self.recurse(id, base.bold(), out),
            "mark" => self.recurse(id, base.bg(Color::Yellow).fg(Color::Black), out),
            "insert" => self.recurse(id, base.underline(), out),
            "delete" => self.recurse(id, base.strikethrough(), out),
            "superscript" | "subscript" => self.recurse(id, base, out),
            "verbatim" | "inline_math" => {
                // No content_span; map the interior to just after the opener.
                push_text(out, node.text.as_deref().unwrap_or(""), node.span.start + 1, base.fg(Color::Green));
            }
            "link" | "url" | "email" => {
                let style = base.fg(Color::Cyan).underline();
                if self.children(id).is_empty() {
                    push_text(out, node.destination.as_deref().or(node.text.as_deref()).unwrap_or("link"), node.span.start, style);
                } else {
                    self.recurse(id, style, out);
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

    fn recurse(&self, id: usize, style: Style, out: &mut Vec<Glyph>) {
        for c in self.children(id) {
            self.inline(c, style, out);
        }
    }

    /// Word-wrap `glyphs` to the available width and push the visual rows,
    /// prefixing the first with `pf` and the rest with `pc`.
    fn emit_wrapped(&mut self, glyphs: Vec<Glyph>, block_start: usize, pf: &[Glyph], pc: &[Glyph]) {
        // Split into words (maximal non-space runs), each carrying the space
        // glyph that followed it (so its source offset is preserved).
        let mut words: Vec<(Vec<Glyph>, Option<Glyph>)> = Vec::new();
        let mut word: Vec<Glyph> = Vec::new();
        for g in glyphs {
            if g.ch == ' ' {
                words.push((std::mem::take(&mut word), Some(g)));
            } else {
                word.push(g);
            }
        }
        if !word.is_empty() {
            words.push((word, None));
        }
        if words.is_empty() {
            // An empty block still occupies one (prefixed) row.
            self.push_row(pf.to_vec(), block_start);
            return;
        }

        let mut line: Vec<Glyph> = Vec::new();
        let mut used = 0usize;
        let mut first = true;
        for (w, space) in words {
            let avail = self
                .width
                .saturating_sub(prefix_width(if first { pf } else { pc }))
                .max(1);
            if used > 0 && used + w.len() > avail {
                let row = concat(if first { pf } else { pc }, &line);
                self.push_row(row, block_start);
                line = Vec::new();
                used = 0;
                first = false;
            }
            used += w.len();
            line.extend(w);
            if let Some(sp) = space {
                used += 1;
                line.push(sp);
            }
        }
        let row = concat(if first { pf } else { pc }, &line);
        self.push_row(row, block_start);
    }

    fn push_row(&mut self, glyphs: Vec<Glyph>, fallback: usize) {
        let end_src = glyphs
            .last()
            .map(|g| g.src + g.ch.len_utf8())
            .unwrap_or(fallback);
        self.last_off = end_src;
        self.rows.push(VRow { glyphs, end_src });
    }

    /// The source offset the caret rests at on the blank line separating a block
    /// that ends at `prev_end` from the next block starting at `next_start`:
    /// just past the newline that terminates the previous block, but kept
    /// strictly before the next block so the offset is unique to this row.
    fn blank_line_offset(&self, prev_end: usize, next_start: usize) -> usize {
        let after_nl = self.source[prev_end..]
            .find('\n')
            .map_or(prev_end, |p| prev_end + p + 1);
        after_nl.min(next_start.saturating_sub(1)).max(prev_end)
    }

    /// Blank lines the user typed past the end of the last block (e.g. two
    /// `Enter`s to open a fresh paragraph) leave no AST node, so nothing renders
    /// and the caret appears stuck on the old line. Reconstruct one empty row
    /// per extra trailing newline from the source, each at its own offset, so
    /// the caret rides down onto the new line the moment it's created.
    fn emit_trailing_blank_lines(&mut self) {
        let last_end = self.rows.last().map_or(0, |r| r.end_src);
        if last_end >= self.source.len() {
            return;
        }
        // The first newline after the last content just terminates that line;
        // every newline beyond it is a blank line the caret can occupy.
        let extra = self.source[last_end..].matches('\n').count();
        for k in 1..extra {
            self.rows.push(VRow {
                glyphs: Vec::new(),
                end_src: last_end + k,
            });
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn push_text(out: &mut Vec<Glyph>, text: &str, base_src: usize, style: Style) {
    for (i, ch) in text.char_indices() {
        out.push(Glyph { ch, style, src: base_src + i });
    }
}

/// Build synthetic prefix glyphs (a bullet, a gutter) all pointing at `src`.
/// `Color::Default` yields the surface's own color (no override).
fn synth(text: &str, color: Color, src: usize) -> Vec<Glyph> {
    let style = Style::default().fg(color);
    text.chars().map(|ch| Glyph { ch, style, src }).collect()
}

fn concat(a: &[Glyph], b: &[Glyph]) -> Vec<Glyph> {
    let mut v = a.to_vec();
    v.extend_from_slice(b);
    v
}

fn prefix_width(prefix: &[Glyph]) -> usize {
    prefix.len()
}

fn heading_style(level: u32) -> Style {
    let base = Style::default().bold();
    match level {
        1 => base.fg(Color::Cyan).underline(),
        2 => base.fg(Color::Green),
        3 => base.fg(Color::Yellow),
        4 => base.fg(Color::Blue),
        5 => base.fg(Color::Magenta),
        _ => base.fg(Color::Gray),
    }
}

fn is_inline(kind: &str) -> bool {
    matches!(
        kind,
        "str" | "soft_break" | "hard_break" | "non_breaking_space" | "emph" | "strong" | "mark"
            | "insert" | "delete" | "verbatim" | "inline_math" | "display_math" | "url" | "email"
            | "link" | "image" | "smart_punctuation" | "superscript" | "subscript" | "span"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use twig::{Editor, Format};

    fn map(src: &str) -> VisualMap {
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        build(&ed.nodes().unwrap(), src, 80)
    }

    fn rendered(m: &VisualMap) -> String {
        m.rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn resolves_markup_to_plain_text() {
        let text = rendered(&map("# Title\n\na **bold** word\n"));
        assert!(!text.contains('#'), "heading marker shown: {text:?}");
        assert!(!text.contains("**"), "strong delimiters shown: {text:?}");
        assert!(text.contains("Title") && text.contains("bold word"));
    }

    #[test]
    fn every_glyph_points_at_its_source_byte() {
        let src = "a **bold** c\n";
        let m = map(src);
        for row in &m.rows {
            for g in &row.glyphs {
                // A real (non-synthetic) glyph's source byte is the glyph's char.
                if g.src < src.len() && src.is_char_boundary(g.src) {
                    if let Some(sc) = src[g.src..].chars().next() {
                        if sc == g.ch {
                            continue;
                        }
                    }
                }
                // Synthetic prefixes (none here) would be the only exceptions.
                panic!("glyph {:?} at src {} doesn't match source", g.ch, g.src);
            }
        }
    }

    #[test]
    fn offset_and_position_round_trip_on_visible_text() {
        let m = map("hello world\n");
        let (r, c) = m.pos_of_offset(6); // the 'w'
        assert_eq!(m.offset_of_pos(r, c), 6);
    }

    #[test]
    fn caret_steps_over_hidden_delimiters() {
        // "a **bold** c": bytes 8,9 are the closing ** — no glyph. Moving right
        // from 'd' (src 7) lands on the space before 'c' (src 10), not inside **.
        let m = map("a **bold** c\n");
        let (r, c) = m.pos_of_offset(7);
        assert_eq!(m.offset_of_pos(r, c + 1), 10);
    }
}
