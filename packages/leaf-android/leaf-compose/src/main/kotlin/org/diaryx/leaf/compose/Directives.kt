package org.diaryx.leaf.compose

import androidx.compose.runtime.Composable
import uniffi.leaf_ffi.DirectiveView

// A host's leaf directives (`::name{…}`): the ones it offers to insert
// ([LeafDirectiveItem], in [LeafFormattingBar]'s Insert menu) and the drawing
// it gives each one in the editor ([LeafDirectiveContent], [LeafEditor]'s
// `directiveContent`). The vocabulary is the host's; leaf draws `⧉ name` for
// whatever no host answers for, and keeps `::page-break` its own.

/**
 * What a host draws for a leaf directive, asked per directive on each frame:
 * a composable to place over the directive's rows, or null to leave core's
 * `⧉ name` placeholder. The composable is measured at the text column's width
 * (less a quote's or a list's indent) and the rows take its height.
 *
 * The drawing is display-only as far as the editor is concerned: the caret
 * stops before and after it and never inside, and a tap on it that the host's
 * content does not consume places the caret on the nearer side. What it does
 * with a tap of its own is the host's.
 */
typealias LeafDirectiveContent = (DirectiveView) -> (@Composable () -> Unit)?

/** leaf's own directive, drawn by the editor and never offered to a host. */
internal const val PAGE_BREAK = "page-break"

/**
 * What a directive is known by — its name, label and attributes, as core keys
 * `set_directive_rows` — so a drawing, keyed by it and its occurrence
 * ([DirectiveSlot]), survives the rows moving under it.
 */
internal data class DirectiveKey(val name: String, val label: String, val attrs: List<Pair<String, String>>)

/** A directive's place among those that say the same thing: its key, and which occurrence of it. */
internal typealias DirectiveSlot = Pair<DirectiveKey, Int>

internal val DirectiveView.key: DirectiveKey
    get() = DirectiveKey(name, label, attrs.map { it.key to it.value })

/**
 * A composition key per directive: its [DirectiveKey] and which occurrence of
 * that key it is, counting from the top — so two identical embeds are two
 * slots, and an edit elsewhere keeps each one's composition.
 */
internal fun <K : Any> slotKeys(keys: List<K>): List<Pair<K, Int>> {
    val seen = HashMap<K, Int>()
    return keys.map { k ->
        val n = seen[k] ?: 0
        seen[k] = n + 1
        k to n
    }
}

/**
 * The caret stop a point at [x] lands on over a host's drawing that starts at
 * [left] and is [width] wide: the stop before it ([before], the row's first
 * character past its prefix) on the left half, the one after ([after], the
 * row's end) on the right.
 */
internal fun directiveStop(x: Float, left: Float, width: Float, before: Int, after: Int): Int =
    if (x < left + width / 2) before else after

/** Whether a caret at [ch] on a directive's row stands after the drawing rather than before it. */
internal fun isAfterDirective(ch: Int, rowLength: Int): Boolean = ch >= rowLength
