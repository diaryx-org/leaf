import XCTest
import CoreText
import LeafFFI
@testable import LeafUI

/// Reproductions from the table audit. Expected failures stay strict so fixes
/// require removing the annotation rather than silently leaving stale repros.
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
        XCTExpectFailure("Table caret confuses UTF-8 bytes with UTF-16 indices") {
            XCTAssertEqual(caret.minX, expectedX, accuracy: 0.1)
        }
        let rect = try XCTUnwrap(layout.tableSelectionRects(from: Int(offset), to: Int(offset + 1)).first)
        XCTExpectFailure("Table selection confuses UTF-8 bytes with UTF-16 indices") {
            XCTAssertEqual(rect.rect.minX, expectedX, accuracy: 0.1)
        }
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
        XCTExpectFailure("Table hit testing loses hidden inline delimiters") {
            XCTAssertEqual(model.doc.snapOffset(off: UInt32(hit)), UInt32(expected))
        }
    }

    func testClickLeftOfGridUsesFirstCell() throws {
        let grid = try XCTUnwrap(TableLayout(mkTable([
            mkTableRow([mkCell("first", start: 2, end: 7), mkCell("last", start: 10, end: 14)])
        ]), theme: theme))
        let hit = try XCTUnwrap(grid.locate(atX: -1, y: grid.rows[0].top + TableMetrics.padY))
        XCTExpectFailure("Left-margin clicks fall back to the last column") {
            XCTAssertEqual(hit.cell.start, 2)
        }
    }
}
