//! IME composition and dead keys: README.md's "What it would take", item 3,
//! for the native backend.
//!
//! A webview editor needs `onbeforeinput` and the composition events, which
//! `dioxus-html` does not have. Native needs neither: winit reports the
//! input method itself — `Ime::Preedit` while a composition is open, with the
//! marked text and a cursor inside it, then `Ime::Commit` with what it
//! settled on. On macOS that covers every input method, *and* dead keys: ⌥E
//! opens a composition of `´`, and the E after it commits `é`. The keystrokes
//! a composition eats never arrive as `KeyboardInput` at all.
//!
//! Not through Blitz: it turns IME on only for its own `<input>`s, and sends
//! `UiEvent::Ime` to the focused node, which is the same unreliable path as
//! `onkeydown` (see `keys.rs`). The editor allows IME on the window itself
//! and reads `WindowEvent::Ime` beside the keys.
//!
//! The marked text is written into the document as it is composed — core's
//! `edit_composing`, each step replacing the last one's provisional bytes, so
//! the run folds into one undo — exactly as the Swift editor writes it with
//! `replaceRange`. That puts it in the frame like any other text, wrapped and
//! hit-tested with the rest, and underlined by the `composing` mark.

use leaf_core::Doc;

/// The open composition, if any.
#[derive(Default, Debug)]
pub struct Composer {
    /// The marked text in the source: where it starts and how many bytes.
    marked: Option<(usize, usize)>,
    /// Where the input method's cursor is within the marked text, in bytes.
    cursor: usize,
    /// Whether a composition run is open in core's undo history — still true
    /// after the marked text is cleared, which winit does just before a
    /// commit, so the commit joins the run it ends.
    open: bool,
}

impl Composer {
    /// The marked text's source range, for the `composing` mark.
    pub fn marked(&self) -> Option<(usize, usize)> {
        self.marked.map(|(s, len)| (s, s + len))
    }

    /// Where to *draw* the caret while composing: the input method's cursor
    /// inside the marked text.
    ///
    /// Drawn, not placed. Moving core's caret — even to where it already is
    /// — ends the edit run, so every step of a composition would be its own
    /// undo; core's caret stays where `edit_composing` left it, at the marked
    /// text's end, and the frame is told where the input method's cursor is.
    pub fn caret(&self) -> Option<usize> {
        self.marked.map(|(s, _)| s + self.cursor)
    }

    /// The input method's current composition. Empty text clears it — the
    /// input method withdrew it, or is about to commit.
    pub fn preedit(&mut self, doc: &mut Doc, text: &str, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            if let Some((s, len)) = self.marked.take() {
                doc.edit_composing(s, s + len, "");
            }
            return;
        }
        let (start, end) = self.target(doc);
        doc.edit_composing(start, end, text);
        self.marked = Some((start, text.len()));
        self.open = true;
        // The end of the input method's cursor range, which is where typing
        // the next character of the reading lands; see `caret`.
        self.cursor = cursor.map_or(text.len(), |(_, b)| b.min(text.len()));
    }

    /// The input method settled on `text`: it replaces the marked text, or
    /// the selection, as the last step of the composition's undo run.
    pub fn commit(&mut self, doc: &mut Doc, text: &str) {
        let (start, end) = self.target(doc);
        self.marked = None;
        doc.edit_composing(start, end, text);
        doc.end_composition();
        self.open = false;
    }

    /// Any other input: a composition withdrawn without a commit ends its
    /// undo run here, or the next one would fold into it.
    pub fn settle(&mut self, doc: &mut Doc) {
        if self.open && self.marked.is_none() {
            doc.end_composition();
            self.open = false;
        }
    }

    /// What the next step replaces: the marked text, or else the selection,
    /// or else nothing at the caret.
    fn target(&self, doc: &Doc) -> (usize, usize) {
        match self.marked {
            Some((s, len)) => (s, s + len),
            None => doc.selection().unwrap_or((doc.caret, doc.caret)),
        }
    }
}

#[cfg(test)]
mod tests {
    use leaf_core::Format;

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.build_visual_unwrapped();
        d
    }

    #[test]
    fn a_dead_key_composes_and_commits_one_character() {
        let mut d = doc("caf\n");
        d.place_caret(3, false);
        let mut c = Composer::default();
        // ⌥E: the accent, marked.
        c.preedit(&mut d, "´", Some((2, 2)));
        assert_eq!(d.source, "caf´\n");
        assert_eq!(c.marked(), Some((3, 5)));
        assert_eq!(c.caret(), Some(5));
        // E: winit clears the preedit, then commits.
        c.preedit(&mut d, "", None);
        c.commit(&mut d, "é");
        assert_eq!(d.source, "café\n");
        assert_eq!(c.marked(), None);
        assert_eq!(d.caret, 5);
    }

    #[test]
    fn a_composition_is_one_undo_step() {
        let mut d = doc("x\n");
        d.place_caret(1, false);
        let mut c = Composer::default();
        for reading in ["か", "かん", "かんじ"] {
            c.preedit(&mut d, reading, None);
        }
        c.preedit(&mut d, "", None);
        c.commit(&mut d, "感じ");
        assert_eq!(d.source, "x感じ\n");
        d.undo();
        assert_eq!(d.source, "x\n");
    }

    #[test]
    fn each_step_replaces_the_last_ones_marked_text() {
        let mut d = doc("ab\n");
        d.place_caret(1, false);
        let mut c = Composer::default();
        c.preedit(&mut d, "n", None);
        c.preedit(&mut d, "に", None);
        assert_eq!(d.source, "aにb\n");
        assert_eq!(c.marked(), Some((1, 1 + "に".len())));
    }

    #[test]
    fn a_composition_replaces_the_selection() {
        let mut d = doc("hello\n");
        d.select_range(0, 5);
        let mut c = Composer::default();
        c.commit(&mut d, "やあ");
        assert_eq!(d.source, "やあ\n");
    }

    #[test]
    fn a_withdrawn_composition_leaves_nothing() {
        let mut d = doc("ab\n");
        d.place_caret(1, false);
        let mut c = Composer::default();
        c.preedit(&mut d, "´", None);
        c.preedit(&mut d, "", None);
        c.settle(&mut d);
        assert_eq!(d.source, "ab\n");
    }
}

