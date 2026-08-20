//  EditorThemeTests.swift
//
//  `metricsDiffer(from:)` decides whether a theme change forces a re-wrap. Getting
//  it wrong reintroduces the relayout⇄state-publish loop (a colour-only change must
//  NOT relayout) or drops a real geometry change — so it's pinned here.

import XCTest
@testable import LeafUI

final class EditorThemeTests: XCTestCase {
    func testColourOnlyChangeDoesNotForceRelayout() {
        let base = EditorTheme.default
        var recoloured = base
        recoloured.selectionColor = .red
        recoloured.caretColor = .blue
        XCTAssertFalse(base.metricsDiffer(from: recoloured))
    }

    func testIdenticalThemeDoesNotDiffer() {
        XCTAssertFalse(EditorTheme.default.metricsDiffer(from: EditorTheme.default))
    }

    func testFontSizeChangeForcesRelayout() {
        let base = EditorTheme.default
        var bigger = base
        bigger.fontSize = base.fontSize + 3
        XCTAssertTrue(base.metricsDiffer(from: bigger))
    }

    func testPaddingChangeForcesRelayout() {
        let base = EditorTheme.default
        var padded = base
        padded.padding = LeafInsets(top: 40, left: 40, bottom: 40, right: 40)
        XCTAssertTrue(base.metricsDiffer(from: padded))
    }

    func testHeadingSizeClamps() {
        let t = EditorTheme.default
        XCTAssertEqual(t.headingSize(0), t.headingSize(1), "levels clamp to 1…6")
        XCTAssertEqual(t.headingSize(9), t.headingSize(6))
    }

    func testHeadingRowTallerThanBody() {
        let t = EditorTheme.default
        XCTAssertGreaterThan(t.rowHeight(heading: 1), t.rowHeight(heading: nil))
    }

    func testMeasureChangeForcesRelayout() {
        let base = EditorTheme.default
        var narrower = base
        narrower.measure = 50
        XCTAssertTrue(base.metricsDiffer(from: narrower))
    }

    // MARK: heading leading

    func testHeadingLeadingTightensAsTypeGrows() {
        let t = EditorTheme.default
        // The ramp runs from the body's ratio at body size to `headingLineRatio`
        // at the largest heading — monotonically, so no level is looser than a
        // larger one.
        XCTAssertEqual(t.lineRatio(forHeadingScale: t.headingScale[0]), t.headingLineRatio, accuracy: 0.001)
        XCTAssertEqual(t.lineRatio(forHeadingScale: 1), t.lineRatio, accuracy: 0.001)
        let ratios = t.headingScale.map { t.lineRatio(forHeadingScale: $0) }
        XCTAssertEqual(ratios, ratios.sorted(), "bigger type, tighter leading")
        // At or below body size (h5/h6) nothing is tightened.
        XCTAssertEqual(t.lineRatio(forHeadingScale: 0.9375), t.lineRatio, accuracy: 0.001)
    }

    func testHeadingLineBoxIsTighterThanTheBodyRatioWouldMakeIt() {
        let t = EditorTheme.default
        XCTAssertLessThan(t.rowHeight(heading: 1), t.headingSize(1) * t.lineRatio,
                          "an h1 set at the body's leading would strand its two lines")
        XCTAssertEqual(t.rowHeight(heading: 6), t.headingSize(6) * t.lineRatio, accuracy: 0.001,
                       "a below-body heading keeps the body's leading")
    }

    // MARK: the measure

    func testColumnIsCappedAndCentredWhenTheViewIsWide() {
        let t = EditorTheme.default
        let (originX, width) = t.column(in: 2000)
        XCTAssertEqual(width, t.measure! * t.averageCharWidth, accuracy: 0.5)
        XCTAssertEqual(originX, 2000 - originX - width, accuracy: 1.0, "equal margins")
    }

    func testColumnClampsToThePaddingWhenTheViewIsNarrow() {
        let t = EditorTheme.default
        let (originX, width) = t.column(in: 200)
        XCTAssertEqual(originX, t.padding.left, accuracy: 0.5)
        XCTAssertEqual(width, 200 - t.padding.left - t.padding.right, accuracy: 0.5)
    }

    func testMeasureIsInCharactersSoTextSizeCarriesIt() {
        // The point width follows the font: the same 68-character measure is a
        // wider column at a larger body size. This is the whole reason the knob is
        // counted in characters rather than points.
        var small = EditorTheme.default
        small.fontSize = 12
        var large = EditorTheme.default
        large.fontSize = 24
        XCTAssertGreaterThan(large.column(in: 4000).width, small.column(in: 4000).width)
    }

    func testAverageCharWidthIsPlausibleForTheBodyFont() {
        let t = EditorTheme.default
        // Somewhere between a third and three-quarters of an em for any text face
        // — pinned loosely so it stays font- and machine-independent, but tight
        // enough to catch a measurement in the wrong units.
        XCTAssertGreaterThan(t.averageCharWidth, t.fontSize * 0.33)
        XCTAssertLessThan(t.averageCharWidth, t.fontSize * 0.75)
    }

    // MARK: Dynamic Type

    // `scaled(by:)` is how the iOS view turns a content-size category into a
    // theme (`LeafTextView.applyDynamicType`). What it must and must not touch is
    // the whole of the contract, so it's pinned here rather than left to the one
    // caller — the view can't be exercised in these tests, which build fixtures
    // in pure Swift and never stand a UIKit view up in a window.

    func testScalingGrowsTheTypeAndTheQuoteGutter() {
        let base = EditorTheme.default
        let big = base.scaled(by: 2)
        XCTAssertEqual(big.fontSize, base.fontSize * 2, accuracy: 0.001)
        XCTAssertEqual(big.lineHeight, base.lineHeight * 2, accuracy: 0.001)
        // The gutter is points that mean an indent: a bar that stayed 22pt out
        // from the edge while the text doubled would sit against its own quote.
        XCTAssertEqual(big.quoteIndent, base.quoteIndent * 2, accuracy: 0.001)
    }

    func testScalingLeavesTheViewsOwnLengthsAlone() {
        let base = EditorTheme.default
        let big = base.scaled(by: 3)
        // A minimum inset from the window's edge, not a piece of typography.
        XCTAssertEqual(big.padding, base.padding)
        // Rules stay a rule's thickness at any text size.
        XCTAssertEqual(big.quoteBarWidth, base.quoteBarWidth)
        XCTAssertEqual(big.ruleThickness, base.ruleThickness)
        // Already counted in characters — scaling it would apply the factor twice.
        XCTAssertEqual(big.measure, base.measure)
    }

    func testScalingKeepsLeadingProportional() {
        let base = EditorTheme.default
        let big = base.scaled(by: 1.75)
        XCTAssertEqual(big.lineRatio, base.lineRatio, accuracy: 0.001)
        XCTAssertEqual(big.headingSize(1), base.headingSize(1) * 1.75, accuracy: 0.001)
        XCTAssertEqual(big.rowHeight(heading: 1), base.rowHeight(heading: 1) * 1.75, accuracy: 0.001)
        XCTAssertEqual(big.blockGap, base.blockGap * 1.75, accuracy: 0.001)
    }

    func testScalingByOneChangesNothingAndForcesNoRelayout() {
        // The loop-breaking invariant: `applyDynamicType` runs on every theme
        // assignment, and SwiftUI re-assigns the theme on every state change. At
        // the default content size that must come out metrically identical, or
        // each relayout publishes state, which re-assigns the theme, forever.
        let base = EditorTheme.default
        XCTAssertFalse(base.metricsDiffer(from: base.scaled(by: 1)))
    }

    func testAScaledThemeForcesARelayout() {
        let base = EditorTheme.default
        XCTAssertTrue(base.metricsDiffer(from: base.scaled(by: 1.3)))
    }

    func testTheColumnWidensWithTheTextSize() {
        // The measure is in characters, so a reader at a larger content size gets
        // the same 68-character line — wider in points, and therefore wrapping in
        // a different place. That reflow is why a content-size change has to go
        // through `relayoutForWidth(force:)` rather than a repaint.
        let base = EditorTheme.default
        XCTAssertGreaterThan(base.scaled(by: 2).column(in: 4000).width,
                             base.column(in: 4000).width)
    }
}
