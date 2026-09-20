//  EditorCountsTests.swift
//
//  The statistics `LeafEditorModel` publishes for a word-count pill and a
//  panel: that they cost nothing until a host asks, that they are the *text's*
//  numbers and not the markup's, that a selection narrows them and clearing it
//  widens them back, and that a page count is the surface's to answer.
//
//  The expected tallies are derived from `leaf_core::counts`' rules rather than
//  copied off a run: a glyph counts when a reader would call it text, so a
//  `**` is no character, a bullet is no character, and a block holding any
//  visible character is one paragraph.

import XCTest
import LeafFFI
@testable import LeafUI

final class EditorCountsTests: XCTestCase {
    /// `a **bold** word` is one paragraph reading `a bold word` — 11 characters
    /// (9 without the two spaces), three words — and `- item` is a second,
    /// reading `item`: the bullet is furniture the renderer drew, so it is no
    /// character of anyone's. Four words, fifteen characters, thirteen without
    /// spaces, two paragraphs.
    private let source = "a **bold** word\n\n- item\n"
    private let whole = TextCounts(words: 4, characters: 15,
                                   charactersWithoutSpaces: 13, paragraphs: 2)

    /// Let the model's deferred publish land and, past the debounce, a
    /// scheduled recount with it. Both are on the main queue, so waiting on a
    /// block enqueued behind them is enough — see `LeafEditorModel.publishCounts`.
    private func settle(_ seconds: TimeInterval = 0.4) {
        let done = expectation(description: "the main queue catches up")
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { done.fulfill() }
        wait(for: [done], timeout: seconds + 2)
    }

    /// Nothing is counted until a host says it wants the numbers. A model that
    /// tallied on `init` would spend a reparse and a layout of the whole
    /// document on every window that has nowhere to show one.
    func testNothingIsCountedUntilItIsAskedFor() throws {
        let model = try LeafEditorModel(source: source)
        XCTAssertFalse(model.countsEnabled)
        XCTAssertNil(model.counts)
        settle()
        XCTAssertNil(model.counts, "still nothing, with no view and no request")
    }

    /// And a model with no view on it yet counts anyway: `document` and
    /// `selection` are the document's, so they hold from the moment a model
    /// exists — which is where a host asks, one line after `init`.
    func testEnablingCountsAViewLessDocumentAtOnce() throws {
        let model = try LeafEditorModel(source: source)
        model.countsEnabled = true
        settle()

        let counts = try XCTUnwrap(model.counts)
        XCTAssertEqual(counts.document, whole)
        XCTAssertNil(counts.selection, "nothing is selected")
        XCTAssertNil(counts.pages, "no view has laid anything onto paper")
    }

    /// A selection narrows the same rules to what it covers. Source bytes 0..<10
    /// are `a **bold**`, whose visible glyphs are `a bold`: two words, six
    /// characters, five without the space, and the one paragraph it clips.
    func testASelectionIsCountedBesideTheDocument() throws {
        let model = try LeafEditorModel(source: source)
        model.countsEnabled = true
        model.debugSelect(anchor: 0, focus: 10)
        settle()

        let counts = try XCTUnwrap(model.counts)
        XCTAssertEqual(counts.document, whole, "the document's numbers do not move")
        XCTAssertEqual(counts.selection,
                       TextCounts(words: 2, characters: 6,
                                  charactersWithoutSpaces: 5, paragraphs: 1))
    }

    /// An empty selection is no selection, so clearing one puts `selection`
    /// back to nil rather than to a row of zeroes — which is what lets a panel
    /// switch back to the document's numbers by asking whether it is there.
    func testClearingTheSelectionClearsItsCounts() throws {
        let model = try LeafEditorModel(source: source)
        model.countsEnabled = true
        model.debugSelect(anchor: 0, focus: 10)
        settle()
        XCTAssertNotNil(model.counts?.selection)

        model.debugSelect(anchor: 5, focus: 5)
        settle()
        XCTAssertNil(model.counts?.selection)
        XCTAssertEqual(model.counts?.document, whole)
    }

    /// Turning them off is not "stop updating" but "there are none": a host
    /// that closed its panel should not be handed a stale tally the next time
    /// it looks.
    func testDisablingDropsTheCounts() throws {
        let model = try LeafEditorModel(source: source)
        model.countsEnabled = true
        settle()
        XCTAssertNotNil(model.counts)

        model.countsEnabled = false
        settle()
        XCTAssertNil(model.counts)
    }

    // ── the surface's half ────────────────────────────────────────────────────
    // Both toolkits lay a document onto sheets and both report them, so these
    // two run on either — `CGRect` and `PageSetup` are the same thing on each,
    // and the page count is read off the layout rather than off an `#if`.

    /// On paper the sheets are counted too, off the layout that drew them — so
    /// a break the author wrote counts like one the text ran into.
    func testPaperReportsItsSheets() throws {
        let paragraphs = (1...120).map {
            "Paragraph \($0) of a long document, long enough to wrap onto more than one line at the sheet's measure."
        }
        let model = try LeafEditorModel(source: paragraphs.joined(separator: "\n\n") + "\n")
        let view = LeafTextView(doc: model.doc, theme: .default)
        view.frame = CGRect(x: 0, y: 0, width: 612, height: 792)
        view.pageSetup = PageSetup(size: CGSize(width: 612, height: 792),
                                   margins: PageSetup.inch, gap: 0, backdrop: 0)
        model.textView = view

        model.countsEnabled = true
        settle()
        XCTAssertGreaterThan(view.pages.count, 1, "the fixture has to overrun one sheet")
        XCTAssertEqual(model.counts?.pages, view.pages.count)
    }

    /// And the continuous flow answers nil rather than one: a scrolling column
    /// is not a page, and "1 page" over a document that would print on nine is
    /// worse than saying nothing at all.
    func testTheContinuousFlowHasNoPageCount() throws {
        let model = try LeafEditorModel(source: source)
        let view = LeafTextView(doc: model.doc, theme: .default)
        view.frame = CGRect(x: 0, y: 0, width: 612, height: 792)
        model.textView = view

        model.countsEnabled = true
        settle()
        XCTAssertNotNil(model.counts, "the document's own numbers are still there")
        XCTAssertNil(model.counts?.pages)
    }
}
