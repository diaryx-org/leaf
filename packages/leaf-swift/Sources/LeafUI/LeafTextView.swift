//  LeafTextView.swift  (AppKit / macOS)
//
//  The macOS editing surface — the peer of leaf-wasm's `LeafEditor`. It owns
//  presentation and input, never the model; `LeafDoc` (leaf-core, over the FFI)
//  stays the single source of truth. Each already-wrapped `Row` is drawn directly
//  (no NSTextView/NSLayoutManager), the caret is placed at `caret_ch`, and every
//  key/mouse intent routes back into core, which edits and returns the next frame.
//  Shared geometry lives in `EditorLayout`; the UIKit peer is `LeafTextView` in
//  `LeafTextViewiOS.swift`.
//
//  ## Native selection on AppKit
//
//  AppKit has no analogue to iOS's `UITextInteraction` — nothing lets the system
//  draw or own a selection over custom-laid-out text, so (like Xcode's own editor)
//  this view paints the selection itself. What makes it *native* is that the OS is
//  told the truth about it: `NSTextInputClient` reports the real `selectedRange` and
//  answers `attributedSubstring`/`firstRect`/`characterIndex`, the view is an
//  `NSServicesMenuRequestor`, and it exposes an `NSAccessibility` text area. So Look
//  Up, the Services menu, dictation, the right-click menu, VoiceOver, and the
//  emphasized/unemphasized (key-window-aware) highlight all behave natively — the
//  same experience the iOS peer gets from `UITextInput`, reached a different way.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import LeafFFI

public final class LeafTextView: NSView, NSTextInputClient, NSServicesMenuRequestor {
    let doc: LeafDoc
    public var theme: EditorTheme {
        didSet {
            // Re-wrap only when the geometry changed; a colour-only (or identical)
            // theme just repaints. Guarding this breaks the relayout⇄state-publish
            // loop that otherwise re-scrolled the view to the caret every frame.
            guard theme.metricsDiffer(from: oldValue) else { needsDisplay = true; return }
            shapeCache.removeAll(keepingCapacity: true)   // shaping is theme-dependent
            relayoutForWidth(force: true)
        }
    }
    /// Fired after every repaint so a host can update a toolbar/footer.
    public var onStateChange: ((EditorState) -> Void)?

    private var docView: DocView
    private var layoutEngine: EditorLayout
    /// The pixel width rows wrap to (content width, minus insets). Core lays out
    /// unwrapped; the view soft-wraps at this width.
    private var wrapWidth: CGFloat = 0
    /// Per-row shaped-text cache reused across frames; an edit re-shapes only the
    /// changed row(s). Cleared when the theme geometry changes (see `theme`).
    private var shapeCache: [Row: ShapedRow] = [:]
    /// The pixel x that ↑/↓ aim for, so repeated vertical motion rides the visual
    /// wrap without drifting through shorter lines. Nil except mid vertical run.
    private var verticalGoalX: CGFloat?
    /// The byte range of the in-flight IME composition (marked text), drawn with a
    /// composing underline. Nil when not composing. Committed text clears it.
    private var markedByteRange: NSRange?

    private var caretVisible = true
    private var blinkTimer: Timer?
    private var isFocused = false
    /// The caret offset the view last scrolled to reveal. Only a *move* re-scrolls,
    /// so passive reflows (width/theme relayout, state refreshes) leave the reader's
    /// scroll position alone instead of yanking it back to the caret.
    private var lastCaretOffset: UInt32?

    public init(doc: LeafDoc, theme: EditorTheme = .default) {
        self.doc = doc
        self.theme = theme
        // Switch core to unwrapped layout (one row per block); the view soft-wraps
        // each row at its own pixel width.
        let first = doc.setUnwrapped()
        self.docView = first
        var seed: [Row: ShapedRow] = [:]
        self.layoutEngine = EditorLayout(first, theme: theme, wrapWidth: 0, cache: &seed)
        self.shapeCache = seed
        super.init(frame: .zero)
        autoresizingMask = [.width]
        // Seed with the initial caret so the first reflow opens at the top rather
        // than scrolling to wherever the caret happens to start.
        lastCaretOffset = doc.caretOffset()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    public override var isFlipped: Bool { true }   // origin top-left → rows top-down
    public override var acceptsFirstResponder: Bool { true }
    public override var isOpaque: Bool { false }

    // MARK: layout / wrap

    public override func layout() {
        super.layout()
        relayoutForWidth(force: false)
    }

    private func relayoutForWidth(force: Bool) {
        let w = bounds.width - theme.padding.left - theme.padding.right
        guard w > 0 else { return }
        if force || abs(w - wrapWidth) > 0.5 {
            wrapWidth = w
            // Re-wrap the current frame at the new pixel width — the unwrapped map is
            // width-independent, so no round trip to core is needed.
            render(docView, keepVerticalGoal: true)
        }
    }

    // MARK: applying a frame

    private func render(_ view: DocView, keepVerticalGoal: Bool = false) {
        if !keepVerticalGoal { verticalGoalX = nil }
        docView = view
        layoutEngine = EditorLayout(view, theme: theme, wrapWidth: wrapWidth, cache: &shapeCache)
        let h = layoutEngine.contentHeight
        if abs(frame.height - h) > 0.5 { setFrameSize(NSSize(width: frame.width, height: h)) }
        needsDisplay = true
        resetBlink()
        // Only follow the caret when it actually moved (typing, motion, click), not
        // on a passive reflow — otherwise every relayout snaps the reader back.
        let caret = doc.caretOffset()
        if caret != lastCaretOffset {
            lastCaretOffset = caret
            scrollCaretToVisible()
        }
        onStateChange?(EditorState(view: view.view, dirty: view.dirty, heading: view.heading, active: view.active))
    }

    // MARK: drawing

    public override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let padX = theme.padding.left
        let fullWidth = bounds.width - theme.padding.left - theme.padding.right
        let active = selectionIsActive
        let selColor = active ? theme.selectionColor : theme.inactiveSelectionColor

        for rl in layoutEngine.rows {
            // Rows are laid out top-down, so cull to the dirty band: skip rows above
            // it, stop once past the bottom. A scroll or caret blink then repaints
            // only the visible rows, not the whole document.
            if rl.top >= dirtyRect.maxY { break }
            if rl.top + rl.height <= dirtyRect.minY { continue }
            // A table draws its own grid (once, on its first picture row); its
            // rows carry no text to paint.
            if let grid = rl.table {
                if rl.tableFirst { drawTable(grid, tableTop: rl.tableTop, selColor: selColor, in: ctx) }
                continue
            }
            let rowRect = CGRect(x: padX, y: rl.top, width: fullWidth, height: rl.height)
            if rl.row.code {
                ctx.setFillColor(theme.codeBackground.cgColor)
                ctx.fill(rowRect.insetBy(dx: -4, dy: 0))
                if let lang = rl.row.codeLang, !lang.isEmpty { drawCodeLang(lang, in: rowRect) }
            }
            layoutEngine.fillSelection(row: rl, padLeft: padX, color: selColor, in: ctx)
            // Draw each wrapped visual line's substring on its own line box.
            for (i, wl) in rl.wrapped.enumerated() {
                let lineTop = rl.top + CGFloat(i) * rl.lineHeight
                if lineTop >= dirtyRect.maxY { break }
                if lineTop + rl.lineHeight <= dirtyRect.minY { continue }
                wl.attributed.draw(with: CGRect(x: padX, y: lineTop, width: fullWidth, height: rl.lineHeight),
                                   options: [.usesLineFragmentOrigin])
            }
        }

        if markedByteRange != nil { drawMarkedUnderline(in: ctx) }

        if active, caretVisible, let rect = layoutEngine.caretRect(docView, theme: theme) {
            ctx.setFillColor(theme.caretColor.cgColor)
            ctx.fill(rect)
        }
    }

    /// Draw a table as a proportional grid — header fill and body stripes, the
    /// cell text, then the grid rules over them — the Apple peer of leaf-gpui's
    /// `table_chrome`. `tableTop` is the grid's top in view coordinates.
    private func drawTable(_ grid: TableLayout, tableTop: CGFloat, selColor: LeafColor, in ctx: CGContext) {
        let left = theme.padding.left
        let border = TableMetrics.border
        let x0 = left + (grid.colX.first ?? 0)
        let x1 = left + (grid.colX.last ?? 0)

        // Fills under the text: the header rows, then every other body row.
        var body = 0
        for row in grid.rows {
            let bg: LeafColor?
            if row.head {
                bg = theme.tableHeaderColor
            } else {
                body += 1
                bg = body % 2 == 0 ? theme.tableStripeColor : nil // first body row clear
            }
            if let bg {
                ctx.setFillColor(bg.cgColor)
                ctx.fill(CGRect(x: x0, y: tableTop + row.top, width: x1 - x0, height: row.height))
            }
        }

        // Selection highlight, behind the cell text — the table peer of the row
        // path's `fillSelection`. One rect per selected span, clipped to its cell
        // line; the row backgrounds above it, the text below, exactly as a plain
        // row layers them.
        ctx.setFillColor(selColor.cgColor)
        for row in grid.rows {
            let selTop = tableTop + row.top + TableMetrics.padY
            for cell in row.cells {
                for (i, line) in cell.lines.enumerated() where !line.selRanges.isEmpty {
                    let y = selTop + CGFloat(i) * grid.lineHeight
                    for (s, e) in line.selRanges {
                        let sx = CTLineGetOffsetForStringIndex(line.line, CFIndex(s), nil)
                        let ex = CTLineGetOffsetForStringIndex(line.line, CFIndex(e), nil)
                        ctx.fill(CGRect(x: left + line.textX + sx, y: y,
                                        width: ex - sx, height: grid.lineHeight))
                    }
                }
            }
        }

        // Cell text — each cell line on its own row within the cell's band.
        for row in grid.rows {
            let top = tableTop + row.top + TableMetrics.padY
            for cell in row.cells {
                for (i, line) in cell.lines.enumerated() {
                    line.attributed.draw(
                        with: CGRect(x: left + line.textX,
                                     y: top + CGFloat(i) * grid.lineHeight,
                                     width: .greatestFiniteMagnitude, height: theme.lineHeight),
                        options: [.usesLineFragmentOrigin])
                }
            }
        }

        // Grid rules over the fills and text.
        ctx.setFillColor(theme.tableBorderColor.cgColor)
        for bx in grid.colX { // verticals, outer two included
            ctx.fill(CGRect(x: left + bx, y: tableTop, width: border, height: grid.height))
        }
        var edgeYs = [tableTop] // horizontals: top, each row boundary, bottom
        for row in grid.rows { edgeYs.append(tableTop + row.top + row.height) }
        for ey in edgeYs {
            ctx.fill(CGRect(x: x0, y: min(ey, tableTop + grid.height - border),
                            width: x1 - x0 + border, height: border))
        }
    }

    /// Underline the in-flight IME composition, one segment per visual line — the
    /// native "you're still composing this" affordance.
    private func drawMarkedUnderline(in ctx: CGContext) {
        guard let m = markedByteRange, m.length > 0 else { return }
        let s = doc.posForOffset(off: UInt32(m.location))
        let e = doc.posForOffset(off: UInt32(m.location + m.length))
        ctx.setFillColor(theme.caretColor.cgColor)
        for row in Int(s.row)...Int(e.row) where layoutEngine.rows.indices.contains(row) {
            let rl = layoutEngine.rows[row]
            let rowFrom = (row == Int(s.row)) ? Int(s.ch) : 0
            let rowTo = (row == Int(e.row)) ? Int(e.ch) : rl.attributed.length
            for (i, wl) in rl.wrapped.enumerated() {
                let lineStart = wl.start, lineEnd = wl.start + wl.length
                let cs = max(rowFrom, lineStart), ce = min(rowTo, lineEnd)
                guard cs < ce else { continue }
                let x0 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(cs - lineStart), nil)
                let x1 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(ce - lineStart), nil)
                let y = rl.top + CGFloat(i) * rl.lineHeight + rl.lineHeight - 1.5
                ctx.fill(CGRect(x: theme.padding.left + x0, y: y, width: x1 - x0, height: 1))
            }
        }
    }

    /// Whether this view owns the text focus right now: first responder **and** in the
    /// key window. Drives the emphasized-vs-unemphasized selection fill and whether the
    /// caret shows — matching a native `NSTextView`, which greys its selection and hides
    /// its caret the moment its window stops being key.
    private var selectionIsActive: Bool { isFocused && (window?.isKeyWindow ?? false) }

    private func drawCodeLang(_ lang: String, in rowRect: CGRect) {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: theme.monospaceFont(size: theme.fontSize * 0.75, bold: false, italic: false),
            .foregroundColor: theme.secondaryColor,
        ]
        let s = lang as NSString
        let size = s.size(withAttributes: attrs)
        s.draw(at: CGPoint(x: rowRect.maxX - size.width - 2, y: rowRect.minY + 1), withAttributes: attrs)
    }

    private func scrollCaretToVisible() {
        if let rect = layoutEngine.caretRect(docView, theme: theme) {
            scrollToVisible(rect.insetBy(dx: 0, dy: -theme.lineHeight))
        }
    }

    // MARK: mouse

    /// The `(row, ch)` a click at `p` resolves to. Inside a table the point is
    /// mapped through the grid to a source offset and back to a picture-row
    /// coordinate, so the ordinary `clickCh` path (which snaps to a cell stop)
    /// still applies; elsewhere it's the plain visual hit-test.
    private func hitRowCh(_ p: CGPoint) -> (Int, Int) {
        if let off = layoutEngine.tableHitOffset(p, theme: theme) {
            let rc = doc.posForOffset(off: UInt32(off))
            return (Int(rc.row), Int(rc.ch))
        }
        return layoutEngine.hit(p, theme: theme)
    }

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let p = convert(event.locationInWindow, from: nil)
        let (row, ch) = hitRowCh(p)
        // ⌘-click opens a link under the pointer (the native convention), leaving the
        // caret there. A plain click still places the caret to edit the link text.
        if event.modifierFlags.contains(.command), event.clickCount == 1 {
            render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: false))
            openLinkAtCaret()
            return
        }
        let extend = event.modifierFlags.contains(.shift)
        switch event.clickCount {
        case 2:  render(doc.selectWordCh(row: UInt32(row), ch: UInt32(ch)))
        case 3:  render(doc.selectBlockCh(row: UInt32(row), ch: UInt32(ch)))
        default: render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: extend))
        }
    }

    /// Open the link under the caret in the default app, if there is one and it
    /// parses as a URL. Used by ⌘-click and the "Open Link" menu item.
    @discardableResult
    private func openLinkAtCaret() -> Bool {
        guard let dest = doc.linkDestinationAtCaret(), let url = URL(string: dest) else { return false }
        NSWorkspace.shared.open(url)
        return true
    }

    @objc private func openLink(_ sender: Any?) { openLinkAtCaret() }

    @objc private func copyLink(_ sender: Any?) {
        guard let dest = doc.linkDestinationAtCaret() else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(dest, forType: .string)
    }

    public override func mouseDragged(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let (row, ch) = hitRowCh(p)
        render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: true))
    }

    // MARK: drag & drop (destination)

    public override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        window?.makeFirstResponder(self)
        moveCaretToDrop(sender)
        return .copy
    }

    public override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        moveCaretToDrop(sender)   // track the drop point so the caret previews it
        return .copy
    }

    public override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        moveCaretToDrop(sender)
        let pb = sender.draggingPasteboard
        if let html = pb.string(forType: .html) {
            render(doc.pasteRich(html: html, text: pb.string(forType: .string) ?? ""))
            return true
        }
        if let text = pb.string(forType: .string) {
            render(doc.paste(text: text))
            return true
        }
        return false
    }

    private func moveCaretToDrop(_ sender: NSDraggingInfo) {
        let p = convert(sender.draggingLocation, from: nil)
        let (row, ch) = hitRowCh(p)
        render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: false))
    }

    // MARK: keyboard — text + IME

    public override func keyDown(with event: NSEvent) {
        // Shift+Return is leaf's in-cell line break. AppKit's default key bindings
        // don't distinguish it from a bare Return — both resolve to
        // `insertNewline:` (only Ctrl+Return maps to `insertLineBreak:`) — so
        // without catching it here it would drop to the next cell instead of
        // breaking the line. Route it straight to the line-break command, which
        // no-ops off a table (falling back to an ordinary newline). Skip while an
        // IME composition is live so a Shift+Return that commits marked text still
        // reaches the input system.
        let mods = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let isReturn = event.keyCode == 36 || event.keyCode == 76 // Return, keypad Enter
        if markedByteRange == nil, isReturn, mods == .shift {
            doCommand(by: #selector(insertLineBreak(_:)))
            return
        }
        if !(inputContext?.handleEvent(event) ?? false) { interpretKeyEvents([event]) }
    }

    public func insertText(_ string: Any, replacementRange: NSRange) {
        let text = (string as? String) ?? (string as? NSAttributedString)?.string ?? ""
        // Committing an IME composition: replace the marked bytes with the final text.
        if let m = markedByteRange {
            markedByteRange = nil
            render(doc.replaceRange(from: UInt32(m.location), to: UInt32(m.location + m.length), text: text))
            return
        }
        guard !text.isEmpty else { return }
        render(doc.insert(text: text))
    }

    public override func doCommand(by selector: Selector) {
        switch selector {
        case #selector(moveLeft(_:)):                       render(doc.moveLeft(extend: false))
        case #selector(moveRight(_:)):                      render(doc.moveRight(extend: false))
        // ↑/↓ ride the pixel wrap, not core's paragraph rows (which the unwrapped map
        // exposes) — computed from the visual geometry, then snapped by `clickCh`.
        case #selector(moveUp(_:)):                         moveVertical(up: true, extend: false)
        case #selector(moveDown(_:)):                       moveVertical(up: false, extend: false)
        case #selector(moveLeftAndModifySelection(_:)):     render(doc.moveLeft(extend: true))
        case #selector(moveRightAndModifySelection(_:)):    render(doc.moveRight(extend: true))
        case #selector(moveUpAndModifySelection(_:)):       moveVertical(up: true, extend: true)
        case #selector(moveDownAndModifySelection(_:)):     moveVertical(up: false, extend: true)
        case #selector(moveWordLeft(_:)):                   render(doc.moveWordLeft(extend: false))
        case #selector(moveWordRight(_:)):                  render(doc.moveWordRight(extend: false))
        case #selector(moveWordLeftAndModifySelection(_:)): render(doc.moveWordLeft(extend: true))
        case #selector(moveWordRightAndModifySelection(_:)):render(doc.moveWordRight(extend: true))
        // Home/End go to the *visual* line's ends, not the whole paragraph's.
        case #selector(moveToLeftEndOfLine(_:)),
             #selector(moveToBeginningOfLine(_:)):          moveToVisualLineBoundary(toStart: true, extend: false)
        case #selector(moveToLeftEndOfLineAndModifySelection(_:)),
             #selector(moveToBeginningOfLineAndModifySelection(_:)): moveToVisualLineBoundary(toStart: true, extend: true)
        case #selector(moveToRightEndOfLine(_:)),
             #selector(moveToEndOfLine(_:)):                moveToVisualLineBoundary(toStart: false, extend: false)
        case #selector(moveToRightEndOfLineAndModifySelection(_:)),
             #selector(moveToEndOfLineAndModifySelection(_:)): moveToVisualLineBoundary(toStart: false, extend: true)
        case #selector(moveToBeginningOfDocument(_:)):      render(doc.moveDocStart(extend: false))
        case #selector(moveToEndOfDocument(_:)):            render(doc.moveDocEnd(extend: false))
        case #selector(moveToBeginningOfDocumentAndModifySelection(_:)): render(doc.moveDocStart(extend: true))
        case #selector(moveToEndOfDocumentAndModifySelection(_:)):       render(doc.moveDocEnd(extend: true))
        // In a table these keys take on grid meanings (see the FFI's `cell_*`):
        // Return drops a cell, Shift+Return breaks a line within one, Tab/Shift+Tab
        // walk the cells. Each returns nil off the table, where the key keeps its
        // ordinary job (newline, indent).
        case #selector(insertNewline(_:)):
            render(doc.cellReturn() ?? doc.newline())
        case #selector(insertLineBreak(_:)):
            render(doc.cellLineBreak() ?? doc.newline())
        case #selector(insertTab(_:)):
            render(doc.cellTab(forward: true) ?? doc.indent())
        case #selector(insertBacktab(_:)):
            render(doc.cellTab(forward: false) ?? doc.outdent())
        case #selector(deleteBackward(_:)):                 render(doc.backspace())
        case #selector(deleteForward(_:)):                  render(doc.deleteForward())
        case #selector(deleteWordBackward(_:)):             render(doc.deleteWordBack())
        case #selector(deleteWordForward(_:)):              render(doc.deleteWordForward())
        default: super.doCommand(by: selector)
        }
    }

    // MARK: visual-line motion (the wrap is ours, so core can't do these)

    /// Move the caret one *visual* line up/down, holding the pixel x it started from
    /// so a run of ↑/↓ doesn't drift through shorter lines. Probes one line-height
    /// past the caret and hit-tests, so it crosses block boundaries naturally.
    private func moveVertical(up: Bool, extend: Bool) {
        guard let caret = layoutEngine.caretRect(docView, theme: theme) else {
            render(up ? doc.moveUp(extend: extend) : doc.moveDown(extend: extend))
            return
        }
        // Probe from the caret's full *line band* — inside a table that clears the
        // cell's vertical padding, which the thin caret rect doesn't, so a step
        // actually crosses into the next line/cell instead of stalling. Hit-test
        // the table-aware way (`hitRowCh`), or a probe into a table resolves to the
        // collapsed picture row and teleports the caret to its top-left cell.
        let band = layoutEngine.caretBand(src: Int(docView.caretSrc))
        let goalX = verticalGoalX ?? caret.minX
        var probeY = up ? (band?.minY ?? caret.minY) - 1 : (band?.maxY ?? caret.maxY) + 1
        var (row, ch) = hitRowCh(CGPoint(x: goalX, y: probeY))
        // A block boundary is drawn as a short blank gap row (`blockGap`, half a
        // line) that holds no caret. Probing one line past the caret lands *inside*
        // that gap, where the hit-test snaps back and forth between the block above
        // and below — so a step from a paragraph into the list or code block under
        // it moves only sometimes. Step over any gap row(s) to the next row that can
        // actually hold the caret; the bounded walk can't outrun the row count.
        let rows = layoutEngine.rows
        var guardCount = 0
        while rows.indices.contains(row), rows[row].row.isBlockGap, guardCount < rows.count {
            let r = rows[row]
            probeY = up ? r.top - 1 : r.top + r.height + 1
            (row, ch) = hitRowCh(CGPoint(x: goalX, y: probeY))
            guardCount += 1
        }
        verticalGoalX = goalX
        render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: extend), keepVerticalGoal: true)
    }

    /// Move to the start or end of the caret's *visual* line. At the end of a
    /// soft-wrapped line, stop before the wrap whitespace so the caret stays on this
    /// line rather than jumping to the next line's start.
    private func moveToVisualLineBoundary(toStart: Bool, extend: Bool) {
        let row = Int(docView.caretRow), ch = Int(docView.caretCh)
        guard let vl = layoutEngine.visualLine(row: row, ch: ch) else {
            render(toStart ? doc.moveHome(extend: extend) : doc.moveEnd(extend: extend))
            return
        }
        var target = toStart ? vl.start : vl.end
        if !toStart, vl.index < layoutEngine.rows[row].wrapped.count - 1,
           layoutEngine.rows[row].wrapped[vl.index].attributed.string.hasSuffix(" ") {
            target -= 1
        }
        render(doc.clickCh(row: UInt32(row), ch: UInt32(target), extend: extend))
    }

    public override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard event.modifierFlags.contains(.command) else { return super.performKeyEquivalent(with: event) }
        let shift = event.modifierFlags.contains(.shift)
        switch event.charactersIgnoringModifiers?.lowercased() {
        case "b": render(doc.toggleBold()); return true
        case "i": render(doc.toggleItalic()); return true
        case "u": render(doc.toggleUnderline()); return true
        case "e": render(doc.toggleView()); return true
        case "z": render(shift ? doc.redo() : doc.undo()); return true
        case "a": render(doc.selectAll()); return true
        case "c": copy(nil); return true
        case "x": cut(nil); return true
        case "v": if shift { pasteAsPlainText(nil) } else { paste(nil) }; return true
        default:  return super.performKeyEquivalent(with: event)
        }
    }

    // MARK: rich clipboard

    @objc public func copy(_ sender: Any?) {
        guard let text = doc.selectedText() else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
        if let html = doc.selectionHtml() { pb.setString(html, forType: .html) }
    }

    @objc public func cut(_ sender: Any?) {
        copy(sender)
        if doc.selectedText() != nil { render(doc.backspace()) }
    }

    @objc public func paste(_ sender: Any?) {
        let pb = NSPasteboard.general
        let html = pb.string(forType: .html)
        let text = pb.string(forType: .string) ?? ""
        guard html != nil || !text.isEmpty else { return }
        render(doc.pasteRich(html: html, text: text))
    }

    /// ⇧⌘V — the plain-flavor escape hatch: insert the pasteboard's text as leaf
    /// *source*, ignoring any rich HTML flavor (mirrors leaf-gpui's ⇧⌘V and
    /// leaf-tui's ⌥V). The Edit menu's "Paste and Match Style" routes here too.
    @objc public func pasteAsPlainText(_ sender: Any?) {
        guard let text = NSPasteboard.general.string(forType: .string), !text.isEmpty else { return }
        render(doc.paste(text: text))
    }

    @objc public override func selectAll(_ sender: Any?) { render(doc.selectAll()) }

    // MARK: contextual menu + macOS text services

    public override func menu(for event: NSEvent) -> NSMenu? {
        window?.makeFirstResponder(self)
        // Right-clicking outside the selection moves the caret there first, like a
        // native text view; a click inside an existing selection keeps it.
        if !hasSelection {
            let p = convert(event.locationInWindow, from: nil)
            let (row, ch) = hitRowCh(p)
            render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: false))
        }

        let menu = NSMenu()
        // A link under the click (the caret was just placed there) leads the menu.
        if doc.linkDestinationAtCaret() != nil {
            menu.addItem(withTitle: loc("menu.openLink", "Open Link"), action: #selector(openLink(_:)), keyEquivalent: "")
            menu.addItem(withTitle: loc("menu.copyLink", "Copy Link"), action: #selector(copyLink(_:)), keyEquivalent: "")
            menu.addItem(.separator())
        }
        if hasSelection {
            menu.addItem(withTitle: loc("menu.cut", "Cut"), action: #selector(cut(_:)), keyEquivalent: "")
            menu.addItem(withTitle: loc("menu.copy", "Copy"), action: #selector(copy(_:)), keyEquivalent: "")
        }
        menu.addItem(withTitle: loc("menu.paste", "Paste"), action: #selector(paste(_:)), keyEquivalent: "")
        menu.addItem(withTitle: loc("menu.pasteMatchStyle", "Paste and Match Style"), action: #selector(pasteAsPlainText(_:)), keyEquivalent: "")
        menu.addItem(withTitle: loc("menu.selectAll", "Select All"), action: #selector(selectAll(_:)), keyEquivalent: "")
        if hasSelection, let text = doc.selectedText(), !text.isEmpty {
            menu.addItem(.separator())
            let shown = text.count > 24 ? text.prefix(24) + "…" : Substring(text)
            menu.addItem(withTitle: String(format: loc("menu.lookUp", "Look Up “%@”"), String(shown)),
                         action: #selector(lookUpSelection(_:)), keyEquivalent: "")
            menu.addItem(withTitle: loc("menu.share", "Share…"), action: #selector(shareSelection(_:)), keyEquivalent: "")
        }
        return menu
    }

    /// A localized UI string with an English fallback, looked up in the bundle this
    /// class ships in — the host app's, for a statically linked package. So a host
    /// can translate the menu (drop a `Localizable.strings` with these keys) without
    /// the library owning a resource bundle, and the English `value` shows otherwise.
    private func loc(_ key: String, _ value: String) -> String {
        NSLocalizedString(key, tableName: nil, bundle: Bundle(for: LeafTextView.self), value: value, comment: "")
    }

    @objc private func lookUpSelection(_ sender: Any?) {
        guard let text = doc.selectedText(), !text.isEmpty else { return }
        let rc = doc.posForOffset(off: UInt32(selLowByte))
        let origin = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch), theme: theme)?.origin ?? .zero
        showDefinition(for: NSAttributedString(string: text),
                       at: NSPoint(x: origin.x, y: origin.y + theme.lineHeight))
    }

    @objc private func shareSelection(_ sender: Any?) {
        guard let text = doc.selectedText(), !text.isEmpty else { return }
        let rc = doc.posForOffset(off: UInt32(selLowByte))
        let anchor = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch), theme: theme) ?? .zero
        NSSharingServicePicker(items: [text]).show(relativeTo: anchor, of: self, preferredEdge: .minY)
    }

    /// Advertise the selection to the Services system: we can *send* a string when
    /// there's a selection and *receive* one to replace it. Pairs with the
    /// `NSServicesMenuRequestor` methods below.
    public override func validRequestor(forSendType sendType: NSPasteboard.PasteboardType?,
                                        returnType: NSPasteboard.PasteboardType?) -> Any? {
        let sendOK = sendType == nil || (sendType == .string && hasSelection)
        let returnOK = returnType == nil || returnType == .string
        if sendOK, returnOK { return self }
        return super.validRequestor(forSendType: sendType, returnType: returnType)
    }

    public func writeSelection(to pboard: NSPasteboard, types: [NSPasteboard.PasteboardType]) -> Bool {
        guard hasSelection, types.contains(.string), let text = doc.selectedText() else { return false }
        pboard.clearContents()
        return pboard.setString(text, forType: .string)
    }

    public func readSelection(from pboard: NSPasteboard) -> Bool {
        guard let text = pboard.string(forType: .string) else { return false }
        render(doc.pasteRich(html: nil, text: text))
        return true
    }

    // MARK: accessibility — expose the document as a native text area

    public override func isAccessibilityElement() -> Bool { true }
    public override func accessibilityRole() -> NSAccessibility.Role? { .textArea }
    public override func accessibilityValue() -> Any? { fullText() }
    public override func accessibilityNumberOfCharacters() -> Int { (fullText() as NSString).length }
    public override func accessibilityInsertionPointLineNumber() -> Int { Int(docView.caretRow) }

    public override func accessibilitySelectedText() -> String? {
        doc.textInRange(from: UInt32(selLowByte), to: UInt32(selHighByte))
    }

    public override func accessibilitySelectedTextRange() -> NSRange {
        let loc = (doc.textInRange(from: 0, to: UInt32(selLowByte)) as NSString).length
        let len = ((accessibilitySelectedText() ?? "") as NSString).length
        return NSRange(location: loc, length: len)
    }

    public override func setAccessibilitySelectedTextRange(_ range: NSRange) {
        let full = fullText() as NSString
        guard range.location >= 0, range.location + range.length <= full.length else { return }
        let fromByte = full.substring(to: range.location).utf8.count
        let toByte = full.substring(to: range.location + range.length).utf8.count
        render(doc.setSelectionOffsets(anchor: UInt32(fromByte), focus: UInt32(toByte)))
    }

    public override func accessibilityString(for range: NSRange) -> String? {
        let full = fullText() as NSString
        guard range.location >= 0, range.location + range.length <= full.length else { return nil }
        return full.substring(with: range)
    }

    // MARK: focus + caret blink

    public override func becomeFirstResponder() -> Bool { isFocused = true; resetBlink(); needsDisplay = true; return true }
    public override func resignFirstResponder() -> Bool { isFocused = false; blinkTimer?.invalidate(); needsDisplay = true; return true }

    private func resetBlink() {
        blinkTimer?.invalidate()
        caretVisible = true
        guard isFocused else { return }
        blinkTimer = Timer.scheduledTimer(withTimeInterval: 0.53, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.caretVisible.toggle()
            if let r = self.layoutEngine.caretRect(self.docView, theme: self.theme) { self.setNeedsDisplay(r) }
        }
    }

    // MARK: window key state — selection emphasis + caret track the key window

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        let nc = NotificationCenter.default
        nc.removeObserver(self, name: NSWindow.didBecomeKeyNotification, object: nil)
        nc.removeObserver(self, name: NSWindow.didResignKeyNotification, object: nil)
        guard let window else { return }
        // Advertise the selection to the app-wide Services menu (Edit ▸ Services).
        NSApp.registerServicesMenuSendTypes([.string], returnTypes: [.string])
        // Accept text/rich content dropped into the editor.
        registerForDraggedTypes([.string, .html])
        nc.addObserver(self, selector: #selector(keyStateChanged), name: NSWindow.didBecomeKeyNotification, object: window)
        nc.addObserver(self, selector: #selector(keyStateChanged), name: NSWindow.didResignKeyNotification, object: window)
    }

    @objc private func keyStateChanged() { resetBlink(); needsDisplay = true }

    deinit { NotificationCenter.default.removeObserver(self) }

    // MARK: selection offsets
    //
    // The character-index space shared by `NSTextInputClient`, Services, and
    // accessibility below is leaf-core's **byte offset** — the same handle the iOS
    // `UITextInput` peer uses. Core owns the offset⇄position mapping (`posForOffset` /
    // `offsetForPos`), so these only have to stay self-consistent, which they do.

    private var caretByte: Int { Int(doc.caretOffset()) }
    private var anchorByte: Int { Int(doc.anchorOffset()) }
    private var selLowByte: Int { min(anchorByte, caretByte) }
    private var selHighByte: Int { max(anchorByte, caretByte) }
    private var hasSelection: Bool { docView.hasSelection }

    /// The document's whole plain text — the buffer those byte offsets count into.
    private func fullText() -> String { doc.textInRange(from: 0, to: doc.docEndOffset()) }

    // MARK: NSTextInputClient — real selection, geometry, and hit-testing
    //
    // With these reporting the true selection, macOS's system text services light up:
    // Look Up, the Services menu, dictation, and IME candidate placement all target
    // the real range. Marked text (the inline IME composition) is inserted as it's
    // composed and drawn with a composing underline; `insertText` commits it.

    public func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let text = (string as? String) ?? (string as? NSAttributedString)?.string ?? ""
        // Bytes to replace: the existing composition, else the proposed replacement,
        // else the current selection.
        let start: Int, end: Int
        if let m = markedByteRange {
            start = m.location; end = m.location + m.length
        } else if replacementRange.location != NSNotFound {
            start = replacementRange.location; end = replacementRange.location + replacementRange.length
        } else {
            start = selLowByte; end = selHighByte
        }
        render(doc.replaceRange(from: UInt32(max(0, start)), to: UInt32(max(start, end)), text: text))
        if text.isEmpty {
            markedByteRange = nil
        } else {
            markedByteRange = NSRange(location: start, length: text.utf8.count)
            // Place the caret within the composition per the IME's selected range.
            let ns = text as NSString
            let uptoUTF16 = min(max(0, selectedRange.location + selectedRange.length), ns.length)
            let caret = start + ns.substring(to: uptoUTF16).utf8.count
            render(doc.setSelectionOffsets(anchor: UInt32(caret), focus: UInt32(caret)))
        }
        needsDisplay = true
    }

    public func unmarkText() { markedByteRange = nil; needsDisplay = true }
    public func hasMarkedText() -> Bool { markedByteRange != nil }
    public func markedRange() -> NSRange { markedByteRange ?? NSRange(location: NSNotFound, length: 0) }
    public func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

    public func selectedRange() -> NSRange {
        NSRange(location: selLowByte, length: selHighByte - selLowByte)
    }

    public func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? {
        let from = max(0, range.location)
        let to = max(from, range.location + range.length)
        actualRange?.pointee = NSRange(location: from, length: to - from)
        return NSAttributedString(string: doc.textInRange(from: UInt32(from), to: UInt32(to)))
    }

    public func characterIndex(for point: NSPoint) -> Int {
        guard let window else { return NSNotFound }
        let local = convert(window.convertPoint(fromScreen: point), from: nil)
        let (row, ch) = layoutEngine.hit(local, theme: theme)
        return Int(doc.offsetForPos(row: UInt32(row), ch: UInt32(ch)))
    }

    public func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let window else { return .zero }
        actualRange?.pointee = range
        let rc = doc.posForOffset(off: UInt32(max(0, range.location)))
        guard let rect = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch), theme: theme) else { return .zero }
        return window.convertToScreen(convert(rect, to: nil))
    }

    // MARK: host access

    public func sourceText() -> String { doc.source() }
    public func markSaved() { render(doc.markSaved()) }
    public func command(_ op: (LeafDoc) -> DocView) { render(op(doc)) }
}
#endif
