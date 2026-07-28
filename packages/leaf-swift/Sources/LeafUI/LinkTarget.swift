//  LinkTarget.swift
//
//  What the caret can activate — the one question both platform views ask when
//  a click, a tap, a ⌘-click or an "Open Link" arrives, kept here so they can't
//  drift apart.
//
//  Almost all of it is core's answer: `linkDestinationAtCaret()` reads the
//  parsed `link`/`url`/`email` node the caret stands in. The exception is the
//  wikilink, which needs a lexer of its own precisely because it *isn't* one of
//  those — see `wikilinkAtCaret`.

import Foundation
import LeafFFI

extension LeafDoc {
    /// The link target the caret stands in, or nil.
    ///
    /// A parsed link node wins; `wikilinks` then decides whether a bare
    /// `[[…]]` construct counts as one too.
    func activatableTargetAtCaret(wikilinks: Bool) -> String? {
        if let destination = linkDestinationAtCaret() { return destination }
        return wikilinks ? wikilinkAtCaret() : nil
    }

    /// The whole `[[…]]` construct the caret stands inside, brackets and all
    /// (`[[notes/a.md]]`, `[[id:6tzwsxg|last week]]`), or nil.
    ///
    /// This is lexical rather than parsed because neither Markdown nor Djot has
    /// a wikilink, so twig has no node to offer: a `[[…]]` reaches the screen as
    /// literal text, and reading it back out of the source is the only way to
    /// know it was there. Which also sets the honest expectation for a caller —
    /// a wikilink can be *followed*, but until the grammar knows it, it will not
    /// be *styled* as a link.
    ///
    /// Returned verbatim, wrapper included, so the host receives what the
    /// document actually says and its own link parser can do the splitting.
    ///
    /// Unlike a scan of the whole body this only has to judge one offset, which
    /// makes the greedy "nearest opener, first closer" rule safe: a stray `[[`
    /// elsewhere in the document can't capture this caret, only an opener that
    /// really does precede it. The newline guard covers the other direction — a
    /// wikilink is an inline construct, so an unclosed `[[` can't reach down the
    /// page to pair with an unrelated `]]`.
    private func wikilinkAtCaret() -> String? {
        // UTF-8 throughout: core's offsets are byte offsets into this same
        // source, and the delimiters are ASCII, so a byte scan needs no
        // character-boundary care to find them.
        let bytes = Array(source().utf8)
        let caret = Int(caretOffset())
        guard caret <= bytes.count else { return nil }

        let open = UInt8(ascii: "["), close = UInt8(ascii: "]"), newline = UInt8(ascii: "\n")

        // The nearest `[[` at or before the caret. `i` indexes the *second*
        // bracket, so it starts one past the caret: a caret resting on the
        // opening `[` of `[[` is inside the construct, and that pair's second
        // bracket is at `caret + 1`.
        var start: Int?
        var i = min(caret + 1, bytes.count - 1)
        while i >= 1 {
            if bytes[i] == newline { return nil }
            if bytes[i] == open && bytes[i - 1] == open { start = i - 1; break }
            i -= 1
        }
        guard let start else { return nil }

        // The first `]]` after it, and the caret has to fall within the pair.
        var end: Int?
        var j = start + 2
        while j + 1 < bytes.count {
            if bytes[j] == newline { return nil }
            if bytes[j] == close && bytes[j + 1] == close { end = j + 2; break }
            j += 1
        }
        guard let end, caret < end else { return nil }

        let text = String(decoding: bytes[start..<end], as: UTF8.self)
        // `[[]]` and `[[   ]]` name nothing; neither is a link to follow.
        guard !text.dropFirst(2).dropLast(2).trimmingCharacters(in: .whitespaces).isEmpty else {
            return nil
        }
        return text
    }
}
