//  LinkPeek.swift
//
//  What a pointer resting on a *link* shows — the other half of the hover
//  `FootnotePeek.swift` already gives a footnote reference.
//
//  A footnote and a link ask the same question and used to get different
//  answers. `[^1a]` says "what does that note say" and gets the note; `[Mosiah
//  1:2](/mosiah/mosiah-1.dj#v2)` says "what does that verse say" and got a
//  filename to click, a document to open, and a place in it to go and find. The
//  difference was never the reader's question, only where the answer happened to
//  live: a note is in this document and a verse is in another one.
//
//  Which is the whole of the seam here. leaf cannot read the other file — it has
//  no vault, no resolver, and no business opening one (see `LinkPeekSource`) —
//  so the host fetches, and everything after that is the footnote path exactly:
//  the bytes become a document, the document lays itself out, and the rows the
//  locator names are sliced out and drawn in the editor's own fonts. There is no
//  second renderer here and no HTML round trip, for `FootnotePeekContent`'s
//  reasons.

import Foundation
import LeafFFI

/// A document a link names, as the host hands it back — what
/// `LeafEditorModel.onPeekLink` answers with.
///
/// The *body* the editor would open, not the file on disk: a vault document
/// begins with metadata that the editor hides and a reader peeking at a verse
/// has no use for, and the host is the only one that knows where the metadata
/// stops. It is the same text the host would pass to `LeafEditorModel(source:)`
/// if the reader opened the document outright, which is the point — a peek that
/// showed something other than the document does is a peek that lies.
public struct LinkPeekSource {
    /// The document's body.
    public let source: String
    /// Its markup (`"markdown"`, `"djot"`, `"html"`, `"xml"`) — the target's
    /// own, which need not be the open document's: one vault holds both.
    public let format: String
    /// The place inside `source` the destination named — the `v2` of a
    /// `…/mosiah-1.dj#v2`, `#` stripped — or nil to show the document's opening.
    ///
    /// Split by the host rather than by leaf, because the host is what defines
    /// the spelling: which `#` starts a locator is bound up with how the rest of
    /// the destination is decoded (a `%23` is a `#` in a filename, not a
    /// separator), and a second copy of that rule here would answer differently
    /// on exactly the destinations that are hard.
    public let locator: String?

    public init(source: String, format: String, locator: String? = nil) {
        self.source = source
        self.format = format
        self.locator = locator
    }
}

extension FootnotePeekContent {
    /// The block `document`'s locator names, rendered — or nil when there is
    /// nothing worth showing.
    ///
    /// Nil rather than a sentence, which is where this parts company with the
    /// footnote peek. "No note defined for `[^99]`" is a fact about the document
    /// the reader is *in*, worth interrupting them with; "the file you are
    /// pointing at has no `v20`" is a fact about a file they have not opened, and
    /// a popover that appears to say so would fire on every hover over every
    /// ordinary link. Silence is the honest answer, and following the link still
    /// takes them to the document's top.
    ///
    /// Nothing in it is followable, deliberately. A `./sibling.md` inside another
    /// document is relative to *that* document, and every route out of here — the
    /// host's `onOpenLink`, the peek's own targets — resolves against the one on
    /// screen. Better a peek that only reads than one whose links quietly land in
    /// the wrong directory.
    init?(peeking document: LinkPeekSource, theme: EditorTheme) {
        guard let doc = try? LeafDoc(source: document.source, format: document.format) else {
            return nil
        }
        // Unwrapped, like the on-screen editor: one row per block, which the
        // popover then wraps at its own width. Wrapping here would wrap to a
        // column count that has nothing to do with how wide the popover is.
        let view = doc.setUnwrapped()
        self.init(rows: Self.rows(of: view, in: doc, at: document.locator), theme: theme)
    }

    /// The block `locator` names in the document already on screen — a `#v2`,
    /// which points *into* the page the reader is looking at.
    ///
    /// Sliced out of the frame being drawn rather than re-parsed from the source,
    /// which is both cheaper and the only way the popover and the page under it
    /// are guaranteed to agree about the same block. The footnote peek is built
    /// this way for the same reason; the initializer above has no such luxury,
    /// because the document it draws is not the one on screen.
    init?(peeking locator: String, of doc: LeafDoc, in view: DocView, theme: EditorTheme) {
        self.init(rows: Self.rows(of: view, in: doc, at: locator), theme: theme)
    }

    private init?(rows: [Row], theme: EditorTheme) {
        let drawn = NSMutableAttributedString()
        for row in rows where !row.decoration {
            if drawn.length > 0 { drawn.append(NSAttributedString(string: "\n")) }
            drawn.append(AttributedRow.make(row, theme: theme))
        }
        guard !drawn.string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return nil
        }
        body = drawn
        isDefined = true
    }

    /// The rows to draw: the block `locator` names, or the document's opening
    /// when it names nothing in particular.
    ///
    /// Capped, because a locator may name a *section* — a heading and everything
    /// under it, which is the right span for "show me that part" and can be the
    /// rest of the file. The popover truncates to a few lines anyway; the cap is
    /// so that a peek at a long chapter doesn't render a thousand rows into an
    /// attributed string to show eight of them.
    private static func rows(of view: DocView, in doc: LeafDoc, at locator: String?) -> [Row] {
        guard let locator else { return Array(view.rows.prefix(maxRows)) }
        // A locator that names nothing: no rows, so the caller shows nothing —
        // better than the document's opening, which would answer a question about
        // verse 20 with verse 1.
        guard let landing = doc.landing(for: locator), landing.end > landing.start else {
            return []
        }
        // Core says which rows the block covers — `FootnotePeekContent`'s
        // question, for its reason: a block ending in a link ends inside the
        // link's hidden destination, and mapping that last byte through
        // `posForOffset` snaps forward onto the block below.
        let span = doc.rowRangeFor(start: landing.start, end: landing.end)
        let first = Int(span.first), last = Int(span.last)
        guard first <= last, view.rows.indices.contains(first), view.rows.indices.contains(last)
        else { return [] }
        return Array(view.rows[first...min(last, first + maxRows - 1)])
    }

    /// Twice what the popover will draw, so a wrapping row or a blank one between
    /// blocks can't cost the reader a visible line.
    private static let maxRows = 16
}

extension LeafDoc {
    /// Where `locator` lands in this document, trying the spelling as written
    /// before the percent-decoded one.
    ///
    /// Literal first because both readings are legitimate: a fragment with a
    /// space in it is conventionally written `#My%20Heading`, and `%` is equally
    /// a legal character in an id someone declared outright. The document's own
    /// answer wins over the guess.
    func landing(for locator: String) -> LandingView? {
        if let landing = locate(id: locator) { return landing }
        guard let decoded = locator.removingPercentEncoding, decoded != locator else { return nil }
        return locate(id: decoded)
    }
}
