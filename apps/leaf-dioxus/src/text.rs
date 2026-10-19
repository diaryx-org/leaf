//! The document's text as the system sees it, for the host's text input
//! client and spelling checker.
//!
//! AppKit speaks in UTF-16 indices into one string: an `NSRange` from
//! `selectedRange`, into `attributedSubstring(forProposedRange:)`, or from
//! `NSSpellChecker`. For the rendered view that string is core's visible text
//! ([`VisualMap::visible_text`](leaf_core::VisualMap::visible_text)), with
//! its markup hidden and each block's end spelled as one `\n`, and core maps
//! an index into it to a source offset and back. For the source view it is
//! the source itself. Inside the editor, offsets stay source bytes; these
//! are the crossing.

use leaf_core::{Doc, Role, View};

/// The text the system indexes.
pub fn visible(doc: &mut Doc) -> String {
    match doc.view {
        View::Source => doc.source.clone(),
        View::Wysiwyg => {
            doc.build_visual_unwrapped();
            doc.vmap.visible_text(0, usize::MAX)
        }
    }
}

/// The UTF-16 index of the source offset `off` in [`visible`].
pub fn utf16_of(doc: &mut Doc, off: usize) -> usize {
    match doc.view {
        View::Source => {
            let off = floor_char(&doc.source, off);
            doc.source[..off].encode_utf16().count()
        }
        View::Wysiwyg => {
            doc.build_visual_unwrapped();
            doc.vmap.visible_utf16_len(0, off)
        }
    }
}

/// The source offset of the character at UTF-16 `index` in [`visible`]; the
/// end of the document at or past its end.
pub fn offset_of(doc: &mut Doc, index: usize) -> usize {
    match doc.view {
        View::Source => {
            let mut units = 0;
            for (i, c) in doc.source.char_indices() {
                if units + c.len_utf16() > index {
                    return i;
                }
                units += c.len_utf16();
            }
            doc.source.len()
        }
        View::Wysiwyg => {
            doc.build_visual_unwrapped();
            doc.vmap
                .offset_at_visible_utf16(usize::MAX, index)
                .unwrap_or_else(|| doc.vmap.snap_to_stop(doc.source.len()))
        }
    }
}

/// The source bytes the UTF-16 range `loc`..`loc + len` of [`visible`]
/// shows, and not a byte more — so a word at the edge of `**bold**` keeps
/// its delimiters — if those bytes spell `original`. `None` for a range that
/// crosses hidden markup or text the source spells with an escape: replacing
/// it would break what it stands in.
pub fn exact(doc: &mut Doc, loc: usize, len: usize, original: &str) -> Option<(usize, usize)> {
    let (from, to) = bounds(doc, loc, len);
    (doc.source.get(from..to) == Some(original)).then_some((from, to))
}

/// The source bytes the UTF-16 range `loc`..`loc + len` of [`visible`]
/// shows: from its first character to the end of its last.
pub fn bounds(doc: &mut Doc, loc: usize, len: usize) -> (usize, usize) {
    let from = offset_of(doc, loc);
    if len == 0 {
        return (from, from);
    }
    let last = offset_of(doc, loc + len - 1);
    let to = doc.source[last..]
        .chars()
        .next()
        .map_or(last, |c| last + c.len_utf8());
    (from, to.max(from))
}

/// [`visible`] with everything but prose blanked to spaces — code, list
/// markers, quote bars, rules, formulas, pictures and a directive's rows —
/// keeping each block's `\n` and every index where it was: what the spelling
/// checker reads, so it never flags an identifier or a bullet. Empty in the
/// source view, where a word is whatever the markup needs it to be.
pub fn prose(doc: &mut Doc) -> String {
    if doc.view == View::Source {
        return String::new();
    }
    let visible: Vec<u16> = visible(doc).encode_utf16().collect();
    let mut keep = vec![false; visible.len()];
    let rows = doc
        .vmap
        .rows
        .iter()
        .filter(|r| !r.decoration && !r.code && !r.directive && r.media.is_none());
    for row in rows {
        for g in row.glyphs.iter().filter(|g| g.stop) {
            let prose = matches!(
                g.style.role,
                Role::Body | Role::Heading(_) | Role::Link | Role::Mark(_)
            );
            if !prose {
                continue;
            }
            let at = doc.vmap.visible_utf16_len(0, g.src);
            for k in keep.iter_mut().skip(at).take(g.ch.len_utf16()) {
                *k = true;
            }
        }
    }
    let blank = u16::from(b' ');
    let newline = u16::from(b'\n');
    let masked: Vec<u16> = visible
        .iter()
        .zip(&keep)
        .map(|(&u, &k)| if k || u == newline { u } else { blank })
        .collect();
    String::from_utf16_lossy(&masked)
}

fn floor_char(s: &str, mut off: usize) -> usize {
    off = off.min(s.len());
    while !s.is_char_boundary(off) {
        off -= 1;
    }
    off
}

#[cfg(test)]
mod tests {
    use leaf_core::Format;

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.set_inline_pictures(true);
        d.build_visual_unwrapped();
        d
    }

    #[test]
    fn the_visible_text_hides_markup_and_maps_back() {
        let mut d = doc("Some **bold** text\n\nnext\n");
        let text = visible(&mut d);
        assert!(text.starts_with("Some bold text\n"), "{text:?}");
        let bold = text.find("bold").unwrap();
        let off = offset_of(&mut d, bold);
        assert_eq!(&d.source[off..off + 4], "bold");
        assert_eq!(utf16_of(&mut d, off), bold);
    }

    #[test]
    fn an_exact_range_keeps_the_delimiters_around_it() {
        let mut d = doc("a **teh** b\n");
        let at = visible(&mut d).find("teh").unwrap();
        let (from, to) = exact(&mut d, at, 3, "teh").expect("the word's bytes");
        assert_eq!(&d.source[from..to], "teh");
        // A range across hidden markup is not one.
        let wide = visible(&mut d).find("a teh").unwrap();
        assert_eq!(exact(&mut d, wide, 5, "a teh"), None);
    }

    #[test]
    fn code_and_bullets_are_blanked_for_the_checker() {
        let mut d = doc("- an itme with `cde` in it\n\n```\nfn mian() {}\n```\n");
        let text = visible(&mut d);
        let prose = prose(&mut d);
        assert_eq!(prose.encode_utf16().count(), text.encode_utf16().count());
        assert!(prose.contains("an itme with"), "{prose:?}");
        assert!(!prose.contains("cde"), "{prose:?}");
        assert!(!prose.contains("mian"), "{prose:?}");
        assert!(!prose.contains('•'), "{prose:?}");
    }
}
