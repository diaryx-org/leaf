//  SheetsTests.swift
//
//  Paper read a sheet at a time (`Sheets.swift`): which sheet a point is on,
//  the band a held sheet scrolls in, and — on the Mac, in a real scroll view
//  over `LeafClipView` — that a held sheet stops the scroll at its edges, that
//  `showSheet` turns to a sheet and takes the caret with it, that the caret
//  carried on to another sheet takes the hold with it, and that a sheet in
//  view is reported as the reader scrolls.

import XCTest
import LeafFFI
@testable import LeafUI

final class SheetsTests: XCTestCase {
    // ── the geometry ─────────────────────────────────────────────────────────

    private let three = [CGRect(x: 0, y: 10, width: 100, height: 200),
                         CGRect(x: 0, y: 230, width: 100, height: 200),
                         CGRect(x: 0, y: 450, width: 100, height: 200)]

    func testAPointIsOnTheSheetItFallsOnOrTheNearest() {
        XCTAssertEqual(Sheets.index(at: 0, in: three), 0, "above the first, the first")
        XCTAssertEqual(Sheets.index(at: 100, in: three), 0)
        XCTAssertEqual(Sheets.index(at: 300, in: three), 1)
        XCTAssertEqual(Sheets.index(at: 214, in: three), 0, "in a gap, the nearer sheet")
        XCTAssertEqual(Sheets.index(at: 226, in: three), 1)
        XCTAssertEqual(Sheets.index(at: 9_000, in: three), 2, "below the last, the last")
        XCTAssertEqual(Sheets.index(at: 50, in: []), 0)
    }

    func testABandIsTheSheetWithItsBackdropAndTheEndsRunToTheDocumentsEnds() {
        XCTAssertEqual(Sheets.band(0, pages: three, backdrop: 10, contentHeight: 660), 0...220)
        XCTAssertEqual(Sheets.band(1, pages: three, backdrop: 10, contentHeight: 660), 220...440)
        XCTAssertEqual(Sheets.band(2, pages: three, backdrop: 10, contentHeight: 700), 440...700)
    }

    #if canImport(AppKit) && !targetEnvironment(macCatalyst)
    // ── the Mac view, in a scroll view ───────────────────────────────────────

    /// Small sheets, so a few paragraphs run to several of them; no backdrop
    /// and a 20-point gap, so the bands are easy to reckon.
    private let page = PageSetup(size: CGSize(width: 300, height: 200),
                                 margins: LeafInsets(top: 20, left: 20, bottom: 20, right: 20),
                                 gap: 20, backdrop: 0)

    private var source: String {
        (1...30).map { "Paragraph \($0), with words enough to wrap on a small sheet." }
            .joined(separator: "\n\n") + "\n"
    }

    /// A view on `page` in a scroll view over `LeafClipView`, as `LeafEditor`
    /// builds it, the viewport one sheet tall at actual size.
    private func host() throws -> (NSScrollView, LeafTextView) {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.pageSetup = page
        view.zoom = .actualSize
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
        let clip = LeafClipView()
        clip.drawsBackground = false
        scroll.contentView = clip
        scroll.documentView = view
        scroll.hasVerticalScroller = false
        scroll.hasHorizontalScroller = false
        let window = NSWindow(contentRect: scroll.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = scroll
        scroll.layoutSubtreeIfNeeded()
        view.layout()
        return (scroll, view)
    }

    private func move(_ scroll: NSScrollView, to y: CGFloat) {
        scroll.contentView.scroll(to: CGPoint(x: 0, y: y))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    func testTheSheetCountIsTheLayoutsAndIsReported() throws {
        let (_, view) = try host()
        XCTAssertGreaterThan(view.sheetCount, 2)
        XCTAssertEqual(view.sheetCount, view.pages.count)
        var told: (Int, Int)?
        view.onSheetChange = { told = ($0, $1) }
        view.showSheet(1)
        XCTAssertEqual(told?.0, 1)
        XCTAssertEqual(told?.1, view.sheetCount)
    }

    func testUnheldTheSheetInViewFollowsTheScroll() throws {
        let (scroll, view) = try host()
        XCTAssertEqual(view.sheet, 0)
        move(scroll, to: view.pages[2].minY)
        XCTAssertEqual(view.sheet, 2)
    }

    func testAHeldSheetStopsTheScrollAtItsEdges() throws {
        let (scroll, view) = try host()
        view.holdsSheet = true
        move(scroll, to: view.pages[2].minY)
        XCTAssertEqual(scroll.contentView.bounds.origin.y, 0, accuracy: 0.5,
                       "the band of the first sheet is one viewport tall: nothing to scroll")
        XCTAssertEqual(view.sheet, 0)
    }

    func testAHeldSheetZoomedInScrollsWithinItselfAndNoFurther() throws {
        let (scroll, view) = try host()
        view.holdsSheet = true
        view.showSheet(1)
        view.zoom = .scale(2)
        move(scroll, to: 0)
        let band = Sheets.band(1, pages: view.pages, backdrop: 0, contentHeight: view.pages.last!.maxY)
        XCTAssertEqual(scroll.contentView.bounds.minY, band.lowerBound * 2, accuracy: 0.5)
        move(scroll, to: 100_000)
        XCTAssertEqual(scroll.contentView.bounds.maxY, band.upperBound * 2, accuracy: 0.5)
        XCTAssertEqual(view.sheet, 1)
    }

    func testShowingASheetTurnsToItAndTakesTheCaret() throws {
        let (scroll, view) = try host()
        view.holdsSheet = true
        view.showSheet(2)
        XCTAssertEqual(view.sheet, 2)
        XCTAssertEqual(scroll.contentView.bounds.origin.y, view.pages[2].minY, accuracy: 0.5)
        // The caret came too: landing on it again leaves the hold where it is.
        let caret = view.doc.caretOffset()
        XCTAssertGreaterThan(caret, 0)
        view.reveal(offset: caret)
        XCTAssertEqual(view.sheet, 2)
    }

    func testPastTheLastIsTheLast() throws {
        let (_, view) = try host()
        view.showSheet(.max)
        XCTAssertEqual(view.sheet, view.sheetCount - 1)
    }

    func testTheCaretCarriedOnToAnotherSheetTakesTheHoldWithIt() throws {
        let (scroll, view) = try host()
        view.holdsSheet = true
        view.reveal(offset: UInt32(source.utf8.count - 2))
        XCTAssertEqual(view.sheet, view.sheetCount - 1)
        XCTAssertGreaterThanOrEqual(scroll.contentView.bounds.origin.y, view.pages.last!.minY - 0.5)
    }

    func testASheetIsDrawnAsAPictureAndPastTheLastIsTheLast() throws {
        let model = try LeafEditorModel(source: source)
        let first = try XCTUnwrap(model.sheetImage(0, theme: .default, page: page, scale: 2))
        XCTAssertEqual(first.sheet, 0)
        XCTAssertGreaterThan(first.sheetCount, 2)
        XCTAssertEqual(first.image.width, 600)
        XCTAssertEqual(first.image.height, 400)
        let last = try XCTUnwrap(model.sheetImage(.max, theme: .default, page: page, scale: 1))
        XCTAssertEqual(last.sheet, first.sheetCount - 1)
    }

    func testASheetAskedForBeforeTheLayoutIsShownOnceThereIsOne() throws {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.showSheet(1)
        view.pageSetup = page
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
        scroll.contentView = LeafClipView()
        scroll.documentView = view
        let window = NSWindow(contentRect: scroll.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = scroll
        scroll.layoutSubtreeIfNeeded()
        view.layout()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(view.sheet, 1)
    }
    #endif
}
