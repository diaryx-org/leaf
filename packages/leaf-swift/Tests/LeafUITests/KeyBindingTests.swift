//  KeyBindingTests.swift
//
//  The rest of Cocoa's standard key bindings, as the macOS view answers them:
//  sent through `doCommand(by:)`, the way `interpretKeyEvents` delivers a
//  chord, so the routing is tested along with the edit.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class KeyBindingTests: XCTestCase {
    override func setUp() {
        super.setUp()
        LeafTextView.killBuffer = ""
    }

    private func editor(_ source: String, caret: Int? = nil, width: CGFloat = 600) throws -> LeafTextView {
        let view = LeafTextView(doc: try LeafDoc(source: source, format: "markdown"), theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: width, height: 400)
        view.layout()
        if let caret { place(view, caret) }
        return view
    }

    private func place(_ view: LeafTextView, _ caret: Int) {
        view.command { $0.selectRange(start: UInt32(caret), end: UInt32(caret)) }
    }

    private func select(_ view: LeafTextView, _ from: Int, _ to: Int) {
        view.command { $0.selectRange(start: UInt32(from), end: UInt32(to)) }
    }

    private func send(_ selector: Selector, to view: LeafTextView) { view.doCommand(by: selector) }
    private func undo(_ view: LeafTextView) { view.command { $0.undo() } }

    // MARK: deleting to an edge, and the kill buffer

    func testControlKKillsToTheParagraphsEndAndControlYGivesItBack() throws {
        let view = try editor("hello world\n\nnext\n", caret: 6)
        send(#selector(NSResponder.deleteToEndOfParagraph(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "hello \n\nnext\n")
        XCTAssertEqual(LeafTextView.killBuffer, "world")
        undo(view)
        XCTAssertEqual(view.sourceText(), "hello world\n\nnext\n", "one undo step")
        send(#selector(NSResponder.deleteToEndOfParagraph(_:)), to: view)
        send(#selector(NSResponder.moveToBeginningOfDocument(_:)), to: view)
        send(#selector(NSResponder.yank(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "worldhello \n\nnext\n")
    }

    func testControlKAtAParagraphsEndJoinsTheNextAndChainsTheKill() throws {
        let view = try editor("one two\n\nthree\n", caret: 4)
        send(#selector(NSResponder.deleteToEndOfParagraph(_:)), to: view)
        send(#selector(NSResponder.deleteToEndOfParagraph(_:)), to: view)
        // At the end the paragraph break goes exactly as Delete would take it.
        let deleted = try editor("one \n\nthree\n", caret: 4)
        send(#selector(NSResponder.deleteForward(_:)), to: deleted)
        XCTAssertEqual(view.sourceText(), deleted.sourceText())
        XCTAssertNotEqual(view.sourceText(), "one \n\nthree\n")
        XCTAssertEqual(LeafTextView.killBuffer, "two\n", "a kill straight after a kill adds to it")
    }

    func testCommandBackspaceDeletesToTheVisualLinesStart() throws {
        let view = try editor("hello world\n", caret: 8)
        send(#selector(NSResponder.deleteToBeginningOfLine(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "rld\n")
        XCTAssertEqual(LeafTextView.killBuffer, "hello wo")
    }

    func testCommandBackspaceStopsAtTheWrapNotTheParagraph() throws {
        let words = Array(repeating: "word", count: 40).joined(separator: " ")
        let view = try editor(words + "\n", width: 300)
        let wrapped = view.layoutEngine.rows[0].wrapped
        try XCTSkipIf(wrapped.count < 2, "needs a paragraph that wraps")
        // The end of the second visual line.
        let secondEnd = wrapped[1].start + wrapped[1].length - 1
        let off = Int(view.doc.offsetForPos(row: 0, ch: UInt32(secondEnd)))
        place(view, off)
        send(#selector(NSResponder.deleteToBeginningOfLine(_:)), to: view)
        XCTAssertTrue(view.sourceText().hasPrefix(String(words.prefix(wrapped[1].start))),
                      "the first visual line is untouched")
        XCTAssertLessThan(view.sourceText().utf8.count, words.utf8.count)
    }

    func testDeletingToAnEdgeTakesTheSelectionFirst() throws {
        let view = try editor("hello world\n")
        select(view, 0, 6)
        send(#selector(NSResponder.deleteToEndOfLine(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "world\n")
    }

    func testAReadOnlyDocumentKillsNothing() throws {
        let view = try editor("hello world\n", caret: 6)
        view.command { $0.setReadOnly(on: true) }
        view.isReadOnly = true
        send(#selector(NSResponder.deleteToEndOfParagraph(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "hello world\n")
        XCTAssertEqual(LeafTextView.killBuffer, "")
    }

    // MARK: motion

    func testControlAAndControlEReachTheParagraphsTextNotItsBullet() throws {
        let view = try editor("- an item here\n", caret: 6)
        send(#selector(NSResponder.moveToBeginningOfParagraph(_:)), to: view)
        XCTAssertEqual(view.doc.caretOffset(), 2, "past the `- `")
        send(#selector(NSResponder.moveToEndOfParagraphAndModifySelection(_:)), to: view)
        XCTAssertEqual(view.doc.selectedText(), "an item here")
    }

    func testParagraphForwardWalksParagraphByParagraph() throws {
        let view = try editor("one\n\ntwo\n\nthree\n", caret: 0)
        send(#selector(NSResponder.moveParagraphForwardAndModifySelection(_:)), to: view)
        XCTAssertEqual(view.doc.selectedText(), "one")
        send(#selector(NSResponder.moveParagraphForwardAndModifySelection(_:)), to: view)
        XCTAssertEqual(view.doc.caretOffset(), 8, "the end of `two`")
    }

    func testForwardAndBackwardAreRightAndLeft() throws {
        let view = try editor("abc\n", caret: 1)
        send(#selector(NSResponder.moveForward(_:)), to: view)
        XCTAssertEqual(view.doc.caretOffset(), 2)
        send(#selector(NSResponder.moveBackwardAndModifySelection(_:)), to: view)
        XCTAssertEqual(view.doc.selectedText(), "b")
    }

    // MARK: small edits

    func testControlTSwapsTheCharactersAroundTheCaret() throws {
        let view = try editor("abcd\n", caret: 1)
        send(#selector(NSResponder.transpose(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "bacd\n")
        XCTAssertEqual(view.doc.caretOffset(), 2)
        place(view, 4)
        send(#selector(NSResponder.transpose(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "badc\n", "at the end, the two before it")
    }

    func testControlTLeavesMarkupItWouldHaveToRewrite() throws {
        let view = try editor("a**b** c\n", caret: 1)
        send(#selector(NSResponder.transpose(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "a**b** c\n")
    }

    func testControlBackspaceTakesTheAccentOff() throws {
        let view = try editor("café\n", caret: 5)
        send(#selector(NSResponder.deleteBackwardByDecomposingPreviousCharacter(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "cafe\n")
        send(#selector(NSResponder.deleteBackwardByDecomposingPreviousCharacter(_:)), to: view)
        XCTAssertEqual(view.sourceText(), "caf\n", "no mark left: an ordinary delete")
    }

    // MARK: selecting by unit, and the mark

    func testSelectingByWordLineAndParagraph() throws {
        let view = try editor("hello there world\n", caret: 8)
        send(#selector(NSResponder.selectWord(_:)), to: view)
        XCTAssertEqual(view.doc.selectedText(), "there")
        send(#selector(NSResponder.selectLine(_:)), to: view)
        XCTAssertEqual(view.doc.selectedText(), "hello there world")
        place(view, 2)
        send(#selector(NSResponder.selectParagraph(_:)), to: view)
        XCTAssertEqual(view.doc.selectedText(), "hello there world")
    }

    func testTheMarkSelectsAndSwaps() throws {
        let view = try editor("hello world\n", caret: 2)
        send(#selector(NSResponder.setMark(_:)), to: view)
        place(view, 8)
        send(#selector(NSResponder.selectToMark(_:)), to: view)
        XCTAssertEqual(view.doc.selectedText(), "llo wo")
        place(view, 8)
        send(#selector(NSResponder.swapWithMark(_:)), to: view)
        XCTAssertEqual(view.doc.caretOffset(), 2)
    }

    // MARK: Edit ▸ Transformations

    func testCaseChangesTheSelectionAsOneUndoStep() throws {
        let view = try editor("hello wORLD\n")
        select(view, 0, 11)
        view.capitalizeWord(nil)
        XCTAssertEqual(view.sourceText(), "Hello World\n")
        XCTAssertEqual(view.doc.selectedText(), "Hello World", "the selection stays over the text")
        undo(view)
        XCTAssertEqual(view.sourceText(), "hello wORLD\n")
    }

    func testCaseChangesTheCaretsWordWithoutASelection() throws {
        let view = try editor("plain **bold** text\n", caret: 9)
        view.uppercaseWord(nil)
        XCTAssertEqual(view.sourceText(), "plain **BOLD** text\n")
    }

    func testCaseChangesLeaveMarkupAndDestinationsAlone() throws {
        let view = try editor("see [the link](http://a.example/Path) now\n")
        view.selectAll(nil)
        view.uppercaseWord(nil)
        XCTAssertEqual(view.sourceText(), "SEE [THE LINK](http://a.example/Path) NOW\n")
    }

    func testTheTransformationsItemsAreValidated() throws {
        let view = try editor("   \n", caret: 1)
        let item = NSMenuItem(title: "Make Upper Case", action: #selector(NSResponder.uppercaseWord(_:)),
                              keyEquivalent: "")
        XCTAssertFalse(view.validateUserInterfaceItem(item), "no word at the caret")
        let worded = try editor("word\n", caret: 1)
        XCTAssertTrue(worded.validateUserInterfaceItem(item))
        worded.isReadOnly = true
        XCTAssertFalse(worded.validateUserInterfaceItem(item))
    }
}
#endif
