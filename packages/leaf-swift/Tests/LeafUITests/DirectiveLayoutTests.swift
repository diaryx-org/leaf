//  DirectiveLayoutTests.swift
//
//  Leaf directives a host draws: the layout collapses the directive's rows
//  onto one box as tall as the host's view, the caret and a click have the
//  picture's two homes, pages flow around the box, `::page-break` is never
//  offered, and a nil answer leaves the placeholder exactly as it was. Then the
//  host's half: views kept by what the directive says and placed over their
//  boxes. The catalogue that writes them is `DirectiveCatalogueTests`.

import XCTest
import LeafFFI
@testable import LeafUI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

final class DirectiveLayoutTests: XCTestCase {
    private let theme = EditorTheme.default

    private func embed(_ startRow: UInt32, _ endRow: UInt32, src: String = "https://example.org") -> DirectiveView {
        mkDirective("embed", startRow: startRow, endRow: endRow,
                    attrs: [DirectiveAttr(key: "src", value: src)])
    }

    private func placeholder(_ name: String = "embed") -> Row {
        row([mkRun("⧉ \(name)", role: "image")], directive: true)
    }

    /// Every host view `height` tall, and every question asked, in order.
    private func host(_ height: CGFloat?) -> (DirectiveMeasure, () -> [String]) {
        var asked: [String] = []
        return ({ d, _, _ in asked.append(d.name); return height }, { asked })
    }

    // MARK: collapsing

    func testTheRowsCollapseOntoOneBoxAsTallAsTheView() {
        // Core reserved three rows (a renderer that reports rows would have
        // asked for them); a proportional one lays them onto one box.
        let frame = docView([row([mkRun("before")]), placeholder(), row([]), row([]), row([mkRun("after")])],
                            directives: [embed(1, 4)])
        let (measure, _) = host(80)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, directives: measure)

        XCTAssertEqual(layout.rows.count, 5, "rows stay 1:1 with core's rows")
        let boxes = layout.rows.filter { $0.directive != nil }
        XCTAssertEqual(boxes.count, 3)
        XCTAssertEqual(boxes.filter(\.directiveFirst).count, 1)
        XCTAssertEqual(Set(boxes.map(\.directiveTop)).count, 1)
        XCTAssertEqual(boxes[0].height, 80 + DirectiveMetrics.gap * 2)
        XCTAssertEqual(boxes.dropFirst().map(\.height), [0, 0])
        XCTAssertEqual(layout.rows[4].top, boxes[0].top + boxes[0].height, accuracy: 0.5,
                       "prose resumes under the view")
        let rect = try? XCTUnwrap(boxes[0].directiveRect)
        XCTAssertEqual(rect?.height, 80)
        XCTAssertEqual(rect?.width, 400, "the view takes the column's width")
        XCTAssertFalse(boxes[0].isChromedDirective, "no dashed frame round the host's drawing")
    }

    func testANilAnswerLeavesThePlaceholder() {
        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)])
        let (measure, asked) = host(nil)
        let hosted = EditorLayout(frame, theme: theme, wrapWidth: 400, directives: measure)
        let plain = EditorLayout(frame, theme: theme, wrapWidth: 400)
        XCTAssertEqual(asked(), ["embed"])
        XCTAssertNil(hosted.rows[1].directive)
        XCTAssertEqual(hosted.rows.map(\.top), plain.rows.map(\.top))
        XCTAssertEqual(hosted.rows.map(\.height), plain.rows.map(\.height))
        XCTAssertTrue(hosted.rows[1].isChromedDirective, "framed as it always was")
    }

    func testAPageBreakIsNeverOffered() {
        let frame = docView([row([mkRun("a")]), pageBreakRow(), row([mkRun("b")])],
                            directives: [mkDirective("page-break", startRow: 1, endRow: 2)])
        let (measure, asked) = host(50)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, directives: measure)
        XCTAssertEqual(asked(), [])
        XCTAssertTrue(layout.rows[1].pageBreak)
        XCTAssertNil(layout.rows[1].directive)
    }

    func testTheSameDirectiveTwiceIsTwoViews() {
        let frame = docView([placeholder(), row([mkRun("between")]), placeholder()],
                            directives: [embed(0, 1), embed(2, 3)])
        let (measure, _) = host(40)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, directives: measure)
        let rects = layout.directiveRects()
        XCTAssertEqual(rects.count, 2, "one view cannot stand in two places")
        XCTAssertEqual(Set(rects.keys.map(\.occurrence)), [0, 1])
    }

    func testTheSourceViewAsksNothing() {
        let frame = docView([row([mkRun("::embed{src=x}")])], directives: [embed(0, 1)], view: "source")
        let (measure, asked) = host(40)
        _ = EditorLayout(frame, theme: theme, wrapWidth: 400, directives: measure)
        XCTAssertEqual(asked(), [])
    }

    // MARK: the caret's two homes

    func testTheCaretRidesTheViewsEdges() throws {
        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)])
        let (measure, _) = host(80)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, directives: measure)
        let box = try XCTUnwrap(layout.rows[1].directiveRect)
        let before = try XCTUnwrap(layout.rect(row: 1, ch: 0))
        let after = try XCTUnwrap(layout.rect(row: 1, ch: "⧉ embed".utf16.count))
        XCTAssertEqual(before.minX, box.minX)
        XCTAssertEqual(after.maxX, box.maxX, accuracy: 0.01)
        XCTAssertEqual(before.height, 80)
        XCTAssertEqual(after.minY, box.minY)
    }

    func testAClickTakesTheNearerHome() {
        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)])
        let (measure, _) = host(80)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, directives: measure)
        let box = layout.rows[1].directiveRect!
        XCTAssertEqual(layout.hit(CGPoint(x: box.midX, y: box.minY + 5)).row, 1)
        XCTAssertEqual(layout.hit(CGPoint(x: box.midX, y: box.minY + 5)).ch, 0)
        XCTAssertEqual(layout.hit(CGPoint(x: box.midX, y: box.maxY - 5)).ch, "⧉ embed".utf16.count)
    }

    func testAVerticalProbeFromEitherHomeLeavesTheBox() throws {
        // ↑/↓ probe a point past the caret's band. The caret is as tall as the
        // view; the band is the whole box, so the probe clears the breathing
        // room and lands on the rows either side rather than back on the view.
        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)])
        let (measure, _) = host(80)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, directives: measure)
        let band = try XCTUnwrap(layout.caretBand(src: 0, row: 1))
        XCTAssertEqual(band.minY, layout.rows[1].top)
        XCTAssertEqual(band.maxY, layout.rows[1].top + layout.rows[1].height)
        XCTAssertEqual(layout.hit(CGPoint(x: 20, y: band.maxY + 1)).row, 2)
        XCTAssertEqual(layout.hit(CGPoint(x: 20, y: band.minY - 1)).row, 0)
        XCTAssertNil(layout.caretBand(src: 0, row: 0), "an ordinary row keeps the caret's own band")
    }

    // MARK: pages

    func testPagesFlowAroundTheViewAsAroundAPicture() {
        // Six body lines to a sheet; four lines of prose leave too little room
        // for a 100pt view, which moves whole to the next sheet.
        let page = PageSetup(size: CGSize(width: 400, height: 200),
                             margins: LeafInsets(top: 20, left: 20, bottom: 20, right: 20),
                             gap: 20, backdrop: 24)
        let lines = (0..<4).map { row([mkRun("line \($0)")]) }
        let frame = docView(lines + [placeholder(), row([mkRun("after")])], directives: [embed(4, 5)])
        let (measure, _) = host(100)
        var cache: [Row: ShapedRow] = [:]
        let layout = EditorLayout(frame, theme: theme, viewWidth: 800, page: page, cache: &cache,
                                  directives: measure)
        let rl = layout.rows[4]
        XCTAssertEqual(rl.page, 1, "the view opens the next sheet")
        XCTAssertEqual(rl.top, page.contentTop(1), accuracy: 0.5)
        XCTAssertEqual(layout.rows[5].top, rl.top + rl.height, accuracy: 0.5)
    }

    // MARK: the host's views

    func testTheHostIsAskedOnceAndItsViewsFollowTheirBoxes() throws {
        let container = LeafView(frame: CGRect(x: 0, y: 0, width: 600, height: 600))
        let host = DirectiveHost()
        var asked = 0
        host.provider = { _ in asked += 1; return Card(height: 72) }

        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)])
        for _ in 0..<3 {
            let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, directives: host.measure)
            host.place(layout.directiveRects(), in: container)
        }
        XCTAssertEqual(asked, 1, "kept by what the directive says, across frames")
        let card = try XCTUnwrap(container.subviews.first as? Card)
        XCTAssertEqual(card.frame.height, 72, "as tall as its intrinsic size")
        XCTAssertEqual(card.frame.width, 400)

        // Edited away: the view goes with it.
        let gone = docView([row([mkRun("before")]), row([mkRun("after")])])
        let layout = EditorLayout(gone, theme: theme, wrapWidth: 400, directives: host.measure)
        host.place(layout.directiveRects(), in: container)
        XCTAssertTrue(container.subviews.isEmpty)
    }

    func testAViewWithNoIntrinsicHeightIsMeasuredAtTheColumnsWidth() {
        let view = LeafView(frame: CGRect(x: 0, y: 0, width: 10, height: 33))
        XCTAssertEqual(DirectiveHost.measure(view, width: 300), 33, "a view sized by hand keeps its height")
    }

    /// A host view with an intrinsic height and no opinion about its width.
    private final class Card: LeafView {
        let height: CGFloat
        init(height: CGFloat) {
            self.height = height
            super.init(frame: .zero)
        }
        required init?(coder: NSCoder) { fatalError() }
        override var intrinsicContentSize: CGSize {
            #if canImport(UIKit)
            CGSize(width: UIView.noIntrinsicMetric, height: height)
            #else
            CGSize(width: NSView.noIntrinsicMetric, height: height)
            #endif
        }
    }

    // MARK: on the surface

    #if canImport(AppKit) && !targetEnvironment(macCatalyst)
    func testTheSurfacePlacesTheHostsViewAndZoomsIt() throws {
        let doc = try LeafDoc(source: "Before.\n\n::embed{src=\"https://x.org\"}\n\nAfter.\n", format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 700, height: 600))
        scroll.documentView = view
        let window = NSWindow(contentRect: scroll.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = scroll
        scroll.layoutSubtreeIfNeeded()
        view.layout()

        var seen: [DirectiveView] = []
        view.directiveView = { d in seen.append(d); return Card(height: 90) }
        XCTAssertEqual(seen.map(\.name), ["embed"])
        XCTAssertEqual(seen.first?.attrs.first?.value, "https://x.org")
        let card = try XCTUnwrap(view.subviews.compactMap { $0 as? Card }.first)
        XCTAssertEqual(card.frame.height, 90)
        let row = try XCTUnwrap(view.layoutEngine.rows.first { $0.directiveFirst })
        XCTAssertEqual(card.frame, row.directiveRect)

        // At 2× the view's frame doubles and its bounds stay the layout's.
        view.zoom = .scale(2)
        XCTAssertEqual(card.frame.height, 180)
        XCTAssertEqual(card.bounds.height, 90)
        view.zoom = .actualSize

        // Taking the hook away takes the view with it.
        view.directiveView = nil
        XCTAssertTrue(view.subviews.compactMap { $0 as? Card }.isEmpty)
        XCTAssertNil(view.layoutEngine.rows.first { $0.directive != nil })
    }
    #endif
}
