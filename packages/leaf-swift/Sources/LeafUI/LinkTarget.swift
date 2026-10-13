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

/// One entry a link offers a menu, in the order a menu should show them.
///
/// Shared vocabulary rather than each platform's own, because the AppKit context
/// menu and the UIKit edit menu answer the same question — what can I do with the
/// link under the caret — and two copies of that answer would drift.
enum LinkAction {
    /// Show what the destination points at, without going there — the link's
    /// answer to a footnote's "Show Note". Offered first, because "what does
    /// that say" is asked far more often than "take me there", and it is the
    /// only one of these a reader can undo by looking away.
    case peek
    /// Leave for the destination. Offered for anything activatable, wikilinks
    /// included.
    case open
    /// Repoint the link at a new destination. Offered only for a *parsed* link
    /// (there is nothing to rewrite in a wikilink) and only when a host is there
    /// to ask for the new one.
    case edit
    /// Put the destination on the pasteboard.
    case copy
}

extension LeafDoc {
    /// The link target the caret stands in, or nil.
    ///
    /// A parsed link node wins; `wikilinks` then decides whether a bare
    /// `[[…]]` construct counts as one too.
    func activatableTargetAtCaret(wikilinks: Bool) -> String? {
        if let destination = linkDestinationAtCaret() { return destination }
        return wikilinks ? wikilinkAtCaret() : nil
    }

    /// Where `destination` lands *in this document*, when it names a place in it
    /// rather than somewhere to leave for — the offset a bare `#v2` sends the
    /// caret to. Nil for every other destination, including a `#v2` this
    /// document has no answer for.
    ///
    /// Handled before the host is asked, for the reason a footnote reference is:
    /// a same-document reference is a move within the page, and there is nothing
    /// in it for a vault to resolve or a browser to open. It is also the one link
    /// spelling that used to do *nothing at all* — not opened, not claimed, not
    /// reported.
    ///
    /// See `landing(for:)` for why both spellings of the fragment are tried.
    func selfLanding(of destination: String) -> UInt32? {
        guard destination.hasPrefix("#") else { return nil }
        return landing(for: String(destination.dropFirst()))?.start
    }

    /// The link entries a menu should show for the caret's position — empty when
    /// the caret stands in no link at all, which is what tells a caller to show
    /// no link section rather than an empty one.
    ///
    /// `canEdit` is the host's answer to "can I ask for a new destination"
    /// (`LeafEditorModel.onEditLink` being set). Without it there is no way to
    /// carry out an edit, so no menu offers one.
    ///
    /// `canPeek` is the same question for the other host hook
    /// (`LeafEditorModel.onPeekLink`) — with one exception the view doesn't have
    /// to be told about: a `#v2` names a place in the document already on
    /// screen, so the editor can show that one on its own.
    func linkActionsAtCaret(wikilinks: Bool, canEdit: Bool, canPeek: Bool) -> [LinkAction] {
        guard let target = activatableTargetAtCaret(wikilinks: wikilinks) else { return [] }
        var actions: [LinkAction] = []
        if canPeek || target.hasPrefix("#") { actions.append(.peek) }
        actions.append(.open)
        if canEdit, linkDestinationAtCaret() != nil { actions.append(.edit) }
        actions.append(.copy)
        return actions
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

/// A stretch of a laid-out document that is a link, by layout position —
/// what `EditorLayout.link(at:also:)` measures a tap against.
struct LinkSpan {
    /// A source offset inside the link: the run's own for a parsed link, the
    /// opening bracket's for a wikilink.
    let src: UInt32
    /// The whole `[[…]]` construct for a wikilink; nil for a parsed link.
    let wikilink: String?
    let from: (row: Int, ch: Int)
    let to: (row: Int, ch: Int)
}

/// A link a finger landed on — what `EditorLayout.link(at:also:)` answers, and
/// what a touch view acts on without asking the caret, for `FootnoteTap`'s
/// reasons: the system places the caret for the same tap in an order of its
/// own choosing, beside the link as often as in it, and in a reader not at all.
struct LinkTap: Equatable {
    /// A source offset inside the link, which `linkDestinationAt` resolves
    /// for a parsed link, and where an edit puts the caret first.
    let src: UInt32
    /// The `[[…]]` construct, verbatim, when the link is a wikilink — its own
    /// destination, since there is no node for the document to ask.
    let wikilink: String?
    /// The glyphs of the line of the link the tap was nearest, in the layout's
    /// coordinates.
    let rect: CGRect
}

extension LeafDoc {
    /// The link a tap landed on, as the document spells its destination — the
    /// offset peer of `activatableTargetAtCaret`, honouring the same switch.
    /// Nil for a run in the link role that leads nowhere, such as a footnote
    /// reference.
    func destination(of tap: LinkTap, wikilinks: Bool) -> String? {
        if let wikilink = tap.wikilink { return wikilinks ? wikilink : nil }
        return linkDestinationAt(off: tap.src)
    }

    /// Every `[[…]]` construct in the source, by byte range, each verbatim —
    /// what a tap is measured against when wikilinks count as links, since none
    /// of them is a node a layout can see (see `wikilinkAtCaret`).
    ///
    /// The same rule as `wikilinkAtCaret`, read from the other end: a span runs
    /// from each `[[` to the first `]]` after it on its line, so that for every
    /// offset inside one, the span whose opener is nearest is the construct
    /// the caret would answer with there. Openers nest (`[[a [[b]]`), so spans
    /// can overlap; they come back innermost first, which is the one a tap
    /// that lands in both means.
    func wikilinkSpans() -> [(text: String, start: UInt32, end: UInt32)] {
        let bytes = Array(source().utf8)
        let open = UInt8(ascii: "["), close = UInt8(ascii: "]"), newline = UInt8(ascii: "\n")
        var spans: [(text: String, start: UInt32, end: UInt32)] = []
        var i = 0
        while i + 1 < bytes.count {
            defer { i += 1 }
            guard bytes[i] == open, bytes[i + 1] == open else { continue }
            var j = i + 2
            while j + 1 < bytes.count, bytes[j] != newline {
                if bytes[j] == close, bytes[j + 1] == close {
                    let text = String(decoding: bytes[i..<(j + 2)], as: UTF8.self)
                    if !text.dropFirst(2).dropLast(2).trimmingCharacters(in: .whitespaces).isEmpty {
                        spans.append((text, UInt32(i), UInt32(j + 2)))
                    }
                    break
                }
                j += 1
            }
        }
        return spans.reversed()
    }
}
