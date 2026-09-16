import XCTest
import LeafFFI
@testable import LeafUI

/// Exercises the public commands used by LeafUI, including actual Rust parsing.
final class TableFormattingTests: XCTestCase {
    private let prefix = "| A | B |\n| --- | --- |\n| "
    private let suffix = " | other |\n"

    private func makeDoc(_ source: String) throws -> LeafDoc {
        let doc = try LeafDoc(source: source, format: "markdown")
        _ = doc.setUnwrapped()
        _ = doc.setMarkupMode(mode: .none)
        return doc
    }

    private func visible(_ doc: LeafDoc) -> [[String]] {
        doc.view().tables.flatMap { table in
            table.grid.map { row in
                row.cells.map { cell in
                    cell.lines.map { $0.runs.map(\.text).joined() }.joined(separator: "\n")
                }
            }
        }
    }

    private func styled(_ run: Run, _ style: String) -> Bool {
        switch style {
        case "bold": return run.bold
        case "italic": return run.italic
        case "strike": return run.strike
        default: return run.role == style
        }
    }

    func testPlainCellStylesApplyRemoveReapplyAndUndoRedo() throws {
        // Underline is deliberately excluded: Markdown does not support it.
        for style in ["bold", "italic", "strike", "code", "mark"] {
            for text in ["hello", "café🙂", "two words"] {
                let source = prefix + text + suffix
                let doc = try makeDoc(source)
                _ = doc.setSelectionOffsets(anchor: UInt32(prefix.utf8.count),
                                            focus: UInt32((prefix + text).utf8.count))
                let toggle: () -> DocView = {
                    switch style {
                    case "bold": return doc.toggleBold()
                    case "italic": return doc.toggleItalic()
                    case "strike": return doc.toggleStrike()
                    case "code": return doc.toggleCode()
                    default: return doc.toggleMark()
                    }
                }
                let expected = visible(doc)
                for iteration in 0..<4 {
                    let frame = toggle()
                    XCTAssertEqual(visible(doc), expected, "\(style), \(text), toggle \(iteration)")
                    let runs = try XCTUnwrap(frame.tables.first?.grid.last?.cells.first).lines.flatMap(\.runs)
                    XCTAssertFalse(runs.isEmpty)
                    XCTAssertTrue(runs.allSatisfy { styled($0, style) == (iteration % 2 == 0) },
                                  "\(style) must actually change, not silently do nothing")
                    if iteration % 2 == 1 { XCTAssertEqual(doc.source(), source) }
                }
                _ = doc.undo()
                XCTAssertNotEqual(doc.source(), source)
                _ = doc.redo()
                XCTAssertEqual(doc.source(), source)
            }
        }
    }

    func testExactSelectionCanRemoveAndReapplyExistingBold() throws {
        let source = prefix + "**bold**" + suffix
        let doc = try makeDoc(source)
        let start = UInt32(prefix.utf8.count + 2)
        _ = doc.selectRange(start: start, end: start + 4)
        _ = doc.toggleBold()
        XCTAssertEqual(doc.source(), prefix + "bold" + suffix)
        _ = doc.toggleBold()
        XCTAssertEqual(doc.source(), source)
        _ = doc.toggleBold()
        XCTAssertEqual(doc.source(), prefix + "bold" + suffix)
    }

    func testInteractiveSelectionOfBoldWordCanRemoveBold() throws {
        let doc = try makeDoc(prefix + "**bold**" + suffix)
        let start = UInt32(prefix.utf8.count + 2)
        _ = doc.setSelectionOffsets(anchor: start, focus: start + 4)
        XCTAssertEqual(doc.caretOffset(), start + 4)
        _ = doc.toggleBold()
        XCTAssertEqual(doc.source(), prefix + "bold" + suffix)
    }

    func testTableCaretReportsExistingStylesToToolbar() throws {
        for (text, style, delimiterLength) in [
            ("**word**", "bold", 2), ("*word*", "italic", 1),
            ("~~word~~", "strike", 2), ("==word==", "mark", 2),
            ("`word`", "code", 1)
        ] {
            // Paragraph control: the same mark is reported correctly outside a table.
            let paragraph = try makeDoc(text + "\n")
            let p = UInt32(delimiterLength + 1)
            XCTAssertTrue(paragraph.setSelectionOffsets(anchor: p, focus: p).active.contains(style))
            let doc = try makeDoc(prefix + text + suffix)
            let offset = UInt32(prefix.utf8.count) + p
            let frame = doc.setSelectionOffsets(anchor: offset, focus: offset)
            let runs = try XCTUnwrap(frame.tables.first?.grid.last?.cells.first).lines.flatMap(\.runs)
            XCTAssertTrue(runs.contains { styled($0, style) })
            XCTAssertTrue(frame.active.contains(style), style)
        }
    }

    func testBoldOverItalicCanBeToggledOffAgain() throws {
        // Include the existing delimiters, as select-all/search/source ranges can.
        for inTable in [false, true] {
            let before = inTable ? prefix : ""
            let after = inTable ? suffix : "\n"
            let source = before + "*word*" + after
            let doc = try makeDoc(source)
            _ = doc.selectRange(start: UInt32(before.utf8.count), end: UInt32(before.utf8.count + 6))
            _ = doc.toggleBold()
            XCTAssertEqual(doc.source(), before + "***word***" + after)
            _ = doc.toggleBold()
            XCTAssertEqual(doc.source(), source)
        }
    }

    func testBoldInsideCodeDoesNotInsertLiteralAsterisks() throws {
        for inTable in [false, true] {
            let before = inTable ? prefix : ""
            let after = inTable ? suffix : "\n"
            let doc = try makeDoc(before + "`word`" + after)
            let start = UInt32(before.utf8.count + 1)
            _ = doc.selectRange(start: start, end: start + 4)
            _ = doc.toggleBold()
            let runs: [Run]
            if inTable {
                runs = try XCTUnwrap(doc.view().tables.first?.grid.last?.cells.first).lines.flatMap(\.runs)
            } else {
                runs = doc.view().rows.flatMap(\.runs)
            }
            XCTAssertEqual(runs.map(\.text).joined(), "word")
            // Bold closed around the code, and comes off again.
            XCTAssertEqual(doc.source(), before + "**`word`**" + after)
            _ = doc.toggleBold()
            XCTAssertEqual(doc.source(), before + "`word`" + after)
        }
    }
}
