//! The clipboard's rich flavor: a selection out as HTML, and HTML pasted back in.

use super::*;

impl Doc {
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
        let (Some(head), Some(tail)) =
            (self.top_block_span(start), self.top_block_span(start + off))
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
}
