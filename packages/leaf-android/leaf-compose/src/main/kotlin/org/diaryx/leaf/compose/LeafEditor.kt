package org.diaryx.leaf.compose

import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.FocusState
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.node.ModifierNodeElement
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalTextToolbar
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.PlatformTextInputMethodRequest
import androidx.compose.ui.platform.PlatformTextInputModifierNode
import androidx.compose.ui.platform.TextToolbar
import androidx.compose.ui.platform.establishTextInputSession
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.focus.FocusEventModifierNode
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import android.view.KeyEvent as AndroidKeyEvent

/**
 * A leaf document, editable: the rendered rows, a caret, a selection with
 * handles, the soft keyboard and a hardware one. The Compose peer of
 * leaf-swift's `LeafTextView` and leaf-web's `LeafEditor`.
 *
 * Only the editing surface: a toolbar ([LeafFormattingBar]), saving, and the
 * window's chrome are the host's. Give it the space the keyboard leaves (an
 * `imePadding()` on an ancestor) and it keeps the caret in view.
 */
@Composable
fun LeafEditor(
    state: LeafEditorState,
    modifier: Modifier = Modifier,
    theme: LeafTheme = LeafTheme(colors = if (isSystemInDarkTheme()) LeafColors.dark() else LeafColors.light()),
    scrollState: ScrollState = rememberScrollState(),
) {
    val density = LocalDensity.current
    val measurer = rememberTextMeasurer(cacheSize = 0)
    val layout = remember(measurer, density) { EditorLayout(measurer, density) }
    state.layout = layout
    state.inputView = LocalView.current
    val toolbar = LocalTextToolbar.current
    val focus = remember { FocusRequester() }
    var coordinates by remember { mutableStateOf<LayoutCoordinates?>(null) }
    var caretOn by remember { mutableStateOf(true) }
    // The frame the selection menu was raised over. Any later frame — a
    // command, a keystroke, a switch of view — puts the menu away, as the
    // platform's own text fields do.
    var menuFrame by remember { mutableStateOf(-1) }

    // A named colour is two inks and core resolves `<picture>` sources by
    // appearance, so it has to know which one it is drawn in.
    LaunchedEffect(state, theme.colors.dark) { state.render(state.doc.setDarkAppearance(theme.colors.dark)) }

    // The text menu is the window's, not this composable's: raised here, it
    // would float on over whatever the host shows next.
    DisposableEffect(toolbar) { onDispose { toolbar.hide() } }

    BoxWithConstraints(modifier.background(theme.colors.background)) {
        val width = constraints.maxWidth
        val viewport = constraints.maxHeight.toFloat()
        val version = state.version
        state.layoutFor(theme, width)
        val contentHeight = with(density) { layout.height.toDp() }

        // The caret blinks, and holds still for a moment after anything moves it.
        LaunchedEffect(version, state.focused) {
            caretOn = true
            while (state.focused) {
                delay(530)
                caretOn = !caretOn
            }
        }

        // Keep the caret in view after every frame that moved it, with a line
        // of room above and below.
        LaunchedEffect(version, viewport) {
            if (!state.revealCaret) return@LaunchedEffect
            state.revealCaret = false
            if (!state.focused) return@LaunchedEffect
            val caret = state.caretRect() ?: return@LaunchedEffect
            val margin = with(density) { theme.lineHeight.toPx() }
            val top = scrollState.value.toFloat()
            val target = when {
                caret.top - margin < top -> caret.top - margin
                caret.bottom + margin > top + viewport -> caret.bottom + margin - viewport
                else -> return@LaunchedEffect
            }
            scrollState.animateScrollTo(target.toInt().coerceIn(0, scrollState.maxValue.coerceAtLeast(0)))
        }

        LaunchedEffect(version) { if (version != menuFrame) toolbar.hide() }

        fun showMenuNow() {
            menuFrame = state.version
            val coords = coordinates ?: return
            val (s, e) = state.selectionEnds()
            val a = layout.caretRect(s.first, s.second) ?: return
            val b = layout.caretRect(e.first, e.second) ?: return
            val local = Rect(minOf(a.left, b.left), a.top, maxOf(a.right, b.right), b.bottom)
            val topLeft = coords.localToRoot(local.topLeft)
            showSelectionMenu(toolbar, state, Rect(topLeft, local.size))
        }

        // The gesture detectors outlive a composition; they reach the menu
        // through this, so they raise it from the current layout's rects.
        val showMenuLatest = rememberUpdatedState(::showMenuNow)
        val showMenu: () -> Unit = { showMenuLatest.value() }

        Box(Modifier.fillMaxSize().verticalScroll(scrollState)) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(maxOf(contentHeight, with(density) { viewport.toDp() }))
                    .onGloballyPositioned { coordinates = it }
                    .then(LeafTextInputElement(state))
                    .onKeyEvent { e ->
                        val native = e.nativeKeyEvent
                        if (state.key(native)) return@onKeyEvent true
                        // A printable key the IME let through — a hardware
                        // keyboard with no IME in the way.
                        val typed = if (native.action == AndroidKeyEvent.ACTION_DOWN) typedChar(native) else null
                        if (typed != null) {
                            state.typeKey(typed)
                            true
                        } else false
                    }
                    .focusRequester(focus)
                    .focusable()
                    .pointerInputTaps(state, focus, toolbar, showMenu)
                    .pointerInputLongPress(state, focus, showMenu, toolbar)
                    .pointerInputHandles(state, layout, theme, showMenu)
                    .drawBehind {
                        // Read here, in the draw phase, so every frame
                        // repaints: the lambda captures nothing that changes
                        // with a frame, and would otherwise be kept as it was.
                        state.version
                        drawEditor(state, layout, theme, scrollState.value.toFloat(), viewport, caretOn && state.focused)
                    },
            )
        }
    }
}

private fun showSelectionMenu(toolbar: TextToolbar, state: LeafEditorState, rect: Rect) {
    val selected = state.chrome.hasSelection
    toolbar.showMenu(
        rect = rect,
        onCopyRequested = if (selected) ({ state.copy(); toolbar.hide() }) else null,
        onPasteRequested = { state.paste(); toolbar.hide() },
        onCutRequested = if (selected) ({ state.cut(); toolbar.hide() }) else null,
        onSelectAllRequested = { state.selectAll(); toolbar.hide() },
    )
}

// MARK: gestures

/**
 * A tap places the caret; a second tap near the first selects the word; a
 * tap on the caret itself offers Paste. Immediate rather than waiting out a
 * double-tap timeout, since the first tap of a double one lands the caret in
 * the word it is about to select anyway.
 */
private fun Modifier.pointerInputTaps(
    state: LeafEditorState,
    focus: FocusRequester,
    toolbar: TextToolbar,
    showMenu: () -> Unit,
): Modifier = this.then(
    Modifier.pointerInput(state) {
        var lastTap = 0L
        var lastPoint = Offset.Zero
        detectTapGestures(
            // No `onLongPress`: given one, this detector consumes the rest of a
            // long press, racing the long-press detector below to the same
            // timeout and cancelling it when it wins. A touch the long-press
            // or handle detector claimed is skipped here instead.
            onTap = { p ->
                if (state.handleGesture || state.longPressed) return@detectTapGestures
                focus.requestFocus()
                val now = System.currentTimeMillis()
                val near = (p - lastPoint).getDistance() < 24.dp.toPx()
                val wasCaret = !state.chrome.hasSelection && state.caretRect()?.let {
                    Rect(it.left - 20.dp.toPx(), it.top, it.right + 20.dp.toPx(), it.bottom).contains(p)
                } == true
                if (near && now - lastTap < 350) {
                    state.selectWord(p)
                    showMenu()
                } else {
                    toolbar.hide()
                    state.tap(p)
                    if (wasCaret && state.focused) showMenu()
                }
                lastTap = now
                lastPoint = p
                state.showKeyboard()
            },
        )
    },
)

/** A long press selects the word under it, and dragging on extends the selection. */
private fun Modifier.pointerInputLongPress(
    state: LeafEditorState,
    focus: FocusRequester,
    showMenu: () -> Unit,
    toolbar: TextToolbar,
): Modifier =
    this.then(
        Modifier.pointerInput(state) {
            detectDragGesturesAfterLongPress(
                onDragStart = { p ->
                    if (!state.handleGesture) {
                        state.longPressed = true
                        focus.requestFocus()
                        toolbar.hide()
                        state.selectWord(p)
                    }
                },
                onDrag = { change, _ -> if (!state.handleGesture) state.extendTo(change.position) },
                onDragEnd = { if (!state.handleGesture) showMenu() },
                onDragCancel = { if (!state.handleGesture) showMenu() },
            )
        },
    )

/**
 * The selection's two handles, draggable. Last in the chain so it sees a
 * touch first, and it consumes only a touch that lands on a handle — anything
 * else goes on to the tap and long-press detectors and the scroll.
 */
private fun Modifier.pointerInputHandles(
    state: LeafEditorState,
    layout: EditorLayout,
    theme: LeafTheme,
    showMenu: () -> Unit,
): Modifier = this.then(
    Modifier.pointerInput(state, layout) {
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false)
            // A new touch: nothing has claimed it yet. First in the chain to
            // see it, so the flags are clear before the other detectors look.
            state.handleGesture = false
            state.longPressed = false
            if (!state.chrome.hasSelection) return@awaitEachGesture
            val handles = handleGeometry(state, layout, theme, this@pointerInput) ?: return@awaitEachGesture
            val reach = 28.dp.toPx()
            val startHit = (down.position - handles.startCenter).getDistance() < reach
            val endHit = (down.position - handles.endCenter).getDistance() < reach
            if (!startHit && !endHit) return@awaitEachGesture
            val dragStart = if (startHit && endHit) {
                (down.position - handles.startCenter).getDistance() < (down.position - handles.endCenter).getDistance()
            } else startHit
            down.consume()
            state.handleGesture = true
            // The end that stays put, as a source offset, and where on the
            // text the finger is holding the moving end: the line's middle, not
            // the handle under the finger.
            val anchor = state.doc.anchorOffset()
            val caret = state.doc.caretOffset()
            val fixed = if (dragStart) maxOf(anchor, caret) else minOf(anchor, caret)
            val rect = if (dragStart) handles.start else handles.end
            val grab = Offset(rect.left, rect.center.y) - down.position
            drag(down.id) { change ->
                change.consume()
                state.dragSelection(fixed, change.position + grab)
            }
            showMenu()
        }
    },
)

private class Handles(val start: Rect, val end: Rect, val startCenter: Offset, val endCenter: Offset, val radius: Float)

private fun handleGeometry(state: LeafEditorState, layout: EditorLayout, theme: LeafTheme, d: androidx.compose.ui.unit.Density): Handles? {
    val (s, e) = state.selectionEnds()
    val a = layout.caretRect(s.first, s.second) ?: return null
    val b = layout.caretRect(e.first, e.second) ?: return null
    val r = with(d) { 11.dp.toPx() }
    return Handles(a, b, Offset(a.left - r, a.bottom + r), Offset(b.left + r, b.bottom + r), r)
}

// MARK: drawing

private fun DrawScope.drawEditor(
    state: LeafEditorState,
    layout: EditorLayout,
    theme: LeafTheme,
    scrollTop: Float,
    viewport: Float,
    caretOn: Boolean,
) {
    if (layout.rowCount == 0) return
    val c = theme.colors
    val first = layout.rowAt(scrollTop)
    val last = layout.rowAt(scrollTop + viewport)
    val x0 = layout.originX
    val col = layout.columnWidth

    // Code blocks: one tinted panel per maximal run of code rows, from past
    // the rows' prefix (a list's bullet stands beside the block, not in it).
    val outset = theme.codeFillOutset.toPx()
    var i = first
    while (i <= last) {
        if (!layout.row(i).code) { i++; continue }
        var a = i
        while (a > 0 && layout.row(a - 1).code) a--
        var b = i
        while (b + 1 < layout.rowCount && layout.row(b + 1).code) b++
        val left = x0 + layout.shape(a).prefixWidth - outset
        val top = layout.top(a) - outset / 2
        val bottom = layout.top(b) + layout.shape(b).height + outset / 2
        drawRoundRect(c.codeBackground, Offset(left, top), Size(x0 + col + outset - left, bottom - top), CornerRadius(6.dp.toPx()))
        i = b + 1
    }

    // Quote bars, one per level, spanning each row so consecutive rows tile.
    val bar = theme.quoteBarWidth.toPx()
    for (r in first..last) {
        val s = layout.shape(r)
        if (s.tablePicture) continue
        for (x in s.quoteBarXs) {
            drawRect(c.quoteBar, Offset(x0 + x, layout.top(r)), Size(bar, s.height))
        }
    }

    // The selection, behind the text.
    val v = state.frame
    if (v.hasSelection) {
        val path = layout.selectionPath(v.anchorRow.toInt() to v.anchorCh.toInt(), v.caretRow.toInt() to v.caretCh.toInt())
        drawPath(path, c.selection)
    }

    for (r in first..last) {
        val s = layout.shape(r)
        val text = s.text ?: continue
        drawText(text, topLeft = Offset(x0, layout.top(r)))
        if (layout.row(r).isThematicBreak) {
            val y = layout.top(r) + s.height / 2
            drawRect(c.separator, Offset(x0 + s.prefixWidth, y), Size(col - s.prefixWidth, theme.ruleThickness.toPx()))
        }
    }

    // What the IME is composing, underlined — where it is on one row.
    state.composing?.let { (from, to) ->
        val a = state.doc.posForOffset(from)
        val b = state.doc.posForOffset(to)
        if (a.row == b.row) {
            val ra = layout.caretRect(a.row.toInt(), a.ch.toInt())
            val rb = layout.caretRect(b.row.toInt(), b.ch.toInt())
            if (ra != null && rb != null && ra.top == rb.top) {
                drawRect(c.composing, Offset(ra.left, ra.bottom - 2.dp.toPx()), Size(rb.left - ra.left, 1.5.dp.toPx()))
            }
        }
    }

    if (!v.hasSelection && caretOn) {
        layout.caretRect(v.caretRow.toInt(), v.caretCh.toInt())?.let {
            drawRect(c.caret, it.topLeft, Size(theme.caretWidth.toPx(), it.height))
        }
    }

    if (v.hasSelection && state.focused) {
        handleGeometry(state, layout, theme, this)?.let { h ->
            // Android's teardrops: a disc with its square corner touching the
            // end of the selection — up and right of the start, up and left of
            // the end.
            drawCircle(c.handle, h.radius, h.startCenter)
            drawRect(c.handle, Offset(h.start.left - h.radius, h.start.bottom), Size(h.radius, h.radius))
            drawCircle(c.handle, h.radius, h.endCenter)
            drawRect(c.handle, Offset(h.end.left, h.end.bottom), Size(h.radius, h.radius))
        }
    }
}

// MARK: the text-input session

/**
 * Opens a text-input session while the editor is focused, handing the IME a
 * [LeafInputConnection]. Before the focus target in the chain, so it hears
 * the editor's focus change.
 */
private data class LeafTextInputElement(val state: LeafEditorState) : ModifierNodeElement<LeafTextInputNode>() {
    override fun create() = LeafTextInputNode(state)
    override fun update(node: LeafTextInputNode) {
        node.state = state
    }
}

private class LeafTextInputNode(var state: LeafEditorState) :
    Modifier.Node(), PlatformTextInputModifierNode, FocusEventModifierNode {
    private var session: Job? = null

    override fun onFocusEvent(focusState: FocusState) {
        state.focused = focusState.isFocused
        if (focusState.isFocused) {
            if (session == null) {
                session = coroutineScope.launch {
                    establishTextInputSession {
                        startInputMethod(
                            PlatformTextInputMethodRequest { info ->
                                LeafInputConnection(state).also { it.configure(info) }
                            },
                        )
                    }
                }
            }
        } else {
            session?.cancel()
            session = null
            state.finishComposing()
        }
    }

    override fun onDetach() {
        session?.cancel()
        session = null
    }
}
