//  SpeechTests.swift
//
//  Edit ▸ Speech and the Substitutions panel on the macOS view: what is
//  said, and when the menu may offer it. Nothing here speaks — a test that
//  talked out loud on the machine running it would be a poor neighbour.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class SpeechTests: XCTestCase {
    private func editor(_ source: String) throws -> LeafTextView {
        let view = LeafTextView(doc: try LeafDoc(source: source, format: "markdown"), theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.layout()
        return view
    }

    private func item(_ action: Selector) -> NSMenuItem {
        NSMenuItem(title: "", action: action, keyEquivalent: "")
    }

    func testTheSelectionIsSpokenWhenThereIsOne() throws {
        let view = try editor("Read **this** aloud\n")
        view.command { $0.selectRange(start: 0, end: 4) }
        XCTAssertEqual(view.textToSpeak(), "Read")
    }

    func testOtherwiseTheWholeDocumentWithoutItsMarkup() throws {
        let view = try editor("Read **this** aloud\n")
        XCTAssertFalse(view.textToSpeak().contains("*"))
        XCTAssertTrue(view.textToSpeak().hasPrefix("Read this aloud"))
    }

    func testTheSpeechItemsAreValidated() throws {
        let view = try editor("words\n")
        XCTAssertTrue(view.validateUserInterfaceItem(item(#selector(LeafTextView.startSpeaking(_:)))))
        XCTAssertFalse(view.validateUserInterfaceItem(item(#selector(LeafTextView.stopSpeaking(_:)))),
                       "nothing is being said")
        let empty = try editor("")
        XCTAssertFalse(empty.validateUserInterfaceItem(item(#selector(LeafTextView.startSpeaking(_:)))))
    }

    func testTheSubstitutionsPanelIsOffered() throws {
        let view = try editor("words\n")
        XCTAssertTrue(view.validateUserInterfaceItem(
            item(#selector(LeafTextView.orderFrontSubstitutionsPanel(_:)))))
    }
}
#endif
