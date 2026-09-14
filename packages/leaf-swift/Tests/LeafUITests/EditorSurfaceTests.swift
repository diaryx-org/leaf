//  EditorSurfaceTests.swift
//
//  What the macOS editor surface tells SwiftUI it is, for each way SwiftUI can
//  ask. Every proposal is answered from the proposal alone: an answer that
//  came from measuring the scroll view is one that changes as the document
//  wraps and its pictures arrive, and a split-view column whose maximum keeps
//  changing is a constraints pass that never settles.

#if canImport(AppKit)
import SwiftUI
import XCTest
@testable import LeafUI

@available(macOS 13.0, *)
final class EditorSurfaceTests: XCTestCase {
    func testAFullProposalIsTheAnswer() {
        let size = LeafEditorSurface.surfaceSize(for: ProposedViewSize(width: 640, height: 480))
        XCTAssertEqual(size, CGSize(width: 640, height: 480))
    }

    func testTheMinimumProbeIsNothing() {
        XCTAssertEqual(LeafEditorSurface.surfaceSize(for: .zero), .zero)
    }

    func testTheIdealProbeIsNotMeasured() {
        // `.unspecified` is the ask that used to fall through to the scroll
        // view's fitting size. It is zero now: the surface has no size of its own.
        XCTAssertEqual(LeafEditorSurface.surfaceSize(for: .unspecified), .zero)
    }

    func testTheMaximumProbePassesInfinityThrough() {
        let size = LeafEditorSurface.surfaceSize(for: .infinity)
        XCTAssertEqual(size.width, .infinity)
        XCTAssertEqual(size.height, .infinity)
    }

    func testOneOpenAxisAnswersTheOther() {
        let size = LeafEditorSurface.surfaceSize(for: ProposedViewSize(width: 300, height: nil))
        XCTAssertEqual(size, CGSize(width: 300, height: 0))
    }
}
#endif
