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

    /// An *edit* issued while no view exists reaches the doc too, and the
    /// model does the view's bookkeeping in its place: the state goes dirty
    /// and the host hears `onEdit`. The case is a host that imports a picture
    /// asynchronously and inserts it on return — by then a picker sheet or a
    /// push may have taken the surface down — and the insert used to vanish.
    func testEditsApplyBeforeTheViewExists() throws {
        let model = try LeafEditorModel(source: "A line.\n")
        var edits = 0
        model.onEdit = { edits += 1 }
        XCTAssertFalse(model.state.dirty)

        model.insertMedia(.image, destination: "attachments/photo.jpg")

        XCTAssertTrue(model.source().contains("attachments/photo.jpg"), model.source())
        XCTAssertTrue(model.state.dirty)
        XCTAssertEqual(edits, 1)
    }

    /// A command that changes nothing is not an edit: no `onEdit`, and the
    /// state is simply republished.
    func testANoOpCommandIsNotAnEdit() throws {
        let model = try LeafEditorModel(source: "A line.\n")
        var edits = 0
        model.onEdit = { edits += 1 }

        model.tableDeleteRow()   // no table under the caret

        XCTAssertEqual(edits, 0)
        XCTAssertFalse(model.state.dirty)
    }
}
