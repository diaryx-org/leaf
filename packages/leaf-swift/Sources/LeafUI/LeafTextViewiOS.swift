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
    /// The host-set theme (base sizes). Internal layout uses `renderTheme`, which
    /// scales this to the user's Dynamic Type content size.
    public var theme: EditorTheme {
        get { hostTheme }
        set { hostTheme = newValue; applyDynamicType() }
    }
    private var hostTheme: EditorTheme
    private var renderTheme: EditorTheme

    /// Scale `hostTheme`'s type to the current Dynamic Type content size and relayout
    /// if the geometry changed. The `metricsDiffer` guard keeps a re-applied theme (or
    /// an unchanged content size) from relayouting — the loop-breaking invariant.
    private func applyDynamicType() {
        let old = renderTheme
        var t = hostTheme
        let factor = UIFontMetrics.default.scaledValue(for: 100, compatibleWith: traitCollection) / 100
        t.fontSize = hostTheme.fontSize * factor
        t.lineHeight = hostTheme.lineHeight * factor
        renderTheme = t
        guard renderTheme.metricsDiffer(from: old) else { setNeedsDisplay(); return }
        shapeCache.removeAll(keepingCapacity: true)
        relayoutForWidth(force: true)
    }

    public override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        if traitCollection.preferredContentSizeCategory != previous?.preferredContentSizeCategory {
            applyDynamicType()   // the user changed their text-size setting
        }
    }
    public var onStateChange: ((EditorState) -> Void)?

    private var docView: DocView
    private var layoutEngine: EditorLayout
    /// The pixel width rows wrap to (content width, minus insets). Core lays out
    /// unwrapped; the view soft-wraps at this width.
    private var wrapWidth: CGFloat = 0
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
        self.hostTheme = theme
        self.renderTheme = theme
        // Unwrapped layout (one row per block); the view soft-wraps at pixel width.
        let first = doc.setUnwrapped()
        self.docView = first
        var seed: [Row: ShapedRow] = [:]
        self.layoutEngine = EditorLayout(first, theme: renderTheme, wrapWidth: 0, cache: &seed)
        self.shapeCache = seed
        super.init(frame: .zero)
        backgroundColor = .clear
        contentMode = .redraw
        addInteraction(textInteraction)
        // Seed with the initial caret so the first reflow opens at the top.
        lastCaretOffset = doc.caretOffset()
        applyDynamicType()   // scale type to the current trait environment
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
        let w = bounds.width - renderTheme.padding.left - renderTheme.padding.right
        guard w > 0 else { return }
        if force || abs(w - wrapWidth) > 0.5 {
            wrapWidth = w
            // Re-wrap the current frame at the new pixel width — no round trip to core.
            render(docView)
        }
    }

    // MARK: applying a frame

    /// Install a fresh `DocView` and repaint. The system re-reads `selectedTextRange`
    /// and re-lays its selection overlays afterward.
    private func render(_ view: DocView) {
        docView = view
        layoutEngine = EditorLayout(view, theme: renderTheme, wrapWidth: wrapWidth, cache: &shapeCache)
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
        guard let caret = layoutEngine.caretRect(docView, theme: renderTheme),
              let scroll = enclosingScrollView() else { return }
        scroll.scrollRectToVisible(convert(caret.insetBy(dx: 0, dy: -renderTheme.lineHeight), to: scroll), animated: false)
    }

    private func enclosingScrollView() -> UIScrollView? {
        var v: UIView? = superview
        while let cur = v { if let s = cur as? UIScrollView { return s }; v = cur.superview }
        return nil
    }

    // MARK: drawing — text + code panels only; the system draws all selection UI

    public override func draw(_ rect: CGRect) {
        guard let ctx = UIGraphicsGetCurrentContext() else { return }
        let padX = renderTheme.padding.left
        let fullWidth = bounds.width - renderTheme.padding.left - renderTheme.padding.right

        drawDirectiveBorders(in: ctx, dirtyRect: rect)

        for rl in layoutEngine.rows {
            // Rows are laid out top-down, so cull to the dirty band: skip rows above
            // it, stop once past the bottom — repaint only the visible rows.
            if rl.top >= rect.maxY { break }
            if rl.top + rl.height <= rect.minY { continue }
            // A table draws its own grid (once, on its first picture row).
            if let grid = rl.table {
                if rl.tableFirst { drawTable(grid, tableTop: rl.tableTop, in: ctx) }
                continue
            }
            let rowRect = CGRect(x: padX, y: rl.top, width: fullWidth, height: rl.height)
            if rl.row.directive, let label = rl.row.directiveLabel, !label.isEmpty {
                drawDirectiveLabel(label, in: rowRect)
            }
            if rl.row.code {
                ctx.setFillColor(renderTheme.codeBackground.cgColor)
                ctx.fill(rowRect.insetBy(dx: -4, dy: 0))
                if let lang = rl.row.codeLang, !lang.isEmpty { drawCodeLang(lang, in: rowRect) }
            }
            // Draw each wrapped visual line's substring on its own line box.
            for (i, wl) in rl.wrapped.enumerated() {
                let lineTop = rl.top + rl.labelInset + CGFloat(i) * rl.lineHeight
                if lineTop >= rect.maxY { break }
                if lineTop + rl.lineHeight <= rect.minY { continue }
                wl.attributed.draw(with: CGRect(x: padX, y: lineTop, width: fullWidth, height: rl.lineHeight),
                                   options: [.usesLineFragmentOrigin], context: nil)
            }
        }
    }

    /// Draw a table as a proportional grid — header fill and body stripes, cell
    /// text, then the grid rules — the UIKit peer of the AppKit `drawTable`.
    private func drawTable(_ grid: TableLayout, tableTop: CGFloat, in ctx: CGContext) {
        let left = renderTheme.padding.left
        let border = TableMetrics.border
        let x0 = left + (grid.colX.first ?? 0)
        let x1 = left + (grid.colX.last ?? 0)

        var body = 0
        for row in grid.rows {
            let bg: LeafColor?
            if row.head {
                bg = renderTheme.tableHeaderColor
            } else {
                body += 1
                bg = body % 2 == 0 ? renderTheme.tableStripeColor : nil
            }
            if let bg {
                ctx.setFillColor(bg.cgColor)
                ctx.fill(CGRect(x: x0, y: tableTop + row.top, width: x1 - x0, height: row.height))
            }
        }
        for row in grid.rows {
            let top = tableTop + row.top + TableMetrics.padY
            for cell in row.cells {
                for (i, line) in cell.lines.enumerated() {
                    line.attributed.draw(
                        with: CGRect(x: left + line.textX,
                                     y: top + CGFloat(i) * grid.lineHeight,
                                     width: .greatestFiniteMagnitude, height: renderTheme.lineHeight),
                        options: [.usesLineFragmentOrigin], context: nil)
                }
            }
        }
        ctx.setFillColor(renderTheme.tableBorderColor.cgColor)
        for bx in grid.colX {
            ctx.fill(CGRect(x: left + bx, y: tableTop, width: border, height: grid.height))
        }
        var edgeYs = [tableTop]
        for row in grid.rows { edgeYs.append(tableTop + row.top + row.height) }
        for ey in edgeYs {
            ctx.fill(CGRect(x: x0, y: min(ey, tableTop + grid.height - border),
                            width: x1 - x0 + border, height: border))
        }
    }

    private func drawCodeLang(_ lang: String, in rowRect: CGRect) {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: renderTheme.monospaceFont(size: renderTheme.fontSize * 0.75, bold: false, italic: false),
            .foregroundColor: renderTheme.secondaryColor,
        ]
        let s = lang as NSString
        let size = s.size(withAttributes: attrs)
        s.draw(at: CGPoint(x: rowRect.maxX - size.width - 2, y: rowRect.minY + 1), withAttributes: attrs)
    }

    /// A directive container's `.class` label, top-left of its first row — the
    /// UIKit peer of the AppKit `drawDirectiveLabel`.
    private func drawDirectiveLabel(_ label: String, in rowRect: CGRect) {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: renderTheme.proportionalFont(size: renderTheme.fontSize * 0.75, bold: false, italic: false),
            .foregroundColor: renderTheme.secondaryColor,
        ]
        (label as NSString).draw(at: CGPoint(x: rowRect.minX + 2, y: rowRect.minY + 1), withAttributes: attrs)
    }

    /// One dashed outline per maximal run of consecutive `directive` rows — the
    /// UIKit peer of the AppKit `drawDirectiveBorders`.
    private func drawDirectiveBorders(in ctx: CGContext, dirtyRect: CGRect) {
        let padX = renderTheme.padding.left
        let fullWidth = bounds.width - renderTheme.padding.left - renderTheme.padding.right
        let rows = layoutEngine.rows
        var i = 0
        while i < rows.count {
            guard rows[i].row.directive, rows[i].table == nil else { i += 1; continue }
            let start = i
            while i < rows.count, rows[i].row.directive, rows[i].table == nil { i += 1 }
            let first = rows[start], last = rows[i - 1]
            let rect = CGRect(x: padX - 4, y: first.top,
                              width: fullWidth + 8, height: last.top + last.height - first.top)
            if rect.maxY < dirtyRect.minY || rect.minY > dirtyRect.maxY { continue }
            ctx.saveGState()
            ctx.setStrokeColor(renderTheme.directiveBorderColor.cgColor)
            ctx.setLineWidth(1)
            ctx.setLineDash(phase: 0, lengths: [3, 3])
            ctx.addPath(CGPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5),
                               cornerWidth: 6, cornerHeight: 6, transform: nil))
            ctx.strokePath()
            ctx.restoreGState()
        }
    }

    // MARK: UIKeyInput — typing + backspace

    public var hasText: Bool { true }

    public func insertText(_ text: String) {
        if let m = marked {
            marked = nil
            render(doc.replaceRange(from: UInt32(m.from.offset), to: UInt32(m.to.offset), text: text))
        } else if text == "\n" {
            // In a table, Return drops a cell; elsewhere it's a newline.
            render(doc.cellReturn() ?? doc.newline())
        } else if text == "\t" {
            // In a table, Tab walks the cells; elsewhere it indents (nesting a
            // list item under its sibling — the core picks the step).
            render(doc.cellTab(forward: true) ?? doc.indent())
        } else {
            render(doc.insert(text: text))
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
                k("v", [.command, .shift]),
                // Shift+Tab: plain Tab arrives through `insertText("\t")`, but the
                // shifted chord doesn't — capture it here to outdent (walk a cell
                // back in a table, unnest a list item otherwise).
                k("\t", .shift),
                // Shift+Return: plain Return arrives through `insertText("\n")`, but
                // the shifted chord doesn't — capture it here for the in-cell line
                // break (an ordinary newline off a table).
                k("\r", .shift)]
    }

    @objc private func handleShortcut(_ cmd: UIKeyCommand) {
        switch (cmd.input?.lowercased(), cmd.modifierFlags.contains(.shift)) {
        case ("b", _): command { $0.toggleBold() }
        case ("i", _): command { $0.toggleItalic() }
        case ("u", _): command { $0.toggleUnderline() }
        case ("\t", true): command { $0.cellTab(forward: false) ?? $0.outdent() }
        case ("\r", true): command { $0.cellLineBreak() ?? $0.newline() }
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
        // ↑/↓ ride the *visual* wrap: probe one line-height past the caret and hit-test,
        // rather than core's paragraph rows (unwrapped map).
        case .up:    o = visualStep(from: o, up: true, times: offset)
        case .down:  o = visualStep(from: o, up: false, times: offset)
        @unknown default: break
        }
        return LeafTextPosition(o)
    }

    /// Move `times` visual lines up/down from source offset `o`, returning the new
    /// offset. Mirrors the AppKit peer's visual-line motion, in offset terms.
    private func visualStep(from o: Int, up: Bool, times: Int) -> Int {
        var cur = o
        for _ in 0..<max(0, times) {
            let rc = doc.posForOffset(off: UInt32(cur))
            guard let caret = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch), theme: renderTheme) else { break }
            // Probe from the caret's full line band (a table cell's padding is
            // cleared) and resolve the table-aware way, or a probe into a table
            // teleports to its top-left cell. See the AppKit peer's `moveVertical`.
            let band = layoutEngine.caretBand(src: cur)
            var probeY = up ? (band?.minY ?? caret.minY) - 1 : (band?.maxY ?? caret.maxY) + 1
            let probe = CGPoint(x: caret.minX, y: probeY)
            let next: Int
            if let off = layoutEngine.tableHitOffset(probe, theme: renderTheme) {
                next = Int(doc.snapOffset(off: UInt32(off)))
            } else {
                var (row, ch) = layoutEngine.hit(probe, theme: renderTheme)
                // Step over the short blank gap row a block boundary is drawn with:
                // probing one line past the caret lands inside it, where the hit
                // snaps back and the step stalls between a paragraph and the list or
                // code block below. See the AppKit peer's `moveVertical`.
                let rows = layoutEngine.rows
                var guardCount = 0
                while rows.indices.contains(row), rows[row].row.isBlockGap, guardCount < rows.count {
                    let r = rows[row]
                    probeY = up ? r.top - 1 : r.top + r.height + 1
                    (row, ch) = layoutEngine.hit(CGPoint(x: caret.minX, y: probeY), theme: renderTheme)
                    guardCount += 1
                }
                next = Int(doc.offsetForPos(row: UInt32(row), ch: UInt32(ch)))
            }
            if next == cur { break }
            cur = next
        }
        return cur
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
        return layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch), theme: renderTheme) ?? .zero
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
            let rowFrom = (row == sRow) ? sCh : 0
            let rowTo = (row == eRow) ? min(eCh, rl.attributed.length) : rl.attributed.length
            // One rect per visual line the selection touches in this block.
            for (i, wl) in rl.wrapped.enumerated() {
                let lineStart = wl.start, lineEnd = wl.start + wl.length
                let cs = max(rowFrom, lineStart), ce = min(rowTo, lineEnd)
                guard cs < ce else { continue }
                let x0 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(cs - lineStart), nil)
                let x1 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(ce - lineStart), nil)
                let rect = CGRect(x: renderTheme.padding.left + x0, y: rl.top + rl.labelInset + CGFloat(i) * rl.lineHeight,
                                  width: x1 - x0, height: rl.lineHeight)
                rects.append(LeafSelectionRect(rect: rect,
                                               containsStart: row == sRow && cs == sCh,
                                               containsEnd: row == eRow && ce == eCh))
            }
        }
        // Tables carry no `wrapped` lines, so the row walk above skips them; add
        // the highlight over any table cells the range covers, keyed by source
        // offset (the coordinate a cell is laid out by).
        rects.append(contentsOf: layoutEngine.tableSelectionRects(
            from: r.from.offset, to: r.to.offset, theme: renderTheme
        ).map { LeafSelectionRect(rect: $0.rect, containsStart: $0.containsStart, containsEnd: $0.containsEnd) })
        return rects
    }

    public func firstRect(for range: UITextRange) -> CGRect {
        selectionRects(for: range).first?.rect ?? .zero
    }

    public func closestPosition(to point: CGPoint) -> UITextPosition? {
        // Inside a table, the point maps through the grid straight to a source
        // offset; elsewhere it's the plain row/ch hit-test.
        if let off = layoutEngine.tableHitOffset(point, theme: renderTheme) {
            return LeafTextPosition(off)
        }
        let (row, ch) = layoutEngine.hit(point, theme: renderTheme)
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
