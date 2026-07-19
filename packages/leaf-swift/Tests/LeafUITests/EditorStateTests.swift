//  EditorStateTests.swift
//
//  The chrome-facing projection of a `DocView` and its `Equatable` conformance —
//  the latter is load-bearing: the SwiftUI host only republishes (and so only
//  triggers a relayout) when the state actually changed. See `LeafEditorModel`.

import XCTest
import LeafFFI
@testable import LeafUI

final class EditorStateTests: XCTestCase {
    func testProjectsDocViewChrome() {
        let dv = docView([row([mkRun("x")])], dirty: true, view: "source", heading: 2, active: ["bold", "italic"])
        let s = EditorState(dv)
        XCTAssertEqual(s.view, "source")
        XCTAssertTrue(s.dirty)
        XCTAssertEqual(s.heading, 2)
        XCTAssertEqual(s.active, ["bold", "italic"])
    }

    func testEquatable() {
        let a = EditorState(view: "wysiwyg", dirty: false, heading: nil, active: [])
        let b = EditorState(view: "wysiwyg", dirty: false, heading: nil, active: [])
        let dirtyChanged = EditorState(view: "wysiwyg", dirty: true, heading: nil, active: [])
        let marksChanged = EditorState(view: "wysiwyg", dirty: false, heading: nil, active: ["bold"])
        XCTAssertEqual(a, b)
        XCTAssertNotEqual(a, dirtyChanged)
        XCTAssertNotEqual(a, marksChanged)
    }
}
