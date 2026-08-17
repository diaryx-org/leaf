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
        let dv = docView([row([mkRun("x")])], dirty: true, view: "source", heading: 2,
                         active: ["bold", "italic"], link: "https://x.dev")
        let s = EditorState(dv)
        XCTAssertEqual(s.view, "source")
        XCTAssertTrue(s.dirty)
        XCTAssertEqual(s.heading, 2)
        XCTAssertEqual(s.active, ["bold", "italic"])
        XCTAssertEqual(s.link, "https://x.dev")
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

    /// The whole reason the destination rides the state: stepping in and out of a
    /// link is a change the toolbar has to be *told about*. `EditorState` is only
    /// republished when it differs, and nothing else on it moves with the caret
    /// here — so if `link` weren't part of the comparison, the Link button's pill
    /// would still be lit on plain text.
    func testSteppingOutOfALinkIsAChangedState() {
        let inALink = EditorState(view: "wysiwyg", dirty: false, heading: nil, active: [], link: "https://x.dev")
        let outside = EditorState(view: "wysiwyg", dirty: false, heading: nil, active: [], link: nil)
        XCTAssertNotEqual(inALink, outside)
        // And one link to another — re-pointing has to relight the seed too.
        let elsewhere = EditorState(view: "wysiwyg", dirty: false, heading: nil, active: [], link: "https://y.dev")
        XCTAssertNotEqual(inALink, elsewhere)
    }

    /// The memberwise initializer keeps compiling for a host that wrote one out
    /// before there was a destination on it.
    func testLinkDefaultsToNone() {
        XCTAssertNil(EditorState(view: "wysiwyg", dirty: false, heading: nil, active: []).link)
    }
}
