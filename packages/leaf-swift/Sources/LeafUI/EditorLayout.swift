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
    /// The destination of the link the caret stands in, or nil — what lights the
    /// toolbar's Link button and seeds an edit of that link.
    ///
    /// A chrome fact rather than a question a toolbar asks for itself, and for a
    /// reason `Equatable` below makes plain: the state is only republished when
    /// it *changes*, so a Link button reading the doc directly would never be
    /// told the caret had stepped out of a link — no mark, heading, or dirty flag
    /// moves with it. Nil for a wikilink, which has no node to repoint.
    public var link: String?
    /// Whether there is a step to undo, and one to redo — for a toolbar's
    /// history buttons to enable by, as the Edit menu's items already do
    /// through the view's `undoManager`. Both false on a read-only document.
    public var canUndo: Bool
    public var canRedo: Bool
    /// The colour of the highlight the caret stands in — which swatch a colour
    /// menu ticks — and nil both outside a highlight and inside an uncoloured
    /// one.
    ///
    /// Here for `link`'s reason, and more sharply: walking from a red highlight
    /// into a blue one moves no mark, no heading and no dirty flag, so a palette
    /// that asked core for itself would never be republished, and would keep the
    /// first colour ticked.
    public var markColor: MarkColor?
    /// Whether a (non-empty) selection is live. What tells "colour the highlight
    /// I'm in" from "highlight what I've chosen, in this colour" — one press
    /// that means both, in `LeafEditorModel.highlight(_:)`.
    public var hasSelection: Bool

    /// `link`, `markColor` and `hasSelection` default so a host that built a
    /// state by hand before any of them existed still compiles; the
    /// frame-projecting initializer below is the real path.
    public init(view: String, dirty: Bool, heading: UInt32?, active: [String], link: String? = nil,
                canUndo: Bool = false, canRedo: Bool = false,
                markColor: MarkColor? = nil, hasSelection: Bool = false) {
        self.view = view; self.dirty = dirty; self.heading = heading
        self.active = active; self.link = link
        self.canUndo = canUndo; self.canRedo = canRedo
        self.markColor = markColor; self.hasSelection = hasSelection
    }

    /// Project a full `DocView` down to the chrome-facing state.
    public init(_ v: DocView) {
        self.init(view: v.view, dirty: v.dirty, heading: v.heading, active: v.active, link: v.link,
                  canUndo: v.canUndo, canRedo: v.canRedo,
                  markColor: v.markColor, hasSelection: v.hasSelection)
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
    /// The sheet this row starts on, in the paginated flow. Always 0 in the
    /// continuous one, which is a single unbounded sheet.
    var page: Int = 0
    /// The origin of each visual line, when the row's lines are *not* evenly
    /// spaced down from `top` at a single x — which is what a break inside a
    /// paragraph makes of them. A page break moves a line's y; a *column* break
    /// moves its x as well, and back up the sheet.
    ///
    /// Empty in the continuous flow, where `(originX, top + labelInset +
    /// i·lineHeight)` is exact and there is nothing to store. `lineOrigin(_:)` is
    /// the one accessor both cases go through, so every caller reads line
    /// positions the same way.
    var lineOrigins: [CGPoint] = []

    var attributed: NSAttributedString { shaped.attributed }
    var wrapped: [WrappedLine] { shaped.wrapped }
    var lineHeight: CGFloat { shaped.lineHeight }
    /// The block's total height — the grid's height on a table's first row, zero
    /// on its other (collapsed) rows, a boundary's contextual gap, else the label
    /// inset (if any) plus one `lineHeight` per visual line.
    ///
    /// A split row measures from the top of its highest line to the bottom of its
    /// lowest — the union of its `bands`, not a reach downward from `top`, since a
    /// column break puts some of its lines *above* where it started. Continuously
    /// that is exactly the old formula.
    var height: CGFloat {
        if let t = table { return tableFirst ? t.height : 0 }
        if let m = media { return mediaFirst ? m.height : 0 }
        if let gapHeight { return gapHeight }
        guard !lineOrigins.isEmpty else {
            return labelInset + CGFloat(shaped.wrapped.count) * shaped.lineHeight
        }
        let boxes = lineBoxes
        guard let first = boxes.first else { return 0 }
        let minY = boxes.reduce(first.minY) { min($0, $1.minY) }
        let maxY = boxes.reduce(first.maxY) { max($0, $1.maxY) }
        return maxY - minY
    }

    /// The origin of visual line `i`. Evenly spaced down from the row's own top at
    /// the row's own x in the continuous flow; read off `lineOrigins` once
    /// pagination has placed the lines itself, which it does the moment a page is
    /// set.
    func lineOrigin(_ i: Int) -> CGPoint {
        lineOrigins.indices.contains(i)
            ? lineOrigins[i]
            : CGPoint(x: originX, y: top + labelInset + CGFloat(i) * lineHeight)
    }

    /// The top of visual line `i` — `lineOrigin(_:).y`, for the callers that only
    /// ever wanted the vertical half.
    func lineTop(_ i: Int) -> CGFloat { lineOrigin(i).y }

    /// The row's drawn boxes, one per visual line — or the single box a table, a
    /// media block, or a boundary row occupies, those being placed whole. What a
    /// hit-test searches, since a point resolves to a *line*.
    var lineBoxes: [CGRect] {
        if table != nil || media != nil || gapHeight != nil {
            return height > 0
                ? [CGRect(x: originX, y: top, width: columnWidth, height: height)]
                : []
        }
        return wrapped.indices.map { i in
            let o = lineOrigin(i)
            return CGRect(x: o.x, y: o.y, width: columnWidth, height: lineHeight)
        }
    }

    /// `lineBoxes` merged wherever two lines actually touch — one band covering
    /// the whole row in the continuous flow, and one per column once a break has
    /// split it.
    ///
    /// This is what anything painted *behind* the text measures itself against: a
    /// code block's fill, a blockquote's gutter bar, a directive's outline. Using
    /// `height` for those would stretch them over the gap between two sheets, or
    /// clean across the gutter between two columns — and the backdrop and the
    /// gutter are the two places a block's own background must never appear. The
    /// label strip belongs to the first band, since it sits above the first line
    /// and travels with it.
    var bands: [CGRect] {
        var out: [CGRect] = []
        for box in lineBoxes {
            if let last = out.last, abs(last.maxY - box.minY) < 0.5, abs(last.minX - box.minX) < 0.5 {
                out[out.count - 1].size.height += box.height
            } else {
                out.append(box)
            }
        }
        if labelInset > 0, !out.isEmpty {
            out[0].origin.y -= labelInset
            out[0].size.height += labelInset
        }
        return out
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
        // One bar per level per band, so a quote broken across two sheets — or two
        // columns — gets a bar down each of them rather than one running over the
        // gap between. The band carries its own x, which is what a column break
        // moves.
        return bands.flatMap { band in
            shaped.quoteBarXs.map { x in
                CGRect(x: band.minX + x, y: band.minY,
                       width: theme.quoteBarWidth, height: band.height)
            }
        }
    }

    /// A thematic break's drawn line, in view coordinates: a hairline centred in
    /// the row's box, running from past the row's own prefix to the right edge of
    /// the text column. `nil` on every other row.
    func ruleLine(theme: EditorTheme) -> CGRect? {
        guard row.isThematicBreak, table == nil else { return nil }
        // Measured off the line's own origin rather than the row's: a break is a
        // single line, but reading its position off the line keeps it right
        // wherever pagination put that line — including in the second column.
        let o = lineOrigin(0)
        let x = o.x + shaped.prefixWidth
        let right = o.x + max(columnWidth, shaped.prefixWidth)
        return CGRect(x: x, y: (o.y + lineHeight * 0.5 - theme.ruleThickness / 2).rounded(),
                      width: max(0, right - x), height: theme.ruleThickness)
    }
}

/// The vertical cursor rows are placed against.
///
/// In the continuous flow it is a plain running `y` and every method here is a
/// no-op — one unbounded sheet, which is what a scrolling document is. With a
/// `PageSetup` it is the same `y` walked down a stack of sheets, jumping to the
/// next sheet's top margin when what comes next won't fit under the current
/// one's bottom. Keeping both cases on one cursor is what lets the layout below
/// stay a single walk: pagination is a change to *how `y` advances*, and nothing
/// downstream of `RowLayout.top` needs to know which flow produced it.
private struct Flow {
    let page: PageSetup?
    /// The stack's left edge — what turns a column number into an x.
    let sheetX: CGFloat
    var y: CGFloat
    /// The sheet `y` currently sits on, and the column within it.
    var index = 0
    var column = 0

    /// The left edge of the column rows are being placed in.
    func originX(_ fallback: CGFloat) -> CGFloat {
        page.map { $0.columnX(column, sheetX: sheetX) } ?? fallback
    }

    /// The reading-order slot the cursor is in. Zero throughout the continuous
    /// flow, which is one unbounded column.
    var slot: Int { page.map { $0.slot(index, column) } ?? 0 }

    /// Whether `height` fits in the room left in this column. Always true in the
    /// continuous flow, which has no bottom to run out of.
    func fits(_ height: CGFloat) -> Bool {
        guard let page else { return true }
        return y + height <= page.contentBottom(index)
    }

    /// Make room for `height`, opening the next column if it doesn't fit here.
    ///
    /// A block too tall for a column is left to overflow rather than bounced
    /// between columns forever: the `y > contentTop` guard means a block that has
    /// a whole empty column and still doesn't fit is simply placed, and `open`
    /// then resumes below wherever it ended — so an oversized figure or table
    /// takes the space it covers to itself instead of hanging the walk.
    mutating func fit(_ height: CGFloat) {
        guard let page, !fits(height), y > page.contentTop(index) else { return }
        open()
    }

    /// Move to the next slot in reading order: the next column of this sheet, or
    /// the first column of the next sheet.
    private mutating func open() {
        guard let page else { return }
        if y > page.contentBottom(index) {
            // The block just placed ran past this sheet altogether, so every
            // column on it is already covered. Resume on the first sheet whose
            // columns start at or below where that block ended.
            repeat { index += 1 } while page.contentTop(index) < y
            column = 0
        } else if column + 1 < page.columns {
            column += 1
        } else {
            column = 0
            index += 1
        }
        y = page.contentTop(index)
    }
}

/// The laid-out rows of one `DocView` plus the geometry queries over them.
struct EditorLayout {
    let rows: [RowLayout]
    /// Total content height including top+bottom padding — the view's fitting size.
    /// With a page set it is the whole stack of sheets plus its backdrop, so the
    /// last sheet shows full height even when the document stops a line into it.
    let contentHeight: CGFloat
    /// The stack's own width with a page set — a sheet plus its backdrop either
    /// side, which is what makes a window narrower than a sheet scroll sideways
    /// instead of cropping it. Zero in the continuous flow, which has no width of
    /// its own: it takes the viewport's.
    let contentWidth: CGFloat
    /// The text column's left edge in view coordinates. Every x here is measured
    /// from this, not from `theme.padding.left`: with a `measure` set the column
    /// is centred in the view, so the padding is only the floor it can't cross.
    /// With a page set it is the sheet's own left margin — the *first* column's,
    /// once there is more than one; a row's own column is `RowLayout.originX` and
    /// a line's is `RowLayout.lineOrigin(_:)`.
    let originX: CGFloat
    /// The sheet this frame was laid onto and where its left edge sits — kept so
    /// the geometry queries can work out which column a point is in. Nil in the
    /// continuous flow.
    let setup: PageSetup?
    private let sheetX: CGFloat
    /// The text column's width — what rows wrap to, and how far a thematic break
    /// or a directive outline runs.
    let columnWidth: CGFloat
    /// Every sheet's frame, top to bottom, in layout coordinates — what the view
    /// paints the paper and its shadow onto. Empty in the continuous flow.
    let pages: [CGRect]
    /// Whether the document holds no glyphs at all — the state a placeholder cue
    /// stands in for. Read off the frame core published rather than off the
    /// source, so it costs a walk over rows already in hand instead of
    /// serializing the document to ask whether it is empty.
    let isEmpty: Bool

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
    /// `page` switches on the paginated flow: rows are broken across a stack of
    /// sheets and wrap to the sheet's margins rather than to the theme's
    /// `measure`, which a page supersedes. `nil` (the default) is the continuous
    /// scrolling flow.
    init(_ docView: DocView, theme: EditorTheme, viewWidth: CGFloat, page: PageSetup? = nil,
         cache: inout [Row: ShapedRow], media: MediaStore? = nil) {
        if let page {
            let x = page.sheetX(in: viewWidth)
            self.init(docView, theme: theme, originX: x + page.margins.left,
                      columnWidth: page.columnWidth, page: page, sheetX: x,
                      cache: &cache, media: media)
        } else {
            let column = theme.column(in: viewWidth)
            self.init(docView, theme: theme, originX: column.originX, columnWidth: column.width,
                      cache: &cache, media: media)
        }
    }

    /// Lay out into an explicit column — the designated initializer the others
    /// resolve to. `columnWidth <= 0` means "don't wrap" (one visual line per
    /// row), the state before a view knows its bounds.
    /// `page`/`sheetX` are the paginated flow's stack: the sheet to break onto and
    /// where its left edge sits. The `viewWidth` initializer above works both out;
    /// nothing else passes them.
    init(_ docView: DocView, theme: EditorTheme, originX: CGFloat, columnWidth: CGFloat,
         page: PageSetup? = nil, sheetX: CGFloat = 0,
         cache: inout [Row: ShapedRow], media: MediaStore? = nil) {
        let wrapWidth = columnWidth
        self.originX = originX
        self.columnWidth = max(0, columnWidth)
        self.setup = page
        self.sheetX = sheetX
        // Every glyph the reader can see is a run's text, including the ones a
        // surface redraws as graphics — a table's box picture, a media row's
        // `🖼 alt`, a break's `───`. So one pass over the runs answers this for
        // every kind of block, with no list of exceptions to keep in step.
        self.isEmpty = docView.rows.allSatisfy { row in row.runs.allSatisfy { $0.text.isEmpty } }
        var layouts: [RowLayout] = []
        layouts.reserveCapacity(docView.rows.count)
        var next = Dictionary<Row, ShapedRow>(minimumCapacity: docView.rows.count)
        // The continuous flow opens under the theme's top padding; the paginated
        // one at the first column's top margin, which is the padding's counterpart.
        var flow = Flow(page: page, sheetX: sheetX, y: page?.contentTop(0) ?? theme.padding.top)

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
                // A grid is atomic: it draws itself in one pass off `tableTop`, so
                // there is nothing to split it at. It moves whole to the next
                // sheet, and one too tall for any sheet gets its own.
                flow.fit(grid.height)
                let tableTop = flow.y
                let tableX = flow.originX(originX)
                // Keep every picture row (rows stay 1:1 with the frame), but
                // collapse them onto the grid: the first carries its height, the
                // rest are zero-height, and all defer drawing/caret to the grid.
                for r in Int(t.startRow)..<Int(t.endRow) where r < docView.rows.count {
                    layouts.append(RowLayout(
                        row: docView.rows[r], shaped: emptyShape, top: tableTop,
                        originX: tableX, columnWidth: wrapWidth,
                        table: grid, tableTop: tableTop, tableFirst: r == Int(t.startRow),
                        page: flow.index
                    ))
                }
                flow.y += grid.height
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
                // Atomic for the same reason a grid is: the box is one picture
                // drawn off `mediaTop`, and half a photograph at the foot of a
                // sheet is not a thing to draw.
                flow.fit(box.height)
                let mediaTop = flow.y
                let mediaX = flow.originX(originX)
                for r in Int(mv.startRow)..<Int(mv.endRow) where r < docView.rows.count {
                    layouts.append(RowLayout(
                        row: docView.rows[r],
                        shaped: r == Int(mv.startRow) ? shaped : emptyShape,
                        top: mediaTop,
                        originX: mediaX, columnWidth: wrapWidth,
                        media: box, mediaTop: mediaTop, mediaFirst: r == Int(mv.startRow),
                        page: flow.index
                    ))
                }
                flow.y += box.height
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
            let labelInset = hasLabel ? theme.directiveLabelHeight : 0

            // A boundary is spaced by what it separates, and core says what that
            // is (`row.boundary`) — so this is a lookup, not a walk over the
            // neighbouring rows.
            if row.isBlockGap {
                // Paragraph spacing exists to hold two blocks apart on the same
                // sheet. A gap that no longer fits under the current one has
                // nothing left to separate, and carried over the break it would
                // push the next block an arbitrary distance down from the top
                // margin — so it collapses, exactly as space-before does at the
                // head of a page. The break itself is the separation now.
                let gap = theme.blockGap(row.boundary)
                let placed = flow.fits(gap) ? gap : 0
                layouts.append(RowLayout(row: row, shaped: shaped, top: flow.y,
                                         originX: flow.originX(originX), columnWidth: wrapWidth,
                                         gapHeight: placed, page: flow.index))
                flow.y += placed
                i += 1
                continue
            }

            let count = max(1, shaped.wrapped.count)
            // A heading is placed whole. Split, its first line is stranded at the
            // foot of one column and the rest of it heads the next, which reads as
            // two headings rather than one — and a heading is short enough that
            // moving it whole costs a line or two of a column, not a screen.
            let splittable = page != nil && row.heading == nil && count > 1
            var lineOrigins: [CGPoint] = []
            var startPage = flow.index
            if page != nil {
                if splittable {
                    for k in 0..<count {
                        // The label strip rides the first line: it names the block
                        // that line opens, so the two travel together.
                        let h = (k == 0 ? labelInset : 0) + shaped.lineHeight
                        flow.fit(h)
                        if k == 0 { startPage = flow.index }
                        // Each line takes the column it actually landed in — a
                        // break between two of them moves the x as well as the y
                        // once a sheet carries more than one.
                        lineOrigins.append(CGPoint(x: flow.originX(originX),
                                                   y: flow.y + (k == 0 ? labelInset : 0)))
                        flow.y += h
                    }
                } else {
                    let h = labelInset + CGFloat(count) * shaped.lineHeight
                    // Keep a heading with the text it introduces. One left alone at
                    // the foot of a sheet announces nothing the reader can see —
                    // they turn the page to find out what it was for — so it asks
                    // for its gap plus a line of that text beyond itself, and moves
                    // down when it can't have them.
                    //
                    // Only when something actually follows: a heading that ends the
                    // document has nothing to be kept with, and bumping it onto a
                    // sheet of its own would be the worse answer. Two rows of
                    // lookahead, because core puts at most one boundary row between
                    // two blocks.
                    let followed = row.heading != nil
                        && docView.rows[(i + 1)..<min(i + 3, docView.rows.count)]
                            .contains { !$0.isBlockGap }
                    flow.fit(h + (followed ? theme.blockGap + theme.lineHeight : 0))
                    startPage = flow.index
                    let x = flow.originX(originX)
                    for k in 0..<count {
                        lineOrigins.append(CGPoint(
                            x: x, y: flow.y + labelInset + CGFloat(k) * shaped.lineHeight))
                    }
                    flow.y += h
                }
            }
            let rl = RowLayout(
                row: row, shaped: shaped,
                top: lineOrigins.first.map { $0.y - labelInset } ?? flow.y,
                // The row's own column is the one it *starts* in; a split row's
                // later lines carry their own (see `lineOrigin(_:)`).
                originX: lineOrigins.first?.x ?? originX, columnWidth: wrapWidth,
                labelInset: labelInset,
                page: startPage, lineOrigins: lineOrigins)
            // Paginated, the lines are already placed and `flow.y` is past them.
            if page == nil { flow.y += rl.height }
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
                                     shaped: emptyShape, top: flow.y,
                                     originX: originX, columnWidth: wrapWidth))
            flow.y += theme.lineHeight
        }
        rows = layouts
        if let page {
            // Every sheet the walk touched, including any an oversized block spilled
            // across — `flow.index` is where the cursor ended, `index(at:)` catches
            // the overflow that ran past it. A hair off `flow.y` so a document that
            // stops exactly on a sheet's top margin doesn't add an empty one after it.
            let last = max(flow.index, page.index(at: max(page.sheetTop(0), flow.y - 1)))
            pages = (0...last).map { page.sheetRect($0, x: sheetX) }
            contentHeight = (pages.last?.maxY ?? 0) + page.backdrop
            contentWidth = page.stackWidth
        } else {
            pages = []
            contentHeight = flow.y + theme.padding.bottom
            contentWidth = 0
        }
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
            let o = rl.lineOrigin(i)
            return CGRect(x: o.x + wl.indent + x, y: o.y, width: 1.5, height: rl.lineHeight)
        }
        let o = rl.lineOrigin(0)
        return CGRect(x: o.x, y: o.y, width: 1.5, height: rl.lineHeight)
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

    /// The line box a placeholder cue draws in — exactly the box the document's
    /// first typed character will take, so the cue and the prose that replaces it
    /// begin at the same point.
    ///
    /// Not `theme.padding`, which is only the floor: a `measure` centres the text
    /// column in the room the padding leaves (`EditorTheme.column(in:)`), and a
    /// page moves it to the sheet's own margin. Both answers live in the layout
    /// and nowhere else, which is why a cue drawn from outside it can only guess.
    ///
    /// Nil once the document holds anything: a cue under prose is a cue over
    /// something the reader wrote.
    var placeholderBox: CGRect? {
        guard isEmpty, let first = rows.first else { return nil }
        let o = first.lineOrigin(0)
        return CGRect(x: o.x, y: o.y, width: columnWidth, height: first.lineHeight)
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
    /// The boxes a range of text occupies, one per visual line it touches, in
    /// layout coordinates — what a selection overlay, a find-bar match, or a
    /// dictation highlight is drawn from. `from`/`to` are core positions
    /// (`posForOffset`); each box says whether it holds the range's start or end,
    /// which is where a touch surface puts its handles. Tables carry no wrapped
    /// lines, so a range over one comes from `tableSelectionRects` instead.
    func rangeRects(from s: (row: Int, ch: Int), to e: (row: Int, ch: Int))
        -> [(rect: CGRect, containsStart: Bool, containsEnd: Bool)]
    {
        guard e.row >= s.row else { return [] }
        var rects: [(rect: CGRect, containsStart: Bool, containsEnd: Bool)] = []
        for row in s.row...e.row where rows.indices.contains(row) {
            let rl = rows[row]
            let rowFrom = (row == s.row) ? s.ch : 0
            let rowTo = (row == e.row) ? min(e.ch, rl.attributed.length) : rl.attributed.length
            for (i, wl) in rl.wrapped.enumerated() {
                let lineStart = wl.start, lineEnd = wl.start + wl.length
                let cs = max(rowFrom, lineStart), ce = min(rowTo, lineEnd)
                guard cs < ce else { continue }
                let x0 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(cs - lineStart), nil)
                let x1 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(ce - lineStart), nil)
                // `lineOrigin` is the one accessor both flows go through: the
                // continuous stack's formula, or where pagination placed the line.
                let o = rl.lineOrigin(i)
                rects.append((CGRect(x: o.x + wl.indent + x0, y: o.y, width: x1 - x0, height: rl.lineHeight),
                              row == s.row && cs == s.ch,
                              row == e.row && ce == e.ch))
            }
        }
        return rects
    }

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
    /// The reading-order slot a layout point falls in — its (sheet, column).
    ///
    /// Zero everywhere in the continuous flow, and just the sheet index while a
    /// sheet has one column, which is what lets `locate` be the same walk in every
    /// flow rather than three of them.
    private func slot(at p: CGPoint) -> Int {
        setup.map { $0.slot(at: p, sheetX: sheetX) } ?? 0
    }

    /// The row a point resolves to, and the visual line within it.
    ///
    /// While a sheet has one text column, `rows` runs top to bottom and this is
    /// the scan it always was. A second column breaks that ordering outright — it
    /// starts back at the *top* of the same sheet, so `top` is no longer
    /// increasing down the array and "the first row the point is above" stops
    /// meaning anything. So the point's slot picks the candidates first, and the
    /// vertical scan runs inside it.
    ///
    /// Over lines rather than rows, because a row can straddle a column boundary
    /// exactly as it can a page one, and its two halves are then in different
    /// slots. A table, a media box, and a boundary gap are each one whole box (see
    /// `lineBoxes`), being placed atomically.
    private func locate(_ point: CGPoint) -> (row: Int, line: Int) {
        let target = slot(at: point)
        var first: (Int, Int)?      // the slot's first line — the point is above it
        var above: (Int, Int)?      // the last line in the slot starting at or above the point
        var before: (Int, Int)?     // the last line in any earlier slot
        for (r, rl) in rows.enumerated() {
            for (i, box) in rl.lineBoxes.enumerated() {
                let s = slot(at: CGPoint(x: box.midX, y: box.midY))
                if s < target { before = (r, i); continue }
                guard s == target else { continue }
                if first == nil { first = (r, i) }
                if point.y >= box.minY { above = (r, i) }
            }
        }
        // In the slot the point landed in; else above everything in it; else the
        // end of the content before it — which is what a click into a column the
        // document never reached should mean.
        return above ?? first ?? before ?? (0, 0)
    }

    func hit(_ point: CGPoint) -> (row: Int, ch: Int) {
        guard !rows.isEmpty else { return (0, 0) }
        let (row, li) = locate(point)
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
        guard rl.wrapped.indices.contains(li) else { return (row, 0) }
        let wl = rl.wrapped[li]
        let localX = point.x - rl.lineOrigin(li).x - wl.indent
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
            let o = rl.lineOrigin(i)
            for (s, e) in ranges {
                let cs = max(s, lineStart), ce = min(e, lineEnd)
                guard cs < ce else { continue }
                let x0 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(cs - lineStart), nil)
                let x1 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(ce - lineStart), nil)
                ctx.fill(CGRect(x: o.x + wl.indent + x0, y: o.y, width: x1 - x0, height: rl.lineHeight))
            }
        }
    }
}
