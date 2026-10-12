//  DirectiveLayoutTests.swift
//
//  Leaf directives a host draws: each is a region, `directive:<name>`, drawn
//  as its renderer's picture. The layout collapses the directive's rows onto
//  one box as tall as the picture, the caret and a click have the picture's
//  two homes, pages flow around the box, `::page-break` is never offered, and
//  a nil answer leaves the placeholder exactly as it was. Then the view's
//  half: the renderer asked once per directive, and an editor that writes the
//  directive's source. The catalogue that writes them is
//  `DirectiveCatalogueTests`.

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

    private let src = DirectiveAttr(key: "src", value: "https://example.org")

    private func embed(_ startRow: UInt32, _ endRow: UInt32) -> DirectiveView {
        mkDirective("embed", startRow: startRow, endRow: endRow, attrs: [src])
    }

    /// The region core publishes for that directive once the view owns
    /// `directive:embed`.
    private func region(_ startRow: UInt32, _ endRow: UInt32, at: UInt32 = 0) -> RegionView {
        let source = "::embed{src=\"https://example.org\"}"
        return RegionView(kind: "directive:embed", inline: false, display: false,
                          name: "embed", label: "", attrs: [src],
                          startRow: startRow, endRow: endRow, src: at, end: at + UInt32(source.utf8.count),
                          contentStart: at, contentEnd: at + UInt32(source.utf8.count), content: source)
    }

    private func placeholder(_ name: String = "embed") -> Row {
        row([mkRun("⧉ \(name)", role: "image")], directive: true)
    }

    /// Pictures `height` tall and as wide as the column, and every directive
    /// the renderer was asked to draw, by name, in order.
    private func renderer(_ height: CGFloat?) -> (RegionDrawing, () -> [String]) {
        var asked: [String] = []
        let pictures = RegionPictures()
        pictures.renderers = ["directive:embed": { r, _ in
            asked.append(r.name)
            return height.map { RegionPicture(width: nil, height: $0) { _, _ in } }
        }]
        return (RegionDrawing(pictures: pictures), { asked })
    }

    // MARK: collapsing

    func testTheRowsCollapseOntoOneBoxAsTallAsThePicture() {
        // Core reserved three rows (a renderer that reports rows would have
        // asked for them); a proportional one lays them onto one box.
        let frame = docView([row([mkRun("before")]), placeholder(), row([]), row([]), row([mkRun("after")])],
                            directives: [embed(1, 4)], regions: [region(1, 4)])
        let (drawing, _) = renderer(80)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, drawing: drawing)

        XCTAssertEqual(layout.rows.count, 5, "rows stay 1:1 with core's rows")
        let boxes = layout.rows.filter { $0.picture != nil }
        XCTAssertEqual(boxes.count, 3)
        XCTAssertEqual(boxes.filter(\.pictureFirst).count, 1)
        XCTAssertEqual(Set(boxes.map(\.pictureTop)).count, 1)
        XCTAssertEqual(boxes[0].height, 80 + MediaMetrics.gap * 2)
        XCTAssertEqual(boxes.dropFirst().map(\.height), [0, 0])
        XCTAssertEqual(layout.rows[4].top, boxes[0].top + boxes[0].height, accuracy: 0.5,
                       "prose resumes under the picture")
        let rect = layout.pictureRects()["directive:embed@0"]
        XCTAssertEqual(rect?.height, 80)
        XCTAssertEqual(rect?.width, 400, "a picture with no width takes the column's")
        XCTAssertFalse(boxes[0].isChromedDirective, "no dashed frame round the host's drawing")
    }

    func testANilAnswerLeavesThePlaceholder() {
        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)], regions: [region(1, 2)])
        let (drawing, asked) = renderer(nil)
        let drawn = EditorLayout(frame, theme: theme, wrapWidth: 400, drawing: drawing)
        let plain = EditorLayout(frame, theme: theme, wrapWidth: 400)
        XCTAssertEqual(asked(), ["embed"])
        XCTAssertNil(drawn.rows[1].picture)
        XCTAssertEqual(drawn.rows.map(\.top), plain.rows.map(\.top))
        XCTAssertEqual(drawn.rows.map(\.height), plain.rows.map(\.height))
        XCTAssertTrue(drawn.rows[1].isChromedDirective, "framed as it always was")
    }

    func testAPageBreakIsNeverOffered() {
        XCTAssertFalse(RegionKinds.hostable("directive:page-break"))
        XCTAssertFalse(RegionKinds.editable("directive:page-break"))
        XCTAssertTrue(RegionKinds.hostable("directive:embed"))
        XCTAssertFalse(RegionKinds.hostable("directive:"))
        // Core publishes no region for one, so nothing draws over it.
        let frame = docView([row([mkRun("a")]), pageBreakRow(), row([mkRun("b")])],
                            directives: [mkDirective("page-break", startRow: 1, endRow: 2)])
        let (drawing, asked) = renderer(50)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, drawing: drawing)
        XCTAssertEqual(asked(), [])
        XCTAssertTrue(layout.rows[1].pageBreak)
        XCTAssertNil(layout.rows[1].picture)
    }

    func testTheSameDirectiveTwiceIsTwoPicturesDrawnOnce() {
        let frame = docView([placeholder(), row([mkRun("between")]), placeholder()],
                            directives: [embed(0, 1), embed(2, 3)],
                            regions: [region(0, 1, at: 0), region(2, 3, at: 50)])
        let (drawing, asked) = renderer(40)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, drawing: drawing)
        XCTAssertEqual(layout.pictureRects().count, 2, "a picture stands in both places")
        XCTAssertEqual(asked(), ["embed"], "the same content is drawn once")
    }

    func testTheSourceViewAsksNothing() {
        let frame = docView([row([mkRun("::embed{src=x}")])], directives: [embed(0, 1)],
                            regions: [region(0, 1)], view: "source")
        let (drawing, asked) = renderer(40)
        _ = EditorLayout(frame, theme: theme, wrapWidth: 400, drawing: drawing)
        XCTAssertEqual(asked(), [])
    }

    // MARK: the caret's two homes

    func testTheCaretRidesThePicturesEdges() throws {
        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)], regions: [region(1, 2)])
        let (drawing, _) = renderer(80)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, drawing: drawing)
        let box = try XCTUnwrap(layout.pictureRects().values.first)
        let before = try XCTUnwrap(layout.rect(row: 1, ch: 0))
        let after = try XCTUnwrap(layout.rect(row: 1, ch: "⧉ embed".utf16.count))
        XCTAssertEqual(before.minX, box.minX)
        XCTAssertEqual(after.maxX, box.maxX, accuracy: 0.01)
        XCTAssertEqual(before.height, 80)
        XCTAssertEqual(after.minY, box.minY)
    }

    func testAClickTakesTheNearerHome() throws {
        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)], regions: [region(1, 2)])
        let (drawing, _) = renderer(80)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, drawing: drawing)
        let box = try XCTUnwrap(layout.pictureRects().values.first)
        XCTAssertEqual(layout.hit(CGPoint(x: box.midX, y: box.minY + 5)).row, 1)
        XCTAssertEqual(layout.hit(CGPoint(x: box.midX, y: box.minY + 5)).ch, 0)
        XCTAssertEqual(layout.hit(CGPoint(x: box.midX, y: box.maxY - 5)).ch, "⧉ embed".utf16.count)
    }

    func testAVerticalProbeFromEitherHomeLeavesTheBox() throws {
        // ↑/↓ probe a point past the caret's band. The caret is as tall as the
        // picture; the band is the whole box, so the probe clears the
        // breathing room and lands on the rows either side rather than back on
        // the picture.
        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)], regions: [region(1, 2)])
        let (drawing, _) = renderer(80)
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400, drawing: drawing)
        let band = try XCTUnwrap(layout.caretBand(src: 0, row: 1))
        XCTAssertEqual(band.minY, layout.rows[1].top)
        XCTAssertEqual(band.maxY, layout.rows[1].top + layout.rows[1].height)
        XCTAssertEqual(layout.hit(CGPoint(x: 20, y: band.maxY + 1)).row, 2)
        XCTAssertEqual(layout.hit(CGPoint(x: 20, y: band.minY - 1)).row, 0)
        XCTAssertNil(layout.caretBand(src: 0, row: 0), "an ordinary row keeps the caret's own band")
    }

    // MARK: pages

    func testPagesFlowAroundThePicture() {
        // Six body lines to a sheet; four lines of prose leave too little room
        // for a 100pt picture, which moves whole to the next sheet.
        let page = PageSetup(size: CGSize(width: 400, height: 200),
                             margins: LeafInsets(top: 20, left: 20, bottom: 20, right: 20),
                             gap: 20, backdrop: 24)
        let lines = (0..<4).map { row([mkRun("line \($0)")]) }
        let frame = docView(lines + [placeholder(), row([mkRun("after")])],
                            directives: [embed(4, 5)], regions: [region(4, 5)])
        let (drawing, _) = renderer(100)
        var cache: [Row: ShapedRow] = [:]
        let layout = EditorLayout(frame, theme: theme, viewWidth: 800, page: page, cache: &cache,
                                  drawing: drawing)
        let rl = layout.rows[4]
        XCTAssertEqual(rl.page, 1, "the picture opens the next sheet")
        XCTAssertEqual(rl.top, page.contentTop(1), accuracy: 0.5)
        XCTAssertEqual(layout.rows[5].top, rl.top + rl.height, accuracy: 0.5)
    }

    // MARK: the renderer

    func testTheRendererIsAskedOnceAcrossFrames() {
        let frame = docView([row([mkRun("before")]), placeholder(), row([mkRun("after")])],
                            directives: [embed(1, 2)], regions: [region(1, 2)])
        let (drawing, asked) = renderer(72)
        for _ in 0..<3 {
            _ = EditorLayout(frame, theme: theme, wrapWidth: 400, drawing: drawing)
        }
        XCTAssertEqual(asked(), ["embed"], "kept by what the directive says, across frames")
        // Edited away: nothing stands where it was.
        let gone = docView([row([mkRun("before")]), row([mkRun("after")])])
        XCTAssertTrue(EditorLayout(gone, theme: theme, wrapWidth: 400, drawing: drawing).pictureRects().isEmpty)
    }

    func testAViewWithNoIntrinsicHeightIsMeasuredAtTheColumnsWidth() {
        let view = LeafView(frame: CGRect(x: 0, y: 0, width: 10, height: 33))
        XCTAssertEqual(RegionViews.measure(view, width: 300), 33, "a view sized by hand keeps its height")
    }

    // MARK: on the surface

    #if canImport(AppKit) && !targetEnvironment(macCatalyst)
    private let page = "Before.\n\n::embed{src=\"https://x.org\"}\n\nAfter.\n"

    private func surface() throws -> LeafTextView {
        let doc = try LeafDoc(source: page, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 700, height: 600))
        scroll.documentView = view
        let window = NSWindow(contentRect: scroll.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = scroll
        scroll.layoutSubtreeIfNeeded()
        view.layout()
        return view
    }

    func testTheSurfaceDrawsTheHostsPictureAndZoomsIt() throws {
        let view = try surface()
        var seen: [RegionView] = []
        view.regionRenderers = ["directive:embed": { r, _ in
            seen.append(r)
            return RegionPicture(width: nil, height: 90) { _, _ in }
        }]
        XCTAssertEqual(seen.map(\.name), ["embed"])
        XCTAssertEqual(seen.first?.attrs.first?.value, "https://x.org")
        XCTAssertEqual(seen.first?.content, "::embed{src=\"https://x.org\"}")
        XCTAssertEqual(view.layoutEngine.pictureRects().values.first?.height, 90)

        // At 2× the layout is the same; the drawing scales.
        view.zoom = .scale(2)
        XCTAssertEqual(view.layoutEngine.pictureRects().values.first?.height, 90)
        view.zoom = .actualSize

        // Taking the renderer away leaves leaf's placeholder.
        view.regionRenderers = [:]
        XCTAssertTrue(view.layoutEngine.pictureRects().isEmpty)
        XCTAssertTrue(view.doc.view().regions.isEmpty, "the view still owns the directive")
    }

    func testADirectivesEditorWritesItsSourceThroughSplices() throws {
        let view = try surface()
        final class Session: RegionSession {
            let host: RegionHost
            init(host: RegionHost) { self.host = host }
        }
        var opened: Session?
        view.regionRenderers = ["directive:embed": { _, _ in RegionPicture(width: nil, height: 40) { _, _ in } }]
        view.regionEditors = ["directive:embed": RegionEditor(open: { r, _, host in
            XCTAssertEqual(r.content, "::embed{src=\"https://x.org\"}")
            let s = Session(host: host)
            opened = s
            return s
        })]
        let r = try XCTUnwrap(view.doc.view().regions.first)
        view.command { $0.selectRange(start: r.src, end: r.src) }
        view.doCommand(by: #selector(NSResponder.moveRight(_:)))
        let s = try XCTUnwrap(opened)
        s.host.replace("::embed[Home]{src=\"https://x.org\"}")
        XCTAssertEqual(view.doc.source(), "Before.\n\n::embed[Home]{src=\"https://x.org\"}\n\nAfter.\n")
        XCTAssertEqual(view.doc.view().regions.first?.label, "Home")
        s.host.undo()
        XCTAssertEqual(view.doc.source(), page)
    }
    #endif
}
