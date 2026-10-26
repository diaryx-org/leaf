package org.diaryx.leaf.compose

import android.os.Bundle
import android.os.Handler
import android.text.InputType
import android.text.TextUtils
import android.view.KeyEvent
import android.view.inputmethod.CompletionInfo
import android.view.inputmethod.CorrectionInfo
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.ExtractedText
import android.view.inputmethod.ExtractedTextRequest
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputContentInfo
import java.text.BreakIterator

/**
 * The soft keyboard's view of a leaf document — the Android peer of the
 * `UITextInput` conformance in leaf-swift's LeafTextViewiOS.swift.
 *
 * An IME speaks in UTF-16 indices into one flat string. leaf's handle is the
 * source byte offset, and the two are not a scale apart: the rendered view
 * hides delimiters, spells a block gap as one `\n`, and a character outside
 * the BMP is two units. Core converts between them over exactly the text it
 * reads back (`text_in_range`, `utf16_index_for_offset`), so the IME, the
 * spell checker and the rendered rows all index the same characters.
 *
 * Composing text — which Gboard makes of almost every word — is written into
 * the document as it is composed, as on iOS, so the line reflows under the
 * fingers. It goes in through core's `insert`, not a raw splice, so it is typed
 * text in every way that matters: an armed Bold takes it, and in hidden-markup
 * mode a `*` stays literal. What `insert` wrote can therefore be more than the
 * composed characters (an escape, a mark's delimiters), so the composing region
 * is re-found afterwards as the last [graphemes] caret stops before the caret,
 * which is the composed text however it was spelled.
 */
internal class LeafInputConnection(private val state: LeafEditorState) : InputConnection {
    private val doc get() = state.doc

    fun configure(info: EditorInfo) {
        info.inputType = InputType.TYPE_CLASS_TEXT or
            InputType.TYPE_TEXT_FLAG_MULTI_LINE or
            InputType.TYPE_TEXT_FLAG_CAP_SENTENCES or
            InputType.TYPE_TEXT_FLAG_AUTO_CORRECT
        // Return is a newline, never an action.
        info.imeOptions = EditorInfo.IME_FLAG_NO_ENTER_ACTION or EditorInfo.IME_ACTION_NONE or
            EditorInfo.IME_FLAG_NO_FULLSCREEN
        val (a, b) = selectionUtf16()
        info.initialSelStart = a
        info.initialSelEnd = b
    }

    private fun selectionOffsets(): Pair<UInt, UInt> {
        val anchor = doc.anchorOffset()
        val caret = doc.caretOffset()
        return minOf(anchor, caret) to maxOf(anchor, caret)
    }

    private fun selectionUtf16(): Pair<Int, Int> {
        val (s, e) = selectionOffsets()
        val idx = doc.utf16IndicesForOffsets(listOf(s, e))
        return idx[0].toInt() to idx[1].toInt()
    }

    private fun graphemes(text: String): Int {
        val it = BreakIterator.getCharacterInstance()
        it.setText(text)
        var n = 0
        while (it.next() != BreakIterator.DONE) n++
        return n
    }

    /** Replace the composing region (or the selection) with [text] as typed text. */
    private fun write(text: String, compose: Boolean) = batch {
        val (s, e) = state.composing ?: selectionOffsets()
        if (s != e) state.render(doc.selectRange(s, e))
        val view = when {
            text.isEmpty() -> if (s != e) doc.replaceRange(s, e, "") else null
            text == "\n" && !compose -> doc.cellReturn() ?: doc.newline()
            else -> doc.insert(text)
        }
        state.render(view)
        state.composing = if (compose && text.isNotEmpty()) {
            val caret = doc.caretOffset()
            doc.stepOffset(caret, -graphemes(text)) to caret
        } else null
    }

    /** Run [body] as one edit as far as the IME is told — it hears the end state once. */
    private inline fun batch(body: () -> Unit) {
        state.batchDepth++
        try {
            body()
        } finally {
            state.batchDepth--
            state.notifyIme()
        }
    }

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        write(text?.toString() ?: "", compose = true)
        return true
    }

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        write(text?.toString() ?: "", compose = false)
        return true
    }

    override fun finishComposingText(): Boolean {
        state.composing = null
        state.notifyIme()
        return true
    }

    override fun setComposingRegion(start: Int, end: Int): Boolean {
        if (start == end) {
            state.composing = null
        } else {
            val a = doc.offsetForUtf16Index(minOf(start, end).toUInt())
            val b = doc.offsetForUtf16Index(maxOf(start, end).toUInt())
            state.composing = a to b
        }
        state.notifyIme()
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean = batch {
        state.composing = null
        val (s, e) = selectionOffsets()
        // The lengths are UTF-16 units of the text the IME read, not caret
        // stops: `b👍` is three units and two stops. Measured in the same units
        // through core, which spelled that text.
        val (s16, e16) = selectionUtf16()
        val from = doc.offsetForUtf16Index((s16 - beforeLength).coerceAtLeast(0).toUInt())
        val to = if (afterLength > 0) doc.offsetForUtf16Index((e16 + afterLength).toUInt()) else e
        // Exactly one character behind a bare caret is Backspace, which is a
        // structural key at a block's start (un-list, un-heading) — the same
        // key a hardware Delete would be.
        if (afterLength == 0 && s == e && beforeLength > 0 && from == doc.stepOffset(s, -1)) {
            state.render(doc.backspace())
            return@batch
        }
        if (to > e) state.render(doc.replaceRange(e, to, ""))
        if (from < s) state.render(doc.replaceRange(from, s, ""))
    }.let { true }

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean =
        deleteSurroundingText(beforeLength, afterLength)

    override fun setSelection(start: Int, end: Int): Boolean {
        val a = doc.offsetForUtf16Index(start.coerceAtLeast(0).toUInt())
        val b = doc.offsetForUtf16Index(end.coerceAtLeast(0).toUInt())
        state.render(doc.setSelectionOffsets(a, b))
        return true
    }

    override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence {
        val (s, _) = selectionOffsets()
        val from = doc.stepOffset(s, -n)
        val text = doc.textInRange(from, s)
        return if (text.length > n) text.substring(text.length - n) else text
    }

    override fun getTextAfterCursor(n: Int, flags: Int): CharSequence {
        val (_, e) = selectionOffsets()
        val to = doc.stepOffset(e, n)
        val text = doc.textInRange(e, to)
        return if (text.length > n) text.substring(0, n) else text
    }

    override fun getSelectedText(flags: Int): CharSequence? {
        val (s, e) = selectionOffsets()
        return if (s == e) null else doc.textInRange(s, e)
    }

    override fun getCursorCapsMode(reqModes: Int): Int {
        val before = getTextBeforeCursor(64, 0).toString()
        return TextUtils.getCapsMode(before, before.length, reqModes)
    }

    // Extracted text is for a fullscreen IME, which this editor opts out of.
    override fun getExtractedText(request: ExtractedTextRequest?, flags: Int): ExtractedText? = null

    override fun sendKeyEvent(event: KeyEvent): Boolean {
        if (event.action == KeyEvent.ACTION_DOWN) {
            if (!state.key(event)) {
                typedChar(event)?.let { write(it, compose = false) }
            }
        }
        return true
    }

    override fun beginBatchEdit(): Boolean {
        if (state.batchDepth++ == 0) doc.beginUndoGroup()
        return true
    }

    override fun endBatchEdit(): Boolean {
        if (state.batchDepth > 0 && --state.batchDepth == 0) {
            doc.endUndoGroup()
            state.notifyIme()
        }
        return state.batchDepth > 0
    }

    override fun performContextMenuAction(id: Int): Boolean {
        when (id) {
            android.R.id.selectAll -> state.selectAll()
            android.R.id.copy -> state.copy()
            android.R.id.cut -> state.cut()
            android.R.id.paste -> state.paste()
            android.R.id.pasteAsPlainText -> state.paste(plain = true)
            else -> return false
        }
        return true
    }

    override fun performEditorAction(editorAction: Int): Boolean {
        write("\n", compose = false)
        return true
    }

    override fun closeConnection() {
        while (state.batchDepth > 0) endBatchEdit()
        state.composing = null
    }

    override fun commitCompletion(text: CompletionInfo?): Boolean = false
    override fun commitCorrection(correctionInfo: CorrectionInfo?): Boolean = true
    override fun clearMetaKeyStates(states: Int): Boolean = true
    override fun reportFullscreenMode(enabled: Boolean): Boolean = false
    override fun performPrivateCommand(action: String?, data: Bundle?): Boolean = false
    override fun requestCursorUpdates(cursorUpdateMode: Int): Boolean = false
    override fun getHandler(): Handler? = null
    override fun commitContent(inputContentInfo: InputContentInfo, flags: Int, opts: Bundle?): Boolean = false
}

/**
 * The text a key types, or null for a key that types nothing — including a
 * dead key, whose `unicodeChar` carries `COMBINING_ACCENT` in its sign bit and
 * is not a character at all.
 */
internal fun typedChar(event: KeyEvent): String? {
    val c = event.unicodeChar
    if (c <= 0 || c and android.view.KeyCharacterMap.COMBINING_ACCENT != 0) return null
    if (event.isCtrlPressed || event.isMetaPressed) return null
    return String(Character.toChars(c))
}
