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

func docView(
    _ rows: [Row],
    caretRow: UInt32 = 0,
    caretCh: UInt32 = 0,
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
        caretRow: caretRow,
        caretCol: 0,
        caretCh: caretCh,
        hasSelection: hasSelection,
        anchorRow: anchorRow,
        anchorCh: anchorCh,
        dirty: dirty,
        view: view,
        heading: heading,
        active: active
    )
}
