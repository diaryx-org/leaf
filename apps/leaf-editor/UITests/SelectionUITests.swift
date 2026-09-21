//  SelectionUITests.swift
//
//  The one thing about the iOS surface no unit test can answer: whether the
//  *system's* selection machinery — `UITextInteraction`'s double-tap, its
//  handles, its edit menu — actually reaches a custom `UITextInput` view. It
//  runs against the real simulator, taps real pixels, and asks the system UI
//  what it did.

import XCTest

final class SelectionUITests: XCTestCase {
    override func setUp() { continueAfterFailure = false }

    /// Double-tapping a word selects it, which the system says by offering Copy.
    /// A tap that only placed the caret offers Select / Select All / Paste and no
    /// Copy at all, so the item's presence is the selection's proof — and since
    /// the view answers `canPerformAction` for Copy/Cut out of the *document's*
    /// `hasSelection`, their presence means core agrees there is one, not just
    /// that the system drew a highlight.
    func testDoubleTapSelectsAWordAndOffersCopy() {
        let app = XCUIApplication()
        // A document app opens to the browser; Create makes the sample under
        // this argument (see `SampleDocument`), so there is text to select.
        app.launchArguments = ["--sample"]
        app.launch()
        let create = app.buttons["Create Document"]
        XCTAssertTrue(create.waitForExistence(timeout: 10), "no document browser")
        create.tap()
        let window = app.windows.firstMatch
        XCTAssertTrue(window.waitForExistence(timeout: 10))

        // Into the opening paragraph — below the navigation bar and the
        // formatting bar, above the keyboard, well inside the text.
        // The surface is one accessibility element whose value is its text.
        let document = app.textViews["Document"]
        XCTAssertTrue(document.waitForExistence(timeout: 20),
                      "no editor surface in:\n\(app.debugDescription)")
        XCTAssertTrue((document.value as? String)?.contains("leaf, natively") == true,
                      "the sample did not open")
        let word = document.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.06))
        word.doubleTap()

        XCTAssertTrue(app.menuItems["Copy"].waitForExistence(timeout: 5),
                      "double-tap left no selection: the edit menu offered no Copy")
        XCTAssertTrue(app.menuItems["Cut"].exists,
                      "the menu offered Copy but not Cut")
    }
}
