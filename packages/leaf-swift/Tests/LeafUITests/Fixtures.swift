//  Fixtures.swift
//
//  Builders for the UniFFI value types (`Run`/`Row`/`DocView`), so a test can
//  assemble a frame in pure Swift — no Rust runtime, no `LeafDoc` — and exercise
//  the renderer's geometry and attribute mapping directly. The records carry
//  public memberwise initializers; these just supply sensible defaults.

import LeafFFI

func mkRun(
    _ text: String,
    role: String = "",
    bold: Bool = false,
    italic: Bool = false,
    underline: Bool = false,
    strike: Bool = false,
    sel: Bool = false
) -> Run {
    Run(text: text, role: role, bold: bold, italic: italic, underline: underline, strike: strike, sel: sel)
}

func row(
    _ runs: [Run],
    decoration: Bool = false,
    code: Bool = false,
    codeLang: String? = nil,
    heading: UInt8? = nil
) -> Row {
    Row(runs: runs, decoration: decoration, code: code, codeLang: codeLang, heading: heading)
}

func mkCell(_ text: String, align: String = "default", start: UInt32 = 0, end: UInt32 = 0) -> TableCellView {
    let line = TableCellLineView(runs: [mkRun(text)], start: start, end: end)
    return TableCellView(lines: [line], align: align, start: start, end: end)
}

/// A single-line cell whose one run core has marked selected — for exercising
/// the table selection highlight.
func mkSelCell(_ text: String, start: UInt32, end: UInt32) -> TableCellView {
    let line = TableCellLineView(runs: [mkRun(text, sel: true)], start: start, end: end)
    return TableCellView(lines: [line], align: "default", start: start, end: end)
}

/// A cell of several lines (an in-cell `<br>`): each `(text, start, end)` triple
/// is one visual line. The whole cell spans the first line's start to the last's
/// end.
func mkCellLines(_ lines: [(String, UInt32, UInt32)], align: String = "default") -> TableCellView {
    let laid = lines.map { TableCellLineView(runs: [mkRun($0.0)], start: $0.1, end: $0.2) }
    return TableCellView(lines: laid, align: align,
                         start: lines.first?.1 ?? 0, end: lines.last?.2 ?? 0)
}

func mkTableRow(_ cells: [TableCellView], head: Bool = false) -> TableRowView {
    TableRowView(head: head, cells: cells)
}

func mkTable(_ grid: [TableRowView], startRow: UInt32 = 0, endRow: UInt32 = 0) -> TableView {
    TableView(startRow: startRow, endRow: endRow, grid: grid)
}

func docView(
    _ rows: [Row],
    tables: [TableView] = [],
    caretRow: UInt32 = 0,
    caretCh: UInt32 = 0,
    caretSrc: UInt32 = 0,
    hasSelection: Bool = false,
    anchorRow: UInt32 = 0,
    anchorCh: UInt32 = 0,
    dirty: Bool = false,
    view: String = "wysiwyg",
    heading: UInt32? = nil,
    active: [String] = []
) -> DocView {
    DocView(
        rows: rows,
        tables: tables,
        caretRow: caretRow,
        caretCol: 0,
        caretCh: caretCh,
        caretSrc: caretSrc,
        hasSelection: hasSelection,
        anchorRow: anchorRow,
        anchorCh: anchorCh,
        dirty: dirty,
        view: view,
        heading: heading,
        active: active
    )
}
