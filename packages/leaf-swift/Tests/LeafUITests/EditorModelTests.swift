//  EditorModelTests.swift
//
//  `LeafEditorModel` without a view attached — the state every host is in while
//  it builds the model, and the one the rendering preferences used to be lost in.

import XCTest
import LeafFFI
@testable import LeafUI

final class EditorModelTests: XCTestCase {
    /// A preference set before SwiftUI has made the text view still reaches the
    /// doc. Hosts configure the model right after `init` (Diaryx does exactly
    /// this when it opens a document), so a preference that only applied through
    /// the view left every freshly opened document rendering at the default.
    func testPreferencesApplyBeforeTheViewExists() throws {
        let model = try LeafEditorModel(source: "I am *all* in.\n")
        XCTAssertEqual(model.markupMode, .none)
        XCTAssertEqual(model.lineFlow, .fold)

        model.setMarkupMode(.full)
        model.setLineFlow(.preserve)

        XCTAssertEqual(model.markupMode, .full)
        XCTAssertEqual(model.lineFlow, .preserve)
    }

    /// And the mode a view-less model was left in is what the first frame renders:
    /// under `.full` the caret's line shows its raw delimiters as `delimiter` runs.
    func testFullModeRevealsDelimitersOnTheFirstFrame() throws {
        let model = try LeafEditorModel(source: "I am *all* in.\n")
        model.setMarkupMode(.full)

        let roles = model.doc.setUnwrapped().rows.flatMap { $0.runs }.map(\.role)
        XCTAssertTrue(roles.contains("delimiter"), "expected revealed `*`, got roles \(roles)")
    }
}
