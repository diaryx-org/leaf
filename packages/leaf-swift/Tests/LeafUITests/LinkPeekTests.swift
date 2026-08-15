//  LinkPeekTests.swift
//
//  What a *link* can do beyond being opened — the two things that gave a
//  destination finer granularity than the file it names.
//
//  Core already tests that `locate` finds the right block (`doc.rs`); what these
//  cover is the layer over it: that a bare `#v2` is recognized as this
//  document's business and not the host's, that a peek draws the block a locator
//  names rather than the document's opening, and that a locator naming nothing
//  produces silence rather than the wrong verse.

import XCTest
import LeafFFI
@testable import LeafUI

final class LinkPeekTests: XCTestCase {
    /// A chapter in the shape a vault of scripture is written in: one block per
    /// verse, each with an id a citation can name.
    private let chapter = """
        {#v1}
        I, Nephi, having been born of goodly parents.

        {#v2}
        Yea, I make a record in the language of my father.

        {#v3}
        And I know that the record which I make is true.
        """

    private func djot(_ source: String) throws -> LeafDoc {
        try LeafDoc(source: source, format: "djot")
    }

    // ── following a locator into this document ────────────────────────────────

    func testABareFragmentIsThisDocumentsBusiness() throws {
        let d = try djot(chapter)
        let landing = try XCTUnwrap(d.selfLanding(of: "#v2"),
                                    "a `#v2` names a place here, not somewhere to leave for")
        let source = d.source()
        let offset = source.utf8.distance(
            from: source.utf8.startIndex,
            to: try XCTUnwrap(source.range(of: "Yea, I make")).lowerBound)
        XCTAssertEqual(Int(landing), offset)
    }

    func testAFragmentNamingNothingIsNotClaimed() throws {
        let d = try djot(chapter)
        // Nil, not zero: claiming it would scroll the reader to the top of the
        // document and report success, which is worse than declining and letting
        // the host (or the system) have its say.
        XCTAssertNil(d.selfLanding(of: "#v99"))
    }

    func testADestinationThatIsNotAFragmentIsLeftAlone() throws {
        let d = try djot(chapter)
        for destination in ["./sibling.md", "/mosiah/mosiah-1.dj#v2", "https://diaryx.org#v2"] {
            XCTAssertNil(d.selfLanding(of: destination),
                         "`\(destination)` names another document, whatever follows its `#`")
        }
    }

    // ── peeking at what a link points at ──────────────────────────────────────

    func testAPeekDrawsTheVerseALocatorNames() throws {
        let peek = try XCTUnwrap(FootnotePeekContent(
            peeking: LinkPeekSource(source: chapter, format: "djot", locator: "v2"),
            theme: .default))
        XCTAssertTrue(peek.body.string.contains("Yea, I make a record"))
        XCTAssertFalse(peek.body.string.contains("having been born"),
                       "the block the locator names, not the document's opening")
        XCTAssertFalse(peek.body.string.contains("{#v2}"),
                       "the rendered rows, not the source's attribute markup")
    }

    func testAPeekWithNoLocatorDrawsTheDocumentsOpening() throws {
        let peek = try XCTUnwrap(FootnotePeekContent(
            peeking: LinkPeekSource(source: chapter, format: "djot"), theme: .default))
        XCTAssertTrue(peek.body.string.contains("having been born"))
    }

    func testALocatorNamingNothingShowsNothingAtAll() throws {
        // Nil rather than the first verse: a reader hovering a citation to verse
        // 99 asked about verse 99, and answering with verse 1 is a wrong answer
        // dressed as a right one.
        XCTAssertNil(FootnotePeekContent(
            peeking: LinkPeekSource(source: chapter, format: "djot", locator: "v99"),
            theme: .default))
    }

    func testAPeekAtAnEmptyDocumentIsNoPeek() throws {
        XCTAssertNil(FootnotePeekContent(
            peeking: LinkPeekSource(source: "   \n\n", format: "markdown"), theme: .default))
    }

    func testAMarkdownHeadingIsFoundByItsWords() throws {
        // The format most vaults are written in mints no ids at all, so a
        // `#the-second-part` can only be the heading's own words — and it has to
        // bring the section's body with it, or the peek shows a title and no text.
        let notes = "# Title\n\nintro\n\n## The Second Part\n\nthe body of it\n"
        let peek = try XCTUnwrap(FootnotePeekContent(
            peeking: LinkPeekSource(source: notes, format: "markdown", locator: "the-second-part"),
            theme: .default))
        XCTAssertTrue(peek.body.string.contains("The Second Part"))
        XCTAssertTrue(peek.body.string.contains("the body of it"))
        XCTAssertFalse(peek.body.string.contains("intro"))
    }

    // ── what a menu offers ────────────────────────────────────────────────────

    func testAPreviewIsOfferedForAFragmentEvenWithNoHostToAsk() throws {
        let source = "See [verse two](#v2) below.\n\n\(chapter)"
        let d = try djot(source)
        let caret = UInt32(source.utf8.distance(
            from: source.utf8.startIndex,
            to: try XCTUnwrap(source.range(of: "verse two")).lowerBound))
        _ = d.setSelectionOffsets(anchor: caret, focus: caret)
        // `canPeek: false` — no host hook at all, and the entry is still there,
        // because this destination is one the editor can answer on its own.
        XCTAssertEqual(d.linkActionsAtCaret(wikilinks: false, canEdit: false, canPeek: false),
                       [.peek, .open, .copy])
    }

    func testNoPreviewIsOfferedForAnOrdinaryLinkWithNoHostToAsk() throws {
        let source = "See [the chapter](./mosiah-1.dj) below.\n"
        let d = try djot(source)
        let caret = UInt32(source.utf8.distance(
            from: source.utf8.startIndex,
            to: try XCTUnwrap(source.range(of: "the chapter")).lowerBound))
        _ = d.setSelectionOffsets(anchor: caret, focus: caret)
        XCTAssertEqual(d.linkActionsAtCaret(wikilinks: false, canEdit: false, canPeek: false),
                       [.open, .copy], "nothing can read another file without a host")
        XCTAssertEqual(d.linkActionsAtCaret(wikilinks: false, canEdit: false, canPeek: true),
                       [.peek, .open, .copy])
    }
}
