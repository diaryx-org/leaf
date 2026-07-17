//  LeafTextView.swift
//
//  The AppKit editing surface — the peer of leaf-wasm's `LeafEditor`, leaf-tui's
//  event loop, and leaf-gpui's widget. It owns exactly what those own:
//  presentation and input, never the model. `LeafDoc` (leaf-core, over the FFI)
//  stays the single source of truth for text, caret math, and selection.
//
//  ## Why a custom NSView, not NSTextView
//
//  The rendered surface is a *projection* of core's AST (WYSIWYG hides markup;
//  list markers and quote gutters are synthetic), so the text system must never
//  own or mutate the text. Every frame is a `DocView` from core: rows already
//  wrapped to a column budget, plus the caret and selection. So each `Row` is one
//  visual line — we draw it directly, place the caret at `caret_ch` (a UTF-16
//  offset, which is what Core Text indexes in), and route every key/mouse intent
//  back into core, which edits and returns the next frame.
//
//  ## The width contract
//
//  Core measures in character *columns*, not pixels. We turn the viewport width
//  into a column budget with the body font's average glyph advance (proportional
//  text makes this a good average, exactly as the web's `_cols()` does), hand it
//  to `set_width`, and core wraps to it. We never multiply `col × cellWidth`.

#if canImport(AppKit)
import AppKit
import LeafFFI

/// Toolbar/chrome state pushed to the host after every repaint — the subset of a
/// `DocView` a surrounding UI reflects (active marks, heading level, dirty, view).
public struct EditorState {
    public var view: String          // "wysiwyg" | "source"
    public var dirty: Bool
    public var heading: UInt32?      // heading level at the caret, or nil
    public var active: [String]      // inline marks active at the caret
}

public final class LeafTextView: NSView, NSTextInputClient {
    // MARK: model + config

    let doc: LeafDoc
    public var theme: EditorTheme { didSet { avgGlyphWidth = nil; relayoutForWidth(force: true) } }
    /// Fired after every repaint so a host can update a toolbar/footer.
    public var onStateChange: ((EditorState) -> Void)?

    private var docView: DocView
    private var rowLayouts: [RowLayout] = []
    private var wrapCols: Int = 0
    private var avgGlyphWidth: CGFloat?

    // MARK: caret blink

    private var caretVisible = true
    private var blinkTimer: Timer?
    private var isFocused = false

    // MARK: init

    public init(doc: LeafDoc, theme: EditorTheme = .default) {
        self.doc = doc
        self.theme = theme
        self.docView = doc.view()
        super.init(frame: .zero)
        autoresizingMask = [.width]        // track the clip view's width
        buildRowLayouts()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    // A flipped view puts the origin at the top-left, so rows lay out top-down.
    public override var isFlipped: Bool { true }
    public override var acceptsFirstResponder: Bool { true }
    public override var isOpaque: Bool { false }

    // MARK: one visual row's laid-out geometry

    private struct RowLayout {
        let row: Row
        let attributed: NSAttributedString
        let line: CTLine
        let top: CGFloat
        let height: CGFloat
    }

    // MARK: layout / wrap

    public override func layout() {
        super.layout()
        relayoutForWidth(force: false)
    }

    /// Recompute the column budget from the current width; if it changed, ask core
    /// to rewrap and repaint. `force` rebuilds even when the width didn't move
    /// (e.g. a theme change altered the glyph advance).
    private func relayoutForWidth(force: Bool) {
        let cols = columnsForWidth()
        guard cols > 0 else { return }
        if force || cols != wrapCols {
            wrapCols = cols
            render(doc.setWidth(cols: UInt32(cols)))
        } else if rowLayouts.isEmpty {
            buildRowLayouts()
        }
    }

    private func columnsForWidth() -> Int {
        let avail = bounds.width - theme.padding.left - theme.padding.right
        let avg = glyphWidth()
        guard avail > 0, avg > 0 else { return 0 }
        return max(1, Int(avail / avg))
    }

    /// The body font's average glyph advance — a lowercase-heavy sample so the
    /// wrap budget tracks real prose rather than capitals (as the web does).
    private func glyphWidth() -> CGFloat {
        if let w = avgGlyphWidth { return w }
        let sample = "the quick brown fox jumps over the lazy dog " as NSString
        let font = theme.proportionalFont(size: theme.fontSize, bold: false, italic: false)
        let width = sample.size(withAttributes: [.font: font]).width / CGFloat(sample.length)
        avgGlyphWidth = width > 0 ? width : theme.fontSize * 0.5
        return avgGlyphWidth!
    }

    // MARK: applying a frame

    /// Install a fresh `DocView`: rebuild the row geometry, resize to fit, repaint,
    /// keep the caret visible, and notify the host. Every command funnels here.
    private func render(_ view: DocView) {
        docView = view
        buildRowLayouts()
        needsDisplay = true
        resetBlink()
        scrollCaretToVisible()
        onStateChange?(EditorState(
            view: view.view, dirty: view.dirty, heading: view.heading, active: view.active
        ))
    }

    private func buildRowLayouts() {
        var layouts: [RowLayout] = []
        layouts.reserveCapacity(docView.rows.count)
        var y = theme.padding.top
        for row in docView.rows {
            let attributed = AttributedRow.make(row, theme: theme)
            let line = CTLineCreateWithAttributedString(attributed)
            let h = theme.rowHeight(heading: row.heading)
            layouts.append(RowLayout(row: row, attributed: attributed, line: line, top: y, height: h))
            y += h
        }
        rowLayouts = layouts

        let contentHeight = y + theme.padding.bottom
        if abs(frame.height - contentHeight) > 0.5 {
            setFrameSize(NSSize(width: frame.width, height: contentHeight))
        }
    }

    // MARK: drawing

    public override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let padX = theme.padding.left
        let fullWidth = bounds.width - theme.padding.left - theme.padding.right

        for rl in rowLayouts {
            let rowRect = CGRect(x: padX, y: rl.top, width: fullWidth, height: rl.height)

            // Fenced/indented code line: a tinted panel behind the whole row, with
            // the language label (chrome, excluded from the row's text) at the right.
            if rl.row.code {
                theme.codeBackground.setFill()
                ctx.fill(rowRect.insetBy(dx: -4, dy: 0))
                if let lang = rl.row.codeLang, !lang.isEmpty {
                    drawCodeLang(lang, in: rowRect)
                }
            }

            drawSelection(for: rl, padX: padX)

            // The row text. `.usesLineFragmentOrigin` anchors it to the rect's top;
            // AppKit flips glyphs upright inside our flipped view.
            rl.attributed.draw(with: rowRect, options: [.usesLineFragmentOrigin])
        }

        drawCaret(ctx)
    }

    /// Fill the selection background behind every run core marked `sel`. Core has
    /// already carved the selection into run boundaries, so this needs no offset
    /// math of its own — the web draws the browser's native selection here instead.
    private func drawSelection(for rl: RowLayout, padX: CGFloat) {
        var utf16 = 0
        for run in rl.row.runs {
            let len = run.text.utf16.count
            if run.sel {
                let x0 = CTLineGetOffsetForStringIndex(rl.line, CFIndex(utf16), nil)
                let x1 = CTLineGetOffsetForStringIndex(rl.line, CFIndex(utf16 + len), nil)
                theme.selectionColor.setFill()
                CGRect(x: padX + x0, y: rl.top, width: x1 - x0, height: rl.height).fill()
            }
            utf16 += len
        }
    }

    private func drawCodeLang(_ lang: String, in rowRect: CGRect) {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: theme.monospaceFont(size: theme.fontSize * 0.75, bold: false, italic: false),
            .foregroundColor: theme.secondaryColor,
        ]
        let s = lang as NSString
        let size = s.size(withAttributes: attrs)
        s.draw(at: CGPoint(x: rowRect.maxX - size.width - 2, y: rowRect.minY + 1), withAttributes: attrs)
    }

    private func drawCaret(_ ctx: CGContext) {
        guard isFocused, caretVisible, let rect = caretRect() else { return }
        theme.caretColor.setFill()
        ctx.fill(rect)
    }

    /// The caret's frame in view coordinates — column `caret_ch` (UTF-16) mapped
    /// through Core Text to a pixel x, spanning its row's height.
    private func caretRect() -> CGRect? {
        let row = Int(docView.caretRow)
        guard rowLayouts.indices.contains(row) else { return nil }
        let rl = rowLayouts[row]
        let x = CTLineGetOffsetForStringIndex(rl.line, CFIndex(docView.caretCh), nil)
        return CGRect(x: theme.padding.left + x, y: rl.top, width: 1.5, height: rl.height)
    }

    private func scrollCaretToVisible() {
        if let rect = caretRect() { scrollToVisible(rect.insetBy(dx: 0, dy: -theme.lineHeight)) }
    }

    // MARK: hit testing

    /// Map a point in view coordinates to core's `(row, ch)` — the row from the
    /// vertical band it falls in, the UTF-16 offset from Core Text's hit-test of
    /// the horizontal position. `click_ch` then clamps `ch` to a real caret stop.
    private func hit(_ point: CGPoint) -> (row: Int, ch: Int) {
        guard !rowLayouts.isEmpty else { return (0, 0) }
        let row = rowLayouts.firstIndex { point.y < $0.top + $0.height } ?? rowLayouts.count - 1
        let rl = rowLayouts[row]
        let localX = point.x - theme.padding.left
        let idx = CTLineGetStringIndexForPosition(rl.line, CGPoint(x: max(0, localX), y: 0))
        let len = rl.attributed.length
        return (row, min(max(0, idx), len))
    }

    // MARK: mouse

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let p = convert(event.locationInWindow, from: nil)
        let (row, ch) = hit(p)
        let extend = event.modifierFlags.contains(.shift)
        switch event.clickCount {
        case 2:  render(doc.selectWordCh(row: UInt32(row), ch: UInt32(ch)))
        case 3:  render(doc.selectBlockCh(row: UInt32(row), ch: UInt32(ch)))
        default: render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: extend))
        }
    }

    public override func mouseDragged(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let (row, ch) = hit(p)
        render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: true))  // extend from anchor
    }

    // MARK: keyboard — text + IME

    public override func keyDown(with event: NSEvent) {
        // Route through the input context so dead keys, option-accents, and IME
        // reach `insertText` / `doCommand(by:)`. Motion & deletion arrive as
        // command selectors; typed text arrives as `insertText`.
        if !(inputContext?.handleEvent(event) ?? false) {
            interpretKeyEvents([event])
        }
    }

    public func insertText(_ string: Any, replacementRange: NSRange) {
        let text = (string as? String) ?? (string as? NSAttributedString)?.string ?? ""
        guard !text.isEmpty else { return }
        render(doc.insert(text: text))
    }

    // MARK: keyboard — command selectors

    public override func doCommand(by selector: Selector) {
        switch selector {
        case #selector(moveLeft(_:)):                       render(doc.moveLeft(extend: false))
        case #selector(moveRight(_:)):                      render(doc.moveRight(extend: false))
        case #selector(moveUp(_:)):                         render(doc.moveUp(extend: false))
        case #selector(moveDown(_:)):                       render(doc.moveDown(extend: false))
        case #selector(moveLeftAndModifySelection(_:)):     render(doc.moveLeft(extend: true))
        case #selector(moveRightAndModifySelection(_:)):    render(doc.moveRight(extend: true))
        case #selector(moveUpAndModifySelection(_:)):       render(doc.moveUp(extend: true))
        case #selector(moveDownAndModifySelection(_:)):     render(doc.moveDown(extend: true))
        case #selector(moveWordLeft(_:)):                   render(doc.moveWordLeft(extend: false))
        case #selector(moveWordRight(_:)):                  render(doc.moveWordRight(extend: false))
        case #selector(moveWordLeftAndModifySelection(_:)): render(doc.moveWordLeft(extend: true))
        case #selector(moveWordRightAndModifySelection(_:)):render(doc.moveWordRight(extend: true))
        case #selector(moveToLeftEndOfLine(_:)),
             #selector(moveToBeginningOfLine(_:)):          render(doc.moveHome(extend: false))
        case #selector(moveToLeftEndOfLineAndModifySelection(_:)),
             #selector(moveToBeginningOfLineAndModifySelection(_:)): render(doc.moveHome(extend: true))
        case #selector(moveToRightEndOfLine(_:)),
             #selector(moveToEndOfLine(_:)):                render(doc.moveEnd(extend: false))
        case #selector(moveToRightEndOfLineAndModifySelection(_:)),
             #selector(moveToEndOfLineAndModifySelection(_:)): render(doc.moveEnd(extend: true))
        case #selector(moveToBeginningOfDocument(_:)):      render(doc.moveDocStart(extend: false))
        case #selector(moveToEndOfDocument(_:)):            render(doc.moveDocEnd(extend: false))
        case #selector(moveToBeginningOfDocumentAndModifySelection(_:)): render(doc.moveDocStart(extend: true))
        case #selector(moveToEndOfDocumentAndModifySelection(_:)):       render(doc.moveDocEnd(extend: true))
        case #selector(insertNewline(_:)),
             #selector(insertLineBreak(_:)):                render(doc.newline())
        case #selector(insertTab(_:)):                      render(doc.insert(text: "  "))
        case #selector(deleteBackward(_:)):                 render(doc.backspace())
        case #selector(deleteForward(_:)):                  render(doc.deleteForward())
        case #selector(deleteWordBackward(_:)):             render(doc.deleteWordBack())
        case #selector(deleteWordForward(_:)):              render(doc.deleteWordForward())
        default: super.doCommand(by: selector)
        }
    }

    // MARK: keyboard — shortcuts + edit menu

    public override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard event.modifierFlags.contains(.command) else {
            return super.performKeyEquivalent(with: event)
        }
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
        case "v": paste(nil); return true
        default:  return super.performKeyEquivalent(with: event)
        }
    }

    // MARK: rich clipboard (mirrors leaf-tui / leaf-gpui / leaf-wasm)

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

    @objc public override func selectAll(_ sender: Any?) { render(doc.selectAll()) }

    // MARK: focus + caret blink

    public override func becomeFirstResponder() -> Bool {
        isFocused = true
        resetBlink()
        needsDisplay = true
        return true
    }

    public override func resignFirstResponder() -> Bool {
        isFocused = false
        blinkTimer?.invalidate()
        needsDisplay = true
        return true
    }

    /// Show the caret solid and (re)start the blink cycle — called after focus and
    /// after every edit/motion so the caret is visible the instant something moves.
    private func resetBlink() {
        blinkTimer?.invalidate()
        caretVisible = true
        guard isFocused else { return }
        blinkTimer = Timer.scheduledTimer(withTimeInterval: 0.53, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.caretVisible.toggle()
            if let r = self.caretRect() { self.setNeedsDisplay(r) }
        }
    }

    // MARK: NSTextInputClient — minimal (no inline marked text yet)
    // Committed text and command selectors go through the paths above. Inline IME
    // composition (marked text) is the one thing the web got free from
    // contenteditable and AppKit needs explicitly — left as a follow-up; committed
    // CJK/emoji still insert correctly.

    public func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {}
    public func unmarkText() {}
    public func hasMarkedText() -> Bool { false }
    public func markedRange() -> NSRange { NSRange(location: NSNotFound, length: 0) }
    public func selectedRange() -> NSRange { NSRange(location: NSNotFound, length: 0) }
    public func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }
    public func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? { nil }
    public func characterIndex(for point: NSPoint) -> Int { NSNotFound }

    /// Where the IME candidate window anchors: the caret rect, in screen space.
    public func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let rect = caretRect(), let window else { return .zero }
        return window.convertToScreen(convert(rect, to: nil))
    }

    // MARK: host access

    /// The current source text — for save / a source panel.
    public func sourceText() -> String { doc.source() }

    /// Clear the dirty flag after the host persisted `sourceText()` its own way.
    public func markSaved() { render(doc.markSaved()) }

    /// Run one of leaf-core's commands and repaint — for toolbar buttons.
    /// e.g. `view.command { $0.setHeading(level: 1) }`.
    public func command(_ op: (LeafDoc) -> DocView) { render(op(doc)) }
}
#endif
