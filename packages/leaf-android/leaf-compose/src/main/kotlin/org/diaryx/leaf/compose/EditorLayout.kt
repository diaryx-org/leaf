package org.diaryx.leaf.compose

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.Placeholder
import androidx.compose.ui.text.PlaceholderVerticalAlign
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.style.BaselineShift
import androidx.compose.ui.text.style.LineHeightStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextIndent
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.leaf_ffi.BlockClass
import uniffi.leaf_ffi.DocView
import uniffi.leaf_ffi.MediaKind
import uniffi.leaf_ffi.Row
import uniffi.leaf_ffi.Run

/**
 * One row shaped: its laid-out text and the chrome geometry read off it.
 * Position-independent, so a row above an edit keeps its shape across frames
 * and only the rows an edit touched are shaped again.
 *
 * Core hands a proportional frontend *unwrapped* rows — one per block, hard
 * breaks still split — and soft wrapping is ours: [text] is wrapped at the
 * column's width, with every line after the first hung under the row's own
 * prefix (a quote's gutters, a list's marker) through [TextIndent], so the
 * caret, the hit test, the selection and the glyphs all come from the one
 * layout and cannot disagree.
 */
internal class ShapedRow(
    /** Null on a block boundary's blank row, which draws nothing but its gutters. */
    val text: TextLayoutResult?,
    val height: Float,
    /** The width of the row's prefix — the hanging indent its continuation lines carry. */
    val prefixWidth: Float,
    /** Where each blockquote level's bar sits, from the column's left edge. */
    val quoteBarXs: List<Float>,
    /** Shaped as part of a table's box-drawn picture: monospaced and never wrapped. */
    val tablePicture: Boolean,
    /**
     * A block image's picture, drawn in place of the row's text, at
     * [mediaSize] and [mediaGap] below the row's top. Only on the first of the
     * media's rows; the rest collapse to nothing.
     */
    val media: ImageBitmap? = null,
    val mediaSize: Size = Size.Zero,
    val mediaGap: Float = 0f,
)

/**
 * The geometry of a frame: every row shaped and stacked down the text column.
 * The Compose peer of leaf-swift's `EditorLayout`, and it answers the same
 * questions — where the caret is, which `(row, ch)` a point hits, how tall the
 * document is. Core measures nothing in pixels; this positions nothing in the
 * model.
 */
internal class EditorLayout(
    private val measurer: TextMeasurer,
    private val density: Density,
) {
    var theme: LeafTheme = LeafTheme()
        private set
    private var viewWidth = 0
    val width: Int get() = viewWidth
    /** Whether the layout has been given a width, and so can follow frames on its own. */
    val ready: Boolean get() = viewWidth > 0

    /** The text column's left edge and width, in px. */
    var originX = 0f
        private set
    var columnWidth = 0f
        private set

    private var rows: List<Row> = emptyList()
    private var shapes: MutableList<ShapedRow> = mutableListOf()
    private var tops = FloatArray(0)
    var height = 0f
        private set
    private var source = false

    /** Where block images come from; null draws every one as its placeholder row. */
    var media: MediaStore? = null

    val rowCount: Int get() = rows.size
    fun row(i: Int): Row = rows[i]
    fun shape(i: Int): ShapedRow = shapes[i]
    fun top(i: Int): Float = tops[i]

    /**
     * Bring the layout up to [frame]. [change] names the rows that differ from
     * the frame this layout was last given (null: none); every row outside it
     * keeps its shape, unless the theme, the width or the view changed, which
     * reshapes everything.
     */
    fun update(frame: DocView, change: RowChange?, theme: LeafTheme, viewWidth: Int) {
        val isSource = frame.view == "source"
        val all = theme != this.theme || viewWidth != this.viewWidth || isSource != source ||
            shapes.size != (rows.size) || rows.isEmpty()
        this.theme = theme
        this.viewWidth = viewWidth
        this.source = isSource
        column()

        val table = BooleanArray(frame.rows.size)
        for (t in frame.tables) {
            for (i in t.startRow.toInt() until t.endRow.toInt().coerceAtMost(table.size)) table[i] = true
        }
        val next = ArrayList<ShapedRow>(frame.rows.size)
        if (all || change == null && frame.rows.size != rows.size) {
            for (i in frame.rows.indices) next.add(shapeRow(frame.rows[i], table[i]))
        } else {
            val c = change ?: RowChange(0, 0, 0, 0)
            for (i in 0 until c.newStart) next.add(keep(i, i, frame.rows[i], table[i]))
            for (i in c.newStart until c.newEnd) next.add(shapeRow(frame.rows[i], table[i]))
            val delta = c.oldEnd - c.newEnd
            for (i in c.newEnd until frame.rows.size) next.add(keep(i + delta, i, frame.rows[i], table[i]))
        }
        rows = frame.rows
        shapes = next
        if (!isSource) placeMedia(frame)

        tops = FloatArray(rows.size)
        var y = with(density) { theme.paddingVertical.toPx() }
        for (i in rows.indices) {
            tops[i] = y
            y += shapes[i].height
        }
        height = y + with(density) { theme.paddingVertical.toPx() }
    }

    /**
     * Each block image whose picture has loaded becomes a box on its first row
     * — fitted to the column, never enlarged past its natural size (its pixels
     * read as dp), and capped in height so one tall picture cannot push a
     * screen of prose away — and the rest of its placeholder rows collapse. A
     * picture still loading, or one that would not, keeps core's placeholder
     * row. The rows keep their text, so the caret and the hit test on the
     * first row are the placeholder's, as they are in leaf-swift.
     */
    private fun placeMedia(frame: DocView) {
        val store = media ?: return
        for (m in frame.media) {
            if (m.kind != MediaKind.IMAGE) continue
            val first = m.startRow.toInt()
            if (first !in shapes.indices) continue
            val still = store.still(m.src) ?: continue
            val d = density.density
            val natural = Size(still.width * d, still.height * d)
            val maxH = with(density) { MEDIA_MAX_HEIGHT.dp.toPx() }
            val scale = minOf(1f, columnWidth / natural.width, maxH / natural.height)
            val size = Size(natural.width * scale, natural.height * scale)
            val gap = with(density) { MEDIA_GAP.dp.toPx() }
            val base = shapes[first]
            shapes[first] = ShapedRow(base.text, size.height + 2 * gap, base.prefixWidth, base.quoteBarXs, false, still, size, gap)
            for (i in first + 1 until m.endRow.toInt().coerceAtMost(shapes.size)) {
                shapes[i] = ShapedRow(null, 0f, 0f, emptyList(), false)
            }
        }
    }

    /** The old shape of row [old], if it still fits the row now at [now]. */
    private fun keep(old: Int, now: Int, row: Row, inTable: Boolean): ShapedRow {
        val s = shapes.getOrNull(old)
        // A gap row's height depends on what it divides, which is the row's own
        // `boundary`; kept rows are the same rows, so only the table flag can
        // have moved under them (a table grown by a row above).
        return if (s != null && s.tablePicture == inTable && (s.text == null) == row.isBlockGap) s
        else shapeRow(row, inTable)
    }

    private fun column() {
        val pad = with(density) { theme.paddingHorizontal.toPx() }
        val available = (viewWidth - 2 * pad).coerceAtLeast(0f)
        val measure = theme.measure
        val width = if (measure != null && measure > 0f) {
            minOf(available, measure * averageCharWidth())
        } else available
        columnWidth = width
        originX = pad + (available - width) / 2f
    }

    private var avgWidth: Pair<LeafTheme, Float>? = null
    private fun averageCharWidth(): Float {
        avgWidth?.let { if (it.first == theme) return it.second }
        val sample = "abcdefghijklmnopqrstuvwxyz "
        val r = measurer.measure(sample, TextStyle(fontFamily = theme.bodyFont, fontSize = theme.fontSize), softWrap = false)
        val w = r.getHorizontalPosition(sample.length, true) / sample.length
        avgWidth = theme to w
        return w
    }

    // MARK: shaping

    private fun shapeRow(row: Row, tablePicture: Boolean): ShapedRow {
        val prefix = row.prefixRuns
        val px = density

        // The prefix on its own line: its width is the hanging indent, and each
        // gutter placeholder's left edge is where that level's bar goes.
        var prefixWidth = 0f
        var bars = emptyList<Float>()
        if (prefix.isNotEmpty()) {
            val (text, holders) = annotate(prefix, row, tablePicture)
            val r = measurer.measure(
                text, baseStyle(row, tablePicture), softWrap = false,
                placeholders = holders, density = px,
            )
            prefixWidth = r.getHorizontalPosition(text.length, true)
            bars = r.placeholderRects.mapNotNull { it?.left }
        }

        if (row.isBlockGap) {
            val lh = theme.blockGap.value * if (row.boundary?.below == BlockClass.HEADING) theme.headingGapScale else 1f
            return ShapedRow(null, with(px) { lh.sp.toPx() }, prefixWidth, bars, tablePicture)
        }

        val drawn = if (row.isThematicBreak) prefix else row.runs
        val (text, holders) = annotate(drawn, row, tablePicture)
        val style = baseStyle(row, tablePicture).copy(
            textIndent = if (prefixWidth > 0f && !tablePicture) {
                TextIndent(restLine = with(px) { prefixWidth.toSp() })
            } else null,
        )
        val wrap = !tablePicture
        val layout = measurer.measure(
            text, style,
            softWrap = wrap,
            placeholders = holders,
            constraints = if (wrap) Constraints(maxWidth = columnWidth.toInt().coerceAtLeast(1))
            else Constraints(),
            density = px,
        )
        return ShapedRow(layout, layout.size.height.toFloat(), prefixWidth, bars, tablePicture)
    }

    /** The row's line box and alignment — everything its runs share. */
    private fun baseStyle(row: Row, tablePicture: Boolean): TextStyle {
        val heading = row.heading?.toInt()
        val base = heading?.let { theme.headingSize(it).value } ?: theme.fontSize.value
        // A run set larger than its row (`xx-large`) grows the whole line box,
        // or its glyphs would overrun the line above.
        val grown = row.runs.maxOfOrNull { theme.runSize(base, it.size) / base }?.coerceAtLeast(1f) ?: 1f
        val line = theme.rowLineHeight(heading) * grown * theme.lineSpacing(row.lineHeight)
        return TextStyle(
            color = theme.colors.text,
            fontFamily = if (source || tablePicture) theme.monoFont else theme.bodyFont,
            fontSize = base.sp,
            lineHeight = line.sp,
            lineHeightStyle = LineHeightStyle(LineHeightStyle.Alignment.Center, LineHeightStyle.Trim.None),
            textAlign = when (row.align) {
                "center" -> TextAlign.Center
                "right" -> TextAlign.End
                "justify" -> TextAlign.Justify
                else -> TextAlign.Start
            },
        )
    }

    /**
     * The runs as styled text, plus a placeholder over each quote gutter. The
     * string is the runs' text exactly, so its UTF-16 indices are core's `ch`;
     * a placeholder stands in for the characters it covers without removing
     * them, so a `│ ` pair keeps its two offsets while taking the theme's
     * gutter width, whatever the row's size — which keeps every level's bar at
     * the same x down a quote whose rows are set at different sizes.
     */
    private fun annotate(runs: List<Run>, row: Row, tablePicture: Boolean): Pair<AnnotatedString, List<AnnotatedString.Range<Placeholder>>> {
        val holders = ArrayList<AnnotatedString.Range<Placeholder>>()
        val heading = row.heading?.toInt()
        val base = heading?.let { theme.headingSize(it).value } ?: theme.fontSize.value
        val gutter = with(density) { theme.quoteIndent.toPx().toSp() }
        val text = buildAnnotatedString {
            for (run in runs) {
                val start = length
                withStyle(runStyle(run, base, heading != null, row.code, tablePicture)) { append(run.text) }
                if (run.role == "quote") {
                    var i = 0
                    while (i < run.text.length) {
                        if (run.text[i] == '│') {
                            val end = if (i + 1 < run.text.length && run.text[i + 1] == ' ') i + 2 else i + 1
                            holders.add(
                                AnnotatedString.Range(
                                    Placeholder(gutter, 1.em, PlaceholderVerticalAlign.TextCenter),
                                    start + i, start + end,
                                ),
                            )
                            i = end
                        } else i++
                    }
                }
            }
        }
        return text to holders
    }

    /** A run's own style — the Compose peer of leaf-swift's `AttributedRow.attributes`. */
    private fun runStyle(run: Run, base: Float, headingRow: Boolean, codeRow: Boolean, tablePicture: Boolean): SpanStyle {
        val c = theme.colors
        val isCode = run.role == "code"
        val bold = run.bold || headingRow || (source && isHeadingRole(run.role))
        val size = theme.runSize(base, run.size) * if (run.sup || run.sub) theme.baselineScale else 1f
        var color = when (run.role) {
            "link" -> c.link
            "code" -> c.syntaxColor(run.token)
            "list", "delimiter", "math" -> c.secondary
            // Drawn clear: a later row's marker-width indent, and a quote's
            // gutter, whose bar the editor paints itself.
            "list-indent", "quote" -> Color.Transparent
            "rule" -> c.separator
            else -> c.text
        }
        // A colour the author named wins over the role's ink — never over a
        // gutter, which is clear on purpose.
        run.textColor?.let { if (run.role != "quote") color = c.ink(it) }
        val background = when {
            run.hl != null -> c.hostWash(run.hlColor)
            run.role == "mark" -> c.wash(run.markColor)
            isCode && !codeRow && !source && !tablePicture -> c.codeBackground
            else -> Color.Unspecified
        }
        val decorations = buildList {
            if (run.underline || run.role == "link") add(TextDecoration.Underline)
            if (run.strike) add(TextDecoration.LineThrough)
        }
        return SpanStyle(
            color = color,
            fontSize = size.sp,
            fontWeight = if (bold) FontWeight.Bold else null,
            // A comment in a highlighted block is italic on top of its colour.
            fontStyle = if (run.italic || (isCode && run.token == "comment")) FontStyle.Italic else null,
            fontFamily = if (isCode || source || tablePicture) theme.monoFont else run.font?.let { theme.family(it) },
            background = background,
            textDecoration = if (decorations.isEmpty()) null else TextDecoration.combine(decorations),
            baselineShift = when {
                run.sup -> BaselineShift(0.34f / theme.baselineScale)
                run.sub -> BaselineShift(-0.16f / theme.baselineScale)
                else -> null
            },
        )
    }

    // MARK: geometry

    /** The caret's rect at row [row], UTF-16 offset [ch], in content coordinates. */
    fun caretRect(row: Int, ch: Int): Rect? {
        if (row !in rows.indices) return null
        val s = shapes[row]
        val top = tops[row]
        val text = s.text ?: return Rect(originX + s.prefixWidth, top, originX + s.prefixWidth, top + s.height)
        val r = text.getCursorRect(ch.coerceIn(0, text.layoutInput.text.length))
        return r.translate(originX, top)
    }

    /**
     * The `(row, ch)` a content-coordinate point hits, or null below the last
     * row — where a tap means "the end of the document". A point on a row that
     * holds no caret (a boundary, a table rule) resolves to the nearest row
     * that does.
     */
    fun hit(point: Offset): Pair<Int, Int>? {
        if (rows.isEmpty()) return null
        if (point.y >= tops[rows.size - 1] + shapes[rows.size - 1].height) return null
        var i = rowAt(point.y)
        if (!rows[i].holdsCaret) {
            val mid = tops[i] + shapes[i].height / 2
            val up = (i - 1 downTo 0).firstOrNull { rows[it].holdsCaret }
            val down = (i + 1 until rows.size).firstOrNull { rows[it].holdsCaret }
            i = (if (point.y < mid) up ?: down else down ?: up) ?: return null
        }
        val text = shapes[i].text ?: return i to 0
        val local = Offset(point.x - originX, (point.y - tops[i]).coerceIn(0f, shapes[i].height - 1f))
        return i to text.getOffsetForPosition(local)
    }

    /** The row whose box holds [y], clamped to the document. */
    fun rowAt(y: Float): Int {
        var lo = 0
        var hi = rows.size - 1
        while (lo < hi) {
            val mid = (lo + hi + 1) / 2
            if (tops[mid] <= y) lo = mid else hi = mid - 1
        }
        return lo
    }

    /**
     * The `(row, ch)` one visual line above or below [row]/[ch], keeping [x] —
     * soft-wrapped lines are ours, so moving between them is too; core's own
     * `move_up` moves between *its* rows, which are whole blocks here. Null at
     * the document's edge.
     */
    fun verticalMove(row: Int, ch: Int, x: Float, down: Boolean): Pair<Int, Int>? {
        val text = shapes.getOrNull(row)?.text ?: return null
        val line = text.getLineForOffset(ch.coerceIn(0, text.layoutInput.text.length))
        val target: Pair<Int, Int> = if (down && line < text.lineCount - 1) row to line + 1
        else if (!down && line > 0) row to line - 1
        else {
            val step = if (down) 1 else -1
            var j = row + step
            while (j in rows.indices && (!rows[j].holdsCaret || shapes[j].text == null)) j += step
            if (j !in rows.indices) return null
            val t = shapes[j].text!!
            j to if (down) 0 else t.lineCount - 1
        }
        val t = shapes[target.first].text!!
        val y = (t.getLineTop(target.second) + t.getLineBottom(target.second)) / 2
        return target.first to t.getOffsetForPosition(Offset(x - originX, y))
    }

    /** The selection's painted shape between two `(row, ch)` ends, in content coordinates. */
    fun selectionPath(from: Pair<Int, Int>, to: Pair<Int, Int>): Path {
        val (s, e) = if (compareValuesBy(from, to, { it.first }, { it.second }) <= 0) from to to else to to from
        val path = Path()
        val nub = 6f * density.density
        for (r in s.first..e.first) {
            if (r !in rows.indices) continue
            val text = shapes[r].text ?: continue
            val len = text.layoutInput.text.length
            val a = if (r == s.first) s.second.coerceIn(0, len) else 0
            val b = if (r == e.first) e.second.coerceIn(0, len) else len
            if (a < b) {
                val p = text.getPathForRange(a, b)
                p.translate(Offset(originX, tops[r]))
                path.addPath(p)
            }
            // A row the selection runs past carries its line break selected too
            // — shown as a nub, so a selected empty line is visible at all.
            if (r < e.first) {
                val end = text.getCursorRect(len)
                path.addRect(Rect(originX + end.left, tops[r] + end.top, originX + end.left + nub, tops[r] + end.bottom))
            }
        }
        return path
    }

    companion object {
        fun isHeadingRole(role: String) = role.length == 2 && role[0] == 'h' && role[1] in '1'..'6'

        /** A block image's tallest box, and the room above and below it, in dp. */
        const val MEDIA_MAX_HEIGHT = 480
        const val MEDIA_GAP = 8
    }
}
