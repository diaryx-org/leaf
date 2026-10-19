import XCTest

/// The spike's accessibility tree as XCUITest sees it: every descendant by
/// type, label and frame; then the button pressed by its accessibility frame.
final class ReadTree: XCTestCase {
    func testTree() throws {
        let app = XCUIApplication(bundleIdentifier: "org.diaryx.leaf.agent-blitz-a11y")
        app.activate()
        sleep(3)
        dump(app, "BEFORE")
        let button = app.buttons["Press me"]
        print("BUTTON exists=\(button.exists)")
        if button.exists {
            print("BUTTON frame=\(button.frame)")
            button.tap()
            sleep(2)
            dump(app, "AFTER")
        }
    }

    private func dump(_ app: XCUIApplication, _ tag: String) {
        let all = app.descendants(matching: .any).allElementsBoundByIndex
        print("\(tag) \(all.count) descendants")
        for e in all {
            print("\(tag)   \(e.elementType.rawValue) label='\(e.label)' value='\(String(describing: e.value))' frame=\(e.frame)")
        }
    }
}
