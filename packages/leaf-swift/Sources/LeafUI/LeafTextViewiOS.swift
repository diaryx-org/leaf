//  LeafTextViewiOS.swift  (UIKit / iOS)
//
//  The iOS editing surface — the UIKit peer of the AppKit `LeafTextView`. Same
//  contract: core owns the model, the view owns the pixels. It draws each
//  already-wrapped `Row` directly and routes input back into core.
//
//  ## Native selection via UITextInput
//
//  This view conforms to `UITextInput` and installs a `UITextInteraction`, so the
//  *system* provides the real selection experience — the caret, the selection
//  highlight, draggable end handles, the magnifier loupe, double/triple-tap word &
//  block selection, and the Cut/Copy/Paste menu — all positioned through the
//  geometry this view answers. A `UITextPosition` here wraps a **source byte
//  offset**; the offset↔screen mapping, stepping, and range editing all delegate
//  to leaf-core over the FFI (`posForOffset` / `offsetForPos` / `stepOffset` /
//  `setSelectionOffsets` / `replaceRange`), so the projection model (WYSIWYG hides
//  markup; rows are soft-wrapped) stays the single source of truth. The view draws
//  only the text and code panels; the system overlays all selection UI.

#if canImport(UIKit)
import UIKit
import LeafFFI

// MARK: - Position / range value types

/// A document position: a source byte offset into leaf-core's buffer.
final class LeafTextPosition: UITextPosition {
    let offset: Int
    init(_ offset: Int) { self.offset = offset }
}

/// A position range, normalised so `start.offset <= end.offset`.
final class LeafTextRange: UITextRange {
    let from: LeafTextPosition
    let to: LeafTextPosition
    init(_ a: LeafTextPosition, _ b: LeafTextPosition) {
        if a.offset <= b.offset { from = a; to = b } else { from = b; to = a }
    }
    override var start: UITextPosition { from }
    override var end: UITextPosition { to }
    override var isEmpty: Bool { from.offset == to.offset }
}

/// One rect of a multi-line selection, tagged with whether it holds an endpoint
/// (so the system draws the start/end handles on the right rects).
final class LeafSelectionRect: UITextSelectionRect {
    private let _rect: CGRect
    private let _containsStart: Bool
    private let _containsEnd: Bool
    init(rect: CGRect, containsStart: Bool, containsEnd: Bool) {
        _rect = rect; _containsStart = containsStart; _containsEnd = containsEnd
    }
    override var rect: CGRect { _rect }
    override var writingDirection: NSWritingDirection { .leftToRight }
    override var containsStart: Bool { _containsStart }
    override var containsEnd: Bool { _containsEnd }
    override var isVertical: Bool { false }
}

// MARK: - The view

public final class LeafTextView: UIView, UITextInput {
    let doc: LeafDoc
    public var theme: EditorTheme {
        didSet {
            // Re-wrap only on a geometry change; a colour-only (or identical) theme
            // just repaints. Guarding this breaks the relayout⇄state-publish loop
            // that otherwise re-scrolled the view to the caret every frame.
            guard theme.metricsDiffer(from: oldValue) else { setNeedsDisplay(); return }
            shapeCache.removeAll(keepingCapacity: true)   // shaping is theme-dependent
            avgGlyphWidth = nil
            relayoutForWidth(force: true)
        }
    }
    public var onStateChange: ((EditorState) -> Void)?

    private var docView: DocView
    private var layoutEngine: EditorLayout
    private var wrapCols: Int = 0
    private var avgGlyphWidth: CGFloat?
    /// The caret offset the view last scrolled to reveal. Only a *move* re-scrolls,
    /// so passive reflows leave the reader's scroll position alone.
    private var lastCaretOffset: UInt32?
    /// Per-row shaped-text cache reused across frames; an edit re-shapes only the
    /// changed row(s). Cleared when the theme geometry changes (see `theme`).
    private var shapeCache: [Row: ShapedRow] = [:]

    // UITextInput plumbing.
    public weak var inputDelegate: UITextInputDelegate?
    public lazy var tokenizer: UITextInputTokenizer = UITextInputStringTokenizer(textInput: self)
    public var markedTextStyle: [NSAttributedString.Key: Any]?
    private var marked: LeafTextRange?
    private lazy var textInteraction: UITextInteraction = {
        let interaction = UITextInteraction(for: .editable)
        interaction.textInput = self
        return interaction
    }()

    public init(doc: LeafDoc, theme: EditorTheme = .default) {
        self.doc = doc
        self.theme = theme
        let first = doc.view()
        self.docView = first
        var seed: [Row: ShapedRow] = [:]
        self.layoutEngine = EditorLayout(first, theme: theme, cache: &seed)
        self.shapeCache = seed
        super.init(frame: .zero)
        backgroundColor = .clear
        contentMode = .redraw
        addInteraction(textInteraction)
        // Seed with the initial caret so the first reflow opens at the top.
        lastCaretOffset = doc.caretOffset()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    public override var canBecomeFirstResponder: Bool { true }
    public override var intrinsicContentSize: CGSize {
        CGSize(width: UIView.noIntrinsicMetric, height: layoutEngine.contentHeight)
    }

    private func off(_ p: UITextPosition) -> Int { (p as? LeafTextPosition)?.offset ?? 0 }

    // MARK: layout / wrap

    public override func layoutSubviews() {
        super.layoutSubviews()
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

    /// Install a fresh `DocView` and repaint. The system re-reads `selectedTextRange`
    /// and re-lays its selection overlays afterward.
    private func render(_ view: DocView) {
        docView = view
        layoutEngine = EditorLayout(view, theme: theme, cache: &shapeCache)
        invalidateIntrinsicContentSize()
        setNeedsDisplay()
        // Only follow the caret when it actually moved, not on a passive reflow.
        let caret = doc.caretOffset()
        if caret != lastCaretOffset {
            lastCaretOffset = caret
            scrollCaretToVisible()
        }
        onStateChange?(EditorState(view))
    }

    private func scrollCaretToVisible() {
        guard let caret = layoutEngine.caretRect(docView, theme: theme),
              let scroll = enclosingScrollView() else { return }
        scroll.scrollRectToVisible(convert(caret.insetBy(dx: 0, dy: -theme.lineHeight), to: scroll), animated: false)
    }

    private func enclosingScrollView() -> UIScrollView? {
        var v: UIView? = superview
        while let cur = v { if let s = cur as? UIScrollView { return s }; v = cur.superview }
        return nil
    }

    // MARK: drawing — text + code panels only; the system draws all selection UI

    public override func draw(_ rect: CGRect) {
        guard let ctx = UIGraphicsGetCurrentContext() else { return }
        let padX = theme.padding.left
        let fullWidth = bounds.width - theme.padding.left - theme.padding.right

        for rl in layoutEngine.rows {
            // Rows are laid out top-down, so cull to the dirty band: skip rows above
            // it, stop once past the bottom — repaint only the visible rows.
            if rl.top >= rect.maxY { break }
            if rl.top + rl.height <= rect.minY { continue }
            let rowRect = CGRect(x: padX, y: rl.top, width: fullWidth, height: rl.height)
            if rl.row.code {
                ctx.setFillColor(theme.codeBackground.cgColor)
                ctx.fill(rowRect.insetBy(dx: -4, dy: 0))
                if let lang = rl.row.codeLang, !lang.isEmpty { drawCodeLang(lang, in: rowRect) }
            }
            rl.attributed.draw(with: rowRect, options: [.usesLineFragmentOrigin], context: nil)
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

    // MARK: UIKeyInput — typing + backspace

    public var hasText: Bool { true }

    public func insertText(_ text: String) {
        if let m = marked {
            marked = nil
            render(doc.replaceRange(from: UInt32(m.from.offset), to: UInt32(m.to.offset), text: text))
        } else {
            render(text == "\n" ? doc.newline() : doc.insert(text: text))
        }
    }

    public func deleteBackward() {
        if let m = marked {
            marked = nil
            render(doc.replaceRange(from: UInt32(m.from.offset), to: UInt32(m.to.offset), text: ""))
        } else {
            render(doc.backspace())
        }
    }

    // MARK: hardware-keyboard formatting shortcuts (motion/selection handled by the
    // text-input system). Arrows, ⌘A/C/X/V come from UIKit for a UITextInput view.

    public override var keyCommands: [UIKeyCommand]? {
        let a = #selector(handleShortcut(_:))
        func k(_ input: String, _ mods: UIKeyModifierFlags) -> UIKeyCommand {
            UIKeyCommand(input: input, modifierFlags: mods, action: a)
        }
        return [k("b", .command), k("i", .command), k("u", .command),
                k("z", .command), k("z", [.command, .shift]),
                k("v", [.command, .shift])]
    }

    @objc private func handleShortcut(_ cmd: UIKeyCommand) {
        switch (cmd.input?.lowercased(), cmd.modifierFlags.contains(.shift)) {
        case ("b", _): command { $0.toggleBold() }
        case ("i", _): command { $0.toggleItalic() }
        case ("u", _): command { $0.toggleUnderline() }
        case ("z", false): command { $0.undo() }
        case ("z", true): command { $0.redo() }
        // ⇧⌘V — plain-flavor escape hatch: paste as leaf source, ignoring rich HTML.
        case ("v", true):
            let text = UIPasteboard.general.string ?? ""
            if !text.isEmpty { command { $0.paste(text: text) } }
        default: break
        }
    }

    // MARK: rich clipboard (edit-menu Cut/Copy/Paste keep twig's HTML flavour)

    public override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        switch action {
        case #selector(copy(_:)), #selector(cut(_:)): return docView.hasSelection
        case #selector(paste(_:)):                    return UIPasteboard.general.hasStrings
        case #selector(selectAll(_:)):                return true
        default: return super.canPerformAction(action, withSender: sender)
        }
    }

    public override func copy(_ sender: Any?) {
        guard let text = doc.selectedText() else { return }
        let pb = UIPasteboard.general
        if let html = doc.selectionHtml() {
            pb.items = [["public.utf8-plain-text": text, "public.html": html]]
        } else {
            pb.string = text
        }
    }

    public override func cut(_ sender: Any?) {
        copy(sender)
        if doc.selectedText() != nil { render(doc.backspace()) }
    }

    /// ⌘V: the rich flavor where the pasteboard has one, the plain flavor otherwise
    /// (mirrors leaf-tui / leaf-gpui / the macOS surface). HTML carries the
    /// formatting a `text/plain` copy out of another app has already lost; core
    /// falls back to the plain flavor when the HTML won't convert.
    public override func paste(_ sender: Any?) {
        let pb = UIPasteboard.general
        let html = pb.data(forPasteboardType: "public.html").flatMap { String(data: $0, encoding: .utf8) }
            ?? (pb.value(forPasteboardType: "public.html") as? String)
        let text = pb.string ?? ""
        guard html != nil || !text.isEmpty else { return }
        command { $0.pasteRich(html: html, text: text) }
    }

    public override func selectAll(_ sender: Any?) {
        notifyingDelegate { render(doc.selectAll()) }
    }

    // MARK: host access

    public func sourceText() -> String { doc.source() }
    public func markSaved() { render(doc.markSaved()) }

    /// Run a leaf-core command from a toolbar. Because this changes text/selection
    /// outside the text-input system, it brackets the change with input-delegate
    /// notifications so the system re-syncs its selection overlays.
    public func command(_ op: (LeafDoc) -> DocView) {
        notifyingDelegate { render(op(doc)) }
    }

    private func notifyingDelegate(_ body: () -> Void) {
        inputDelegate?.selectionWillChange(self)
        inputDelegate?.textWillChange(self)
        body()
        inputDelegate?.textDidChange(self)
        inputDelegate?.selectionDidChange(self)
    }

    // MARK: UITextInput — text & marked text

    public func text(in range: UITextRange) -> String? {
        guard let r = range as? LeafTextRange else { return nil }
        return doc.textInRange(from: UInt32(r.from.offset), to: UInt32(r.to.offset))
    }

    public func replace(_ range: UITextRange, withText text: String) {
        guard let r = range as? LeafTextRange else { return }
        render(doc.replaceRange(from: UInt32(r.from.offset), to: UInt32(r.to.offset), text: text))
    }

    public var selectedTextRange: UITextRange? {
        get {
            LeafTextRange(LeafTextPosition(Int(doc.anchorOffset())),
                          LeafTextPosition(Int(doc.caretOffset())))
        }
        set {
            guard let r = newValue as? LeafTextRange else { return }
            render(doc.setSelectionOffsets(anchor: UInt32(r.from.offset), focus: UInt32(r.to.offset)))
        }
    }

    public var markedTextRange: UITextRange? { marked }

    public func setMarkedText(_ markedText: String?, selectedRange: NSRange) {
        let text = markedText ?? ""
        let start: Int
        let end: Int
        if let m = marked {
            start = m.from.offset; end = m.to.offset
        } else {
            start = min(Int(doc.anchorOffset()), Int(doc.caretOffset()))
            end = max(Int(doc.anchorOffset()), Int(doc.caretOffset()))
        }
        render(doc.replaceRange(from: UInt32(start), to: UInt32(end), text: text))
        let newEnd = start + text.utf8.count
        marked = text.isEmpty ? nil : LeafTextRange(LeafTextPosition(start), LeafTextPosition(newEnd))
        render(doc.setSelectionOffsets(anchor: UInt32(newEnd), focus: UInt32(newEnd)))
    }

    public func unmarkText() { marked = nil }

    // MARK: UITextInput — positions & ranges

    public var beginningOfDocument: UITextPosition { LeafTextPosition(0) }
    public var endOfDocument: UITextPosition { LeafTextPosition(Int(doc.docEndOffset())) }

    public func textRange(from: UITextPosition, to toPosition: UITextPosition) -> UITextRange? {
        LeafTextRange(LeafTextPosition(off(from)), LeafTextPosition(off(toPosition)))
    }

    public func position(from position: UITextPosition, offset: Int) -> UITextPosition? {
        LeafTextPosition(Int(doc.stepOffset(off: UInt32(off(position)), delta: Int32(clamping: offset))))
    }

    public func position(from position: UITextPosition, in direction: UITextLayoutDirection, offset: Int) -> UITextPosition? {
        var o = off(position)
        switch direction {
        case .left:  o = Int(doc.stepOffset(off: UInt32(o), delta: Int32(clamping: -offset)))
        case .right: o = Int(doc.stepOffset(off: UInt32(o), delta: Int32(clamping: offset)))
        case .up:    for _ in 0..<max(0, offset) { if let v = doc.verticalOffset(off: UInt32(o), down: false) { o = Int(v) } else { break } }
        case .down:  for _ in 0..<max(0, offset) { if let v = doc.verticalOffset(off: UInt32(o), down: true) { o = Int(v) } else { break } }
        @unknown default: break
        }
        return LeafTextPosition(o)
    }

    public func compare(_ position: UITextPosition, to other: UITextPosition) -> ComparisonResult {
        let a = off(position), b = off(other)
        return a < b ? .orderedAscending : (a > b ? .orderedDescending : .orderedSame)
    }

    public func offset(from: UITextPosition, to toPosition: UITextPosition) -> Int {
        Int(doc.distanceOffset(from: UInt32(off(from)), to: UInt32(off(toPosition))))
    }

    public func position(within range: UITextRange, farthestIn direction: UITextLayoutDirection) -> UITextPosition? {
        switch direction {
        case .left, .up:    return range.start
        case .right, .down: return range.end
        @unknown default:   return range.start
        }
    }

    public func characterRange(byExtending position: UITextPosition, in direction: UITextLayoutDirection) -> UITextRange? {
        let o = off(position)
        switch direction {
        case .left, .up:
            return LeafTextRange(LeafTextPosition(Int(doc.stepOffset(off: UInt32(o), delta: -1))), LeafTextPosition(o))
        case .right, .down:
            return LeafTextRange(LeafTextPosition(o), LeafTextPosition(Int(doc.stepOffset(off: UInt32(o), delta: 1))))
        @unknown default:
            return nil
        }
    }

    // MARK: UITextInput — writing direction (LTR only)

    public func baseWritingDirection(for position: UITextPosition, in direction: UITextStorageDirection) -> NSWritingDirection { .leftToRight }
    public func setBaseWritingDirection(_ writingDirection: NSWritingDirection, for range: UITextRange) {}

    // MARK: UITextInput — geometry

    public func caretRect(for position: UITextPosition) -> CGRect {
        let rc = doc.posForOffset(off: UInt32(off(position)))
        return layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch), theme: theme) ?? .zero
    }

    public func selectionRects(for range: UITextRange) -> [UITextSelectionRect] {
        guard let r = range as? LeafTextRange else { return [] }
        let s = doc.posForOffset(off: UInt32(r.from.offset))
        let e = doc.posForOffset(off: UInt32(r.to.offset))
        let sRow = Int(s.row), sCh = Int(s.ch)
        let eRow = Int(e.row), eCh = Int(e.ch)
        guard eRow >= sRow else { return [] }

        var rects: [UITextSelectionRect] = []
        for row in sRow...eRow where layoutEngine.rows.indices.contains(row) {
            let rl = layoutEngine.rows[row]
            let len = rl.attributed.length
            let fromCh = (row == sRow) ? sCh : 0
            let toCh = (row == eRow) ? min(eCh, len) : len
            let x0 = CTLineGetOffsetForStringIndex(rl.line, CFIndex(min(fromCh, len)), nil)
            let x1 = CTLineGetOffsetForStringIndex(rl.line, CFIndex(toCh), nil)
            let rect = CGRect(x: theme.padding.left + x0, y: rl.top,
                              width: max(x1 - x0, row == eRow ? 0 : 2), height: rl.height)
            rects.append(LeafSelectionRect(rect: rect, containsStart: row == sRow, containsEnd: row == eRow))
        }
        return rects
    }

    public func firstRect(for range: UITextRange) -> CGRect {
        selectionRects(for: range).first?.rect ?? .zero
    }

    public func closestPosition(to point: CGPoint) -> UITextPosition? {
        let (row, ch) = layoutEngine.hit(point, theme: theme)
        return LeafTextPosition(Int(doc.offsetForPos(row: UInt32(row), ch: UInt32(ch))))
    }

    public func closestPosition(to point: CGPoint, within range: UITextRange) -> UITextPosition? {
        guard let p = closestPosition(to: point) else { return nil }
        return LeafTextPosition(min(max(off(p), off(range.start)), off(range.end)))
    }

    public func characterRange(at point: CGPoint) -> UITextRange? {
        guard let p = closestPosition(to: point) else { return nil }
        let o = off(p)
        return LeafTextRange(LeafTextPosition(o), LeafTextPosition(Int(doc.stepOffset(off: UInt32(o), delta: 1))))
    }
}
#endif
