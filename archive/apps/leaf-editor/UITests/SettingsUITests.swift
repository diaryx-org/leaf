//  SettingsUITests.swift
//
//  The iOS Settings sheet, end to end: that the document's bar opens it, that
//  its choices land, and that the open document redraws under them. On the
//  Mac the same form is the Settings window; here it is the only way to reach
//  these choices at all.

import XCTest

final class SettingsUITests: XCTestCase {
    override func setUp() { continueAfterFailure = false }

    func testSettingsChangeTheOpenDocument() {
        let app = XCUIApplication()
        app.launchArguments = ["--sample"]
        app.launch()
        let create = app.buttons["Create Document"]
        XCTAssertTrue(create.waitForExistence(timeout: 10), "no document browser")
        create.tap()
        let document = app.textViews["Document"]
        XCTAssertTrue(document.waitForExistence(timeout: 20),
                      "no editor surface in:\n\(app.debugDescription)")

        // From the defaults, whatever an earlier run left behind.
        openSettings(app)
        app.buttons["Restore Defaults"].tap()
        closeSettings(app)
        // The screen, not the surface: the surface runs the document's whole
        // height, past what XCTest will render an element's picture of.
        let before = keep(XCUIScreen.main.screenshot(), as: "defaults")

        openSettings(app)
        // The font list is pushed, and long: the family has to be scrolled to.
        app.buttons["Font"].tap()
        let family = app.buttons["Avenir Next"]
        for _ in 0..<12 where !family.isHittable { app.swipeUp() }
        family.tap()
        // Choosing a family pops the list; Back only if it has not.
        if !app.buttons["Font"].waitForExistence(timeout: 3) {
            app.navigationBars.buttons["Settings"].tap()
        }
        XCTAssertTrue(app.buttons["Font"].waitForExistence(timeout: 5),
                      "not back on the form:\n\(app.debugDescription)")
        XCTAssertTrue(shown(app.buttons["Font"]).contains("Avenir Next"),
                      "the font row says \(shown(app.buttons["Font"]))")

        row(app, "Line spacing").tap()
        app.buttons["Relaxed"].tap()
        XCTAssertTrue(shown(row(app, "Line spacing")).contains("Relaxed"),
                      "the spacing row says \(shown(row(app, "Line spacing")))")
        closeSettings(app)

        XCTAssertNotEqual(keep(XCUIScreen.main.screenshot(), as: "Avenir Next, relaxed"), before,
                          "the document did not redraw under the new settings")

        openSettings(app)
        app.buttons["Restore Defaults"].tap()
        XCTAssertTrue(shown(app.buttons["Font"]).contains("Iowan Old Style"),
                      "Restore Defaults left the font row at \(shown(app.buttons["Font"]))")
        closeSettings(app)
    }

    /// The screen, kept in the result bundle so a run can be looked at as
    /// well as believed, and its bytes to compare.
    private func keep(_ shot: XCUIScreenshot, as name: String) -> Data {
        let attachment = XCTAttachment(screenshot: shot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        return shot.pngRepresentation
    }

    /// A form row by its name. A pushed picker's row is labelled with the
    /// name alone, a menu picker's with the name and its choice.
    private func row(_ app: XCUIApplication, _ name: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", name)).firstMatch
    }

    /// What a form row says: its name in the label, its choice in the value.
    private func shown(_ row: XCUIElement) -> String {
        "\(row.label) \(row.value as? String ?? "")"
    }

    /// Done, and the sheet gone: a screenshot taken while it slides away
    /// would differ from any other for no reason of the document's.
    private func closeSettings(_ app: XCUIApplication) {
        let done = app.buttons["Done"]
        done.tap()
        XCTAssertTrue(done.waitForNonExistence(timeout: 5), "the Settings sheet did not close")
    }

    private func openSettings(_ app: XCUIApplication) {
        app.navigationBars.buttons["Document Options"].tap()
        let settings = app.buttons["Settings…"]
        XCTAssertTrue(settings.waitForExistence(timeout: 5),
                      "no Settings item in:\n\(app.debugDescription)")
        settings.tap()
        XCTAssertTrue(app.buttons["Done"].waitForExistence(timeout: 5), "the Settings sheet did not open")
    }
}
