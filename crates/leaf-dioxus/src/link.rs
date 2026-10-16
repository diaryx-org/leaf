//! Following what the caret stands on: a link (⌘-click, *Open Link*) or a
//! footnote (⌘-click, *Go to Note* / *Back to Reference*), as the Swift
//! editor's `LeafTextView` does — and what a right-click menu offers, which
//! the host shows ([`crate::Host::context_menu`]).
//!
//! The questions are core's (`link_destination_at`, `locate`, `footnote_at`,
//! `footnote_definition_at`); what is here is Swift's `LinkTarget.swift`,
//! `FootnoteTarget.swift` and `LinkDestination.swift` — which way a footnote
//! goes, and whether a destination is a place in this document, a URL, or a
//! file beside it.

use std::path::Path;

use leaf_core::Doc;

use crate::editor::Editor;
use crate::trace::trace;

/// What a right-click offers, read off the caret after the click placed it:
/// Swift's `menu(for:)` asks the same three things.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    /// There is a selection, so Cut and Copy.
    pub selection: bool,
    /// The caret is in a link, so Open Link and Copy Link.
    pub link: bool,
    /// The caret is on a footnote, and which way it goes.
    pub footnote: Option<Footnote>,
}

/// Which way a footnote goes from where the caret is. There is never more
/// than one: the caret stands on a reference, in a note, or in neither.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Footnote {
    GoToNote,
    BackToReference,
}

/// The footnote at `off` and the offset following it lands on: a reference's
/// note, or a note's (first) reference. `None` where there is no footnote or
/// nowhere to go — a reference to a note never written, a note nothing cites.
pub fn footnote_jump(doc: &mut Doc, off: usize) -> Option<(Footnote, usize)> {
    if let Some(r) = doc.footnote_at(off) {
        return r.offset.map(|o| (Footnote::GoToNote, o));
    }
    let d = doc.footnote_definition_at(off)?;
    d.offset.map(|o| (Footnote::BackToReference, o))
}

/// Where a link's destination leads.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    /// A `#fragment` this document answers to: a move within the page.
    Here(usize),
    /// A URL, or a file that exists, for the system to open.
    Away(String),
}

/// Resolve a destination, as Swift's `selfLanding` and `LinkDestination.url`
/// do: `#v2` is a place here; anything with a scheme is a URL; anything else
/// is a path, relative to the document's folder, that must exist. `None` for
/// a fragment nothing here answers to, or a file that is not there — a beep
/// in Swift, the status line here.
pub fn resolve(doc: &mut Doc, destination: &str) -> Option<Target> {
    let dest = destination.trim();
    if let Some(id) = dest.strip_prefix('#') {
        return doc.locate(id).map(|l| Target::Here(l.start));
    }
    if has_scheme(dest) {
        return Some(Target::Away(dest.to_string()));
    }
    let path = dest.split(['#', '?']).next().unwrap_or_default();
    if path.is_empty() {
        return None;
    }
    let path = percent_decode(path);
    let full = match Path::new(&path) {
        p if p.is_absolute() => p.to_path_buf(),
        p => doc.path.parent().filter(|_| !doc.is_untitled())?.join(p),
    };
    full.exists()
        .then(|| Target::Away(full.to_string_lossy().into_owned()))
}

/// `scheme:` per RFC 3986 — a letter, then letters, digits, `+`, `-`, `.`.
fn has_scheme(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
}

/// `%20` and friends back to bytes; a malformed escape is kept as written.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Editor {
    /// ⌘-click, after the click placed the caret: follow the footnote there,
    /// or else the link — Swift's order. Nothing there leaves the caret where
    /// the click put it.
    pub fn follow_at_caret(&self) {
        if !self.follow_footnote() {
            self.open_link();
        }
    }

    /// Move to the note the caret's reference names, or back to the
    /// reference the caret's note is for. Answers whether there was one.
    pub fn follow_footnote(&self) -> bool {
        let jump = {
            let mut doc = self.st.doc.borrow_mut();
            let at = doc.caret;
            footnote_jump(&mut doc, at)
        };
        trace(|| format!("follow footnote -> {jump:?}"));
        let Some((_, to)) = jump else { return false };
        self.st.doc.borrow_mut().place_caret(to, false);
        self.refresh();
        true
    }

    /// Open the link the caret stands in: a `#fragment` here moves the caret,
    /// anything else goes to the system.
    pub fn open_link(&self) -> bool {
        let target = {
            let mut doc = self.st.doc.borrow_mut();
            let Some(dest) = doc.link_destination_at_caret() else {
                return false;
            };
            let target = resolve(&mut doc, &dest);
            if target.is_none() {
                doc.status = Some(format!("nothing to open at {dest}"));
            }
            target
        };
        trace(|| format!("open link -> {target:?}"));
        match target {
            Some(Target::Here(at)) => self.st.doc.borrow_mut().place_caret(at, false),
            Some(Target::Away(what)) => self.st.host.open(&what),
            None => {}
        }
        self.refresh();
        true
    }

    /// Put the destination of the link the caret stands in on the clipboard.
    pub fn copy_link(&self) {
        let Some(dest) = self.st.doc.borrow_mut().link_destination_at_caret() else {
            return;
        };
        let _ = self.st.host.write_clipboard(&dest, None);
    }

    /// A right-click, after it placed the caret: have the host pop up the
    /// context menu for what the caret is on.
    pub(crate) fn context_menu(&self) {
        let cx = {
            let mut doc = self.st.doc.borrow_mut();
            let at = doc.caret;
            Context {
                selection: doc.anchor.is_some_and(|a| a != doc.caret),
                link: doc.link_destination_at(at).is_some(),
                footnote: footnote_jump(&mut doc, at).map(|(f, _)| f),
            }
        };
        self.st.host.context_menu(&cx);
    }
}

#[cfg(test)]
mod tests {
    use leaf_core::Format;

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.to_string(), Format::Markdown).expect("a document");
        d.build_visual(80);
        d
    }

    #[test]
    fn a_footnote_goes_to_its_note_and_back() {
        let src = "See this.[^1]\n\n[^1]: The note.\n";
        let mut d = doc(src);
        let reference = src.find("[^1]").expect("the reference") + 2;
        let (dir, note) = footnote_jump(&mut d, reference).expect("a jump");
        assert_eq!(dir, Footnote::GoToNote);
        assert_eq!(&src[note..note + 3], "The");
        let (dir, back) = footnote_jump(&mut d, note).expect("a jump back");
        assert_eq!(dir, Footnote::BackToReference);
        assert!(src[back..].starts_with('1'), "{back}");
        assert_eq!(footnote_jump(&mut d, 1), None);
    }

    #[test]
    fn a_fragment_lands_on_its_heading() {
        let src = "[go](#second-part)\n\n# First\n\n# Second part\n";
        let mut d = doc(src);
        assert_eq!(
            resolve(&mut d, "#second-part"),
            Some(Target::Here(src.find("# Second").unwrap()))
        );
        assert_eq!(resolve(&mut d, "#nowhere"), None);
    }

    #[test]
    fn a_url_goes_to_the_system_and_a_missing_file_nowhere() {
        let mut d = doc("x\n");
        assert_eq!(
            resolve(&mut d, "https://example.com/a"),
            Some(Target::Away("https://example.com/a".into()))
        );
        assert_eq!(
            resolve(&mut d, "mailto:a@b.c"),
            Some(Target::Away("mailto:a@b.c".into()))
        );
        // Untitled, so a relative path has nothing to be relative to.
        assert_eq!(resolve(&mut d, "notes.md"), None);
        assert_eq!(resolve(&mut d, "/no/such/file%20here.md"), None);
        assert_eq!(resolve(&mut d, "/tmp#x"), Some(Target::Away("/tmp".into())));
    }

    #[test]
    fn percent_escapes_decode() {
        assert_eq!(percent_decode("a%20b%2"), "a b%2");
    }
}
