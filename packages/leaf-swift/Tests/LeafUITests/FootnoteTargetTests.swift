//  FootnoteTargetTests.swift
//
//  What a footnote gesture can do, pinned here rather than through either
//  platform's menu — it is the one answer the AppKit context menu and the UIKit
//  edit menu share, and neither menu can be raised in a unit test.
//
//  Core already tests that `[^1]` resolves to its note; what these cover is the
//  direction-agnostic shape built on top: that one query answers for a reference
//  and the other for a definition, that a footnote with nowhere to go offers
//  nothing, and that a peek still reports the case a jump declines.

import XCTest
import LeafFFI
@testable import LeafUI

final class FootnoteTargetTests: XCTestCase {
    /// A doc over `source` with the caret parked at byte `offset`.
    private func doc(_ source: String, caret offset: UInt32) throws -> LeafDoc {
        let d = try LeafDoc(source: source, format: "markdown")
        _ = d.setSelectionOffsets(anchor: offset, focus: offset)
        return d
    }

    /// The byte offset of the first occurrence of `needle` in `source`.
    private func offset(of needle: String, in source: String) -> UInt32 {
        UInt32(source.range(of: needle).map {
            source.utf8.distance(from: source.utf8.startIndex, to: $0.lowerBound)
        } ?? 0)
    }

    private let cited = "A claim[^1] and more.\n\n[^1]: the note\n"

    func testReferenceLeadsToTheNote() throws {
        let d = try doc(cited, caret: offset(of: "^1]", in: cited))
        let jump = try XCTUnwrap(d.footnoteJumpAtCaret())
        XCTAssertEqual(jump.action, .goToNote)
        XCTAssertEqual(jump.label, "1")
        XCTAssertEqual(jump.offset, offset(of: "the note", in: cited),
                       "the note's first word, not the `[^1]:` marker")
    }

    /// The return leg — the half that makes following a footnote a round trip
    /// rather than a fall, since the notes sit at the foot of the document.
    func testNoteLeadsBackToTheReference() throws {
        let d = try doc(cited, caret: offset(of: "the note", in: cited))
        let jump = try XCTUnwrap(d.footnoteJumpAtCaret())
        XCTAssertEqual(jump.action, .backToReference)
        XCTAssertEqual(jump.label, "1")
        XCTAssertEqual(jump.offset, offset(of: "^1]", in: cited) + 1,
                       "the reference's label, the only part of it a caret fits on")
    }

    /// Down and back up, each leg found from the document rather than from a
    /// memory of the other — so it works for a reader who scrolled to the notes
    /// instead of jumping there, and stays right after an edit moves either end.
    ///
    /// Through `caretMoved`, which places a caret and so snaps it to a real
    /// stop: an offset naming a byte the caret can't occupy passes every test
    /// that reads it and still lands the reader in the wrong block.
    func testFollowingIsARoundTrip() throws {
        let d = try doc(cited, caret: offset(of: "^1]", in: cited))
        let down = try XCTUnwrap(d.footnoteJumpAtCaret())
        _ = d.caretMoved(to: down.offset)
        XCTAssertEqual(d.caretOffset(), down.offset, "the note is somewhere the caret fits")

        let up = try XCTUnwrap(d.footnoteJumpAtCaret())
        XCTAssertEqual(up.action, .backToReference)
        _ = d.caretMoved(to: up.offset)
        XCTAssertEqual(d.caretOffset(), up.offset, "and so is the reference")

        XCTAssertEqual(d.footnoteJumpAtCaret()?.action, .goToNote,
                       "back on the reference we started from")
    }

    /// The authoring gesture lands the author on the *near end of a working
    /// round trip*: press Footnote, and the caret is in the new note with the way
    /// back already on offer. Both halves have to exist for that — a reference
    /// with no definition offers no jump at all (see below) — so this is the one
    /// test that says the button wrote a whole footnote rather than half of one.
    func testInsertingAFootnoteLeavesTheCaretInTheNoteWithAWayBack() throws {
        let src = "A claim and more.\n"
        let d = try doc(src, caret: offset(of: " and", in: src))
        _ = d.insertFootnote()
        XCTAssertTrue(d.source().hasPrefix("A claim[^1] and more."), d.source())

        let back = try XCTUnwrap(d.footnoteJumpAtCaret(), "the caret is in the note it just made")
        XCTAssertEqual(back.action, .backToReference)
        XCTAssertEqual(back.label, "1")
        XCTAssertEqual(back.offset, offset(of: "^1]", in: d.source()) + 1)

        // …and typing there types into the note, not near it.
        _ = d.insert(text: "the note")
        XCTAssertEqual(d.footnoteAt(off: offset(of: "^1]", in: d.source()))?.text, "the note")
    }

    func testProseOffersNothing() throws {
        let d = try doc(cited, caret: 2)
        XCTAssertNil(d.footnoteJumpAtCaret())
        XCTAssertEqual(d.footnoteActionsAtCaret(), [])
    }

    /// A `[^99]` nothing defines has nowhere to go, so no menu offers a jump —
    /// the rule `linkActionsAtCaret` follows for an edit no host can carry out.
    func testUndefinedReferenceOffersNoJump() throws {
        let src = "A claim[^99] and more.\n"
        let d = try doc(src, caret: offset(of: "^99]", in: src))
        XCTAssertNil(d.footnoteJumpAtCaret())
        XCTAssertEqual(d.footnoteActionsAtCaret(), [])
    }

    /// …but the peek still reports it. It is the one place the state can be
    /// shown: the popover is already opening, and "nothing defines this" is a
    /// fact about the document, not a reason to say nothing.
    func testPeekReportsAnUndefinedReference() throws {
        let src = "A claim[^99] and more.\n"
        let d = try doc(src, caret: 0)
        let peek = try XCTUnwrap(d.footnotePeek(at: offset(of: "^99]", in: src)))
        XCTAssertEqual(peek.label, "99")
        XCTAssertNil(peek.text)

        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(
            at: offset(of: "^99]", in: src), in: view, theme: .default))
        XCTAssertFalse(content.isDefined, "leaf's sentence, not the document's words")
        XCTAssertTrue(content.body.string.contains("99"), "and it names the label it looked for")
    }

    // MARK: what the peek draws
    //
    // The note arrives rendered — the rows the document itself draws — rather
    // than as the source bytes `text` carries. A reader hovering `see *later*`
    // should get italics, not asterisks.

    func testPeekRendersTheNotesMarkupRatherThanItsSource() throws {
        let src = "Claim[^a].\n\n[^a]: see *emphasis* and `code` here.\n"
        let d = try doc(src, caret: offset(of: "^a]", in: src))
        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(
            at: offset(of: "^a]", in: src), in: view, theme: .default))

        XCTAssertTrue(content.isDefined)
        XCTAssertFalse(content.body.string.contains("*"), "the emphasis markers are resolved away")
        XCTAssertFalse(content.body.string.contains("`"), "and so are the code fences")
        XCTAssertTrue(content.body.string.contains("emphasis"))
        // The source answer deliberately still carries them, for a caller that
        // wants the note as written.
        XCTAssertEqual(d.footnotePeek(at: offset(of: "^a]", in: src))?.text,
                       "see *emphasis* and `code` here.")
    }

    /// The marker rides along with the rendered row, which is how the popover
    /// says *which* note answered without a caption of its own.
    func testPeekCarriesTheNotesMarker() throws {
        let src = "Claim[^a].\n\n[^a]: the note.\n"
        let d = try doc(src, caret: offset(of: "^a]", in: src))
        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(
            at: offset(of: "^a]", in: src), in: view, theme: .default))
        XCTAssertTrue(content.body.string.hasPrefix("[a]"), "got \(content.body.string)")
    }

    /// A note written across two source lines is one paragraph, and the peek
    /// shows it as one — the continuation's newline and indent are markup
    /// holding the note together, not text the reader wrote.
    ///
    /// This is the case the source-bytes answer reads worst on: `text` hands
    /// back the literal `"first line\n    continued line"`, so a popover built
    /// from it drew the note with a hard break and four spaces of indent that
    /// exist nowhere on the page.
    func testPeekFlowsANoteWrittenAcrossTwoLines() throws {
        let src = "Claim[^a].\n\n[^a]: first line\n    continued line\n"
        let d = try doc(src, caret: offset(of: "^a]", in: src))
        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(
            at: offset(of: "^a]", in: src), in: view, theme: .default))
        XCTAssertTrue(content.body.string.contains("first line continued line"),
                      "got \(content.body.string)")
        XCTAssertFalse(content.body.string.contains("\n"))
        XCTAssertEqual(d.footnotePeek(at: offset(of: "^a]", in: src))?.text,
                       "first line\n    continued line",
                       "while the source answer keeps the line as written")
    }

    /// A `[^1]:` with nothing after the colon is defined and says nothing. An
    /// empty popover would tell a reader less than a sentence does.
    func testPeekExplainsAnEmptyNote() throws {
        let src = "Claim[^a].\n\n[^a]:\n"
        let d = try doc(src, caret: offset(of: "^a]", in: src))
        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(
            at: offset(of: "^a]", in: src), in: view, theme: .default))
        XCTAssertFalse(content.isDefined)
        XCTAssertFalse(content.body.string.isEmpty)
    }

    func testPeekContentOverProseIsNil() throws {
        let d = try doc(cited, caret: 0)
        let view = d.setUnwrapped()
        XCTAssertNil(d.footnotePeekContent(at: 2, in: view, theme: .default))
    }

    // MARK: what the peek can follow
    //
    // A run says how a span *looks*, never what it means — `link` is a colour,
    // not a destination. These pin the mapping back through each run's source
    // offset, which is what makes a note's links and nested references clickable
    // in a popover that has no caret in it.

    /// The peek target for the first run tagged as followable, with the range it
    /// covers, or nil when nothing in the note leads anywhere.
    private func firstTarget(in content: FootnotePeekContent) -> (FootnotePeekTarget, NSRange)? {
        var found: (FootnotePeekTarget, NSRange)?
        content.body.enumerateAttribute(
            .footnoteTarget, in: NSRange(location: 0, length: content.body.length)
        ) { value, range, stop in
            if let target = value as? FootnotePeekTarget {
                found = (target, range)
                stop.pointee = true
            }
        }
        return found
    }

    func testALinkInsideANoteCarriesItsDestination() throws {
        let src = "Claim[^a].\n\n[^a]: see [the site](https://x.dev) for more.\n"
        let d = try doc(src, caret: offset(of: "^a]", in: src))
        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(
            at: offset(of: "^a]", in: src), in: view, theme: .default))

        let (target, range) = try XCTUnwrap(firstTarget(in: content))
        guard case .link(let destination) = target.kind else {
            return XCTFail("expected a link, got \(target.kind)")
        }
        XCTAssertEqual(destination, "https://x.dev")
        // …and it is tagged over the link's own words, not the whole note.
        XCTAssertEqual(content.body.attributedSubstring(from: range).string, "the site")
    }

    /// The reported symptom: hovering `[^2]` opened a popover showing notes 2
    /// *and* 3.
    ///
    /// Every note here ends in a link, which is what a real citation block looks
    /// like — and a note ending in a link has its last byte inside the hidden
    /// destination. `noteRows` used to map that byte through `posForOffset`,
    /// whose forward snap to the next visible glyph carried it off the note's
    /// own row and onto the next note's, so the slice took both. Notes ending in
    /// visible punctuation, which is what every other test here uses, were never
    /// affected — which is exactly why this survived so long.
    func testANoteEndingInALinkPeeksOnlyItself() throws {
        let src = """
            A[^1] B[^2] C[^3].

            [^1]: https://en.wikipedia.org/wiki/Moravec%27s_paradox

            [^2]: ["How to Get Startup Ideas," Nov 2012](https://www.paulgraham.com/startupideas.html)

            [^3]: [Alma 37:46](https://www.churchofjesuschrist.org/study/scriptures/bofm/alma/37?lang=eng&id=p46#p46)

            """
        let at = offset(of: "^2] C", in: src)
        let d = try doc(src, caret: at)
        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(at: at, in: view, theme: .default))

        XCTAssertTrue(content.isDefined)
        XCTAssertTrue(content.body.string.contains("How to Get Startup Ideas"),
                      "got \(content.body.string)")
        XCTAssertFalse(content.body.string.contains("Alma"),
                       "note 3 leaked into note 2's peek: \(content.body.string)")

        // And note 3, the last in the file, still peeks itself rather than
        // running off the end.
        let at3 = offset(of: "^3].", in: src)
        let third = try XCTUnwrap(d.footnotePeekContent(at: at3, in: view, theme: .default))
        XCTAssertTrue(third.body.string.contains("Alma"), "got \(third.body.string)")
        XCTAssertFalse(third.body.string.contains("Startup"), "got \(third.body.string)")
    }

    /// A reference inside a note is a footnote, not a link — even though it is
    /// drawn with the link role, which is how it gets its colour. Answering it
    /// as a link would send the reader out of the document.
    func testANestedReferenceIsAFootnoteTargetNotALink() throws {
        let src = "Claim[^a].\n\n[^a]: see also[^b].\n\n[^b]: the inner note.\n"
        let d = try doc(src, caret: offset(of: "^a]", in: src))
        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(
            at: offset(of: "^a]", in: src), in: view, theme: .default))

        let (target, _) = try XCTUnwrap(firstTarget(in: content))
        guard case .footnote(let offset) = target.kind else {
            return XCTFail("expected a footnote, got \(target.kind)")
        }
        // The offset names the nested reference, so following it resolves to the
        // note that reference points at.
        let jump = try XCTUnwrap(d.footnotePeek(at: offset))
        XCTAssertEqual(jump.label, "b")
        XCTAssertEqual(jump.text, "the inner note.")
    }

    /// Ordinary prose is not a door. A note with nothing followable in it must
    /// tag nothing, or the presenter underlines the whole thing.
    func testPlainNoteHasNoTargets() throws {
        let src = "Claim[^a].\n\n[^a]: just words, one of them *emphasised*.\n"
        let d = try doc(src, caret: offset(of: "^a]", in: src))
        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(
            at: offset(of: "^a]", in: src), in: view, theme: .default))
        XCTAssertNil(firstTarget(in: content))
    }

    /// The tag's range is computed in UTF-16 — what the attributed string is
    /// indexed in — so multi-byte prose ahead of a link must not shift it.
    func testATargetsRangeSurvivesMultibyteProse() throws {
        let src = "Claim[^a].\n\n[^a]: 日記 café [the site](https://x.dev).\n"
        let d = try doc(src, caret: offset(of: "^a]", in: src))
        let view = d.setUnwrapped()
        let content = try XCTUnwrap(d.footnotePeekContent(
            at: offset(of: "^a]", in: src), in: view, theme: .default))

        let (target, range) = try XCTUnwrap(firstTarget(in: content))
        guard case .link(let destination) = target.kind else {
            return XCTFail("expected a link, got \(target.kind)")
        }
        XCTAssertEqual(destination, "https://x.dev")
        XCTAssertEqual(content.body.attributedSubstring(from: range).string, "the site")
    }

    /// The whole reason the peek is offset-based: a hover must not drag the
    /// caret out of wherever the reader was typing.
    func testPeekLeavesTheCaretAlone() throws {
        let d = try doc(cited, caret: 0)
        let peek = try XCTUnwrap(d.footnotePeek(at: offset(of: "^1]", in: cited)))
        XCTAssertEqual(peek.text, "the note")
        XCTAssertEqual(d.caretOffset(), 0)
    }

    func testPeekOverProseIsNil() throws {
        let d = try doc(cited, caret: 0)
        XCTAssertNil(d.footnotePeek(at: 2))
    }

    /// A note nothing cites is somewhere a reader can stand, but not somewhere
    /// they can leave from — an orphan has no reference to return to.
    func testOrphanNoteOffersNoJump() throws {
        let src = "A claim[^1].\n\n[^1]: cited\n\n[^2]: orphan\n"
        let d = try doc(src, caret: offset(of: "orphan", in: src))
        XCTAssertNil(d.footnoteJumpAtCaret())
    }

    /// A footnote and a link are separate vocabularies, and the caret is only
    /// ever in one of them — which is what keeps ⌘-click unambiguous.
    func testAFootnoteIsNotALinkAndViceVersa() throws {
        let src = "a[^1] b [t](https://x.dev)\n\n[^1]: note\n"
        let d = try doc(src, caret: offset(of: "^1]", in: src))
        XCTAssertNotNil(d.footnoteJumpAtCaret())
        XCTAssertEqual(d.linkActionsAtCaret(wikilinks: true, canEdit: true, canPeek: false), [])

        _ = d.setSelectionOffsets(anchor: offset(of: "t](", in: src),
                                  focus: offset(of: "t](", in: src))
        XCTAssertNil(d.footnoteJumpAtCaret())
        XCTAssertEqual(d.linkActionsAtCaret(wikilinks: true, canEdit: true, canPeek: false), [.open, .edit, .copy])
    }

    /// `caretMoved` collapses the selection: arriving with the note highlighted
    /// would mean the reader's next keystroke replaced it.
    func testCaretMovedCollapsesTheSelection() throws {
        let d = try doc(cited, caret: 0)
        _ = d.setSelectionOffsets(anchor: 0, focus: 7)
        let target = offset(of: "the note", in: cited)
        let view = d.caretMoved(to: target)
        XCTAssertEqual(d.caretOffset(), target)
        XCTAssertFalse(view.hasSelection)
    }
}
