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
    sup: Bool = false,
    sub: Bool = false,
    // Where the run's first glyph came from. Zero unless a test is about the
    // mapping back to the source (a peek's followable runs), since the geometry
    // and attribute tests this file mostly serves never look at it.
    src: UInt32 = 0,
    sel: Bool = false,
    // A host highlight over the run: its id and `#RRGGBB` hint, or neither.
    hl: String? = nil,
    hlColor: String? = nil,
    // The colour an author named on a `==mark==`, by name — nil for a plain
    // highlight and for every other role.
    markColor: String? = nil,
    // What a `code` run is to its block's language, by class id — nil for
    // plain code and for every other role.
    token: String? = nil,
    // The presentation vocabulary's run-level facts, by the names core carries:
    // a size step (`large`), a generic face (`serif`), and an ink (`red`). Nil
    // is what every run said before the vocabulary existed.
    size: String? = nil,
    font: String? = nil,
    textColor: String? = nil
) -> Run {
    Run(text: text, role: role, bold: bold, italic: italic, underline: underline,
        strike: strike, sup: sup, sub: sub, src: src, sel: sel, hl: hl, hlColor: hlColor,
        markColor: markColor, token: token, size: size, font: font, textColor: textColor)
}

func row(
    _ runs: [Run],
    decoration: Bool = false,
    code: Bool = false,
    codeLang: String? = nil,
    directive: Bool = false,
    directiveLabel: String? = nil,
    heading: UInt8? = nil,
    // The presentation vocabulary's block-level facts, by the names core carries
    // on every row the block emits: an alignment (`center`) and a line spacing
    // (`1.5`).
    align: String? = nil,
    lineHeight: String? = nil,
    boundary: Boundary? = nil
) -> Row {
    Row(
        runs: runs,
        decoration: decoration,
        code: code,
        codeLang: codeLang,
        directive: directive,
        directiveLabel: directiveLabel,
        heading: heading,
        align: align,
        lineHeight: lineHeight,
        boundary: boundary
    )
}

/// A leaf directive's frame entry — what tells the layout that a placeholder row
/// is a page break rather than some host's own `::toc`.
func mkDirective(_ name: String, startRow: UInt32, endRow: UInt32,
                 label: String = "", attrs: [DirectiveAttr] = []) -> DirectiveView {
    DirectiveView(startRow: startRow, endRow: endRow, name: name, label: label, attrs: attrs)
}

/// The placeholder row core draws a page break with, and the directive that
/// names it — the pair a frame carries, since neither means a break alone.
func pageBreakRow() -> Row { row([mkRun("⧉ page-break", role: "image")], directive: true) }

/// The blank row core spells a block boundary with — `decoration` plus the
/// label saying which pair it divides, exactly as `emit_separators_before`
/// emits it. Fixtures build gaps through this so a test can't invent one core
/// wouldn't produce (an unlabelled "gap" is no longer a gap at all).
func gapRow(_ above: BlockClass, _ below: BlockClass, prefix: [Run] = []) -> Row {
    row(prefix, decoration: true, boundary: Boundary(above: above, below: below))
}

func mkCell(_ text: String, align: String = "default", start: UInt32 = 0, end: UInt32 = 0) -> TableCellView {
    // A plain cell's one run begins where the cell does, as core's would.
    let line = TableCellLineView(runs: [mkRun(text, src: start)], start: start, end: end)
    return TableCellView(lines: [line], align: align, start: start, end: end)
}

/// A single-line cell whose one run core has marked selected — for exercising
/// the table selection highlight.
func mkSelCell(_ text: String, start: UInt32, end: UInt32) -> TableCellView {
    let line = TableCellLineView(runs: [mkRun(text, src: start, sel: true)], start: start, end: end)
    return TableCellView(lines: [line], align: "default", start: start, end: end)
}

/// A cell of several lines (an in-cell `<br>`): each `(text, start, end)` triple
/// is one visual line. The whole cell spans the first line's start to the last's
/// end.
func mkCellLines(_ lines: [(String, UInt32, UInt32)], align: String = "default") -> TableCellView {
    let laid = lines.map { TableCellLineView(runs: [mkRun($0.0, src: $0.1)], start: $0.1, end: $0.2) }
    return TableCellView(lines: laid, align: align,
                         start: lines.first?.1 ?? 0, end: lines.last?.2 ?? 0)
}

func mkTableRow(_ cells: [TableCellView], head: Bool = false) -> TableRowView {
    TableRowView(head: head, cells: cells)
}

func mkTable(_ grid: [TableRowView], startRow: UInt32 = 0, endRow: UInt32 = 0) -> TableView {
    TableView(startRow: startRow, endRow: endRow, grid: grid)
}

func mkMedia(
    _ src: String,
    kind: MediaKind = .image,
    startRow: UInt32 = 0,
    endRow: UInt32 = 1,
    poster: String = "",
    alt: String = "",
    sources: [MediaSourceView] = []
) -> MediaView {
    MediaView(startRow: startRow, endRow: endRow, kind: kind, src: src,
              poster: poster, alt: alt, sources: sources)
}

func docView(
    _ rows: [Row],
    tables: [TableView] = [],
    directives: [DirectiveView] = [],
    media: [MediaView] = [],
    caretRow: UInt32 = 0,
    caretCh: UInt32 = 0,
    caretSrc: UInt32 = 0,
    hasSelection: Bool = false,
    anchorRow: UInt32 = 0,
    anchorCh: UInt32 = 0,
    dirty: Bool = false,
    view: String = "wysiwyg",
    heading: UInt32? = nil,
    active: [String] = [],
    link: String? = nil,
    canUndo: Bool = false,
    canRedo: Bool = false,
    // The colour of the highlight at the caret — what a colour menu ticks.
    markColor: MarkColor? = nil
) -> DocView {
    DocView(
        rows: rows,
        tables: tables,
        directives: directives,
        media: media,
        caretRow: caretRow,
        caretCol: 0,
        caretCh: caretCh,
        caretSrc: caretSrc,
        hasSelection: hasSelection,
        anchorRow: anchorRow,
        anchorCh: anchorCh,
        dirty: dirty,
        canUndo: canUndo,
        canRedo: canRedo,
        view: view,
        heading: heading,
        active: active,
        link: link,
        markColor: markColor
    )
}
