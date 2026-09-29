package org.diaryx.leaf.compose

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.InputMethodManager
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import uniffi.leaf_ffi.Capabilities
import uniffi.leaf_ffi.DocView
import uniffi.leaf_ffi.LeafDoc

/**
 * One document being edited: the [LeafDoc] core owns, the frame it last
 * answered with, and the layout of that frame. Every gesture, key and command
 * goes in as a core call and comes back as a frame, which is applied here and
 * repainted — core is the single source of truth for the text and the caret,
 * and nothing on this side edits either.
 *
 * Core answers with *changes* (`set_incremental_frames`): a keystroke lifts
 * the row it landed on, not the document, which is what keeps typing cheap on
 * a long document when every frame crosses JNA.
 *
 * Main thread only, as the binding asks.
 */
@Stable
class LeafEditorState(val doc: LeafDoc) {
    /** The frame on screen, whole — every change core sends is applied to it. */
    var frame: DocView
        private set

    /** Bumped on every applied frame, so a composable reading it repaints. */
    internal var version by mutableIntStateOf(0)
        private set

    /** What a toolbar lights itself from. Republished only when it changes. */
    var chrome by mutableStateOf(LeafEditorChrome())
        private set

    /** What the document's format can spell — a toolbar dims what it cannot. */
    val capabilities: Capabilities = doc.capabilities()

    /** Called after every change to the document's text (not the caret) — a host's autosave hook. */
    var onEdit: (() -> Unit)? = null

    /**
     * Host hook for opening a link: called with the link's raw destination
     * before the editor hands it to the system; return true to claim it. This
     * is how a host follows a destination only it can make sense of — a note
     * app's `./sibling.md` or `id:6tzwsxg` names a document in its own
     * workspace, not a URL. Null (or false) leaves it to the system. The same
     * hook as leaf-swift's `onOpenLink`.
     */
    var onOpenLink: ((String) -> Boolean)? = null

    internal var layout: EditorLayout? = null

    /** Set when the caret should be scrolled into view at the next layout. */
    internal var revealCaret by mutableStateOf(false)

    /**
     * Bumped when the caret is put somewhere to be read — a [goTo] — rather
     * than moved by the person typing. The editor answers it apart from
     * [revealCaret]: scrolled to even while unfocused, near the top, and not
     * cancelled by the frames that follow a document's opening.
     */
    internal var landings by mutableIntStateOf(0)

    /** Whether the host's software keyboard should show for this editor. */
    internal var focused by mutableStateOf(false)

    /** Bumped by [requestFocus]; the editor takes focus when it changes. */
    internal var focusRequests by mutableIntStateOf(0)

    // IME plumbing, filled in by the text-input session.
    internal var inputView: View? = null
    /** The source range an IME is composing into, as byte offsets, or null. */
    internal var composing: Pair<UInt, UInt>? = null
    internal var batchDepth = 0
    private var restartPending = false

    /**
     * The x a run of vertical moves keeps to, so walking down past a short
     * line comes back out at the column it started from. Cleared by any other
     * gesture.
     */
    private var goalX: Float? = null

    init {
        // One row per block: soft wrapping is this frontend's, done in pixels.
        doc.setUnwrapped()
        doc.setIncrementalFrames(true)
        frame = doc.view()
        chrome = LeafEditorChrome.of(frame)
    }

    // MARK: frames

    /**
     * Apply a frame core answered with and repaint. A frame that changed the
     * text tells the host; with [restart] it also tells the IME to read the
     * text again, which is for a change the IME had no part in — a toolbar
     * command, an undo, a paste — and never for typing, which it is tracking.
     */
    internal fun render(next: DocView?, restart: Boolean = false) {
        next ?: return
        val before = frame
        val (whole, change) = applyFrame(before, next) ?: applyFrame(null, doc.view())!!
        // An edit is rows that read differently — or source that moved under
        // rows that read the same (a link repointed, a comment pasted between
        // two paragraphs), or a dirty flag that flipped. A switch of view
        // changes every row and edits nothing.
        val edited = whole.view == before.view && (
            next.srcShift != 0 || whole.dirty != before.dirty ||
                change != null && !sameText(before, whole, change)
            )
        frame = whole
        layout?.let { if (it.ready) it.update(frame, change, it.theme, it.width) }
        chrome = LeafEditorChrome.of(frame).let { if (it == chrome) chrome else it }
        version++
        // Only a frame that moved the caret or the text brings it back into
        // view — not a save or an appearance change while the reader has
        // scrolled away from it.
        if (edited || whole.caretRow != before.caretRow || whole.caretCh != before.caretCh ||
            whole.anchorRow != before.anchorRow || whole.anchorCh != before.anchorCh
        ) revealCaret = true
        if (edited) {
            // An IME that did not make this change holds text that is no
            // longer there; it re-reads after a restart.
            if (restart) {
                composing = null
                restartPending = true
            }
            onEdit?.invoke()
        }
        notifyIme()
    }

    /**
     * Whether the rows [change] replaced say the same as the rows that replaced
     * them — the same text in the same marks, whatever is selected and wherever
     * the source moved. A drag splits and flags runs without editing anything,
     * and must not read as an edit.
     */
    private fun sameText(old: DocView, new: DocView, change: RowChange): Boolean {
        if (change.oldEnd - change.oldStart != change.newEnd - change.newStart) return false
        for (k in 0 until change.newEnd - change.newStart) {
            val a = old.rows.getOrNull(change.oldStart + k) ?: return false
            val b = new.rows.getOrNull(change.newStart + k) ?: return false
            if (a.copy(runs = emptyList()) != b.copy(runs = emptyList())) return false
            if (merged(a.runs) != merged(b.runs)) return false
        }
        return true
    }

    /** Runs with the selection and the offsets forgotten, merged where the selection parted them. */
    private fun merged(runs: List<uniffi.leaf_ffi.Run>): List<uniffi.leaf_ffi.Run> {
        val out = ArrayList<uniffi.leaf_ffi.Run>(runs.size)
        for (r in runs) {
            val run = r.copy(sel = false, src = 0u)
            val last = out.lastOrNull()
            if (last != null && last.copy(text = "") == run.copy(text = "")) {
                out[out.size - 1] = last.copy(text = last.text + run.text)
            } else out.add(run)
        }
        return out
    }

    /**
     * Bring [layout] to [theme] at [width] — a no-op unless either changed,
     * since every frame reaches the layout as it is applied (see [render]).
     */
    internal fun layoutFor(theme: LeafTheme, width: Int): EditorLayout? {
        val l = layout ?: return null
        l.update(frame, null, theme, width)
        return l
    }

    /** Run a core command from a toolbar or a key and repaint. */
    fun command(op: (LeafDoc) -> DocView?) {
        finishComposing()
        render(op(doc), restart = true)
    }

    /**
     * A character from a hardware key no IME took — typed text, so the IME
     * follows it by selection rather than being restarted per letter.
     */
    internal fun typeKey(text: String) = edit { it.insert(text) }

    /**
     * Set while a touch that began on a selection handle is in progress, so the
     * tap and long-press detectors — which see the same touch — leave it alone.
     */
    internal var handleGesture = false

    /** Set once a touch has become a long press, so its release is not also a tap. */
    internal var longPressed = false

    /** A keystroke's edit: like [command], but the IME follows it by selection alone. */
    private fun edit(op: (LeafDoc) -> DocView?) {
        finishComposing()
        render(op(doc))
    }

    /** The document's source, for saving. */
    fun source(): String = doc.source()

    /** Mark the current text as saved, so [LeafEditorChrome.dirty] clears. */
    fun markSaved() = render(doc.markSaved())

    /**
     * Take focus and raise the soft keyboard, as a tap would — for a host that
     * opens a document to be written in rather than read, a new one say.
     */
    fun requestFocus() {
        focusRequests++
    }

    /**
     * Open the link the caret stands in. A `#fragment` naming a place in this
     * document moves the caret there, since there is nothing in it for a host
     * or a browser; anything else goes to [onOpenLink] first, then to whatever
     * the system opens it with. False when the caret is on no link, or nothing
     * would take it.
     */
    fun openLinkAtCaret(): Boolean {
        val destination = doc.linkDestinationAtCaret() ?: return false
        selfLanding(destination)?.let {
            reveal(it)
            return true
        }
        if (onOpenLink?.invoke(destination) == true) return true
        val context = inputView?.context ?: return false
        return try {
            context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(destination)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            true
        } catch (_: ActivityNotFoundException) {
            false
        }
    }

    /**
     * Land on the place a locator names — a heading's slug, a block's id, the
     * `v2` of a link's `#v2` — and scroll it into view. False when the
     * document has no such place. A host calls it in the same breath as
     * opening the document; the landing waits for the editor's first layout.
     */
    fun goTo(locator: String): Boolean {
        val landing = doc.locate(locator) ?: return false
        reveal(landing.start)
        return true
    }

    /** Put the caret at `offset`, nothing selected, and land the reader on it. */
    fun reveal(offset: UInt) {
        render(doc.setSelectionOffsets(offset, offset))
        landings++
    }

    /**
     * Where a `#fragment` lands in this document, or null. Both spellings of
     * the fragment are tried — as written, then percent-decoded — as
     * leaf-swift's `landing(for:)` does.
     */
    private fun selfLanding(destination: String): UInt? {
        if (!destination.startsWith("#")) return null
        val id = destination.drop(1)
        val landing = doc.locate(id) ?: Uri.decode(id).takeIf { it != id }?.let { doc.locate(it) }
        return landing?.start
    }

    /** Switch between the rendered view and the source. */
    fun toggleView() = command { it.toggleView() }

    fun undo() = command { it.undo() }
    fun redo() = command { it.redo() }

    // MARK: gestures

    internal fun tap(point: Offset) {
        finishComposing()
        goalX = null
        val l = layout ?: return
        val hit = l.hit(point) ?: return render(doc.clickPastEnd())
        val (row, ch) = hit
        // A tap on a task's box ticks it — where the format can spell one.
        val marker = l.row(row).taskMarker
        if (marker != null && capabilities.task && ch in marker.second..marker.second + 1 && !chrome.hasSelection) {
            return render(doc.toggleTaskAt(marker.first.src.toULong()))
        }
        render(doc.clickCh(row.toUInt(), ch.toUInt(), false))
    }

    internal fun selectWord(point: Offset) {
        finishComposing()
        val (row, ch) = layout?.hit(point) ?: return
        render(doc.selectWordCh(row.toUInt(), ch.toUInt()))
    }

    internal fun extendTo(point: Offset) {
        val l = layout ?: return
        val hit = l.hit(point)
        if (hit == null) {
            val end = doc.docEndOffset()
            return render(doc.setSelectionOffsets(doc.anchorOffset(), end))
        }
        render(doc.clickCh(hit.first.toUInt(), hit.second.toUInt(), true))
    }

    /** The source offset a content point lands on, snapped to a caret stop. */
    internal fun offsetAt(point: Offset): UInt {
        val hit = layout?.hit(point) ?: return doc.docEndOffset()
        return doc.offsetForPos(hit.first.toUInt(), hit.second.toUInt())
    }

    /** Drag one end of the selection to [point], keeping [fixed] where it is. */
    internal fun dragSelection(fixed: UInt, point: Offset) {
        val moving = offsetAt(point)
        if (moving == fixed) return
        render(doc.setSelectionOffsets(fixed, moving))
    }

    /** The two ends of the selection as `(row, ch)`, in document order — for handles. */
    internal fun selectionEnds(): Pair<Pair<Int, Int>, Pair<Int, Int>> {
        val caret = frame.caretRow.toInt() to frame.caretCh.toInt()
        val anchor = frame.anchorRow.toInt() to frame.anchorCh.toInt()
        return if (compareValuesBy(anchor, caret, { it.first }, { it.second }) <= 0) anchor to caret else caret to anchor
    }

    /** The caret's rect in content coordinates, or null before the first layout. */
    internal fun caretRect(): Rect? = layout?.caretRect(frame.caretRow.toInt(), frame.caretCh.toInt())

    // MARK: keys

    /**
     * A hardware key — or one an IME sent as a key rather than as text. The
     * keys that are gestures (arrows, Return, Tab, Delete) and the chords
     * every editor answers; everything else is left to the text path.
     */
    internal fun key(e: KeyEvent): Boolean {
        if (e.action != KeyEvent.ACTION_DOWN) return isEditorKey(e)
        val ctrl = e.isCtrlPressed || e.isMetaPressed
        val shift = e.isShiftPressed
        val alt = e.isAltPressed
        val keepGoal = e.keyCode == KeyEvent.KEYCODE_DPAD_UP || e.keyCode == KeyEvent.KEYCODE_DPAD_DOWN
        if (!keepGoal) goalX = null
        val handled = when (e.keyCode) {
            KeyEvent.KEYCODE_DEL -> { edit { if (ctrl || alt) it.deleteWordBack() else it.backspace() }; true }
            KeyEvent.KEYCODE_FORWARD_DEL -> { edit { if (ctrl || alt) it.deleteWordForward() else it.deleteForward() }; true }
            KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> {
                edit { if (shift) it.cellLineBreak() ?: it.newline() else it.cellReturn() ?: it.newline() }; true
            }
            KeyEvent.KEYCODE_TAB -> { edit { if (shift) it.cellTab(false) ?: it.outdent() else it.cellTab(true) ?: it.indent() }; true }
            KeyEvent.KEYCODE_DPAD_LEFT -> { edit { if (ctrl || alt) it.moveWordLeft(shift) else it.moveLeft(shift) }; true }
            KeyEvent.KEYCODE_DPAD_RIGHT -> { edit { if (ctrl || alt) it.moveWordRight(shift) else it.moveRight(shift) }; true }
            KeyEvent.KEYCODE_DPAD_UP -> { if (alt) edit { it.moveBlockUp() } else vertical(false, shift); true }
            KeyEvent.KEYCODE_DPAD_DOWN -> { if (alt) edit { it.moveBlockDown() } else vertical(true, shift); true }
            KeyEvent.KEYCODE_MOVE_HOME -> { edit { if (ctrl) it.moveDocStart(shift) else it.moveHome(shift) }; true }
            KeyEvent.KEYCODE_MOVE_END -> { edit { if (ctrl) it.moveDocEnd(shift) else it.moveEnd(shift) }; true }
            else -> if (ctrl) chord(e.keyCode, shift) else false
        }
        return handled
    }

    private fun isEditorKey(e: KeyEvent): Boolean = when (e.keyCode) {
        KeyEvent.KEYCODE_DEL, KeyEvent.KEYCODE_FORWARD_DEL, KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER,
        KeyEvent.KEYCODE_TAB, KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.KEYCODE_DPAD_RIGHT, KeyEvent.KEYCODE_DPAD_UP,
        KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_MOVE_HOME, KeyEvent.KEYCODE_MOVE_END -> true
        else -> false
    }

    private fun chord(code: Int, shift: Boolean): Boolean {
        when (code) {
            KeyEvent.KEYCODE_B -> command { it.toggleBold() }
            KeyEvent.KEYCODE_I -> command { it.toggleItalic() }
            KeyEvent.KEYCODE_U -> command { it.toggleUnderline() }
            KeyEvent.KEYCODE_Z -> if (shift) redo() else undo()
            KeyEvent.KEYCODE_Y -> redo()
            KeyEvent.KEYCODE_A -> command { it.selectAll() }
            KeyEvent.KEYCODE_C -> copy()
            KeyEvent.KEYCODE_X -> cut()
            KeyEvent.KEYCODE_V -> paste(plain = shift)
            else -> return false
        }
        return true
    }

    /** Up/Down over our own wrapped lines, keeping the column a run of them started at. */
    private fun vertical(down: Boolean, extend: Boolean) {
        finishComposing()
        val l = layout ?: return
        val row = frame.caretRow.toInt()
        val ch = frame.caretCh.toInt()
        val x = goalX ?: l.caretRect(row, ch)?.left ?: return
        val target = l.verticalMove(row, ch, x, down)
        if (target == null) {
            edit { if (down) it.moveDocEnd(extend) else it.moveDocStart(extend) }
        } else {
            render(doc.clickCh(target.first.toUInt(), target.second.toUInt(), extend))
        }
        goalX = x
    }

    // MARK: clipboard

    private fun clipboard(): ClipboardManager? =
        inputView?.context?.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager

    /** Copy the selection, with its HTML flavour where core can write one. */
    fun copy() {
        val text = doc.selectedText() ?: return
        val html = doc.selectionHtml()
        val clip = if (html != null) ClipData.newHtmlText("leaf", text, html) else ClipData.newPlainText("leaf", text)
        clipboard()?.setPrimaryClip(clip)
    }

    fun cut() {
        if (doc.selectedText() == null) return
        copy()
        command { it.backspace() }
    }

    /**
     * Paste the rich flavour where the clipboard has one — formatting a plain
     * copy out of another app has already lost — and the plain one otherwise.
     * [plain] pastes the text as leaf source whatever else is there.
     */
    fun paste(plain: Boolean = false) {
        val clip = clipboard()?.primaryClip ?: return
        if (clip.itemCount == 0) return
        val item = clip.getItemAt(0)
        val text = item.text?.toString() ?: inputView?.context?.let { item.coerceToText(it).toString() } ?: return
        val html = if (plain) null else item.htmlText
        if (html == null && text.isEmpty()) return
        command { if (html != null) it.pasteRich(html, text) else it.paste(text) }
    }

    fun selectAll() = command { it.selectAll() }

    // MARK: IME

    internal fun finishComposing() {
        composing = null
    }

    /**
     * Tell the IME where the selection and the composing region are now, in
     * the UTF-16 indices of the visible text it reads — or, after a change it
     * did not make, that it must read again. Deferred to the end of a batch
     * edit, where the IME expects to hear.
     */
    internal fun notifyIme() {
        if (batchDepth > 0) return
        val view = inputView ?: return
        val imm = view.context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager ?: return
        if (restartPending) {
            restartPending = false
            imm.restartInput(view)
            return
        }
        val anchor = doc.anchorOffset()
        val caret = doc.caretOffset()
        val comp = composing
        val offs = mutableListOf(minOf(anchor, caret), maxOf(anchor, caret))
        if (comp != null) offs += listOf(comp.first, comp.second)
        val idx = doc.utf16IndicesForOffsets(offs).map { it.toInt() }
        imm.updateSelection(view, idx[0], idx[1], if (comp != null) idx[2] else -1, if (comp != null) idx[3] else -1)
    }

    internal fun showKeyboard() {
        val view = inputView ?: return
        val imm = view.context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager ?: return
        imm.showSoftInput(view, 0)
    }
}
