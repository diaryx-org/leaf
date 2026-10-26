//  RegionRenderingTests.swift
//
//  A renderer that draws later (`RegionRenderer(later:)`): the region stands
//  as leaf's placeholder, or as the last picture's size, until the answer
//  comes; the answer is laid out when it arrives; an answer for content the
//  region no longer has, or from before `redrawRegions`, is dropped; and
//  paper waits for its pictures rather than printing placeholders.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class RegionRenderingTests: XCTestCase {
    private let source = "intro\n\n```demo\nA\n```\n\nend\n"

    /// A renderer that answers only when told: each request is kept, with
    /// the content and context it was for.
    private final class Later {
        var requests: [(content: String, dark: Bool, done: (RegionPicture?) -> Void)] = []
        /// How many times a picture was drawn on paper.
        var drawn = 0

        lazy var renderer = RegionRenderer(later: { [unowned self] region, context, done in
            self.requests.append((region.content, context.dark, done))
        })

        /// A picture that counts the times it is drawn on paper — into a
        /// context that is not a bitmap, as the screen's kept one is.
        func picture(height: CGFloat) -> RegionPicture {
            RegionPicture(width: 100, height: height) { [weak self] ctx, _ in
                if ctx.makeImage() == nil { self?.drawn += 1 }
            }
        }

        /// Answer request `i` from another thread, as a renderer on a queue of
        /// its own does.
        func answer(_ i: Int, height: CGFloat) {
            let done = requests[i].done
            let picture = picture(height: height)
            let sent = DispatchSemaphore(value: 0)
            DispatchQueue.global().async {
                done(picture)
                sent.signal()
            }
            sent.wait()
        }
    }

    private func view(_ later: Later, source: String? = nil) throws -> LeafTextView {
        let doc = try LeafDoc(source: source ?? self.source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.regionRenderers = ["fence:demo": later.renderer]
        view.layout()
        return view
    }

    private func drawn(_ view: LeafTextView) -> CGRect? {
        view.layoutEngine.pictureRects().first { $0.key.hasPrefix("fence:demo@") }?.value
    }

    /// Let the main queue deliver what arrived.
    private func settle() {
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    }

    func testThePlaceholderStandsUntilThePictureArrivesAndThenOnlyItsRegionIsLaidOut() throws {
        let later = Later()
        let view = try view(later)
        XCTAssertEqual(later.requests.map(\.content), ["A\n"])
        XCTAssertNil(drawn(view), "a picture stood before it was drawn")
        let above = view.layoutEngine.rows[0].top
        later.answer(0, height: 70)
        XCTAssertNil(drawn(view), "the answer was laid out before the main queue had it")
        settle()
        XCTAssertEqual(drawn(view)?.height, 70)
        XCTAssertEqual(view.layoutEngine.rows[0].top, above)
        XCTAssertEqual(later.requests.count, 1, "the picture was asked for again")
    }

    func testAViewMadeAgainOverTheSamePicturesHearsTheirAnswers() throws {
        // A host makes a second view for a model whose first is going: the
        // second shares the first's pictures, asked for and not yet answered.
        let later = Later()
        var first: LeafTextView? = try view(later)
        let doc = try XCTUnwrap(first?.doc)
        let pictures = try XCTUnwrap(first?.regionSlot.pictures)
        let second = LeafTextView(doc: doc, theme: .default,
                                  regions: RegionSetup(renderers: ["fence:demo": later.renderer],
                                                       pictures: pictures))
        second.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        second.layout()
        first = nil
        _ = first
        let pending = later.requests.count - 1
        later.answer(pending, height: 60)
        settle()
        XCTAssertEqual(drawn(second)?.height, 60, "the answer went to the view that was replaced")
    }

    func testAnAnswerForContentTheRegionNoLongerHasIsDropped() throws {
        let later = Later()
        let view = try view(later)
        let r = try XCTUnwrap(view.doc.view().regions.first)
        // The fence is edited while its picture is being drawn.
        view.command { $0.editSource(from: r.contentStart, to: r.contentEnd, text: "B\n") }
        XCTAssertEqual(later.requests.map(\.content), ["A\n", "B\n"])
        later.answer(0, height: 999)
        settle()
        XCTAssertNil(drawn(view), "the old content's picture landed on the new content")
        later.answer(1, height: 50)
        settle()
        XCTAssertEqual(drawn(view)?.height, 50)
    }

    func testAnAnswerFromBeforeARedrawIsDropped() throws {
        let later = Later()
        let view = try view(later)
        view.redrawRegions(kind: "fence:demo")
        XCTAssertEqual(later.requests.count, 2, "the redraw did not ask again")
        later.answer(0, height: 999)
        settle()
        XCTAssertNil(drawn(view), "an answer from before the redraw was kept")
        later.answer(1, height: 40)
        settle()
        XCTAssertEqual(drawn(view)?.height, 40)
    }

    func testTheSameContentInANewStyleKeepsItsLastSizeWhileItIsDrawn() throws {
        let later = Later()
        let view = try view(later)
        later.answer(0, height: 80)
        settle()
        XCTAssertEqual(drawn(view)?.height, 80)
        view.appearance = NSAppearance(named: .darkAqua)
        view.redrawRegions(kind: "fence:other") // a frame laid out in the new appearance
        XCTAssertEqual(later.requests.last?.dark, true)
        XCTAssertEqual(drawn(view)?.height, 80, "the region jumped while its dark picture was drawn")
    }

    func testAPrintoutWaitsForItsPictures() throws {
        let later = Later()
        let view = try view(later)
        // Nothing answered on screen yet. The sheet asks for a light picture
        // and waits; this renderer answers it from a queue of its own.
        let before = later.requests.count
        let answering = DispatchQueue(label: "answering")
        later.renderer = RegionRenderer(later: { region, context, done in
            later.requests.append((region.content, context.dark, done))
            answering.asyncAfter(deadline: .now() + 0.2) { done(later.picture(height: 60)) }
        })
        // A new renderer is a redraw: the screen asks again too.
        view.regionRenderers = ["fence:demo": later.renderer]
        later.drawn = 0
        let pdf = view.pdfData(page: .usLetter)
        XCTAssertFalse(pdf.isEmpty)
        XCTAssertGreaterThan(later.requests.count, before)
        XCTAssertGreaterThan(later.drawn, 0, "the printout drew a placeholder for the picture")
    }

    func testAPrintoutDoesNotWaitForeverForAPictureThatNeverComes() throws {
        let later = Later()
        let view = try view(later)
        let wait = RegionPictures.paperWait
        RegionPictures.paperWait = 0.2
        defer { RegionPictures.paperWait = wait }
        let start = Date()
        XCTAssertFalse(view.pdfData(page: .usLetter).isEmpty)
        XCTAssertLessThan(Date().timeIntervalSince(start), 5)
        XCTAssertEqual(later.drawn, 0)
    }

    func testARendererThatDrawsAtOnceStillDoesSo() throws {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.regionRenderers = ["fence:demo": RegionRenderer { _, _ in RegionPicture(width: 10, height: 30) { _, _ in } }]
        view.layout()
        XCTAssertEqual(drawn(view)?.height, 30, "a synchronous picture waited for a frame")
    }
}
#endif
