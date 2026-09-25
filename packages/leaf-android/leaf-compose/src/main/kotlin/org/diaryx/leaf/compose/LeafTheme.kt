package org.diaryx.leaf.compose

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * The presentation knobs — the peer of leaf-swift's `EditorTheme` and
 * leaf-web's CSS. Everything here is *look*, never model: it maps a run's
 * semantic `role` to a face, size, weight and colour. Core decides what a
 * glyph is; this decides how it is painted.
 *
 * Headings are told apart by size and weight alone, as every other frontend
 * tells them, so [headingScale] is the whole hierarchy. The numbers are the
 * Swift theme's, so a document reads at the same proportions on every
 * platform; the colours are Material's, in two appearances ([light], [dark]).
 */
@Immutable
data class LeafTheme(
    val bodyFont: FontFamily = FontFamily.Default,
    val monoFont: FontFamily = FontFamily.Monospace,
    val fontSize: TextUnit = 17.sp,
    /** Body line height. A heading's rows grow from it — see [rowHeightScale]. */
    val lineHeight: TextUnit = 26.sp,
    /**
     * The height of the blank row core spells a block boundary with, as a
     * fraction of [lineHeight] — so a boundary reads as paragraph spacing
     * rather than as an empty line nobody typed.
     */
    val blockGapScale: Float = 0.5f,
    /** The gap above a heading, as a multiple of the ordinary block gap. */
    val headingGapScale: Float = 1.8f,
    val headingScale: List<Float> = listOf(1.625f, 1.375f, 1.1875f, 1.0625f, 1.0f, 0.9375f),
    /** The line-height ratio the largest heading reaches, from the body's. */
    val headingLineRatio: Float = 1.2f,
    /** A superscript's or subscript's size, against the run it would have been. */
    val baselineScale: Float = 0.72f,
    /** The CSS size keywords, as multiples of the size a run would otherwise take. */
    val sizeSteps: Map<String, Float> = mapOf(
        "xx-small" to 0.5625f, "x-small" to 0.625f, "small" to 0.8125f,
        "large" to 1.125f, "x-large" to 1.5f, "xx-large" to 2f, "xxx-large" to 3f,
    ),
    /** How wide the text column may grow, in average characters; null for the whole view. */
    val measure: Float? = 68f,
    val paddingHorizontal: Dp = 20.dp,
    val paddingVertical: Dp = 16.dp,
    /** One blockquote level's inset — the gutter a bar is drawn in. */
    val quoteIndent: Dp = 22.dp,
    val quoteBarWidth: Dp = 3.dp,
    val ruleThickness: Dp = 1.dp,
    val caretWidth: Dp = 2.dp,
    /** How far a code block's tint reaches past its text on either side. */
    val codeFillOutset: Dp = 6.dp,
    val colors: LeafColors = LeafColors.light(),
) {
    val blockGap: TextUnit get() = (lineHeight.value * blockGapScale).sp

    /** The font size of a heading row of `level` (1–6). */
    fun headingSize(level: Int): TextUnit = (fontSize.value * headingScale[level.coerceIn(1, 6) - 1]).sp

    private val lineRatio: Float get() = lineHeight.value / fontSize.value

    /**
     * A row's line box, in sp, before the block's own line spacing: the body
     * line for a paragraph, and for a heading its size times a ratio that eases
     * from the body's toward [headingLineRatio] as the heading grows — a big
     * heading at the body's ratio would stand in a moat.
     */
    fun rowLineHeight(heading: Int?): Float {
        if (heading == null) return lineHeight.value
        val scale = headingScale[heading.coerceIn(1, 6) - 1]
        val top = headingScale.max()
        val ratio = if (scale > 1f && top > 1f) {
            val t = ((scale - 1f) / (top - 1f)).coerceAtMost(1f)
            lineRatio + (headingLineRatio - lineRatio) * t
        } else lineRatio
        return fontSize.value * scale * ratio
    }

    /** The size a run is set at: its row's, scaled by a keyword or replaced by an exact `pt`. */
    fun runSize(base: Float, token: String?): Float {
        if (token == null) return base
        sizeSteps[token]?.let { return base * it }
        // `14pt` is exactly that many points of the sheet; a point is a sp here.
        return token.removeSuffix("pt").toFloatOrNull()?.takeIf { it > 0f } ?: base
    }

    /** A block's line spacing token (`"1.5"`) as a ratio, never tighter than a half. */
    fun lineSpacing(token: String?): Float =
        (token?.toFloatOrNull()?.takeIf { it > 0f } ?: 1f).coerceAtLeast(0.5f)

    /** The face a run names: a CSS generic, or a family the platform may or may not have. */
    fun family(name: String?): FontFamily = when (name) {
        null -> bodyFont
        "monospace" -> monoFont
        "serif" -> FontFamily.Serif
        "sans-serif" -> FontFamily.SansSerif
        "cursive" -> FontFamily.Cursive
        // A named family is resolved by the platform, which falls back to the
        // body face where it is not installed — the portability the author
        // traded away by naming one.
        else -> bodyFont
    }
}

/**
 * The theme's colours, for one appearance. Separate from the metrics because
 * an appearance change repaints and a metrics change relays out.
 */
@Immutable
data class LeafColors(
    val background: Color,
    val text: Color,
    val secondary: Color,
    val tertiary: Color,
    val link: Color,
    val separator: Color,
    val codeBackground: Color,
    val quoteBar: Color,
    val selection: Color,
    val caret: Color,
    val handle: Color,
    val composing: Color,
    val markBackground: Color,
    val hostHighlight: Color,
    /** The syntax palette, keyed by a code run's `token` class. */
    val syntax: Map<String, Color>,
    /** The seven named inks a `data-color` run is painted in. */
    val inks: Map<String, Color>,
    /** The seven named washes a coloured `==mark==` is painted over. */
    val washes: Map<String, Color>,
    val dark: Boolean,
) {
    fun syntaxColor(token: String?): Color = token?.let { syntax[it] } ?: text

    fun ink(name: String): Color = inks[name] ?: hex(name) ?: text

    fun wash(name: String?): Color = name?.let { washes[it] } ?: markBackground

    fun hostWash(hint: String?): Color = hint?.let { hex(it)?.copy(alpha = 0.32f) } ?: hostHighlight

    companion object {
        /** The seven highlight hues, shared by both appearances at a wash's strength. */
        private fun washes(): Map<String, Color> = mapOf(
            "red" to Color(0xFFFF3B30), "orange" to Color(0xFFFF9500),
            "yellow" to Color(0xFFFFCC00), "green" to Color(0xFF34C759),
            "blue" to Color(0xFF007AFF), "purple" to Color(0xFFAF52DE),
            "brown" to Color(0xFFA2845E),
        ).mapValues { it.value.copy(alpha = 0.28f) }

        fun light() = LeafColors(
            background = Color(0xFFFFFBFE),
            text = Color(0xFF1C1B1F),
            secondary = Color(0xFF6B6870),
            tertiary = Color(0xFFA9A5AE),
            link = Color(0xFF0B57D0),
            separator = Color(0xFFD5D2DA),
            codeBackground = Color(0x146B6870),
            quoteBar = Color(0xFFC5C1CB),
            selection = Color(0x4D2F6FED),
            caret = Color(0xFF0B57D0),
            handle = Color(0xFF0B57D0),
            composing = Color(0xFF6B6870),
            markBackground = Color(0x47FFCC00),
            hostHighlight = Color(0x5CFFCC00),
            syntax = mapOf(
                "punctuation" to Color(0xFF6A7580), "keyword" to Color(0xFF9526A0),
                "entity" to Color(0xFF2B62D9), "support" to Color(0xFF016A99),
                "constant" to Color(0xFF98590A), "string" to Color(0xFF0A7040),
                "comment" to Color(0xFF6A7580), "invalid" to Color(0xFFC02617),
            ),
            inks = mapOf(
                "red" to Color(0xFFD70015), "orange" to Color(0xFFC93400),
                "yellow" to Color(0xFF946200), "green" to Color(0xFF248A3D),
                "blue" to Color(0xFF0040DD), "purple" to Color(0xFF8944AB),
                "brown" to Color(0xFF7F6545),
            ),
            washes = washes(),
            dark = false,
        )

        fun dark() = LeafColors(
            background = Color(0xFF1C1B1F),
            text = Color(0xFFE6E1E5),
            secondary = Color(0xFFA09CA6),
            tertiary = Color(0xFF6E6A74),
            link = Color(0xFFA8C7FA),
            separator = Color(0xFF45424A),
            codeBackground = Color(0x1FA09CA6),
            quoteBar = Color(0xFF55515B),
            selection = Color(0x664C8DF6),
            caret = Color(0xFFA8C7FA),
            handle = Color(0xFFA8C7FA),
            composing = Color(0xFFA09CA6),
            markBackground = Color(0x47FFD60A),
            hostHighlight = Color(0x5CFFD60A),
            syntax = mapOf(
                "punctuation" to Color(0xFF7F8B99), "keyword" to Color(0xFFC678DD),
                "entity" to Color(0xFF7AA2F7), "support" to Color(0xFF56B6C2),
                "constant" to Color(0xFFE0AF68), "string" to Color(0xFF9ECE6A),
                "comment" to Color(0xFF7F8B99), "invalid" to Color(0xFFF7768E),
            ),
            inks = mapOf(
                "red" to Color(0xFFFF6961), "orange" to Color(0xFFFFB340),
                "yellow" to Color(0xFFFFD426), "green" to Color(0xFF30DB5B),
                "blue" to Color(0xFF409CFF), "purple" to Color(0xFFDA8FFF),
                "brown" to Color(0xFFB59469),
            ),
            washes = washes(),
            dark = true,
        )

        /** `#rrggbb` as a colour, or null for anything else — deliberately strict. */
        fun hex(s: String): Color? {
            val t = s.trim()
            if (t.length != 7 || t[0] != '#') return null
            val v = t.substring(1).toLongOrNull(16) ?: return null
            return Color(0xFF000000 or v)
        }
    }
}
