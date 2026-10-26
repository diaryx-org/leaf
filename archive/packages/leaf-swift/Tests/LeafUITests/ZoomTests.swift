//  ZoomTests.swift
//
//  `Zoom` is a scale or a rule that becomes one, and the arithmetic of both is
//  pinned here: what a fit resolves to, where a step lands from between two
//  stops, and — on the Mac, in a real scroll view — that a fit follows the
//  viewport, that a zoom about a point keeps that point where it was, and that
//  a change of page re-resolves the fit before anything is drawn.

import XCTest
import LeafFFI
@testable import LeafUI

final class ZoomTests: XCTestCase {
    private let letter = PageSetup.usLetter   // 612 wide, 24 backdrop: a 660 stack

    // ── resolving ────────────────────────────────────────────────────────────

    func testAScaleIsItselfClampedToTheRange() {
        XCTAssertEqual(Zoom.scale(1.5).resolve(in: CGSize(width: 800, height: 600), page: letter), 1.5)
        XCTAssertEqual(Zoom.scale(9).resolve(in: CGSize(width: 800, height: 600), page: letter), 4)
        XCTAssertEqual(Zoom.scale(0.01).resolve(in: CGSize(width: 800, height: 600), page: nil), 0.25)
    }

    func testAFactorMultipliesTheZoomAndTheProductIsClampedOnce() {
        let viewport = CGSize(width: 402, height: 700)
        XCTAssertEqual(Zoom.fitWidth.resolve(in: viewport, page: letter, factor: 2), 2 * 402 / letter.stackWidth, accuracy: 1e-9)
        XCTAssertEqual(Zoom.scale(1.5).resolve(in: viewport, page: letter, factor: 2), 3)
        XCTAssertEqual(Zoom.scale(3).resolve(in: viewport, page: letter, factor: 2), 4, "the product is held to the range")
        XCTAssertEqual(Zoom.scale(0.2).resolve(in: viewport, page: letter, factor: 2.5), 0.5, accuracy: 1e-9,
                       "and a scale below the floor is not raised before it is multiplied")
    }

    func testFitWidthSetsTheStackToTheViewportsWidth() {
        let scale = Zoom.fitWidth.resolve(in: CGSize(width: 990, height: 600), page: letter)
        XCTAssertEqual(scale, 990 / letter.stackWidth, accuracy: 1e-9)
        XCTAssertEqual(letter.stackWidth * scale, 990, accuracy: 1e-6)
    }

    func testFitPageShowsAWholeSheetOnTheTighterAxis() {
        // A tall narrow viewport is width-bound; a short wide one is height-bound.
        let narrow = Zoom.fitPage.resolve(in: CGSize(width: 330, height: 2000), page: letter)
        XCTAssertEqual(narrow, 330 / letter.stackWidth, accuracy: 1e-9)
        let short = Zoom.fitPage.resolve(in: CGSize(width: 2000, height: 420), page: letter)
        XCTAssertEqual(short, 420 / (letter.size.height + letter.backdrop * 2), accuracy: 1e-9)
    }

    func testTheFitsAreTheIdentityOffPaper() {
        XCTAssertEqual(Zoom.fitWidth.resolve(in: CGSize(width: 990, height: 600), page: nil), 1)
        XCTAssertEqual(Zoom.fitPage.resolve(in: CGSize(width: 990, height: 600), page: nil), 1)
    }

    func testAFitInAnUnsizedViewportIsTheIdentityRatherThanNothing() {
        XCTAssertEqual(Zoom.fitWidth.resolve(in: .zero, page: letter), 1)
    }

    // ── stepping ─────────────────────────────────────────────────────────────

    func testAStepFromBetweenTwoStopsLandsOnTheNextStop() {
        XCTAssertEqual(Zoom.stepUp(from: 1.37), 1.5)
        XCTAssertEqual(Zoom.stepDown(from: 1.37), 1.25)
    }

    func testAStepFromAStopIsTheNeighbouringStop() {
        XCTAssertEqual(Zoom.stepUp(from: 1), 1.25)
        XCTAssertEqual(Zoom.stepDown(from: 1), 0.75)
    }

    func testTheLadderEndsAtTheRange() {
        XCTAssertEqual(Zoom.stepUp(from: 4), 4)
        XCTAssertEqual(Zoom.stepDown(from: 0.25), 0.25)
    }

    // ── the theme fitted to a page ───────────────────────────────────────────

    func testAFittedThemeSetsTheMeasureIntoTheColumn() {
        // The whole point: `measure` characters of the fitted body fill one
        // column of the sheet, to within the half-point the size rounds to.
        for (page, measure) in [(PageSetup.usLetter, CGFloat(68)), (.a4, 68), (.usLetter, 52), (.usLetter.columned(2), 40)] {
            let fitted = EditorTheme.default.fitted(to: page, measure: measure)
            let characters = page.columnWidth / fitted.averageCharWidth
            XCTAssertEqual(characters, measure, accuracy: 2.5,
                           "\(measure) on a \(page.columnWidth)pt column came out \(characters) at \(fitted.fontSize)pt")
            XCTAssertEqual(fitted.fontSize * 2, (fitted.fontSize * 2).rounded(), "rounded to a half point")
        }
    }

    func testTheDefaultBodyIsTooLargeForALetterColumnAndTheFitBringsItDown() {
        // The finding the helper exists for: 16pt sets Letter shorter than the
        // flow's own 68, and the fit lands in the range a page is usually set at.
        let unfitted = PageSetup.usLetter.columnWidth / EditorTheme.default.averageCharWidth
        XCTAssertLessThan(unfitted, 62)
        let fitted = EditorTheme.default.fitted(to: .usLetter)
        XCTAssertLessThan(fitted.fontSize, EditorTheme.default.fontSize)
        XCTAssertGreaterThan(fitted.fontSize, 12)
        // The leading scales with it.
        XCTAssertEqual(fitted.lineHeight / fitted.fontSize, EditorTheme.default.lineRatio, accuracy: 1e-9)
    }

    func testAFittedThemeKeepsWhatIsNotType() {
        let fitted = EditorTheme.default.fitted(to: .a4)
        XCTAssertEqual(fitted.padding, EditorTheme.default.padding)
        XCTAssertEqual(fitted.measure, EditorTheme.default.measure)
    }

    #if canImport(AppKit) && !targetEnvironment(macCatalyst)
    // ── the Mac view, in a scroll view ───────────────────────────────────────

    private func host(_ source: String = "One.\n\nTwo.\n", width: CGFloat, height: CGFloat = 600,
                      page: PageSetup? = nil) throws -> (NSScrollView, LeafTextView) {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.pageSetup = page
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: width, height: height))
        scroll.documentView = view
        scroll.hasVerticalScroller = false
        scroll.hasHorizontalScroller = false
        // A window, so the clip view's frame notifications reach the view.
        let window = NSWindow(contentRect: scroll.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = scroll
        scroll.layoutSubtreeIfNeeded()
        view.layout()
        return (scroll, view)
    }

    func testAScaleIsAppliedToTheFrameAndReportedBack() throws {
        let (scroll, view) = try host(width: 800, page: letter)
        var reported: (Zoom, CGFloat)?
        view.onZoomChange = { reported = ($0, $1) }
        view.zoom = .scale(2)
        XCTAssertEqual(view.zoomScale, 2)
        XCTAssertEqual(view.frame.width, letter.stackWidth * 2, "the stack is twice as wide on screen")
        XCTAssertEqual(reported?.0, .scale(2))
        XCTAssertEqual(reported?.1, 2)
        _ = scroll
    }

    func testFitWidthResolvesAgainstTheViewportAndFollowsIt() throws {
        let (scroll, view) = try host(width: 990, page: letter)
        view.zoom = .fitWidth
        XCTAssertEqual(view.zoomScale, 990 / letter.stackWidth, accuracy: 1e-6)
        XCTAssertEqual(view.frame.width, 990, accuracy: 0.5, "the stack exactly fills the viewport")

        scroll.setFrameSize(NSSize(width: 1320, height: 600))
        scroll.layoutSubtreeIfNeeded()
        view.layout()
        XCTAssertEqual(view.zoom, .fitWidth, "still the rule, not the number it was")
        XCTAssertEqual(view.zoomScale, 1320 / letter.stackWidth, accuracy: 1e-6)
    }

    func testAFitReResolvesWhenThePageChanges() throws {
        let (_, view) = try host(width: 990, page: nil)
        view.zoom = .fitWidth
        XCTAssertEqual(view.zoomScale, 1, "the identity off paper")
        var reported: CGFloat?
        view.onZoomChange = { reported = $1 }
        view.pageSetup = letter
        XCTAssertEqual(view.zoomScale, 990 / letter.stackWidth, accuracy: 1e-6)
        XCTAssertEqual(reported, view.zoomScale)
        view.pageSetup = nil
        XCTAssertEqual(view.zoomScale, 1)
    }

    func testZoomingAboutAPointKeepsItWhereItWas() throws {
        let long = (1...80).map { "Paragraph \($0), long enough to wrap onto a second line at the sheet's measure, and then some." }
            .joined(separator: "\n\n") + "\n"
        let (scroll, view) = try host(long, width: 800, page: letter)
        let clip = scroll.contentView
        // Somewhere down the document, with the anchor a third of the way into
        // the viewport rather than at its centre.
        clip.scroll(to: NSPoint(x: 0, y: 1000))
        scroll.reflectScrolledClipView(clip)
        let anchor = NSPoint(x: 300, y: 1000 + 200)
        let inViewport = NSPoint(x: anchor.x - clip.bounds.minX, y: anchor.y - clip.bounds.minY)
        let layoutPoint = NSPoint(x: anchor.x / view.zoomScale, y: anchor.y / view.zoomScale)

        view.setZoom(.scale(1.5), anchor: anchor)

        // The same layout point, at the new scale, is at the same place in the viewport.
        let after = NSPoint(x: layoutPoint.x * 1.5 - clip.bounds.minX, y: layoutPoint.y * 1.5 - clip.bounds.minY)
        XCTAssertEqual(after.x, inViewport.x, accuracy: 0.5)
        XCTAssertEqual(after.y, inViewport.y, accuracy: 0.5)
    }

    func testZoomingWithNoAnchorHoldsTheViewportsCentre() throws {
        let long = (1...80).map { "Paragraph \($0), long enough to wrap onto a second line at the sheet's measure." }
            .joined(separator: "\n\n") + "\n"
        let (scroll, view) = try host(long, width: 800, page: letter)
        let clip = scroll.contentView
        clip.scroll(to: NSPoint(x: 0, y: 1500))
        scroll.reflectScrolledClipView(clip)
        let centre = NSPoint(x: clip.bounds.midX, y: clip.bounds.midY)
        let layoutCentre = NSPoint(x: centre.x / view.zoomScale, y: centre.y / view.zoomScale)

        view.zoom = .scale(0.5)

        // Vertically the same layout point is still at the centre. Not
        // horizontally: at 50% the stack is narrower than the viewport, so it
        // recentres and there is nothing to scroll — the clamp, not the anchor.
        XCTAssertEqual(layoutCentre.y * 0.5, clip.bounds.midY, accuracy: 0.5)
        XCTAssertEqual(clip.bounds.minX, 0)
    }

    func testTheOffsetIsClampedToTheDocument() throws {
        let (scroll, view) = try host(width: 800, page: letter)
        let clip = scroll.contentView
        view.setZoom(.scale(0.25), anchor: NSPoint(x: 0, y: 0))
        XCTAssertGreaterThanOrEqual(clip.bounds.minY, 0)
        XCTAssertLessThanOrEqual(clip.bounds.maxY, max(view.frame.height, clip.bounds.height) + 0.5)
    }

    func testAPinchCompoundsAndClamps() throws {
        let (_, view) = try host(width: 800, page: letter)
        view.zoom = .scale(3.5)
        // A magnification event is +50% of the current scale: 3.5 → 5.25, held to 4.
        let event = try XCTUnwrap(NSEvent.magnifyEvent(magnification: 0.5, at: NSPoint(x: 100, y: 100), in: view))
        view.magnify(with: event)
        XCTAssertEqual(view.zoomScale, 4)
        XCTAssertEqual(view.zoom, .scale(4))
    }
    #endif
}

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
private extension NSEvent {
    /// A synthetic trackpad pinch over `view`. `CGEvent` has no magnify
    /// constructor, so this goes through the gesture-event initializer with the
    /// magnification supplied by a private-free route: an `NSEvent` of type
    /// `.magnify` reads its `magnification` from the underlying `CGEvent`'s
    /// field, which `otherEvent` cannot set — so the test asks the view through
    /// a subclass that answers the one field it reads.
    static func magnifyEvent(magnification: CGFloat, at point: NSPoint, in view: NSView) -> NSEvent? {
        guard let window = view.window else { return nil }
        let inWindow = view.convert(point, to: nil)
        return MagnifyEvent.make(magnification: magnification, at: inWindow, window: window)
    }
}

/// `NSEvent` cannot be constructed as a magnify event with a chosen
/// magnification from public API, but its `magnification` is an ordinary
/// property a subclass may answer; the event otherwise only has to carry a
/// window location.
private final class MagnifyEvent: NSEvent {
    private var value: CGFloat = 0
    override var magnification: CGFloat { value }
    override var type: NSEvent.EventType { .magnify }

    static func make(magnification: CGFloat, at location: NSPoint, window: NSWindow) -> NSEvent? {
        guard let base = MagnifyEvent.otherEvent(
            with: .applicationDefined, location: location, modifierFlags: [], timestamp: 0,
            windowNumber: window.windowNumber, context: nil, subtype: 0, data1: 0, data2: 0
        ) as? MagnifyEvent else { return nil }
        base.value = magnification
        return base
    }
}
#endif

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
extension ZoomTests {
    func testFitPageInAWideShortWindowIsHeightBound() throws {
        let long = (1...40).map { "Paragraph \($0)." }.joined(separator: "\n\n") + "\n"
        let doc = try LeafDoc(source: long, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.pageSetup = .usLetter
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 726, height: 600))
        scroll.documentView = view
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        let window = NSWindow(contentRect: scroll.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = scroll
        scroll.layoutSubtreeIfNeeded()
        view.layout()
        view.zoom = .scale(1.01)
        view.zoom = .fitPage
        XCTAssertEqual(view.zoomScale, 600 / (792 + 48), accuracy: 0.01)
        view.layout()
        XCTAssertEqual(view.zoomScale, 600 / (792 + 48), accuracy: 0.01)
    }
}
#endif

#if canImport(UIKit)
import UIKit

/// The UIKit view in its wrapper, in a scroll view — what `LeafEditor` builds.
extension ZoomTests {
    private func host(_ source: String = "One.\n\nTwo.\n", width: CGFloat = 402, height: CGFloat = 700,
                      page: PageSetup? = nil) throws -> (UIScrollView, LeafZoomView, LeafTextView) {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.pageSetup = page
        let zoomed = LeafZoomView(textView: view)
        let scroll = UIScrollView(frame: CGRect(x: 0, y: 0, width: width, height: height))
        scroll.addSubview(zoomed)
        zoomed.translatesAutoresizingMaskIntoConstraints = false
        let equal = zoomed.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor)
        equal.priority = .defaultHigh
        NSLayoutConstraint.activate([
            zoomed.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            zoomed.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            zoomed.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            zoomed.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            zoomed.widthAnchor.constraint(greaterThanOrEqualTo: scroll.frameLayoutGuide.widthAnchor),
            equal,
        ])
        let window = UIWindow(frame: scroll.frame)
        window.addSubview(scroll)
        window.isHidden = false
        scroll.layoutIfNeeded()
        return (scroll, zoomed, view)
    }

    func testTheWrapperCarriesTheScaledSizeToTheScrollView() throws {
        let (scroll, zoomed, view) = try host(page: letter)
        view.zoom = .scale(2)
        scroll.layoutIfNeeded()
        XCTAssertEqual(view.zoomScale, 2)
        XCTAssertEqual(zoomed.bounds.width, letter.stackWidth * 2, accuracy: 0.5, "the stack, scaled")
        XCTAssertEqual(scroll.contentSize.width, letter.stackWidth * 2, accuracy: 0.5, "and the scroll view scrolls to it")
        XCTAssertEqual(view.bounds.width, letter.stackWidth, accuracy: 0.5, "the text view lays out unscaled")
        XCTAssertEqual(view.transform.a, 2, "and is drawn scaled")
        XCTAssertEqual(view.contentScaleFactor, view.traitCollection.displayScale * 2, accuracy: 0.01, "sharp at that scale")
    }

    func testFitWidthOnAPhoneIsTheStackToTheScreen() throws {
        let (scroll, zoomed, view) = try host(page: letter)
        view.zoom = .fitWidth
        scroll.layoutIfNeeded()
        XCTAssertEqual(view.zoomScale, 402 / letter.stackWidth, accuracy: 1e-6)
        XCTAssertEqual(zoomed.bounds.width, 402, accuracy: 0.5)
        XCTAssertEqual(scroll.contentSize.width, 402, accuracy: 0.5, "nothing to scroll sideways")
        XCTAssertEqual(view.bounds.width, letter.stackWidth, accuracy: 0.5)
    }

    func testTheFitFollowsARotation() throws {
        let (scroll, _, view) = try host(page: letter)
        view.zoom = .fitWidth
        scroll.layoutIfNeeded()
        scroll.frame = CGRect(x: 0, y: 0, width: 874, height: 402)
        scroll.layoutIfNeeded()
        XCTAssertEqual(view.zoom, .fitWidth)
        XCTAssertEqual(view.zoomScale, 874 / letter.stackWidth, accuracy: 1e-6)
    }

    func testActualSizeFromAFitIsOne() throws {
        let (scroll, _, view) = try host(page: letter)
        view.zoom = .fitWidth
        scroll.layoutIfNeeded()
        view.zoom = .actualSize
        scroll.layoutIfNeeded()
        XCTAssertEqual(view.zoom, .actualSize)
        XCTAssertEqual(view.zoomScale, 1)
        XCTAssertEqual(view.transform.a, 1)
        XCTAssertEqual(scroll.contentSize.width, letter.stackWidth, accuracy: 0.5)
    }

    func testTheFlowOffPaperStillFillsTheWidthAtAnyZoom() throws {
        let (scroll, zoomed, view) = try host(page: nil)
        view.zoom = .scale(2)
        scroll.layoutIfNeeded()
        XCTAssertEqual(zoomed.bounds.width, 402, accuracy: 0.5, "no width of its own")
        XCTAssertEqual(view.bounds.width, 201, accuracy: 0.5, "so the flow re-wraps to half the points")
    }

    func testZoomingAboutAPointKeepsItWhereItWasOnUIKit() throws {
        let long = (1...80).map { "Paragraph \($0), long enough to wrap onto a second line at the sheet's measure, and then some." }
            .joined(separator: "\n\n") + "\n"
        let (scroll, _, view) = try host(long, page: letter)
        view.zoom = .fitWidth
        scroll.layoutIfNeeded()
        scroll.contentOffset = CGPoint(x: 0, y: 900)
        // A point on screen, and the layout point under it.
        let onScreen = CGPoint(x: 150, y: 300)
        let p = view.convert(onScreen, from: scroll.superview!)
        view.setZoom(.scale(2), anchor: p)
        scroll.layoutIfNeeded()
        let after = view.convert(p, to: scroll.superview!)
        XCTAssertEqual(after.x, onScreen.x, accuracy: 1)
        XCTAssertEqual(after.y, onScreen.y, accuracy: 1)
    }

    // ── Dynamic Type ─────────────────────────────────────────────────────────
    // On paper the reader's text size is a zoom, not a repagination; in the
    // flow it is bigger type, as it always was.

    private static let large = UIContentSizeCategory.accessibilityExtraExtraExtraLarge

    /// The factor a content size scales body type by, `1` at the default.
    private func factor(_ size: UIContentSizeCategory) -> CGFloat {
        UIFontMetrics.default.scaledValue(for: 100, compatibleWith: UITraitCollection(preferredContentSizeCategory: size)) / 100
    }

    /// Put the view at a content size, as Settings ▸ Display & Brightness ▸
    /// Text Size does, and let the trait change reach it.
    private func setTextSize(_ size: UIContentSizeCategory, on view: LeafTextView, in scroll: UIScrollView) throws {
        guard #available(iOS 17, *) else { throw XCTSkip("trait overrides are iOS 17") }
        view.traitOverrides.preferredContentSizeCategory = size
        view.updateTraitsIfNeeded()
        scroll.layoutIfNeeded()
        XCTAssertEqual(view.traitCollection.preferredContentSizeCategory, size)
    }

    /// Where every visual line starts and how high it sits: a layout's line
    /// breaks, and so its pagination.
    private func lineBreaks(_ view: LeafTextView) -> [[CGFloat]] {
        view.layoutEngine.rows.map { row in [row.top] + row.shaped.wrapped.map { CGFloat($0.start) } }
    }

    private var long: String {
        (1...60).map { "Paragraph \($0), long enough to wrap onto a second line at the sheet's measure, and then some." }
            .joined(separator: "\n\n") + "\n"
    }

    func testOnPaperALargerTextSizeKeepsThePagesAndTheLineBreaks() throws {
        let (scroll, _, view) = try host(long, page: letter)
        view.zoom = .fitWidth
        scroll.layoutIfNeeded()
        let pages = view.pages
        let breaks = lineBreaks(view)
        XCTAssertGreaterThan(pages.count, 2)

        try setTextSize(Self.large, on: view, in: scroll)
        XCTAssertEqual(view.pages, pages, "the same sheets")
        XCTAssertEqual(lineBreaks(view), breaks, "broken at the same places")
        XCTAssertEqual(view.renderTheme.fontSize, EditorTheme.default.fontSize, "the sheet's type is the document's")

        // And the PDF breaks the lines the screen does.
        let pdf = try XCTUnwrap(CGPDFDocument(CGDataProvider(data: view.pdfData(page: letter) as CFData)!))
        XCTAssertEqual(pdf.numberOfPages, pages.count)
    }

    func testOnPaperTheZoomIsTheFitTimesTheTextSize() throws {
        let (scroll, zoomed, view) = try host(long, page: letter)
        view.zoom = .fitWidth
        scroll.layoutIfNeeded()
        let fit = 402 / letter.stackWidth
        XCTAssertEqual(view.zoomScale, fit, accuracy: 1e-6)

        try setTextSize(Self.large, on: view, in: scroll)
        let f = factor(Self.large)
        XCTAssertGreaterThan(f, 1.5)
        XCTAssertEqual(view.zoom, .fitWidth, "still a fit")
        XCTAssertEqual(view.zoomScale, fit * f, accuracy: 1e-6, "the fit times the factor")
        XCTAssertEqual(view.transform.a, fit * f, accuracy: 1e-6)
        XCTAssertEqual(zoomed.bounds.width, letter.stackWidth * fit * f, accuracy: 0.5, "a sheet wider than the screen")

        // A scale is multiplied as a fit is, and reported back before the factor.
        view.zoom = .actualSize
        XCTAssertEqual(view.zoomScale, f, accuracy: 1e-6)
        view.zoom = .scale(9)
        XCTAssertEqual(view.zoomScale, Zoom.range.upperBound, "the product is held to the range, and no lower")
        XCTAssertEqual(view.zoom, .scale(Zoom.range.upperBound / f))

        // Back to the default size, back to the plain fit.
        view.zoom = .fitWidth
        try setTextSize(.large, on: view, in: scroll)
        XCTAssertEqual(view.zoomScale, fit, accuracy: 1e-6)
    }

    func testTheContinuousFlowStillScalesTheType() throws {
        let (scroll, _, view) = try host(long, page: nil)
        view.zoom = .fitWidth
        scroll.layoutIfNeeded()
        let rows = view.layoutEngine.rows.last?.top ?? 0

        try setTextSize(Self.large, on: view, in: scroll)
        let f = factor(Self.large)
        XCTAssertEqual(view.renderTheme.fontSize, EditorTheme.default.fontSize * f, accuracy: 0.01, "bigger type")
        XCTAssertEqual(view.zoomScale, 1, "and no zoom")
        XCTAssertGreaterThan(view.layoutEngine.rows.last?.top ?? 0, rows * 1.5, "so the column reflows longer")
    }

    func testMovingOnAndOffPaperMovesTheFactorBetweenTheTypeAndTheZoom() throws {
        let (scroll, _, view) = try host(long, page: nil)
        view.zoom = .fitWidth
        scroll.layoutIfNeeded()
        try setTextSize(Self.large, on: view, in: scroll)
        let f = factor(Self.large)
        XCTAssertEqual(view.renderTheme.fontSize, EditorTheme.default.fontSize * f, accuracy: 0.01)
        XCTAssertEqual(view.zoomScale, 1)

        view.pageSetup = letter
        scroll.layoutIfNeeded()
        XCTAssertEqual(view.renderTheme.fontSize, EditorTheme.default.fontSize, "onto paper: the type is the host's")
        XCTAssertEqual(view.zoomScale, 402 / letter.stackWidth * f, accuracy: 1e-6, "and the zoom carries the size")

        view.pageSetup = nil
        scroll.layoutIfNeeded()
        XCTAssertEqual(view.renderTheme.fontSize, EditorTheme.default.fontSize * f, accuracy: 0.01, "off it: back in the type")
        XCTAssertEqual(view.zoomScale, 1, "and out of the zoom")
    }
}
#endif
