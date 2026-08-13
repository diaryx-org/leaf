//  EditorLayout.swift
//
//  The platform-neutral geometry of a rendered frame. In the proportional GUI,
//  core hands back an *unwrapped* `DocView` — one `Row` per block (hard breaks
//  still split; soft wrapping is ours) — and this wraps each row to the view's
//  pixel width with Core Text, into a stack of visual lines. It answers the
//  geometry both the AppKit and UIKit views need: where the caret sits, which
//  `(row, ch)` a point hits, and how tall the content is. All Core Text +
//  Foundation, so it compiles once for both toolkits.

import CoreGraphics
import CoreText
import Foundation
import LeafFFI

/// Toolbar/chrome state pushed to the host after every repaint — the subset of a
/// `DocView` a surrounding UI reflects. Platform-neutral so both views share it.
public struct EditorState: Equatable {
    public var view: String          // "wysiwyg" | "source"
    public var dirty: Bool
    public var heading: UInt32?      // heading level at the caret, or nil
    public var active: [String]      // inline marks active at the caret

    public init(view: String, dirty: Bool, heading: UInt32?, active: [String]) {
        self.view = view; self.dirty = dirty; self.heading = heading; self.active = active
    }

    /// Project a full `DocView` down to the chrome-facing state.
    public init(_ v: DocView) {
        self.init(view: v.view, dirty: v.dirty, heading: v.heading, active: v.active)
    }
}

extension Row {
    /// Whether this is the blank row core spells a block boundary with — no caret
    /// home, and drawn short so a boundary reads as spacing rather than as an
    /// empty line the user didn't type.
    ///
    /// Core's own answer, not a guess read back out of the glyphs. It used to be
    /// the latter — a decoration row whose runs were all whitespace once the
    /// quote/list prefix was dropped — which had to know that a boundary *inside*
    /// a blockquote still carries the quote's gutter, and would have gone on
    /// growing a clause per block decoration core learned to draw. `boundary`
    /// says it once, for every frontend.
    var isBlockGap: Bool { boundary != nil }

    /// The row's leading block decoration — a blockquote's `│ ` gutters and a
    /// list's indent/bullet. Core emits these as synthetic glyphs in front of the
    /// row's real content, one gutter per nesting level, so the prefix is exactly
    /// the run of leading `quote`/`list` runs.
    var prefixRuns: [Run] {
        Array(runs.prefix { $0.role == "quote" || $0.role == "list" })
    }

    /// Whether this row is a thematic break — a `---` drawn as a line across the
    /// text column rather than as core's row of `─` glyphs.
    ///
    /// A break is everything after the prefix being rule-role dashes. That tells
    /// it apart from the other rows carrying `Role::Rule`: a table's box-drawing
    /// rules are `decoration` rows, and a table's content rows mix their `│`
    /// separators with real cell text.
    var isThematicBreak: Bool {
        guard !decoration, !code else { return false }
        let body = runs.drop { $0.role == "quote" || $0.role == "list" }
        guard !body.isEmpty else { return false }
        return body.allSatisfy { run in
            run.role == "rule" && !run.text.isEmpty && run.text.allSatisfy { $0 == "─" }
        }
    }
}

/// One pixel-wrapped visual line within a logical row. Its `CTLine` is built over
/// the line's *substring*, so its string indices are relative to `start`; callers
/// convert with `ch - start`. `start`/`length` are UTF-16 offsets into the row.
struct WrappedLine {
    let attributed: NSAttributedString   // the substring this visual line draws
    let line: CTLine                     // geometry over that substring (indices relative to `start`)
    let start: Int                       // absolute UTF-16 offset of the line within the row
    let length: Int                      // UTF-16 length of the line
    let width: CGFloat                   // typographic width, points
    /// How far right of the text margin this line is drawn. Zero on a row's first
    /// visual line (its own prefix glyphs already inset it) and the prefix's width
    /// on every continuation line, so a wrapped quote or list item hangs under its
    /// own text rather than sliding back under the gutter.
    var indent: CGFloat = 0
}

/// The expensive, position-independent shaping of one row: its attributed string
/// and the visual lines it wrapped into at `wrapWidth`. Cached across frames keyed
/// by the row's *value*, so an edit re-shapes only the row(s) that changed —
/// everything else, including every row below an insert/delete, is reused. A cache
/// hit is only valid at the same `wrapWidth`; a resize rebuilds. (A selection-only
/// edit flips a run's `sel` and re-shapes that row, which is harmless — `sel` isn't
/// in the attributed string; the selection is filled separately.)
struct ShapedRow {
    let attributed: NSAttributedString
    let wrapped: [WrappedLine]
    let lineHeight: CGFloat
    let wrapWidth: CGFloat
    /// The width of the row's leading block decoration (quote gutters, list
    /// indent) — the hanging indent its continuation lines carry.
    var prefixWidth: CGFloat = 0
    /// Where each blockquote gutter bar sits, relative to the text margin: one x
    /// per nesting level, measured at the level's `│` glyph. Empty off a quote.
    var quoteBarXs: [CGFloat] = []
}

/// One logical row (block) placed in the document: its shaping plus a top offset.
///
/// Table rows are the exception. To keep `rows` 1:1 with the frame's rows (every
/// caret/click path indexes `rows` by a core row index), a table's box-glyph
/// picture rows are kept — but each carries the laid-out `table` grid instead of
/// text, only the FIRST one (`tableFirst`) has any height, and all of them draw
/// the grid rather than their glyphs. The caret and hit-testing over a table read
/// the grid, not these rows' shaping.
struct RowLayout {
    let row: Row
    let shaped: ShapedRow
    let top: CGFloat
    /// The text column this row was laid out in — its left edge and width in view
    /// coordinates. Uniform across a frame (every row shares the layout's column),
    /// but carried here so a row can answer where its own chrome goes without the
    /// caller threading the column through. See `EditorTheme.column(in:)`.
    var originX: CGFloat = 0
    var columnWidth: CGFloat = 0
    /// The grid, on every picture row of a table; `nil` for an ordinary row.
    var table: TableLayout? = nil
    /// The grid's top (all of a table's rows share it — they collapse onto it).
    var tableTop: CGFloat = 0
    /// The one picture row that carries the grid's height and paints it.
    var tableFirst: Bool = false
    /// The media box, on every placeholder row of a block image/video/audio;
    /// `nil` for an ordinary row. Collapsed exactly as a table's rows are — see
    /// `mediaFirst`.
    var media: MediaLayout? = nil
    /// The box's top (all of a media's reserved rows share it).
    var mediaTop: CGFloat = 0
    /// The one placeholder row that carries the box's height and paints it.
    var mediaFirst: Bool = false
    /// Header space reserved above this row's own text for a directive's audience
    /// label (nonzero only on a directive block's first, labeled row) — keeps the
    /// label from painting over that row's real content. See `EditorTheme.directiveLabelHeight`.
    var labelInset: CGFloat = 0
    /// The height of a block-boundary gap row, which depends on the blocks it
    /// falls *between* (see `EditorTheme.blockGap(between:and:)`) and so can't
    /// come from the row's own shaping — the same blank row is a wide margin above
    /// a heading and a tight one inside a list. Nil on every other row, and the
    /// reason gap height isn't folded into `ShapedRow`, whose cache is keyed by
    /// row *value* and would hand a heading's margin to a list.
    var gapHeight: CGFloat? = nil

    var attributed: NSAttributedString { shaped.attributed }
    var wrapped: [WrappedLine] { shaped.wrapped }
    var lineHeight: CGFloat { shaped.lineHeight }
    /// The block's total height — the grid's height on a table's first row, zero
    /// on its other (collapsed) rows, a boundary's contextual gap, else the label
    /// inset (if any) plus one `lineHeight` per visual line.
    var height: CGFloat {
        if let t = table { return tableFirst ? t.height : 0 }
        if let m = media { return mediaFirst ? m.height : 0 }
        if let gapHeight { return gapHeight }
        return labelInset + CGFloat(shaped.wrapped.count) * shaped.lineHeight
    }

    /// The blockquote gutter bars this row carries, in view coordinates — one
    /// rect per nesting level, spanning the row's whole height so consecutive
    /// quoted rows tile into one unbroken bar. A table's picture rows are skipped:
    /// the grid they draw isn't inset by the prefix, so a bar there would sit on
    /// top of the table rather than beside it.
    func quoteBars(theme: EditorTheme) -> [CGRect] {
        // A table is skipped (its grid isn't prefix-inset, so a bar would land on
        // top of it), but a media box *is* drawn inset by the row's prefix — core
        // emits the same gutter glyphs in front of a block image as any other
        // block — so a quoted picture keeps its bar beside it.
        guard table == nil else { return [] }
        return shaped.quoteBarXs.map { x in
            CGRect(x: originX + x, y: top,
                   width: theme.quoteBarWidth, height: height)
        }
    }

    /// A thematic break's drawn line, in view coordinates: a hairline centred in
    /// the row's box, running from past the row's own prefix to the right edge of
    /// the text column. `nil` on every other row.
    func ruleLine(theme: EditorTheme) -> CGRect? {
        guard row.isThematicBreak, table == nil else { return nil }
        let x = originX + shaped.prefixWidth
        let right = originX + max(columnWidth, shaped.prefixWidth)
        return CGRect(x: x, y: (top + labelInset + height * 0.5 - theme.ruleThickness / 2).rounded(),
                      width: max(0, right - x), height: theme.ruleThickness)
    }
}

/// The laid-out rows of one `DocView` plus the geometry queries over them.
struct EditorLayout {
    let rows: [RowLayout]
    /// Total content height including top+bottom padding — the view's fitting size.
    let contentHeight: CGFloat
    /// The text column's left edge in view coordinates. Every x here is measured
    /// from this, not from `theme.padding.left`: with a `measure` set the column
    /// is centred in the view, so the padding is only the floor it can't cross.
    let originX: CGFloat
    /// The text column's width — what rows wrap to, and how far a thematic break
    /// or a directive outline runs.
    let columnWidth: CGFloat

    /// Lay out `docView` in a view `viewWidth` points wide, wrapping each row to
    /// the text column `theme` puts inside it (see `EditorTheme.column(in:)`).
    /// Reuses shaped rows from `cache` (same content *and* same column width) and
    /// replaces it with the exact set this frame used, so deleted rows are evicted
    /// and the cache stays bounded. The caller must clear `cache` when the theme's
    /// geometry changes.
    /// `media` loads the stills a block image or video poster draws. `nil` (the
    /// default, and what the tests use) still lays every media box out — at its
    /// no-picture size, as a labelled chip — so geometry can be exercised without
    /// touching the filesystem.
    init(_ docView: DocView, theme: EditorTheme, viewWidth: CGFloat,
         cache: inout [Row: ShapedRow], media: MediaStore? = nil) {
        let column = theme.column(in: viewWidth)
        self.init(docView, theme: theme, originX: column.originX, columnWidth: column.width,
                  cache: &cache, media: media)
    }

    /// Lay out into an explicit column — the designated initializer the others
    /// resolve to. `columnWidth <= 0` means "don't wrap" (one visual line per
    /// row), the state before a view knows its bounds.
    init(_ docView: DocView, theme: EditorTheme, originX: CGFloat, columnWidth: CGFloat,
         cache: inout [Row: ShapedRow], media: MediaStore? = nil) {
        let wrapWidth = columnWidth
        self.originX = originX
        self.columnWidth = max(0, columnWidth)
        var layouts: [RowLayout] = []
        layouts.reserveCapacity(docView.rows.count)
        var next = Dictionary<Row, ShapedRow>(minimumCapacity: docView.rows.count)
        var y = theme.padding.top

        // A table's box-glyph picture rows are replaced by one grid element that
        // stands in for the whole `[startRow, endRow)` span.
        var tableAt: [Int: TableView] = [:]
        for t in docView.tables { tableAt[Int(t.startRow)] = t }

        // A block image / video / audio replaces its placeholder rows the same
        // way, with one box standing in for the whole `[startRow, endRow)` span.
        var mediaAt: [Int: MediaView] = [:]
        for m in docView.media { mediaAt[Int(m.startRow)] = m }

        // An empty stand-in shape for a table's collapsed picture rows (they draw
        // the grid, never their own glyphs).
        let emptyShape = ShapedRow(
            attributed: NSAttributedString(),
            wrapped: [WrappedLine(attributed: NSAttributedString(),
                                  line: CTLineCreateWithAttributedString(NSAttributedString()),
                                  start: 0, length: 0, width: 0)],
            lineHeight: theme.lineHeight,
            wrapWidth: wrapWidth
        )

        var i = 0
        while i < docView.rows.count {
            if let t = tableAt[i], let grid = TableLayout(t, theme: theme) {
                let tableTop = y
                // Keep every picture row (rows stay 1:1 with the frame), but
                // collapse them onto the grid: the first carries its height, the
                // rest are zero-height, and all defer drawing/caret to the grid.
                for r in Int(t.startRow)..<Int(t.endRow) where r < docView.rows.count {
                    layouts.append(RowLayout(
                        row: docView.rows[r], shaped: emptyShape, top: tableTop,
                        originX: originX, columnWidth: wrapWidth,
                        table: grid, tableTop: tableTop, tableFirst: r == Int(t.startRow)
                    ))
                }
                y += grid.height
                i = Int(t.endRow)
                continue
            }

            if let mv = mediaAt[i] {
                // Shaped first, and kept: unlike a table, a media row's own
                // glyphs (core's `🖼 alt` label) are the fallback drawn when the
                // box has no picture, and its prefix width is what insets the box
                // inside a quote or a list.
                let placeholder = docView.rows[i]
                let shaped = EditorLayout.shape(placeholder, theme: theme, wrapWidth: wrapWidth)
                let box = MediaLayout(mv, still: media?.still(for: mv),
                                      contentWidth: max(0, wrapWidth - shaped.prefixWidth),
                                      theme: theme)
                let mediaTop = y
                for r in Int(mv.startRow)..<Int(mv.endRow) where r < docView.rows.count {
                    layouts.append(RowLayout(
                        row: docView.rows[r],
                        shaped: r == Int(mv.startRow) ? shaped : emptyShape,
                        top: mediaTop,
                        originX: originX, columnWidth: wrapWidth,
                        media: box, mediaTop: mediaTop, mediaFirst: r == Int(mv.startRow)
                    ))
                }
                y += box.height
                i = Int(mv.endRow)
                continue
            }

            let row = docView.rows[i]
            let shaped: ShapedRow
            if let hit = cache[row] ?? next[row], hit.wrapWidth == wrapWidth {
                shaped = hit
            } else {
                shaped = EditorLayout.shape(row, theme: theme, wrapWidth: wrapWidth)
            }
            next[row] = shaped
            let hasLabel = row.directive && !(row.directiveLabel ?? "").isEmpty
            let rl = RowLayout(
                row: row, shaped: shaped, top: y,
                originX: originX, columnWidth: wrapWidth,
                labelInset: hasLabel ? theme.directiveLabelHeight : 0,
                // A boundary is spaced by what it separates, and core says what
                // that is (`row.boundary`) — so this is a lookup, not a walk over
                // the neighbouring rows.
                gapHeight: row.isBlockGap ? theme.blockGap(row.boundary) : nil)
            y += rl.height
            layouts.append(rl)
            i += 1
        }
        // An empty document has no rows at all: core describes blocks, and there
        // is no block to describe. The caret still has a home there (offset 0),
        // so it needs a line box to stand in — without one there is no row for
        // `rect` to answer about and the caret is simply never drawn, which is
        // what a brand-new note looked like until the first keystroke brought a
        // row into being. The same reason `wrap` gives an empty row one empty
        // line: a caret needs somewhere to be.
        if layouts.isEmpty {
            layouts.append(RowLayout(row: Row(runs: [], decoration: false, code: false,
                                              codeLang: nil, directive: false,
                                              directiveLabel: nil, heading: nil,
                                              boundary: nil),
                                     shaped: emptyShape, top: y,
                                     originX: originX, columnWidth: wrapWidth))
            y += theme.lineHeight
        }
        rows = layouts
        contentHeight = y + theme.padding.bottom
        cache = next
    }

    /// The media whose box contains `point`, of any kind, or `nil`. `point` is in
    /// view coordinates.
    ///
    /// Whether a hit here is worth *acting on* is not geometry's question —
    /// video and audio always are, an image only while there is nothing drawn in
    /// its box and the host might still supply it. The views decide that, since
    /// only they can see what the media store has loaded.
    func mediaBox(at point: CGPoint) -> MediaView? {
        for rl in rows {
            guard rl.mediaFirst, let box = rl.media else { continue }
            let r = box.rect(top: rl.mediaTop, left: rl.originX + rl.shaped.prefixWidth)
            if r.contains(point) { return box.media }
        }
        return nil
    }

    /// The playable media whose box contains `point`, or `nil` — `mediaBox`
    /// narrowed to the kinds that have something to play.
    func playableMedia(at point: CGPoint) -> MediaView? {
        guard let hit = mediaBox(at: point), hit.kind != .image else { return nil }
        return hit
    }

    /// Every media box's rect in view coordinates, keyed by `src` — what an
    /// installed player is positioned onto, and the set that decides which
    /// players are still wanted. Keyed by `src` rather than row index because
    /// rows renumber on every keystroke while a source doesn't: typing above a
    /// playing video should move it, not restart it.
    ///
    /// A document naming the same `src` twice collapses to one entry, and only
    /// one of the two boxes can host a player. That is the honest consequence of
    /// keying by source, and a rare enough shape not to complicate this for.
    func mediaRects() -> [String: CGRect] {
        var out: [String: CGRect] = [:]
        for rl in rows where rl.mediaFirst {
            guard let box = rl.media else { continue }
            out[box.media.src] = box.rect(top: rl.mediaTop,
                                          left: rl.originX + rl.shaped.prefixWidth)
        }
        return out
    }

    /// Lay out into a column `wrapWidth` wide at the theme's left inset — the text
    /// column stated directly rather than worked back out of a view width and a
    /// measure. Convenience for tests.
    init(_ docView: DocView, theme: EditorTheme, wrapWidth: CGFloat,
         cache: inout [Row: ShapedRow], media: MediaStore? = nil) {
        self.init(docView, theme: theme, originX: theme.padding.left, columnWidth: wrapWidth,
                  cache: &cache, media: media)
    }

    /// The same with no cross-frame cache — every row shaped fresh.
    init(_ docView: DocView, theme: EditorTheme, wrapWidth: CGFloat, media: MediaStore? = nil) {
        var scratch: [Row: ShapedRow] = [:]
        self.init(docView, theme: theme, wrapWidth: wrapWidth, cache: &scratch, media: media)
    }

    /// Lay out for a view `viewWidth` wide with no cross-frame cache — the
    /// column-from-the-theme path (`measure` and all), one frame at a time.
    init(_ docView: DocView, theme: EditorTheme, viewWidth: CGFloat, media: MediaStore? = nil) {
        var scratch: [Row: ShapedRow] = [:]
        self.init(docView, theme: theme, viewWidth: viewWidth, cache: &scratch, media: media)
    }

    /// Build unwrapped (one visual line per row). Convenience for tests.
    init(_ docView: DocView, theme: EditorTheme) {
        self.init(docView, theme: theme, wrapWidth: 0)
    }

    /// Shape one row: its attributed text, the visual lines it wraps into, and the
    /// block-decoration geometry the view paints over it (the quote bars' x's, the
    /// prefix width its continuation lines hang from).
    ///
    /// A thematic break is shaped from its *prefix alone* — the `───` glyphs are
    /// dropped, because the view draws a real line across the column instead.
    /// Keeping them would wrap a long rule onto a second line and leave a caret
    /// that walks across invisible dashes; dropping them leaves a one-line row
    /// whose caret sits at its left edge (any `caret_ch` core reports on the rule
    /// clamps there), exactly as a table's collapsed picture rows defer to the grid.
    static func shape(_ row: Row, theme: EditorTheme, wrapWidth: CGFloat) -> ShapedRow {
        let prefix = row.prefixRuns
        let drawn = row.isThematicBreak ? prefix : row.runs
        let attributed = AttributedRow.make(drawn, row: row, theme: theme)

        // The prefix's own geometry, measured on its own line: its total width (the
        // hanging indent) and where each level's bar glyph starts.
        var prefixWidth: CGFloat = 0
        var barXs: [CGFloat] = []
        if !prefix.isEmpty {
            let prefixText = AttributedRow.make(prefix, row: row, theme: theme)
            let line = CTLineCreateWithAttributedString(prefixText as CFAttributedString)
            prefixWidth = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
            let s = prefixText.string as NSString
            let bar = String(AttributedRow.quoteBar)
            for i in 0..<s.length where s.substring(with: NSRange(location: i, length: 1)) == bar {
                barXs.append(CTLineGetOffsetForStringIndex(line, CFIndex(i), nil))
            }
        }

        return ShapedRow(
            attributed: attributed,
            wrapped: wrap(attributed, width: wrapWidth, indent: prefixWidth),
            lineHeight: theme.rowHeight(for: row),
            wrapWidth: wrapWidth,
            prefixWidth: prefixWidth,
            quoteBarXs: barXs
        )
    }

    /// Break `attributed` into visual lines at `width` points via Core Text. Each
    /// line owns a `CTLine` over its substring (relative indices). `width <= 0`
    /// keeps the whole row on one line; an empty row is one empty line so it still
    /// occupies a line box and holds a caret.
    /// `indent` hangs every line after the first that far right of the margin (and
    /// takes that much off its wrap budget), so a wrapped quote or list item lines
    /// its continuations up with its own text instead of under its gutter.
    static func wrap(_ attributed: NSAttributedString, width: CGFloat, indent: CGFloat = 0) -> [WrappedLine] {
        let len = attributed.length
        if len == 0 {
            return [WrappedLine(attributed: attributed, line: CTLineCreateWithAttributedString(attributed),
                                start: 0, length: 0, width: 0)]
        }
        let typesetter = CTTypesetterCreateWithAttributedString(attributed as CFAttributedString)
        var lines: [WrappedLine] = []
        var start = 0
        while start < len {
            // The first line pays for the prefix in glyphs; the rest pay for it in
            // indent. Never let the indent eat the whole budget.
            let hang = lines.isEmpty ? 0 : min(indent, max(0, width - 1))
            let budget = width - hang
            let count: Int = width > 0
                ? max(1, CTTypesetterSuggestLineBreak(typesetter, start, Double(budget)))
                : len - start
            let sub = attributed.attributedSubstring(from: NSRange(location: start, length: count))
            let line = CTLineCreateWithAttributedString(sub as CFAttributedString)
            lines.append(WrappedLine(
                attributed: sub,
                line: line,
                start: start,
                length: count,
                width: CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil)),
                indent: hang
            ))
            start += count
        }
        return lines
    }

    // MARK: geometry

    /// A 1.5pt-wide vertical rect at row `row`, UTF-16 offset `ch` — the geometry a
    /// caret or a selection endpoint occupies, resolved to the visual line `ch` falls
    /// on. `nil` if the row is out of range. At a soft-wrap boundary the position
    /// belongs to the *start* of the following line.
    func rect(row: Int, ch: Int) -> CGRect? {
        guard rows.indices.contains(row) else { return nil }
        let rl = rows[row]
        if rl.media != nil { return mediaCaretRect(rl, ch: ch) }
        let lines = rl.wrapped
        for (i, wl) in lines.enumerated() where ch < wl.start + wl.length || i == lines.count - 1 {
            let x = CTLineGetOffsetForStringIndex(wl.line, CFIndex(max(0, ch - wl.start)), nil)
            let y = rl.top + rl.labelInset + CGFloat(i) * rl.lineHeight
            return CGRect(x: rl.originX + wl.indent + x, y: y, width: 1.5, height: rl.lineHeight)
        }
        return CGRect(x: rl.originX, y: rl.top + rl.labelInset, width: 1.5, height: rl.lineHeight)
    }

    /// The caret's frame on a block media row: the box's leading edge in front of
    /// the picture, its trailing edge past it, and as tall as the box either way.
    ///
    /// Core gives a block image, video, or audio exactly two caret homes — one
    /// before it and one just after it, with nothing inside the markup (see
    /// `block_media`). The row's own glyphs are the `🖼 alt` label the box is
    /// painted *over*, so measuring the caret against them puts it at some
    /// arbitrary point inside the picture: a blinking bar in the middle of a
    /// photo that says nothing about where the next character will land. Riding
    /// the box's edges says what a word processor's caret beside a figure says.
    private func mediaCaretRect(_ rl: RowLayout, ch: Int) -> CGRect? {
        guard let box = rl.media else { return nil }
        let r = box.rect(top: rl.mediaTop, left: rl.originX + rl.shaped.prefixWidth)
        // Past the label's last glyph is core's trailing stop. The reserved rows
        // below the first carry no glyphs at all, so they are only ever "after" —
        // which is right: they are the picture's lower half.
        let after = ch >= rl.attributed.length
        return CGRect(x: after ? r.maxX - 1.5 : r.minX, y: r.minY, width: 1.5, height: r.height)
    }

    /// The caret's frame — `caret_ch` (UTF-16, within its block row) mapped through
    /// the pixel wrap to a rect. `nil` if the caret row is out of range. Inside a
    /// table the caret rides the grid (by its source offset), not the collapsed
    /// picture row `caret_row` names.
    func caretRect(_ docView: DocView, theme: EditorTheme) -> CGRect? {
        let cr = Int(docView.caretRow)
        if rows.indices.contains(cr), let grid = rows[cr].table {
            return tableCaretRect(grid, tableTop: rows[cr].tableTop, originX: rows[cr].originX,
                                  caretSrc: Int(docView.caretSrc), theme: theme)
        }
        return rect(row: cr, ch: Int(docView.caretCh))
    }

    /// The caret's frame inside a table: the cell *line* its source offset falls
    /// on, at the x the offset maps to within that line and the y of the line's
    /// band. A multi-line cell (an in-cell `<br>`) puts later offsets lower.
    private func tableCaretRect(_ grid: TableLayout, tableTop: CGFloat, originX: CGFloat,
                                caretSrc: Int, theme: EditorTheme) -> CGRect? {
        guard let (row, _, line, lineIndex) = grid.locate(src: caretSrc) else { return nil }
        // Byte offset within the line ≈ UTF-16 index (exact for ASCII text). The
        // line carries no break, so this holds even across an in-cell `<br>`.
        let idx = max(0, min(caretSrc - line.start, line.attributed.length))
        let dx = CTLineGetOffsetForStringIndex(line.line, CFIndex(idx), nil)
        return CGRect(x: originX + line.textX + dx,
                      y: tableTop + row.top + TableMetrics.padY + CGFloat(lineIndex) * grid.lineHeight,
                      width: 1.5, height: theme.lineHeight)
    }

    /// The source offset a click at `point` resolves to when it lands in a table,
    /// else `nil` (the caller falls back to the row/ch hit path). The offset is
    /// approximate for a cell with inline markup; core snaps it to a real stop.
    func tableHitOffset(_ point: CGPoint) -> Int? {
        for rl in rows {
            guard let grid = rl.table, rl.tableFirst else { continue }
            let yInTable = point.y - rl.tableTop
            guard yInTable >= 0, yInTable < grid.height else { continue }
            let xInTable = point.x - rl.originX
            guard let (_, _, line, _) = grid.locate(atX: xInTable, y: yInTable) else { return nil }
            let rel = CTLineGetStringIndexForPosition(
                line.line, CGPoint(x: max(0, xInTable - line.textX), y: 0))
            let clamped = max(0, min(rel, line.attributed.length))
            let prefix = (line.attributed.string as NSString).substring(to: clamped)
            return line.start + prefix.utf8.count
        }
        return nil
    }

    /// The selection rectangles for the source range `[from, to)` that fall
    /// inside tables — one per cell line the range touches, in view coordinates.
    /// Empty when the range meets no table. The peer of `fillSelection` for the
    /// grid: a table's picture rows carry no `wrapped` lines, so the ordinary
    /// row-based selection walk skips right over them and the system (iOS) or the
    /// caller would otherwise draw no highlight over a table. `from`/`to` are
    /// source byte offsets; each rect flags whether it holds an endpoint.
    func tableSelectionRects(from: Int, to: Int)
        -> [(rect: CGRect, containsStart: Bool, containsEnd: Bool)]
    {
        guard to > from else { return [] }
        var out: [(rect: CGRect, containsStart: Bool, containsEnd: Bool)] = []
        for rl in rows {
            guard let grid = rl.table, rl.tableFirst else { continue }
            for row in grid.rows {
                for cell in row.cells {
                    for (i, line) in cell.lines.enumerated() {
                        let cs = max(from, line.start), ce = min(to, line.end)
                        guard cs < ce else { continue }
                        // Byte offset within the line ≈ UTF-16 index (exact for
                        // ASCII), the same approximation the table caret rides.
                        let sIdx = max(0, min(cs - line.start, line.attributed.length))
                        let eIdx = max(0, min(ce - line.start, line.attributed.length))
                        let x0 = CTLineGetOffsetForStringIndex(line.line, CFIndex(sIdx), nil)
                        let x1 = CTLineGetOffsetForStringIndex(line.line, CFIndex(eIdx), nil)
                        let y = rl.tableTop + row.top + TableMetrics.padY
                            + CGFloat(i) * grid.lineHeight
                        out.append((
                            CGRect(x: rl.originX + line.textX + x0, y: y,
                                   width: x1 - x0, height: grid.lineHeight),
                            cs == from, ce == to
                        ))
                    }
                }
            }
        }
        return out
    }

    /// The vertical band of the cell line holding source offset `src` — a full
    /// table-cell band (clearing the cell's top/bottom padding) at the cell's
    /// first/last line, and the bare line band between. A vertical probe just past
    /// this band lands on the next line, the next cell, or out of the table,
    /// whichever is adjacent. `nil` when `src` isn't in a table (the caller uses
    /// the caret/line rect, whose thin height is already the right band there).
    func caretBand(src: Int) -> (minY: CGFloat, maxY: CGFloat)? {
        for rl in rows {
            guard let grid = rl.table, rl.tableFirst,
                  let (row, cell, _, lineIndex) = grid.locate(src: src)
            else { continue }
            let top = rl.tableTop + row.top
            let lineTop = top + TableMetrics.padY + CGFloat(lineIndex) * grid.lineHeight
            // On the table's first/last grid row, reach all the way to the table's
            // true outer edge (`tableTop` / `tableTop + height`), not just the
            // cell's band — a grid row sits a border's-width inside the box
            // (`row.top` starts at `TableMetrics.border`). Without this, ↑ from the
            // top row probes `minY - 1`, which lands *on* the top border line; the
            // hit-test resolves that back into the table and the caret never
            // reaches the block above it. The bottom edge is the symmetric peer.
            let atTop = row.top == grid.rows.first?.top
            let atBottom = row.top == grid.rows.last?.top
            let minY = lineIndex == 0
                ? (atTop ? rl.tableTop : top)
                : lineTop
            let maxY = lineIndex == cell.lines.count - 1
                ? (atBottom ? rl.tableTop + grid.height : top + row.height)
                : lineTop + grid.lineHeight
            return (minY, maxY)
        }
        return nil
    }

    /// Map a point (view coordinates) to core's `(row, ch)`: the block row from the
    /// vertical band it lands in, the visual line within it from the y offset, and
    /// the UTF-16 offset from Core Text's hit-test of the horizontal position.
    /// `click_ch` then clamps `ch` to a real caret stop.
    func hit(_ point: CGPoint) -> (row: Int, ch: Int) {
        guard !rows.isEmpty else { return (0, 0) }
        let row = rows.firstIndex { point.y < $0.top + $0.height } ?? rows.count - 1
        let rl = rows[row]
        // A picture holds no text positions: the caret's only homes on a media
        // row are in front of the box and past it, and which one a point wants is
        // which half of the box it fell in. Hit-testing the label glyphs the box
        // covers would answer "in front of it" for a tap anywhere on the picture,
        // including the blank space under a document that *ends* with one — where
        // the clamp above lands every point on this row.
        if let box = rl.media {
            let r = box.rect(top: rl.mediaTop, left: rl.originX + rl.shaped.prefixWidth)
            return (row, point.y < r.midY ? 0 : rl.attributed.length)
        }
        let lines = rl.wrapped
        let li = min(max(0, Int((point.y - rl.top - rl.labelInset) / rl.lineHeight)), lines.count - 1)
        let wl = lines[li]
        let localX = point.x - rl.originX - wl.indent
        let rel = CTLineGetStringIndexForPosition(wl.line, CGPoint(x: max(0, localX), y: 0))
        let ch = wl.start + min(max(0, rel), wl.length)
        return (row, ch)
    }

    /// The visual line index within row `row` that offset `ch` sits on, and that
    /// line's `[start, end)` UTF-16 range — for visual-line motion (Home/End/↑/↓).
    /// Returns `nil` if the row is out of range.
    func visualLine(row: Int, ch: Int) -> (index: Int, start: Int, end: Int)? {
        guard rows.indices.contains(row) else { return nil }
        let lines = rows[row].wrapped
        for (i, wl) in lines.enumerated() where ch < wl.start + wl.length || i == lines.count - 1 {
            return (i, wl.start, wl.start + wl.length)
        }
        return (0, 0, 0)
    }

    /// Fill the selection background behind the runs core marked `sel`, split across
    /// the row's visual lines, into `ctx`. Core carves the selection into run
    /// boundaries; we coalesce those into ranges and clip each to a visual line.
    func fillSelection(row rl: RowLayout, color: LeafColor, in ctx: CGContext) {
        var ranges: [(Int, Int)] = []
        var utf16 = 0
        for run in rl.row.runs {
            let len = run.text.utf16.count
            if run.sel {
                if let last = ranges.last, last.1 == utf16 {
                    ranges[ranges.count - 1].1 = utf16 + len       // merge adjacent runs
                } else {
                    ranges.append((utf16, utf16 + len))
                }
            }
            utf16 += len
        }
        guard !ranges.isEmpty else { return }
        ctx.setFillColor(color.cgColor)
        for (i, wl) in rl.wrapped.enumerated() {
            let lineStart = wl.start, lineEnd = wl.start + wl.length
            let y = rl.top + rl.labelInset + CGFloat(i) * rl.lineHeight
            for (s, e) in ranges {
                let cs = max(s, lineStart), ce = min(e, lineEnd)
                guard cs < ce else { continue }
                let x0 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(cs - lineStart), nil)
                let x1 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(ce - lineStart), nil)
                ctx.fill(CGRect(x: rl.originX + wl.indent + x0, y: y, width: x1 - x0, height: rl.lineHeight))
            }
        }
    }
}
