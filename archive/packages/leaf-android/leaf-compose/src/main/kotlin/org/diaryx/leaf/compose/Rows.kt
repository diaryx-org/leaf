package org.diaryx.leaf.compose

import uniffi.leaf_ffi.DocView
import uniffi.leaf_ffi.Row
import uniffi.leaf_ffi.Run

// What a frontend reads off a core `Row` beyond its runs — the peers of the
// `Row` extensions in leaf-swift's EditorLayout.swift, and spelled the same
// way so the two can be read side by side.

/** A quote gutter, a list marker, or the clear indent a marker becomes on an item's later rows. */
internal fun isPrefixRole(role: String) = role == "quote" || role == "list" || role == "list-indent"

/** The row's leading block decoration — the runs a continuation line hangs under. */
internal val Row.prefixRuns: List<Run> get() = runs.takeWhile { isPrefixRole(it.role) }

/** Whether this is the blank row core spells a block boundary with. Core's answer, not a guess. */
internal val Row.isBlockGap: Boolean get() = boundary != null

/** A row a caret can stand on: neither a boundary nor a table's box-drawn rule. */
internal val Row.holdsCaret: Boolean get() = !decoration

/**
 * Whether this row is a thematic break — a `---`, drawn as a line across the
 * column rather than as core's row of `─` glyphs. Everything after the prefix
 * is rule-role dashes, which tells it from a table's box-drawing rows.
 */
internal val Row.isThematicBreak: Boolean
    get() {
        if (decoration || code) return false
        val body = runs.dropWhile { isPrefixRole(it.role) }
        return body.isNotEmpty() && body.all { r -> r.role == "rule" && r.text.isNotEmpty() && r.text.all { it == '─' } }
    }

/**
 * The task checkbox this row draws — core's `☐ `/`☑ ` marker run and the
 * UTF-16 offset it starts at — or null on every row with no box. Its `src` is
 * the item's own: what `toggleTaskAt` wants.
 */
internal val Row.taskMarker: Pair<Run, Int>?
    get() {
        var start = 0
        for (run in prefixRuns) {
            if (run.role == "list" && (run.text.startsWith("☐") || run.text.startsWith("☑"))) return run to start
            start += run.text.length
        }
        return null
    }

/** The row's text, as the runs spell it — the string every `ch` indexes. */
internal val Row.text: String get() = runs.joinToString("") { it.text }

/**
 * Where two frames' rows differ: the span of the new frame's rows that are not
 * the old one's, and the span of the old frame's they replaced.
 */
internal data class RowChange(val newStart: Int, val newEnd: Int, val oldStart: Int, val oldEnd: Int)

/**
 * Apply [change], a frame core answered as the change since [held], making it
 * the whole frame it stands for: its rows spliced over the ones they replace,
 * every run after them moved by `srcShift` (an edit moves the source of
 * everything below it, and those rows are otherwise the frame before's), and
 * the rest of the frame taken as it came, since it is complete on every frame.
 *
 * Null when [change] is not against [held] — the caller has lost step and asks
 * for a whole frame. A whole frame applies to anything and changes every row.
 */
internal fun applyFrame(held: DocView?, change: DocView): Pair<DocView, RowChange?>? {
    if (change.basis == 0u) {
        val old = held?.rows?.size ?: 0
        return change to RowChange(0, change.rows.size, 0, old)
    }
    if (held == null || held.frame != change.basis) return null
    val start = change.rowStart.toInt()
    val end = start + change.replaced.toInt()
    if (end > held.rows.size) return null
    val rows = ArrayList<Row>(change.rowCount.toInt())
    rows.addAll(held.rows.subList(0, start))
    rows.addAll(change.rows)
    val shift = change.srcShift
    for (row in held.rows.subList(end, held.rows.size)) {
        rows.add(if (shift == 0) row else row.copy(runs = row.runs.map { it.copy(src = (it.src.toLong() + shift).toUInt()) }))
    }
    val whole = change.copy(rows = rows, basis = 0u, rowStart = 0u, replaced = 0u, srcShift = 0)
    val nothing = change.rows.isEmpty() && change.replaced == 0u && shift == 0
    return whole to if (nothing) null else RowChange(start, start + change.rows.size, start, end)
}

/**
 * The chrome-facing state of a frame — what a toolbar lights its buttons from.
 * Republished only when it changes, which is why facts like [link] and
 * [codeBlock] ride here rather than being asked for: walking the caret out of
 * a link moves no mark, and a button asking for itself would never be told.
 */
data class LeafEditorChrome(
    val view: String = "wysiwyg",
    val dirty: Boolean = false,
    val heading: Int? = null,
    val active: Set<String> = emptySet(),
    val link: String? = null,
    val canUndo: Boolean = false,
    val canRedo: Boolean = false,
    val hasSelection: Boolean = false,
    val codeBlock: Boolean = false,
    /** Whether the caret stands in a verse — what lights the Verse key. */
    val verse: Boolean = false,
    val blockquote: Boolean = false,
    /** True ticked, false an empty box, null a plain item or no item at all. */
    val task: Boolean? = null,
) {
    companion object {
        fun of(v: DocView) = LeafEditorChrome(
            view = v.view,
            dirty = v.dirty,
            heading = v.heading?.toInt(),
            active = v.active.toSet(),
            link = v.link,
            canUndo = v.canUndo,
            canRedo = v.canRedo,
            hasSelection = v.hasSelection,
            codeBlock = v.codeBlock,
            verse = v.verse,
            blockquote = v.blockquote,
            task = v.task,
        )
    }
}
