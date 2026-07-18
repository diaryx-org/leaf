//  LeafTextView.swift  (AppKit / macOS)
//
//  The macOS editing surface — the peer of leaf-wasm's `LeafEditor`. It owns
//  presentation and input, never the model; `LeafDoc` (leaf-core, over the FFI)
//  stays the single source of truth. Each already-wrapped `Row` is drawn directly
//  (no NSTextView/NSLayoutManager), the caret is placed at `caret_ch`, and every
//  key/mouse intent routes back into core, which edits and returns the next frame.
//  Shared geometry lives in `EditorLayout`; the UIKit peer is `LeafTextView` in
//  `LeafTextViewiOS.swift`.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import LeafFFI

public final class LeafTextView: NSView, NSTextInputClient {
    let doc: LeafDoc
    public var theme: EditorTheme { didSet { avgGlyphWidth = nil; relayoutForWidth(force: true) } }
    /// Fired after every repaint so a host can update a toolbar/footer.
    public var onStateChange: ((EditorState) -> Void)?

    private var docView: DocView
    private var layoutEngine: EditorLayout
    private var wrapCols: Int = 0
    private var avgGlyphWidth: CGFloat?

    private var caretVisible = true
    private var blinkTimer: Timer?
    private var isFocused = false

    public init(doc: LeafDoc, theme: EditorTheme = .default) {
        self.doc = doc
        self.theme = theme
        let first = doc.view()
        self.docView = first
        self.layoutEngine = EditorLayout(first, theme: theme)
        super.init(frame: .zero)
        autoresizingMask = [.width]
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
        let cols = columnsForWidth()
        guard cols > 0 else { return }
        if force || cols != wrapCols {
            wrapCols = cols
            render(doc.setWidth(cols: UInt32(cols)))
        }
    }

    private func columnsForWidth() -> Int {
        let avail = bounds.width - theme.padding.left - theme.padding.right
        let avg = glyphWidth()
        guard avail > 0, avg > 0 else { return 0 }
        return max(1, Int(avail / avg))
    }

    private func glyphWidth() -> CGFloat {
        if let w = avgGlyphWidth { return w }
        let sample = "the quick brown fox jumps over the lazy dog " as NSString
        let font = theme.proportionalFont(size: theme.fontSize, bold: false, italic: false)
        let width = sample.size(withAttributes: [.font: font]).width / CGFloat(sample.length)
        avgGlyphWidth = width > 0 ? width : theme.fontSize * 0.5
        return avgGlyphWidth!
    }

    // MARK: applying a frame

    private func render(_ view: DocView) {
        docView = view
        layoutEngine = EditorLayout(view, theme: theme)
        let h = layoutEngine.contentHeight
        if abs(frame.height - h) > 0.5 { setFrameSize(NSSize(width: frame.width, height: h)) }
        needsDisplay = true
        resetBlink()
        scrollCaretToVisible()
        onStateChange?(EditorState(view: view.view, dirty: view.dirty, heading: view.heading, active: view.active))
    }

    // MARK: drawing

    public override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let padX = theme.padding.left
        let fullWidth = bounds.width - theme.padding.left - theme.padding.right

        for rl in layoutEngine.rows {
            let rowRect = CGRect(x: padX, y: rl.top, width: fullWidth, height: rl.height)
            if rl.row.code {
                ctx.setFillColor(theme.codeBackground.cgColor)
                ctx.fill(rowRect.insetBy(dx: -4, dy: 0))
                if let lang = rl.row.codeLang, !lang.isEmpty { drawCodeLang(lang, in: rowRect) }
            }
            layoutEngine.fillSelection(row: rl, padLeft: padX, color: theme.selectionColor, in: ctx)
            rl.attributed.draw(with: rowRect, options: [.usesLineFragmentOrigin])
        }

        if isFocused, caretVisible, let rect = layoutEngine.caretRect(docView, theme: theme) {
            ctx.setFillColor(theme.caretColor.cgColor)
            ctx.fill(rect)
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

    private func scrollCaretToVisible() {
        if let rect = layoutEngine.caretRect(docView, theme: theme) {
            scrollToVisible(rect.insetBy(dx: 0, dy: -theme.lineHeight))
        }
    }

    // MARK: mouse

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let p = convert(event.locationInWindow, from: nil)
        let (row, ch) = layoutEngine.hit(p, theme: theme)
        let extend = event.modifierFlags.contains(.shift)
        switch event.clickCount {
        case 2:  render(doc.selectWordCh(row: UInt32(row), ch: UInt32(ch)))
        case 3:  render(doc.selectBlockCh(row: UInt32(row), ch: UInt32(ch)))
        default: render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: extend))
        }
    }

    public override func mouseDragged(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let (row, ch) = layoutEngine.hit(p, theme: theme)
        render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: true))
    }

    // MARK: keyboard — text + IME

    public override func keyDown(with event: NSEvent) {
        if !(inputContext?.handleEvent(event) ?? false) { interpretKeyEvents([event]) }
    }

    public func insertText(_ string: Any, replacementRange: NSRange) {
        let text = (string as? String) ?? (string as? NSAttributedString)?.string ?? ""
        guard !text.isEmpty else { return }
        render(doc.insert(text: text))
    }

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
        case #selector(insertNewline(_:)), #selector(insertLineBreak(_:)): render(doc.newline())
        case #selector(insertTab(_:)):                      render(doc.insert(text: "  "))
        case #selector(deleteBackward(_:)):                 render(doc.backspace())
        case #selector(deleteForward(_:)):                  render(doc.deleteForward())
        case #selector(deleteWordBackward(_:)):             render(doc.deleteWordBack())
        case #selector(deleteWordForward(_:)):              render(doc.deleteWordForward())
        default: super.doCommand(by: selector)
        }
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
        case "v": paste(nil); return true
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

    @objc public override func selectAll(_ sender: Any?) { render(doc.selectAll()) }

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

    // MARK: NSTextInputClient — minimal (committed text + candidate placement)

    public func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {}
    public func unmarkText() {}
    public func hasMarkedText() -> Bool { false }
    public func markedRange() -> NSRange { NSRange(location: NSNotFound, length: 0) }
    public func selectedRange() -> NSRange { NSRange(location: NSNotFound, length: 0) }
    public func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }
    public func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? { nil }
    public func characterIndex(for point: NSPoint) -> Int { NSNotFound }
    public func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let rect = layoutEngine.caretRect(docView, theme: theme), let window else { return .zero }
        return window.convertToScreen(convert(rect, to: nil))
    }

    // MARK: host access

    public func sourceText() -> String { doc.source() }
    public func markSaved() { render(doc.markSaved()) }
    public func command(_ op: (LeafDoc) -> DocView) { render(op(doc)) }
}
#endif
