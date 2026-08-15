//  FootnoteTarget.swift
//
//  What the caret can do with a footnote — the one question both platform views
//  ask when a ⌘-click, a long press, a hover or a "Go to Note" arrives, kept here
//  for the reason `LinkTarget.swift` keeps its own: the AppKit context menu and
//  the UIKit edit menu answer the same question, and two copies of the answer
//  would drift.
//
//  A footnote is not a link, and this file exists because of the difference. A
//  link names somewhere to *leave for*, so its vocabulary is open/edit/copy. A
//  reference names a note already in this document, so following one is a move
//  within the page — and a move within a page has a way back, which is the half
//  a "click the reference, land at the note" implementation usually forgets.
//  Both legs are core's (`footnoteAt`, `footnoteDefinitionAtCaret`); what's here
//  is the direction-agnostic shape a menu and a gesture can both be written
//  against.

import Foundation
import LeafFFI

/// One entry a footnote offers a menu.
///
/// There is never more than one: the caret is either up in the prose standing on
/// a reference or down in the notes standing on a definition, and core's two
/// queries answer for disjoint places. The enum is what lets one gesture and one
/// menu item mean "follow this footnote" without either having to know which way
/// the reader is going.
enum FootnoteAction: Equatable {
    /// Move the caret to the note this reference names.
    case goToNote
    /// Move the caret back to the reference that names this note.
    case backToReference
}

/// What a peek shows for the footnote reference under a pointer or a finger.
///
/// The note arrives already *rendered* — `AttributedRow` over the rows the
/// document itself draws — so `see *later*` reads as italics in the popover the
/// same way it does on the page. That is the whole reason this carries an
/// attributed string rather than the `text` core also offers: `text` is source
/// bytes, asterisks and backticks included, which is the right answer for a
/// search index and the wrong one to show a reader.
///
/// It costs nothing to produce, either. leaf has no separate read-only renderer
/// and needs none: the visual map has already laid the definition out, because
/// the note is a block of the same document, and slicing its rows back out is a
/// lookup rather than a second pass. (The alternative — markup to HTML to
/// `NSAttributedString` — would style from HTML defaults instead of
/// `EditorTheme`, and leans on a WebKit-backed importer far too slow to hang a
/// hover on.)
struct FootnotePeekContent {
    /// The note as the document draws it, marker and all — `[1] Moby-Dick, ch.
    /// 42`. The marker rides along because it is part of the rendered row, and
    /// it earns its place: it says which note answered, which a reader hovering
    /// the third `[3]` of a paragraph would otherwise have to take on trust.
    ///
    /// For a reference nothing defines this is leaf's own sentence instead —
    /// see `isDefined`.
    let body: NSAttributedString
    /// Whether `body` is the note itself. `false` means it is leaf explaining
    /// why there isn't one, which a presenter draws in the secondary colour: it
    /// is chrome, not the document's words, and must not be mistaken for them.
    let isDefined: Bool

    /// The note at `rows`, or leaf's explanation when there is no note to draw.
    ///
    /// `rows` is what the caller sliced out of the `DocView` for this note;
    /// empty means the reference resolved to nothing, or to a note whose body is
    /// blank. Both get a sentence rather than an empty popover — a peek that
    /// opens saying nothing is worse than one that says why.
    ///
    /// `doc` is asked what the followable runs point *at*: a `link` role says a
    /// span is drawn as a link and not where it goes, so the destination comes
    /// from the source offset each run carries. Runs that lead somewhere get
    /// `.footnoteTarget`, which is what makes the popover's text clickable
    /// without the presenter knowing anything about footnotes or links.
    init(_ view: FootnoteView, rows: [Row], theme: EditorTheme, doc: LeafDoc) {
        let drawn = NSMutableAttributedString()
        for row in rows where !row.decoration {
            if drawn.length > 0 { drawn.append(NSAttributedString(string: "\n")) }
            let start = drawn.length
            drawn.append(AttributedRow.make(row, theme: theme))
            FootnotePeekContent.markTargets(in: drawn, row: row, rowStart: start, doc: doc)
        }
        if drawn.length > 0, !drawn.string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            body = drawn
            isDefined = true
            return
        }
        let sentence = view.text == nil
            ? String(format: loc("footnote.noNote", "No note defined for [^%@]."), view.label)
            : loc("footnote.emptyNote", "This note is empty.")
        body = NSAttributedString(string: sentence)
        isDefined = false
    }

    /// Tag the runs of `row` that lead somewhere, over the range they occupy in
    /// `text` (which `rowStart` locates, since a note may be several rows).
    ///
    /// Range arithmetic in UTF-16, because that is what `AttributedRow` appended
    /// and what an `NSAttributedString` is indexed in — deliberately *not* the
    /// row's columns, which are display cells and count a wide glyph twice.
    private static func markTargets(in text: NSMutableAttributedString,
                                    row: Row, rowStart: Int, doc: LeafDoc) {
        var offset = rowStart
        for run in row.runs {
            let length = run.text.utf16.count
            defer { offset += length }
            guard length > 0, let target = doc.peekTarget(of: run) else { continue }
            text.addAttribute(.footnoteTarget, value: target, range: NSRange(location: offset, length: length))
        }
    }
}

/// Where a run inside a peek leads, when it leads anywhere.
///
/// Boxed as a class so it can ride in an `NSAttributedString` attribute, which
/// takes reference types rather than Swift enums.
final class FootnotePeekTarget {
    enum Kind {
        /// A link. The host gets first refusal, exactly as it does in the
        /// document — a `./sibling.md` in a note means the same thing there.
        case link(String)
        /// A footnote reference *inside* the note. Following one navigates the
        /// document rather than opening a second popover: a peek stacked on a
        /// peek is a stack the reader then has to unwind, and the note it names
        /// is a place in this document with a scroll position of its own.
        case footnote(UInt32)
    }
    let kind: Kind
    init(_ kind: Kind) { self.kind = kind }
}

extension NSAttributedString.Key {
    /// Set over the runs of a peek that lead somewhere; the value is a
    /// `FootnotePeekTarget`. Its own key rather than `.link` because `.link`
    /// takes a URL and half of these are document offsets, and because it lets
    /// the presenter draw and hit-test them without a URL round trip.
    static let footnoteTarget = NSAttributedString.Key("LeafFootnoteTarget")
}

extension LeafDoc {
    /// Where `run` leads, or nil for ordinary prose.
    ///
    /// Asked per run rather than per click so the presenter can underline what
    /// is followable and set a pointing cursor over it — a link a reader can't
    /// see is a link they won't try.
    fileprivate func peekTarget(of run: Run) -> FootnotePeekTarget? {
        // A footnote reference first: it is drawn with the link role too (that
        // is how it gets its colour), so asking the link question first would
        // answer for one and send a reader out of the document.
        if run.sup, footnoteAt(off: run.src) != nil {
            return FootnotePeekTarget(.footnote(run.src))
        }
        guard run.role == "link", let destination = linkDestinationAt(off: run.src) else { return nil }
        return FootnotePeekTarget(.link(destination))
    }
}

/// A footnote the caret can follow, and where following it lands.
struct FootnoteJump: Equatable {
    let action: FootnoteAction
    /// The footnote's label — the `1` of `[^1]`, brackets and marker stripped.
    let label: String
    /// The byte offset to move the caret to.
    let offset: UInt32
}

extension LeafDoc {
    /// The footnote the caret can follow, or nil.
    ///
    /// Nil covers three cases a caller treats alike — the caret stands on no
    /// footnote at all, on a reference whose note the document never defines, or
    /// in a note nothing refers to. All three have nowhere to go, and a menu
    /// entry or a ⌘-click that lands nowhere is worse than one that isn't
    /// offered. What a reader *is* told about the second case is the peek's job
    /// (`footnotePeek`), which can say "no note defined" where a jump can only
    /// fail silently.
    ///
    /// The reference is asked first only because it is the commoner place to
    /// stand; the two can't both answer.
    func footnoteJumpAtCaret() -> FootnoteJump? {
        if let ref = footnoteAtCaret(), let offset = ref.offset {
            return FootnoteJump(action: .goToNote, label: ref.label, offset: offset)
        }
        if let def = footnoteDefinitionAtCaret(), let offset = def.offset {
            return FootnoteJump(action: .backToReference, label: def.label, offset: offset)
        }
        return nil
    }

    /// The footnote entries a menu should show for the caret's position — empty
    /// when there is no footnote to follow, which is what tells a caller to show
    /// no footnote section rather than an empty one.
    ///
    /// The peer of `linkActionsAtCaret`, and an array for the same reason even
    /// though it holds at most one: a menu builder loops over what it is given
    /// and stays right if a footnote ever grows a second thing to do to it.
    func footnoteActionsAtCaret() -> [FootnoteAction] {
        footnoteJumpAtCaret().map { [$0.action] } ?? []
    }

    /// The reference at byte offset `off` and the note it names — what a peek
    /// shows, and deliberately *not* caret-based: a pointer resting on a `[1]`
    /// asks what note it names, and moving the caret to find out would yank the
    /// reader out of wherever they were typing.
    ///
    /// Unlike `footnoteJumpAtCaret` this answers for an undefined reference too,
    /// with an empty `text`. A peek is the one place that state can be reported:
    /// the popover is already opening, and "nothing defines `[^99]`" is a fact
    /// about the document worth showing rather than swallowing.
    func footnotePeek(at off: UInt32) -> FootnoteView? {
        footnoteAt(off: off)
    }

    /// What a peek should put on screen for the reference at `off`, or nil when
    /// `off` stands on no reference and there is nothing to show.
    ///
    /// `view` is the frame the caller is drawing — the note's rows are sliced
    /// out of it, because the document has already laid the definition out and
    /// re-rendering it would only risk disagreeing with the page underneath.
    ///
    /// This lives here rather than in either presenter because both platforms
    /// show the same thing, and a peek that said one thing on a Mac and another
    /// on a phone would be the drift this file exists to prevent.
    func footnotePeekContent(at off: UInt32, in view: DocView, theme: EditorTheme) -> FootnotePeekContent? {
        guard let note = footnotePeek(at: off) else { return nil }
        return FootnotePeekContent(note, rows: noteRows(note, in: view), theme: theme, doc: self)
    }

    /// The rows `note`'s body occupies in `view` — empty for a reference nothing
    /// defines, which is what makes the caller's "say why instead" path fire.
    ///
    /// A note is one row today: the rows are the unwrapped map's (one per
    /// block), and a definition is one block — a continuation line folds into
    /// it, and an indented paragraph after a blank line is not part of it at all
    /// but an indented *code block* beside it. Written as a range anyway, since
    /// the cost is a second `posForOffset` and the alternative is a silent
    /// half-note the day a note can hold two blocks.
    ///
    /// Slicing rather than re-rendering is what keeps the popover and the page
    /// from ever disagreeing about the same note.
    private func noteRows(_ note: FootnoteView, in view: DocView) -> [Row] {
        guard let start = note.offset, let end = note.end else { return [] }
        // `end` is exclusive, so the last byte *in* the note is what identifies
        // the last row. An empty body has no last byte and no rows to find.
        guard end > start else { return [] }
        let first = Int(posForOffset(off: start).row)
        let last = Int(posForOffset(off: end - 1).row)
        guard first <= last, view.rows.indices.contains(first), view.rows.indices.contains(last)
        else { return [] }
        return Array(view.rows[first...last])
    }

    /// Put the caret at `off` with nothing selected, and hand back the view to
    /// render — how both platforms carry out a jump.
    ///
    /// A collapsed selection rather than a selection of the note: arriving with
    /// the note highlighted would mean the reader's next keystroke replaced it.
    func caretMoved(to off: UInt32) -> DocView {
        setSelectionOffsets(anchor: off, focus: off)
    }
}
