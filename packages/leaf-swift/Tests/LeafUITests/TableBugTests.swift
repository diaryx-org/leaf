import XCTest
import CoreText
import LeafFFI
@testable import LeafUI

/// Reproductions from the table audit, kept as regression tests.
final class TableBugTests: XCTestCase {
    private let theme = EditorTheme.default

    func testUnicodeCaretAndSelectionUseSourceBytesAsUTF16() throws {
        let model = try LeafEditorModel(source: "| Name |\n| --- |\n| caféXYZ |\n")
        let frame = model.doc.setUnwrapped()
        let table = try XCTUnwrap(frame.tables.first)
        let cell = try XCTUnwrap(table.grid.last?.cells.first)
        let offset = cell.lines[0].start + UInt32("café".utf8.count)
        let selected = model.doc.setSelectionOffsets(anchor: offset, focus: offset)
        let layout = EditorLayout(selected, theme: theme)
        let rl = try XCTUnwrap(layout.rows.first { $0.tableFirst })
        let grid = try XCTUnwrap(rl.table)
        let location = try XCTUnwrap(grid.locate(src: Int(offset)))
        let expectedX = rl.originX + location.line.textX
            + CTLineGetOffsetForStringIndex(location.line.line, 4, nil)
        let caret = try XCTUnwrap(layout.caretRect(selected, theme: theme))
        XCTAssertEqual(caret.minX, expectedX, accuracy: 0.1)
        let rect = try XCTUnwrap(layout.tableSelectionRects(from: Int(offset), to: Int(offset + 1)).first)
        XCTAssertEqual(rect.rect.minX, expectedX, accuracy: 0.1)
    }

    func testClickAfterInlineMarkupResolvesToVisibleCharacter() throws {
        let model = try LeafEditorModel(source: "| Name |\n| --- |\n| **bold** tail |\n")
        model.setMarkupMode(.none)
        let frame = model.doc.setUnwrapped()
        let layout = EditorLayout(frame, theme: theme)
        let rl = try XCTUnwrap(layout.rows.first { $0.tableFirst })
        let grid = try XCTUnwrap(rl.table)
        let row = try XCTUnwrap(grid.rows.last)
        let line = try XCTUnwrap(row.cells.first?.lines.first)
        XCTAssertEqual(line.attributed.string, "bold tail")
        let point = CGPoint(x: rl.originX + line.textX + CTLineGetOffsetForStringIndex(line.line, 5, nil),
                            y: rl.tableTop + row.top + TableMetrics.padY + 1)
        let hit = try XCTUnwrap(layout.tableHitOffset(point))
        let expected = "| Name |\n| --- |\n| **bold** ".utf8.count
        XCTAssertEqual(model.doc.snapOffset(off: UInt32(hit)), UInt32(expected))
    }

    /// The iOS view answers `caretRect(for:)` for *any* position, not just the
    /// caret's, and it used to answer through the picture-row path alone — so
    /// every position inside a table came back as the grid's top-left corner:
    /// the system drew the caret there, anchored the edit menu there, and
    /// scrolled there. The position-general `caretRect(src:row:ch:theme:)`
    /// must agree with the caret's own table-aware rect.
    func testCaretRectForAPositionInACellRidesTheGrid() throws {
        let model = try LeafEditorModel(source: "| Name |\n| --- |\n| Tables | editable |\n")
        let frame = model.doc.setUnwrapped()
        let layout = EditorLayout(frame, theme: theme)
        let rl = try XCTUnwrap(layout.rows.first { $0.tableFirst })
        let grid = try XCTUnwrap(rl.table)
        let cell = try XCTUnwrap(grid.rows.last?.cells.last)
        let end = cell.end
        let rc = model.doc.posForOffset(off: UInt32(end))
        let general = try XCTUnwrap(layout.caretRect(src: end, row: Int(rc.row), ch: Int(rc.ch), theme: theme))
        let placed = model.doc.setSelectionOffsets(anchor: UInt32(end), focus: UInt32(end))
        let own = try XCTUnwrap(layout.caretRect(placed, theme: theme))
        XCTAssertEqual(general, own)
        // And it is the cell's, not the picture row's: past the cell's text,
        // on the body row's band.
        let line = try XCTUnwrap(cell.lines.first)
        let textEnd = rl.originX + line.textX
            + CTLineGetOffsetForStringIndex(line.line, line.attributed.length, nil)
        XCTAssertEqual(general.minX, textEnd, accuracy: 0.1)
        XCTAssertGreaterThan(general.minY, rl.tableTop + grid.rows[0].height)
    }

    func testClickLeftOfGridUsesFirstCell() throws {
        let grid = try XCTUnwrap(TableLayout(mkTable([
            mkTableRow([mkCell("first", start: 2, end: 7), mkCell("last", start: 10, end: 14)])
        ]), theme: theme))
        let hit = try XCTUnwrap(grid.locate(atX: -1, y: grid.rows[0].top + TableMetrics.padY))
        XCTAssertEqual(hit.cell.start, 2)
    }
}
