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
}
