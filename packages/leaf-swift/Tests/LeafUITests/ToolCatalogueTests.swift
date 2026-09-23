//  ToolCatalogueTests.swift
//
//  The formatting catalogue's promises that no screenshot checks: that every
//  tool's ID is the one it shipped with (a reader's saved arrangement will be a
//  list of them), that the lights read the published state, and — on iOS — that
//  the panel takes the height of the keyboard for the orientation it is in.

import XCTest
import LeafFFI
@testable import LeafUI

final class ToolCatalogueTests: XCTestCase {
    private func catalogue(_ source: String) throws -> (LeafEditorModel, ToolCatalogue) {
        let model = try LeafEditorModel(source: source)
        return (model, ToolCatalogue(editor: model, beginLink: {}))
    }

    /// The IDs, spelled out. Renaming one is a change a reader with a saved
    /// row would feel, so it should take editing this list to do it.
    func testTheIDsAreTheOnesTheyShippedWith() throws {
        let (_, tools) = try catalogue("Some text.\n")
        let items = [
            tools.bold, tools.italic, tools.underline, tools.strikethrough, tools.code,
            tools.highlight, tools.link, tools.style, tools.heading(1), tools.paragraph,
            tools.quote, tools.codeBlock, tools.list, tools.bulletList, tools.numberedList,
            tools.checklist, tools.indent, tools.outdent, tools.moveUp, tools.moveDown,
            tools.align, tools.text, tools.insert, tools.rule, tools.footnote,
            tools.pageBreak, tools.undo, tools.redo, tools.format, tools.lists,
        ]
        XCTAssertEqual(items.map(\.id), [
            "bold", "italic", "underline", "strikethrough", "code",
            "highlight", "link", "style", "heading-1", "body",
            "quote", "code-block", "list", "bullet-list", "numbered-list",
            "checklist", "indent", "outdent", "move-up", "move-down",
            "align", "text", "insert", "rule", "footnote",
            "page-break", "undo", "redo", "format", "lists",
        ])
        XCTAssertEqual(Set(items.map(\.id)).count, items.count, "every ID is its own")
    }

    /// Style names the caret's block, and its menu is the whole tool.
    func testStyleNamesTheCaretsBlock() throws {
        let (model, tools) = try catalogue("# Title\n\nBody.\n")
        XCTAssertEqual(model.state.heading, 1)
        XCTAssertEqual(tools.style.glyph, .text("H1"))
        XCTAssertEqual(tools.styleName(short: false), "Heading 1")
        XCTAssertNil(tools.style.action, "Style opens its menu on a tap")
        XCTAssertNotNil(tools.style.variants)
    }

    /// Every name Style can show is among the ones the bar sizes its button
    /// to, or the button would clip a name it had not measured.
    func testEveryStyleNameIsMeasured() throws {
        for source in ["# One\n", "###### Six\n", "Body.\n", "```\ncode\n```\n"] {
            let (_, tools) = try catalogue(source)
            XCTAssertTrue(ToolCatalogue.styleNames(short: true).contains(tools.styleName(short: true)), source)
            XCTAssertTrue(ToolCatalogue.styleNames(short: false).contains(tools.styleName(short: false)), source)
        }
    }

    /// A light follows the published state: Bold lights inside bold text.
    func testBoldLightsInsideBoldText() throws {
        let (model, _) = try catalogue("**bold** plain\n")
        model.debugSelect(anchor: 3, focus: 3)
        XCTAssertTrue(ToolCatalogue(editor: model, beginLink: {}).bold.active)
    }

    /// Move Up and Move Down dim where the format has no blocks to move.
    func testMovingABlockIsGatedOnTheFormat() throws {
        let (model, tools) = try catalogue("A.\n\nB.\n")
        XCTAssertEqual(tools.moveUp.enabled, model.capabilities.moveBlock)
        XCTAssertEqual(tools.moveDown.enabled, model.capabilities.moveBlock)
    }

    #if canImport(UIKit)
    /// The panel takes the keyboard's height for the orientation the window is
    /// in, and a guess near a phone keyboard's before one has been measured —
    /// so a turn to landscape with the panel up doesn't leave a portrait-height
    /// panel over half the screen.
    func testThePanelTakesThisOrientationsKeyboardHeight() throws {
        FormattingPanelHost.keyboardHeights = [:]
        defer { FormattingPanelHost.keyboardHeights = [:] }
        let model = try LeafEditorModel(source: "Text.\n")
        let panel = FormattingPanelHost(editor: model)

        let portrait = UIWindow(frame: CGRect(x: 0, y: 0, width: 402, height: 874))
        let landscape = UIWindow(frame: CGRect(x: 0, y: 0, width: 874, height: 402))
        let view = UIView()
        portrait.addSubview(view)

        panel.fit(to: view)
        let guessed = panel.frame.height
        XCTAssertTrue((216...336).contains(guessed), "a phone keyboard's height, not \(guessed)")
        XCTAssertEqual(panel.frame.width, 402)

        // A portrait keyboard 336 tall under a 44pt accessory, and a landscape
        // one 209 tall under the same.
        let accessory = UIView(frame: CGRect(x: 0, y: 0, width: 402, height: 44))
        FormattingPanelHost.noteKeyboard(frame: CGRect(x: 0, y: 874 - 380, width: 402, height: 380),
                                         accessory: accessory, in: portrait)
        FormattingPanelHost.noteKeyboard(frame: CGRect(x: 0, y: 402 - 253, width: 874, height: 253),
                                         accessory: accessory, in: landscape)

        XCTAssertTrue(panel.fit(to: view), "the measured height is a move from the guess")
        XCTAssertEqual(panel.frame.height, 336)
        XCTAssertFalse(panel.fit(to: view), "and fitting again moves nothing")

        landscape.addSubview(view)
        XCTAssertTrue(panel.fit(to: view))
        XCTAssertEqual(panel.frame.height, 209)
        XCTAssertEqual(panel.frame.width, 874)
    }

    /// A keyboard going away, and a hardware keyboard's accessory-only sliver,
    /// are not heights to copy.
    func testAHiddenOrHardwareKeyboardIsNotMeasured() {
        FormattingPanelHost.keyboardHeights = [:]
        defer { FormattingPanelHost.keyboardHeights = [:] }
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 402, height: 874))
        let accessory = UIView(frame: CGRect(x: 0, y: 0, width: 402, height: 44))
        FormattingPanelHost.noteKeyboard(frame: CGRect(x: 0, y: 874, width: 402, height: 380),
                                         accessory: accessory, in: window)
        FormattingPanelHost.noteKeyboard(frame: CGRect(x: 0, y: 874 - 55, width: 402, height: 55),
                                         accessory: accessory, in: window)
        XCTAssertTrue(FormattingPanelHost.keyboardHeights.isEmpty)
    }
    #endif
}
