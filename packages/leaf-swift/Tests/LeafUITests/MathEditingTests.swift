//  MathEditingTests.swift
//
//  A formula edited in place as structure, as the macOS view answers it:
//  arrows, typing and Backspace sent the way AppKit delivers them, and the
//  document's TeX checked after — see `MathEditing.swift`.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class MathEditingTests: XCTestCase {
    private let none = NSRange(location: NSNotFound, length: 0)

    private func editor(_ source: String) throws -> LeafTextView {
        let doc = try LeafDoc(source: source, format: "markdown")
        // What `LeafEditorModel` says for every document it shows.
        _ = doc.setInlinePictures(on: true)
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.layout()
        return view
    }

    private func math(_ view: LeafTextView) -> MathView { view.doc.view().math[0] }

    private func place(_ view: LeafTextView, _ at: UInt32) {
        view.command { $0.selectRange(start: at, end: at) }
    }

    private func send(_ selector: Selector, to view: LeafTextView) { view.doCommand(by: selector) }

    func testAnArrowOntoAFormulaOpensItAndWhatIsTypedIsTeX() throws {
        let view = try editor("say $x$ here\n")
        place(view, math(view).src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        XCTAssertTrue(view.isEditingFormula, "→ in front of the formula did not open it")
        send(#selector(NSResponder.moveRight(_:)), to: view)
        view.insertText("^2", replacementRange: none)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        view.insertText("+1/2", replacementRange: none)
        XCTAssertEqual(view.doc.source(), "say $x^{2}+\\frac{1}{2}$ here\n")
        // Out of the denominator, then out of the formula, behind it.
        send(#selector(NSResponder.moveRight(_:)), to: view)
        XCTAssertTrue(view.isEditingFormula, "left the formula a stop early")
        send(#selector(NSResponder.moveRight(_:)), to: view)
        XCTAssertFalse(view.isEditingFormula, "→ at the last stop did not leave the formula")
        XCTAssertEqual(view.doc.caretOffset(), math(view).end)
        // ← from behind it opens it again; Escape leaves it.
        send(#selector(NSResponder.moveLeft(_:)), to: view)
        XCTAssertTrue(view.isEditingFormula)
        send(#selector(NSResponder.cancelOperation(_:)), to: view)
        XCTAssertFalse(view.isEditingFormula)
    }

    func testOnTheCaretsLineAFormulaStaysItsPicture() throws {
        let view = try editor("say $x+y$ here\n")
        place(view, 1)
        XCTAssertEqual(view.doc.view().math.count, 1, "the formula revealed its TeX")
        view.editsFormulasAsStructure = false
        place(view, 2)
        XCTAssertTrue(view.doc.view().math.isEmpty, "with structure off, the caret's line still shows a picture")
    }

    func testAKeystrokeIsOneUndoStepAndUndoLeavesTheFormula() throws {
        let view = try editor("say $x$ here\n")
        place(view, math(view).end)
        send(#selector(NSResponder.moveLeft(_:)), to: view)
        // A key at a time, as the keyboard sends them. `sqrt`'s last letter
        // makes two splices: the `t`, then the root.
        for c in "+sqrt" { view.insertText(String(c), replacementRange: none) }
        XCTAssertEqual(view.doc.source(), "say $x+\\sqrt{}$ here\n")
        view.undo(nil)
        XCTAssertFalse(view.isEditingFormula, "undo did not close the formula")
        XCTAssertEqual(view.doc.source(), "say $x+sqr$ here\n")
    }

    func testBackspaceDissolvesWhatItEmptied() throws {
        let view = try editor("say $x$ here\n")
        place(view, math(view).src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        view.insertText("/", replacementRange: none)
        XCTAssertEqual(view.doc.source(), "say $\\frac{}{}x$ here\n")
        send(#selector(NSResponder.deleteBackward(_:)), to: view)
        XCTAssertEqual(view.doc.source(), "say $x$ here\n")
    }

    func testADisplayBlocksTeXStaysBetweenItsOwnLines() throws {
        let view = try editor("intro\n\n$$\n\\int_0^1 x\n$$\n\nend\n")
        let m = try XCTUnwrap(view.doc.view().math.first { !$0.inline })
        place(view, m.end)
        send(#selector(NSResponder.moveLeft(_:)), to: view)
        XCTAssertTrue(view.isEditingFormula)
        view.insertText("+1", replacementRange: none)
        XCTAssertEqual(view.doc.source(), "intro\n\n$$\n\\int_0^1 x+1\n$$\n\nend\n")
    }

    func testTheOpenFormulaIsDrawnFromItsSessionWithItsCaret() throws {
        let view = try editor("say $x$ here\n")
        place(view, math(view).src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        // The layout names the open formula by its stand-in, which the store
        // answers with the session's picture — wider than the typeset `x` by
        // nothing, but a different picture: it carries the caret.
        let laid = try XCTUnwrap(view.layoutEngine.math.first)
        XCTAssertTrue(laid.tex.hasPrefix(MathStore.openPrefix))
        XCTAssertNotNil(MathStore.glyph(tex: laid.tex, display: false, size: 16, ink: view.theme.textColor))
    }

    func testAClickOnAnInlinePictureFindsItsFormula() throws {
        let view = try editor("say $\\frac{a}{b}$ here\n")
        let row = try XCTUnwrap(view.layoutEngine.rows.firstIndex { $0.row.runs.contains { $0.role == "math" } })
        let rl = view.layoutEngine.rows[row]
        // Somewhere over the picture: right of `say `, through the line.
        let base = try XCTUnwrap(view.layoutEngine.baselineOrigin(row: row, ch: 4))
        let hit = view.layoutEngine.mathPicture(at: CGPoint(x: base.x + 2, y: base.y - 2), theme: view.theme)
        XCTAssertEqual(hit?.math.src, math(view).src)
        XCTAssertNil(view.layoutEngine.mathPicture(at: CGPoint(x: rl.originX + 1, y: base.y - 2), theme: view.theme))
    }
}
#endif

#if canImport(UIKit)
import UIKit
import XCTest
import LeafFFI
@testable import LeafUI

/// The iOS view's half: typing and Backspace through `UIKeyInput`, the arrows
/// through the key commands it takes from the system only where a formula
/// needs them.
final class MathEditingTests: XCTestCase {
    private func editor(_ source: String) throws -> LeafTextView {
        let doc = try LeafDoc(source: source, format: "markdown")
        _ = doc.setInlinePictures(on: true)
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = CGRect(x: 0, y: 0, width: 402, height: 700)
        view.layoutIfNeeded()
        return view
    }

    private func place(_ view: LeafTextView, _ at: UInt32) {
        view.command { $0.selectRange(start: at, end: at) }
    }

    /// Press `input` as a hardware keyboard would: through the view's key
    /// command for it. `false` when the view does not take that key here.
    @discardableResult
    private func press(_ view: LeafTextView, _ input: String) -> Bool {
        guard let c = view.keyCommands?.first(where: { $0.input == input && $0.modifierFlags.isEmpty }),
              let action = c.action
        else { return false }
        view.perform(action, with: c)
        return true
    }

    func testAnArrowOntoAFormulaOpensItAndWhatIsTypedIsTeX() throws {
        let view = try editor("say $x$ here\n")
        let m = view.doc.view().math[0]
        place(view, 1)
        XCTAssertFalse(press(view, UIKeyCommand.inputRightArrow), "→ away from a formula is not the system's")
        place(view, m.src)
        XCTAssertTrue(press(view, UIKeyCommand.inputRightArrow))
        XCTAssertTrue(view.isEditingFormula)
        press(view, UIKeyCommand.inputRightArrow)
        for c in "^2" { view.insertText(String(c)) }
        press(view, UIKeyCommand.inputRightArrow)
        for c in "+1/2" { view.insertText(String(c)) }
        XCTAssertEqual(view.doc.source(), "say $x^{2}+\\frac{1}{2}$ here\n")
        view.deleteBackward()
        XCTAssertEqual(view.doc.source(), "say $x^{2}+\\frac{1}{}$ here\n")
        press(view, UIKeyCommand.inputRightArrow)
        press(view, UIKeyCommand.inputRightArrow)
        XCTAssertFalse(view.isEditingFormula, "→ at the last stop did not leave the formula")
        XCTAssertEqual(view.doc.caretOffset(), view.doc.view().math[0].end)
    }

    func testReturnLeavesAndASelectionElsewhereCloses() throws {
        let view = try editor("say $x$ here\n")
        let m = view.doc.view().math[0]
        place(view, m.end)
        XCTAssertTrue(press(view, UIKeyCommand.inputLeftArrow))
        XCTAssertTrue(view.isEditingFormula)
        view.insertText("\n")
        XCTAssertFalse(view.isEditingFormula)
        XCTAssertEqual(view.doc.source(), "say $x$ here\n", "Return in a formula wrote a newline")
        press(view, UIKeyCommand.inputLeftArrow)
        XCTAssertTrue(view.isEditingFormula)
        view.selectedTextRange = LeafTextRange(LeafTextPosition(0), LeafTextPosition(0))
        XCTAssertFalse(view.isEditingFormula)
    }
}
#endif
