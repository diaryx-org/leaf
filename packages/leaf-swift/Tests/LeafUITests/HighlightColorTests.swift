//  HighlightColorTests.swift
//
//  The colour of a `==🔴 text==` highlight, from the three angles the frontend
//  touches it: the palette a menu is built from, the state a menu ticks from,
//  and the gesture itself through a real `LeafDoc`.

import XCTest
import LeafFFI
@testable import LeafUI

final class HighlightColorTests: XCTestCase {
    // MARK: the palette

    /// Every colour the menu offers can be drawn and can be written: a wash from
    /// the theme's own palette, a distinct swatch, and a name core reads back.
    /// The list is hand-written (the binding's enum isn't `CaseIterable`), so
    /// this is what keeps it whole.
    func testPaletteIsWholeAndDrawable() {
        XCTAssertEqual(MarkColor.palette.count, 7)
        XCTAssertEqual(Set(MarkColor.palette).count, 7, "no colour twice")
        XCTAssertEqual(Set(MarkColor.palette.map(\.name)).count, 7)
        XCTAssertEqual(Set(MarkColor.palette.map(\.swatch)).count, 7)
        for color in MarkColor.palette {
            XCTAssertNotNil(
                Palette.markBackground(named: color.name),
                "\(color.name) is offered by the menu and has no wash to draw it"
            )
            XCTAssertTrue(color.menuTitle.hasPrefix(color.swatch), "\(color.name)")
        }
    }

    /// The names are core's, not this file's invention: a document written with
    /// each colour reads that colour back across the binding.
    func testEveryColourRoundTripsThroughADocument() throws {
        for color in MarkColor.palette {
            let doc = try LeafDoc(source: "a ==word== b\n", format: "markdown")
            doc.setSelectionOffsets(anchor: 5, focus: 5)
            let view = doc.setMarkColor(color: color)
            XCTAssertEqual(view.markColor, color, "\(color.name)")
            XCTAssertTrue(
                doc.source().contains("==\(color.swatch) word=="),
                "\(color.name) wrote \(doc.source())"
            )
        }
    }

    // MARK: the state a menu ticks from

    func testStateCarriesTheCaretsColour() {
        let dv = docView([row([mkRun("x", role: "mark", markColor: "red")])], markColor: .red)
        XCTAssertEqual(EditorState(dv).markColor, .red)
        XCTAssertNil(EditorState(docView([row([mkRun("x")])])).markColor)
    }

    /// Moving between two coloured highlights changes nothing else on the frame,
    /// so the colour has to be part of what makes a state unequal — otherwise the
    /// host never republishes and the tick never moves.
    func testColourIsPartOfTheState() {
        let red = EditorState(docView([row([mkRun("x")])], markColor: .red))
        let blue = EditorState(docView([row([mkRun("x")])], markColor: .blue))
        let none = EditorState(docView([row([mkRun("x")])]))
        XCTAssertNotEqual(red, blue)
        XCTAssertNotEqual(red, none)
    }

    /// The same for the selection flag, which decides whether a swatch colours
    /// the highlight the caret is in or makes one out of what is chosen.
    func testSelectionIsPartOfTheState() {
        let bare = EditorState(docView([row([mkRun("x")])]))
        let chosen = EditorState(docView([row([mkRun("x")])], hasSelection: true))
        XCTAssertFalse(bare.hasSelection)
        XCTAssertTrue(chosen.hasSelection)
        XCTAssertNotEqual(bare, chosen)
    }

    // MARK: the gesture, and when a menu may offer it

    /// A colour belongs to a highlight that already exists. The document says so
    /// twice — through the caret, and through the format, which are the two
    /// halves `canColourHighlight` puts together.
    func testTheTwoGatesAskDifferentQuestions() throws {
        let md = try LeafDoc(source: "a ==word== b\n", format: "markdown")
        md.setSelectionOffsets(anchor: 5, focus: 5)
        XCTAssertTrue(md.capabilities().markColor, "markdown spells `==🔴 x==`")
        XCTAssertTrue(md.caretInMark())

        md.setSelectionOffsets(anchor: 0, focus: 0)
        XCTAssertFalse(md.caretInMark(), "no highlight at the start of the line")

        let dj = try LeafDoc(source: "a {=word=} b\n", format: "djot")
        dj.setSelectionOffsets(anchor: 5, focus: 5)
        XCTAssertTrue(dj.caretInMark(), "djot writes the highlight")
        XCTAssertFalse(dj.capabilities().markColor, "and no colour on it")
    }

    /// The model's own gate, over a document with no view attached — which is
    /// where a host configures it, and where the toolbar reads it.
    func testTheModelDimsThePaletteWithNothingToColour() throws {
        let model = try LeafEditorModel(source: "a ==word== b\n")
        XCTAssertTrue(model.capabilities.markColor)
        XCTAssertFalse(model.caretInMark, "the caret opens at the top of the document")
        XCTAssertFalse(model.canColourHighlight, "so there is nothing for a swatch to land on")

        model.doc.setSelectionOffsets(anchor: 5, focus: 5)
        XCTAssertTrue(model.caretInMark)
        XCTAssertTrue(model.canColourHighlight)
    }

    /// A colour is not a highlight: pressed where there is neither a highlight
    /// nor a selection, it writes nothing rather than inventing one.
    func testAColourWithoutAHighlightWritesNothing() throws {
        let doc = try LeafDoc(source: "a word b\n", format: "markdown")
        doc.setSelectionOffsets(anchor: 3, focus: 3)
        XCTAssertNil(doc.setMarkColor(color: .red).markColor)
        XCTAssertEqual(doc.source(), "a word b\n")
    }

    /// And the sequence a swatch actually performs over a selection: the
    /// highlight, then its colour — two edits, because core keeps one gesture to
    /// one splice, and the second finds the highlight the first just made even
    /// though the caret it left is past the closing `==`.
    func testHighlightingThenColouringASelection() throws {
        let doc = try LeafDoc(source: "a word b\n", format: "markdown")
        doc.setSelectionOffsets(anchor: 2, focus: 6)
        doc.toggleMark()
        XCTAssertEqual(doc.source(), "a ==word== b\n")

        let view = doc.setMarkColor(color: .green)
        XCTAssertEqual(doc.source(), "a ==\u{1F7E2} word== b\n")
        XCTAssertEqual(view.markColor, .green)

        // Taking the colour off leaves the highlight, which is the other half of
        // "No Colour" being its own row rather than a second press of the swatch.
        XCTAssertNil(doc.setMarkColor(color: nil).markColor)
        XCTAssertEqual(doc.source(), "a ==word== b\n")
        XCTAssertTrue(doc.caretInMark())
    }
}
