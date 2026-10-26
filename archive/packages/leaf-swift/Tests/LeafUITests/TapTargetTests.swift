//  TapTargetTests.swift
//
//  Where a finger lands on a link, a highlight's wash and a highlight's margin
//  marker — the hit tests a touch view runs before it lets a tap be a caret
//  placement, pinned here because neither a tap nor the popover it raises can
//  be driven from a unit test. `FootnoteTargetTests` covers the footnote's.

import XCTest
import LeafFFI
@testable import LeafUI

final class TapTargetTests: XCTestCase {
    /// The byte offset of the first occurrence of `needle` in `source`.
    private func offset(of needle: String, in source: String) -> UInt32 {
        UInt32(source.range(of: needle).map {
            source.utf8.distance(from: source.utf8.startIndex, to: $0.lowerBound)
        } ?? 0)
    }

    /// A doc over `source`, laid out the way a view lays it out.
    private func laidOut(_ source: String) throws -> (LeafDoc, EditorLayout) {
        let d = try LeafDoc(source: source, format: "markdown")
        _ = d.setRaisedSuperscripts(on: true)
        let view = d.setUnwrapped()
        return (d, EditorLayout(view, theme: .default, wrapWidth: 400))
    }

    /// The glyphs of the source range `[start, end)`, on its first line.
    private func glyphs(_ start: UInt32, _ end: UInt32, in d: LeafDoc,
                        _ layout: EditorLayout) throws -> CGRect {
        let a = d.posForOffset(off: start), b = d.posForOffset(off: end)
        return try XCTUnwrap(layout.rangeRects(from: (Int(a.row), Int(a.ch)),
                                               to: (Int(b.row), Int(b.ch))).first?.rect)
    }

    // MARK: links

    private let prose = """
        A first line of plain prose to sit above the link.

        Read [a](https://example.com/a) word here.

        A last line of plain prose to sit below the link.

        """

    /// A one-letter link answers a finger well above and below its glyphs —
    /// to `linkTargetHeight` — and only a few points either side, where the
    /// words of its own sentence are.
    func testAShortLinkAnswersAboveAndBelowButNotBeside() throws {
        let (d, layout) = try laidOut(prose)
        let start = offset(of: "a](", in: prose)
        let rect = try glyphs(start, start + 1, in: d, layout)
        XCTAssertLessThan(rect.height, EditorLayout.linkTargetHeight, "a line is shorter than a target")

        let onIt = try XCTUnwrap(layout.link(at: CGPoint(x: rect.midX, y: rect.midY)))
        XCTAssertEqual(onIt.rect, rect, "anchored on the glyphs, not on the grown target")
        XCTAssertNil(onIt.wikilink)
        XCTAssertEqual(d.destination(of: onIt, wikilinks: false), "https://example.com/a")

        let reach = EditorLayout.linkTargetHeight / 2 - 1
        XCTAssertNotNil(layout.link(at: CGPoint(x: rect.midX, y: rect.midY - reach)), "above")
        XCTAssertNotNil(layout.link(at: CGPoint(x: rect.midX, y: rect.midY + reach)), "below")
        XCTAssertNil(layout.link(at: CGPoint(x: rect.midX, y: rect.midY + reach + 2)))

        XCTAssertNotNil(layout.link(at: CGPoint(x: rect.maxX + EditorLayout.linkSlop - 1, y: rect.midY)))
        XCTAssertNil(layout.link(at: CGPoint(x: rect.maxX + EditorLayout.linkSlop + 2, y: rect.midY)),
                     "the next word is the sentence's, not the link's")
    }

    /// Two links on neighbouring lines both reach into the gap between them;
    /// the one whose glyphs are nearer wins, and a point on a link's own
    /// glyphs is that link's whatever the other's padding covers.
    func testTheNearerOfTwoStackedLinksWins() throws {
        let src = "[up](https://example.com/up)\n\n[down](https://example.com/down)\n"
        let (d, layout) = try laidOut(src)
        let up = try glyphs(offset(of: "up]", in: src), offset(of: "up]", in: src) + 2, in: d, layout)
        let down = try glyphs(offset(of: "down]", in: src), offset(of: "down]", in: src) + 4, in: d, layout)
        let x = min(up.maxX, down.maxX) / 2
        let nearUp = try XCTUnwrap(layout.link(at: CGPoint(x: x, y: up.maxY + 1)))
        XCTAssertEqual(d.destination(of: nearUp, wikilinks: false), "https://example.com/up")
        let onDown = try XCTUnwrap(layout.link(at: CGPoint(x: x, y: down.midY)))
        XCTAssertEqual(d.destination(of: onDown, wikilinks: false), "https://example.com/down")
    }

    /// A wikilink is no run the layout can see; it answers when the caller
    /// hands it in, and its destination is the construct itself — or nothing,
    /// with the switch off.
    func testAWikilinkAnswersThroughItsSpan() throws {
        let src = "see [[notes/a.md]] for more\n"
        let (d, layout) = try laidOut(src)
        let spans = d.wikilinkSpans()
        XCTAssertEqual(spans.map(\.text), ["[[notes/a.md]]"])
        let span = try XCTUnwrap(spans.first)
        let rect = try glyphs(span.start, span.end, in: d, layout)
        XCTAssertNil(layout.link(at: CGPoint(x: rect.midX, y: rect.midY)), "not a link to the layout")

        let a = d.posForOffset(off: span.start), b = d.posForOffset(off: span.end)
        let extra = [LinkSpan(src: span.start, wikilink: span.text,
                              from: (Int(a.row), Int(a.ch)), to: (Int(b.row), Int(b.ch)))]
        let tap = try XCTUnwrap(layout.link(at: CGPoint(x: rect.midX, y: rect.midY), also: extra))
        XCTAssertEqual(d.destination(of: tap, wikilinks: true), "[[notes/a.md]]")
        XCTAssertNil(d.destination(of: tap, wikilinks: false))
    }

    /// The scan agrees with the caret's rule: each opener to the first closer
    /// on its line, nested openers innermost first, blanks and line breaks
    /// naming nothing.
    func testWikilinkSpansFollowTheCaretsRule() throws {
        let src = "[[a]] and [[x [[b]] and [[ ]] and [[open\nclose]]"
        let d = try LeafDoc(source: src, format: "markdown")
        XCTAssertEqual(d.wikilinkSpans().map(\.text), ["[[b]]", "[[x [[b]]", "[[a]]"])
        let inner = try XCTUnwrap(d.wikilinkSpans().first)
        XCTAssertEqual(inner.start, offset(of: "[[b]]", in: src))
        XCTAssertEqual(inner.end, offset(of: "[[b]]", in: src) + 5)
    }

    // MARK: a highlight's wash

    /// `glyph(at:)` answers only on ink: a point on a character names it, and
    /// one past the end of the line — where `hit` still finds a caret stop —
    /// names nothing.
    func testAGlyphIsFoundOnlyUnderTheFinger() throws {
        let src = "The quick brown fox.\n"
        let (d, layout) = try laidOut(src)
        let q = offset(of: "quick", in: src)
        let rect = try glyphs(q, q + 1, in: d, layout)
        let found = try XCTUnwrap(layout.glyph(at: CGPoint(x: rect.midX, y: rect.midY)))
        XCTAssertEqual(d.offsetForPos(row: UInt32(found.row), ch: UInt32(found.ch)), q)
        // The right half of a glyph is nearest the stop after it, and is
        // still the glyph's.
        let right = try XCTUnwrap(layout.glyph(at: CGPoint(x: rect.maxX - 0.5, y: rect.midY)))
        XCTAssertEqual(d.offsetForPos(row: UInt32(right.row), ch: UInt32(right.ch)), q)

        let end = try glyphs(offset(of: ".", in: src), offset(of: ".", in: src) + 1, in: d, layout)
        XCTAssertNil(layout.glyph(at: CGPoint(x: end.maxX + 40, y: end.midY)), "past the line's end")
    }

    /// A tap on a wash finds its highlight through the glyph's offset — the
    /// question core answers for painting too — and a tap beside it does not.
    func testAWashIsFoundByTheGlyphUnderIt() throws {
        let src = "The quick brown fox jumps.\n"
        let (d, layout) = try laidOut(src)
        let start = offset(of: "brown", in: src)
        _ = d.setHighlights(highlights: [
            Highlight(start: UInt64(start), end: UInt64(start + 5), id: "h", color: nil, marker: nil)
        ])
        func washed(_ point: CGPoint) -> String? {
            layout.glyph(at: point).flatMap {
                d.highlightAt(offset: d.offsetForPos(row: UInt32($0.row), ch: UInt32($0.ch)))
            }
        }
        let on = try glyphs(start + 2, start + 3, in: d, layout)
        XCTAssertEqual(washed(CGPoint(x: on.midX, y: on.midY)), "h")
        let beside = try glyphs(offset(of: "fox", in: src), offset(of: "fox", in: src) + 1, in: d, layout)
        XCTAssertNil(washed(CGPoint(x: beside.midX, y: beside.midY)))
    }

    // MARK: a highlight's margin marker

    /// The marker answers over a 44-point square, all of it inside the view:
    /// centred on the glyph, then slid back from the trailing edge.
    func testAMarkersTargetIsAFingertipAndOnScreen() {
        let width: CGFloat = 402
        let frame = EditorLayout.markerFrame(lineMidY: 300, trailingEdge: width)
        XCTAssertEqual(frame.maxX, width - EditorLayout.markerInset)
        XCTAssertEqual(frame.midY, 300)
        let target = EditorLayout.markerTarget(frame, trailingEdge: width)
        XCTAssertEqual(target.width, 44)
        XCTAssertEqual(target.height, 44)
        XCTAssertEqual(target.maxX, width, "slid back inside the edge, not hung off it")
        XCTAssertEqual(target.midY, frame.midY)
        XCTAssertTrue(target.contains(frame))

        // A glyph clear of the edge keeps a target centred on it.
        let inner = CGRect(x: 100, y: 100, width: 20, height: 20)
        XCTAssertEqual(EditorLayout.markerTarget(inner, trailingEdge: width),
                       CGRect(x: 88, y: 88, width: 44, height: 44))
    }

    /// Two marked lines one above the other overlap; the nearer marker wins.
    func testTheNearerOfTwoMarkersWins() {
        let width: CGFloat = 402
        let a = EditorLayout.markerTarget(EditorLayout.markerFrame(lineMidY: 300, trailingEdge: width),
                                          trailingEdge: width)
        let b = EditorLayout.markerTarget(EditorLayout.markerFrame(lineMidY: 325, trailingEdge: width),
                                          trailingEdge: width)
        let hits = [(rect: a, id: "a"), (rect: b, id: "b")]
        XCTAssertEqual(EditorLayout.marker(at: CGPoint(x: 390, y: 310), in: hits), "a")
        XCTAssertEqual(EditorLayout.marker(at: CGPoint(x: 390, y: 316), in: hits), "b")
        XCTAssertNil(EditorLayout.marker(at: CGPoint(x: 340, y: 300), in: hits), "left of the square")
    }
}
