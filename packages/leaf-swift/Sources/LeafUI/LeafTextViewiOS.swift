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

// MARK: - Tokenizer

/// What the system asks about text units. `UITextInputStringTokenizer` answers
/// characters, words, sentences and paragraphs by reading `text(in:)`, and by
/// its own documentation cannot answer *lines*: a line is a fact about layout,
/// which the base class has none of, so it says "no" to every line question.
///
/// That silence moved carets. `UITextInteraction` places a tap that lands at a
/// word's end one position further on — past the space that follows the word,
/// which is where the stock text view puts it too — *unless* the tokenizer says
/// the position is a line end. With no line ends anywhere, every paragraph's
/// last word and every table cell's last word were word ends and nothing more,
/// so a tap past "iOS." landed at the start of the next heading, and a tap past
/// "editable" landed in the next row's first cell. This answers lines from the
/// layout, so those taps stay where they landed.
final class LeafTokenizer: UITextInputStringTokenizer {
    private unowned let view: LeafTextView

    init(view: LeafTextView) {
        self.view = view
        super.init(textInput: view)
    }

    /// `UITextDirection` is a storage direction (`.forward`/`.backward`) or a
    /// layout one (`.right`/`.left`/`.up`/`.down`) behind one raw value; reading
    /// on is forward, right, or down.
    private func reads(on direction: UITextDirection) -> Bool {
        switch direction.rawValue {
        case UITextStorageDirection.forward.rawValue,
             UITextLayoutDirection.right.rawValue,
             UITextLayoutDirection.down.rawValue:
            return true
        default:
            return false
        }
    }

    private func line(at position: UITextPosition) -> (start: Int, end: Int, continues: Bool)? {
        guard let o = (position as? LeafTextPosition)?.offset else { return nil }
        return view.visualLineBounds(at: o)
    }

    override func isPosition(_ position: UITextPosition, atBoundary granularity: UITextGranularity,
                             inDirection direction: UITextDirection) -> Bool {
        guard granularity == .line else {
            return super.isPosition(position, atBoundary: granularity, inDirection: direction)
        }
        guard let o = (position as? LeafTextPosition)?.offset, let line = line(at: position) else { return false }
        // A soft wrap is one offset that ends a line and starts the next; it is a
        // boundary read either way.
        return reads(on: direction)
            ? o == line.end || (o == line.start && line.continues)
            : o == line.start
    }

    override func position(from position: UITextPosition, toBoundary granularity: UITextGranularity,
                           inDirection direction: UITextDirection) -> UITextPosition? {
        guard granularity == .line else {
            return super.position(from: position, toBoundary: granularity, inDirection: direction)
        }
        guard let line = line(at: position) else { return nil }
        return LeafTextPosition(reads(on: direction) ? line.end : line.start)
    }

    override func rangeEnclosingPosition(_ position: UITextPosition, with granularity: UITextGranularity,
                                         inDirection direction: UITextDirection) -> UITextRange? {
        guard granularity == .line else {
            return super.rangeEnclosingPosition(position, with: granularity, inDirection: direction)
        }
        guard let line = line(at: position) else { return nil }
        return LeafTextRange(LeafTextPosition(line.start), LeafTextPosition(line.end))
    }

    override func isPosition(_ position: UITextPosition, withinTextUnit granularity: UITextGranularity,
                             inDirection direction: UITextDirection) -> Bool {
        guard granularity == .line else {
            return super.isPosition(position, withinTextUnit: granularity, inDirection: direction)
        }
        guard let o = (position as? LeafTextPosition)?.offset, let line = line(at: position) else { return false }
        return reads(on: direction) ? o < line.end : o > line.start
    }
}

// MARK: - The view

public final class LeafTextView: UIView, UITextInput {
    let doc: LeafDoc
    /// The host-set theme (base sizes). Internal layout uses `renderTheme`, which
    /// in the continuous flow scales this to the user's Dynamic Type content size,
    /// and on paper is this unchanged — see `typeZoom`.
    public var theme: EditorTheme {
        get { hostTheme }
        set { hostTheme = newValue; applyDynamicType() }
    }
    private var hostTheme: EditorTheme
    private(set) var renderTheme: EditorTheme

    /// The part of the reader's Dynamic Type factor the zoom carries: all of it
    /// on paper, none of it in the continuous flow, where `renderTheme` carries
    /// it instead.
    ///
    /// Bigger type in the flow is bigger type: the column reflows. On a sheet it
    /// would be a different document — other line breaks, another page count,
    /// and not the one `pdfData(page:)` prints, which is at the theme's stated
    /// size. So on paper the sheet keeps the host's type and the setting reaches
    /// it the way it reaches a PDF in Preview, by making the sheet larger on
    /// screen: every zoom resolves to its usual scale times this. See `zoom`.
    private(set) var typeZoom: CGFloat = 1

    /// The Dynamic Type factor for the current content size, `1` at the default.
    private var dynamicTypeFactor: CGFloat {
        UIFontMetrics.default.scaledValue(for: 100, compatibleWith: traitCollection) / 100
    }

    /// Give the Dynamic Type factor to whichever of the theme and the zoom
    /// carries it here — the theme in the flow, the zoom on paper — and nothing
    /// else: the caller relayouts or re-resolves as the change needs.
    private func distributeDynamicType() {
        let factor = dynamicTypeFactor
        let onPaper = pageSetup != nil
        renderTheme = onPaper ? hostTheme : hostTheme.scaled(by: factor)
        // A PDF's sheet is never on a screen, so has no text size to show.
        typeZoom = onPaper && !isPaper ? factor : 1
    }

    /// Apply the current Dynamic Type content size (or a new host theme): in the
    /// flow, scale the type and relayout if the geometry changed; on paper,
    /// leave the type and re-resolve the zoom, which is a redraw and not a
    /// relayout. The `metricsDiffer` guard keeps a re-applied theme (or an
    /// unchanged content size) from relayouting — the loop-breaking invariant.
    ///
    /// Which lengths the factor reaches is `EditorTheme.scaled(by:)`'s answer, not
    /// this method's: the same question comes up wherever a theme is resized, and
    /// only the theme knows which of its numbers are typography.
    private func applyDynamicType() {
        let old = renderTheme
        let oldTypeZoom = typeZoom
        distributeDynamicType()
        if typeZoom != oldTypeZoom { reresolveZoom() }
        guard renderTheme.metricsDiffer(from: old) else { setNeedsDisplay(); return }
        shapeCache.removeAll(keepingCapacity: true)
        relayoutForWidth(force: true)
    }

    public override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        if traitCollection.preferredContentSizeCategory != previous?.preferredContentSizeCategory {
            applyDynamicType()   // the user changed their text-size setting
        } else if traitCollection.hasDifferentColorAppearance(comparedTo: previous) {
            // A formula's picture is set in the ink the layout resolved (see
            // `render`), so light to dark is a relayout, not just a repaint.
            shapeCache.removeAll(keepingCapacity: true)
            relayoutForWidth(force: true)
        }
    }
    public var onStateChange: ((EditorState) -> Void)?
    /// Fired after a repaint that laid the frame out again — its rows changed,
    /// by an edit or a selection, or the geometry under them did, by a width
    /// or a page — and not after one that only moved the caret. What a count
    /// hangs on: a caret move changed no count, and a recount of a long
    /// document is not free. See `LeafEditorModel.scheduleCounts`.
    public var onLayoutChange: (() -> Void)?
    /// Fired after a repaint whose text is not the last one's — an edit, an
    /// undo, a paste — and not after a caret step, a reflow or a view toggle.
    /// See `LeafEditorModel.onEdit`.
    public var onEdit: (() -> Void)?

    /// The sheet this document is laid onto, or `nil` (the default) for the
    /// continuous scrolling flow — the same mode the AppKit peer offers, and the
    /// same `PageSetup` describes it. On a phone it is mostly the shape a PDF
    /// takes (`pdfData(page:)`), but it is a view mode here too: rows break
    /// across a stack of sheets and wrap to the sheet's margins instead of the
    /// theme's `measure`. A sheet is a fixed width, so a host that puts one on
    /// screen gives its scroll view the stack's width rather than the
    /// viewport's.
    public var pageSetup: PageSetup? {
        didSet {
            guard pageSetup != oldValue else { return }
            // The column width changed, and the shape cache is only valid at the
            // width it was built for.
            shapeCache.removeAll(keepingCapacity: true)
            // Onto paper the Dynamic Type factor moves from the type into the
            // zoom, and off it back again — see `typeZoom`.
            distributeDynamicType()
            // A fit is a rule about the sheet, and the sheet just changed (or
            // went away, which makes a fit the identity). Resolve it over the
            // new one before the layout that will draw it.
            let scale = resolvedScale(zoomMode)
            let rescaled = scale != zoomScale
            if rescaled { applyZoomScale(scale, anchor: nil, sharpen: true, settle: false) }
            relayoutForWidth(force: true)
            zoomHost?.invalidateIntrinsicContentSize()
            if rescaled { onZoomChange?(zoomMode, zoomScale) }
        }
    }

    // MARK: zoom

    /// How large the document is on screen — see `Zoom`, and the AppKit peer's
    /// `zoom`, which this is the same value as. `zoomScale` is what it currently
    /// resolves to.
    ///
    /// On paper, every zoom is multiplied by the reader's Dynamic Type factor
    /// (`typeZoom`): `.fitWidth` at an accessibility size is the fit times that
    /// size's factor, a pinch moves on from there, and `zoomScale` is the
    /// product — what a "125%" label should say on that phone. The product is
    /// held to `Zoom.range` like any zoom and has no cap of its own: a fit-width
    /// sheet at the largest sizes is scrolled sideways, as a Larger Text reader's
    /// PDF is in Preview. A `.scale` here is the zoom *before* the factor, so
    /// the same scale follows the reader's text size, and one the view reports
    /// back (after a pinch, or a clamp) is `zoomScale` divided by it. In the
    /// continuous flow the factor is in the type instead, and a zoom is as asked.
    ///
    /// Applied as the view's own `transform`, not as a scale inside `draw` as
    /// the AppKit peer does it: UIKit converts touches, the system's selection
    /// geometry (`UITextInput`'s rects go out through the view hierarchy), the
    /// players' frames and the peek's anchors through a transform on its own, so
    /// nothing in this file learns the scale exists — where the AppKit view has
    /// no system selection to keep in step and divides its own clicks out. The
    /// one thing the transform alone would not do is keep the glyphs sharp — a
    /// scaled layer is a scaled bitmap — so the backing store is rendered at
    /// the zoomed resolution (`contentScaleFactor`), once a pinch has ended
    /// rather than at every frame of it.
    ///
    /// Auto Layout does not read transforms, so the scroll view is given the
    /// scaled size by `LeafZoomView`, which this view sits in — `LeafEditor`
    /// does that, and a UIKit host embedding this view itself does the same.
    public var zoom: Zoom {
        get { zoomMode }
        set { setZoom(newValue, anchor: nil) }
    }
    private var zoomMode: Zoom = .actualSize

    /// The scale `zoom` resolves to on this viewport, `1` being one layout point
    /// per screen point.
    public private(set) var zoomScale: CGFloat = 1

    /// Fired when `zoom` or `zoomScale` changes for any reason — a pinch, a
    /// resize under a fit, a page set or cleared — so a host that owns the zoom
    /// (the model does) learns what the surface decided.
    public var onZoomChange: ((Zoom, CGFloat) -> Void)?

    /// Set the zoom, keeping the layout under `anchor` (a point in this view's
    /// coordinates) at the same place on screen — the point under a pinch, or,
    /// for `nil`, the centre of what is visible.
    public func setZoom(_ zoom: Zoom, anchor: CGPoint? = nil) {
        let scale = resolvedScale(zoom)
        // A scale past the range is held at its edge as a mode too, so what is
        // reported back is the scale the view is at, not the one it was asked
        // — before the Dynamic Type factor, which a `.scale` does not include.
        let zoom = zoom.isFit ? zoom : .scale(scale / typeZoom)
        let changed = zoom != zoomMode || scale != zoomScale
        zoomMode = zoom
        if scale != zoomScale { applyZoomScale(scale, anchor: anchor, sharpen: true, settle: true) }
        if changed { onZoomChange?(zoomMode, zoomScale) }
    }

    /// The wrapper that gives Auto Layout the scaled size, when there is one.
    var zoomHost: LeafZoomView? { superview as? LeafZoomView }

    /// Re-resolve a fit against the viewport as it is now — the wrapper calls
    /// this from its layout, so a rotation under a fit re-fits. The top-left
    /// of what is visible stays put.
    func refitZoom() {
        guard zoomMode.isFit else { return }
        let scale = resolvedScale(zoomMode)
        guard scale != zoomScale else { return }
        let anchor = enclosingScrollView().map { convert($0.bounds.origin, from: $0) }
        applyZoomScale(scale, anchor: anchor, sharpen: true, settle: false)
        onZoomChange?(zoomMode, zoomScale)
    }

    /// What `zoom` comes to on this viewport, over this page, at the reader's
    /// text size.
    private func resolvedScale(_ zoom: Zoom) -> CGFloat {
        zoom.resolve(in: viewportSize, page: pageSetup, factor: typeZoom)
    }

    /// Re-resolve the zoom after the factor it is multiplied by changed — the
    /// reader's text size, on paper. The top-left of what is visible stays put,
    /// as it does under a re-fit.
    private func reresolveZoom() {
        let scale = resolvedScale(zoomMode)
        guard scale != zoomScale else { return }
        let anchor = enclosingScrollView().map { convert($0.bounds.origin, from: $0) }
        applyZoomScale(scale, anchor: anchor, sharpen: true, settle: false)
        onZoomChange?(zoomMode, zoomScale)
    }

    /// The viewport in screen points: the scroll view's bounds less the bars it
    /// runs under. Not less the keyboard — that is a content inset, and a fit
    /// that shrank every time the keyboard rose would zoom the page on every
    /// tap into it.
    private var viewportSize: CGSize {
        guard let scroll = enclosingScrollView() else { return bounds.size }
        let safe = scroll.safeAreaInsets
        return CGSize(width: scroll.bounds.width - safe.left - safe.right,
                      height: scroll.bounds.height - safe.top - safe.bottom)
    }

    /// Move to `scale`, holding the layout point under `anchor` (this view's
    /// coordinates; nil for the viewport's centre) at the same place in the
    /// viewport. The scroll offset moves by `p · (s′ − s)` for that point `p`,
    /// as the AppKit peer's `applyZoomScale` derives.
    ///
    /// `sharpen` re-renders the backing store at the new resolution, which a
    /// pinch defers to its end. `settle` lays the scroll view out first so the
    /// offset is clamped against the real content size; from inside a layout
    /// pass (a re-fit) it is estimated instead, since laying out from within
    /// layout is the loop UIKit warns about.
    private func applyZoomScale(_ scale: CGFloat, anchor: CGPoint?, sharpen: Bool, settle: Bool) {
        let scroll = enclosingScrollView()
        let p = anchor ?? scroll.map { convert(CGPoint(x: $0.bounds.midX, y: $0.bounds.midY), from: $0) }
            ?? CGPoint(x: bounds.midX, y: bounds.midY)
        let delta = scale - zoomScale
        zoomScale = scale
        transform = CGAffineTransform(scaleX: scale, y: scale)
        if sharpen { sharpenBackingStore() }
        zoomHost?.invalidateIntrinsicContentSize()
        zoomHost?.setNeedsLayout()
        guard let scroll else { return }
        if settle { scroll.layoutIfNeeded() }
        let content: CGSize = settle ? scroll.contentSize : {
            let host = zoomHost
            let intrinsic = host?.intrinsicContentSize ?? CGSize(width: 0, height: intrinsicContentSize.height * scale)
            return CGSize(width: max(scroll.contentSize.width, intrinsic.width),
                          height: (host?.frame.minY ?? 0) + intrinsic.height)
        }()
        let inset = scroll.adjustedContentInset
        var offset = scroll.contentOffset
        offset.x += p.x * delta
        offset.y += p.y * delta
        let maxX = max(-inset.left, content.width + inset.right - scroll.bounds.width)
        let maxY = max(-inset.top, content.height + inset.bottom - scroll.bounds.height)
        offset.x = min(max(offset.x, -inset.left), maxX)
        offset.y = min(max(offset.y, -inset.top), maxY)
        scroll.contentOffset = offset
    }

    /// Render the backing store at the zoomed resolution, so text at 200% is
    /// drawn at 200% rather than scaled up from 100%. UIKit redraws on the
    /// change; unchanged, it is left alone.
    private func sharpenBackingStore() {
        let wanted = traitCollection.displayScale * zoomScale
        guard abs(contentScaleFactor - wanted) > 0.001 else { return }
        contentScaleFactor = wanted
        layer.contentsScale = wanted
        setNeedsDisplay()
    }

    /// The scale a pinch started from, so each frame of the gesture is the
    /// gesture's own cumulative scale over it rather than a compound of frames.
    private var pinchStart: CGFloat = 1

    /// A pinch, about the point between the fingers. The backing store is
    /// left at the old resolution until the fingers lift — every frame of a
    /// pinch is a redraw of the whole document otherwise.
    @objc private func handlePinch(_ gesture: UIPinchGestureRecognizer) {
        switch gesture.state {
        case .began:
            pinchStart = zoomScale
            // The scroll view's pan has usually begun already, on whichever
            // finger landed first, and would go on scrolling by that finger
            // through the pinch — and decelerating after it — which is a zoom
            // that drifts off the point it is about. Disabling a recognizer
            // cancels it; it is enabled again when the pinch ends, for the
            // next touch. The cancel bounces an over-scrolled view back with an
            // animation that would race the offsets set below, so the offset is
            // brought into range here, at once.
            if let scroll = enclosingScrollView() {
                scroll.panGestureRecognizer.isEnabled = false
                let inset = scroll.adjustedContentInset
                var offset = scroll.contentOffset
                offset.x = min(max(offset.x, -inset.left), max(-inset.left, scroll.contentSize.width + inset.right - scroll.bounds.width))
                offset.y = min(max(offset.y, -inset.top), max(-inset.top, scroll.contentSize.height + inset.bottom - scroll.bounds.height))
                scroll.setContentOffset(offset, animated: false)
            }
        case .changed:
            let anchor = gesture.location(in: self)
            let scale = Zoom.clamp(pinchStart * gesture.scale)
            guard scale != zoomScale else { return }
            zoomMode = .scale(scale / typeZoom)
            applyZoomScale(scale, anchor: anchor, sharpen: false, settle: true)
            onZoomChange?(zoomMode, zoomScale)
        case .ended, .cancelled, .failed:
            enclosingScrollView()?.panGestureRecognizer.isEnabled = true
            sharpenBackingStore()
        default:
            break
        }
    }

    /// Whether this view is the sheet a PDF is drawn from rather than a surface
    /// on screen — see `pdfData(page:)`. On paper there is no landing flash, no
    /// placeholder cue, no marker in the margin, and no picture of a sheet on a
    /// backdrop: the sheet is the paper. UIKit has no `currentContextDrawingToScreen`
    /// for `draw` to ask, so the sheet is told.
    ///
    /// And no line revealed: the frame core hands the screen shows the caret's
    /// line as source — a formula as its TeX, and under the full markup mode
    /// its delimiters — which paper, having no caret, lays out from
    /// `paperView()` instead.
    var isPaper = false {
        didSet { if isPaper, !oldValue { render(doc.paperView(), reflow: true) } }
    }

    private var docView: DocView
    private(set) var layoutEngine: EditorLayout
    /// Every sheet's frame in layout coordinates, top to bottom — what a PDF
    /// takes one page from each of. Empty in the continuous flow.
    var pages: [CGRect] { layoutEngine.pages }
    /// The view width the current layout was built for. The text column inside it
    /// — where it starts, how wide it wraps — is the theme's to decide (see
    /// `EditorTheme.column(in:)`), and the layout carries the answer.
    private var viewWidth: CGFloat = 0
    /// The caret offset the view last scrolled to reveal. Only a *move* re-scrolls,
    /// so passive reflows leave the reader's scroll position alone.
    private var lastCaretOffset: UInt32?
    /// Per-row shaped-text cache reused across frames; an edit re-shapes only the
    /// changed row(s). Cleared when the theme geometry changes (see `theme`).
    private var shapeCache: [Row: ShapedRow] = [:]

    /// The history, as the responder chain asks for it: the three-finger
    /// swipe, the shake, and the edit menu's own Undo/Redo all reach for the
    /// first responder's undo manager, and get twig's history through this —
    /// see `UndoBridge.swift`. The hardware ⌘Z below is only a shortcut to it.
    private lazy var historyManager = LeafUndoManager(
        state: { [weak self] in
            guard let self else { return (false, false) }
            return (self.docView.canUndo, self.docView.canRedo)
        },
        undo: { [weak self] in self?.command { $0.undo() } },
        redo: { [weak self] in self?.command { $0.redo() } })
    public override var undoManager: UndoManager? { historyManager }

    // MARK: UITextInputTraits — what the keyboard may do to the text
    //
    // A `UITextInput` view inherits the traits' defaults, and the defaults are
    // for prose: smart quotes turn `"` into `“`, smart dashes turn `--` into `—`,
    // autocorrect rewrites a word it doesn't know. In the WYSIWYG view that is
    // right — the text *is* prose, and core spells the markup. In the source view
    // every one of those is a rewrite of the markup itself: a straight quote is
    // an attribute delimiter, `--` is what the reader typed, and `**bold**` is
    // not a misspelling. So the substitutions follow the view, read afresh by
    // UIKit each time the keyboard comes up (`render` reloads it on a view
    // change). What a host sets here is the WYSIWYG value; the source view
    // answers `.no` regardless.
    private var isSourceView: Bool { docView.view == "source" }
    private var hostAutocorrection: UITextAutocorrectionType = .default
    private var hostSpellChecking: UITextSpellCheckingType = .default
    private var hostSmartQuotes: UITextSmartQuotesType = .default
    private var hostSmartDashes: UITextSmartDashesType = .default
    private var hostSmartInsertDelete: UITextSmartInsertDeleteType = .default
    public var autocorrectionType: UITextAutocorrectionType {
        get { isSourceView ? .no : hostAutocorrection }
        set { hostAutocorrection = newValue }
    }
    public var spellCheckingType: UITextSpellCheckingType {
        get { isSourceView ? .no : hostSpellChecking }
        set { hostSpellChecking = newValue }
    }
    public var smartQuotesType: UITextSmartQuotesType {
        get { isSourceView ? .no : hostSmartQuotes }
        set { hostSmartQuotes = newValue }
    }
    public var smartDashesType: UITextSmartDashesType {
        get { isSourceView ? .no : hostSmartDashes }
        set { hostSmartDashes = newValue }
    }
    public var smartInsertDeleteType: UITextSmartInsertDeleteType {
        get { isSourceView ? .no : hostSmartInsertDelete }
        set { hostSmartInsertDelete = newValue }
    }
    /// Sentences in prose; nothing in source, where a line may open with `#`,
    /// `-`, or `[` and the word after it is not a sentence's first.
    private var hostAutocapitalization: UITextAutocapitalizationType = .sentences
    public var autocapitalizationType: UITextAutocapitalizationType {
        get { isSourceView ? .none : hostAutocapitalization }
        set { hostAutocapitalization = newValue }
    }
    public var keyboardType: UIKeyboardType = .default
    public var returnKeyType: UIReturnKeyType = .default

    // Accessibility state — the answers live in the extension below; the
    // storage has to be here, since an extension cannot add a stored property.
    /// A host's `accessibilityLabel`, if it set one.
    private var hostAccessibilityLabel: String?
    /// The visual lines VoiceOver reads, with their frames in the view's
    /// coordinates. Dropped on every `render`, since that is when the layout
    /// moved; rebuilt on the next question.
    private var readingLines: [(text: String, frame: CGRect)]?

    // UITextInput plumbing.
    public weak var inputDelegate: UITextInputDelegate?
    public lazy var tokenizer: UITextInputTokenizer = LeafTokenizer(view: self)
    public var markedTextStyle: [NSAttributedString.Key: Any]?
    private var marked: LeafTextRange?
    private lazy var textInteraction: UITextInteraction = {
        let interaction = UITextInteraction(for: .editable)
        interaction.textInput = self
        return interaction
    }()

    /// Whether this surface is a *reader*: selection, scrolling, copy and the
    /// menus all work, and nothing edits.
    ///
    /// The document is the enforcement — leaf-core's read-only gate refuses
    /// every splice — so what the view owns here is the chrome that would
    /// otherwise promise an edit it cannot deliver: the interaction swaps to
    /// `.nonEditable` (no caret placement idiom, selection handles only), the
    /// keyboard stays down (`inputView`), and Cut/Paste leave the menu
    /// (`canPerformAction`).
    public var isReadOnly: Bool = false {
        didSet {
            guard isReadOnly != oldValue else { return }
            removeInteraction(textInteraction)
            let interaction = UITextInteraction(for: isReadOnly ? .nonEditable : .editable)
            interaction.textInput = self
            textInteraction = interaction
            addInteraction(interaction)
            if isFirstResponder { reloadInputViews() }
        }
    }

    /// Called with a highlight's `id` when a tap lands on its margin marker —
    /// how a host's painted annotation opens. The marker, not the wash: the
    /// wash is ink a reader should be able to select and copy through without
    /// a card leaping at them, and the glyph in the margin is the control that
    /// says so. Nil leaves every highlight purely visual.
    public var onTapHighlight: ((String) -> Void)?

    /// The margin markers' hit targets, rebuilt on every draw — each a little
    /// larger than the glyph it stands behind, since a 17pt symbol under a
    /// thumb is a miss.
    private var markerHits: [(rect: CGRect, id: String)] = []

    /// Extra actions for the *selection's* edit menu, ahead of the system's
    /// Copy/Look Up — how a host puts its own verbs where a reader's thumb
    /// already is (cite this, annotate this). Asked each time the menu is
    /// built, so the answer can depend on what is selected; nil (the default)
    /// leaves the system menu alone. See `editMenu(for:suggestedActions:)`.
    public var selectionMenuActions: (() -> [UIMenuElement])?

    /// Host hook for link activation. Called with the link's raw destination
    /// before the view falls back to opening it with the system; return `true`
    /// to claim it. This is how a host resolves destinations only *it* can make
    /// sense of — a note app's `./sibling.md` or `id:6tzwsxg` names a document
    /// in its own workspace, not a URL, and handing either to `UIApplication`
    /// is at best a no-op. Nil (or a `false` return) keeps the system behaviour.
    public var onOpenLink: ((String) -> Bool)?

    /// Asked to edit the destination of the link under the caret, with its
    /// current destination to seed a field with. See `LeafEditorModel.onEditLink`.
    public var onEditLink: ((String) -> Void)?

    /// Asked what a link points at, so a long press can show it. See
    /// `LeafEditorModel.onPeekLink`.
    public var onPeekLink: ((String, @escaping (LinkPeekSource?) -> Void) -> Void)?

    /// Whether a bare `[[…]]` counts as a link to follow. Off by default: it is
    /// not Markdown, not Djot, and not something twig parses, so the editor
    /// makes no claim about it unless a host whose documents use the convention
    /// asks. See `LeafDoc.activatableTargetAtCaret`.
    public var recognizesWikilinks = false

    /// Host hook for activating a block video or audio — called with its raw
    /// `src` when the editor isn't playing it itself.
    ///
    /// With `mediaPlayback == .inline` (the default) a tap installs an AVKit
    /// player over the box and this is never called, *except* for a source the
    /// editor's own loader can't resolve to a local file — a remote URL, which a
    /// host is better placed to handle since it can fetch asynchronously.
    /// With `.host` it is called for every activation. Nil in either case leaves
    /// a tap doing nothing but placing the caret.
    public var onOpenMedia: ((String) -> Void)?

    /// Host hook for *showing* an attachment — called with a media box's raw
    /// `src` when the reader asks to go to the attachment itself rather than to
    /// have it loaded here. See `LeafEditorModel.onShowMedia`.
    ///
    /// Nil leaves the edit menu exactly as it was.
    public var onShowMedia: ((String) -> Void)?

    /// The media box the long press that is raising the edit menu landed on.
    ///
    /// The press, not the caret: a `<video>` has no image node for core to find,
    /// and a press inside a block box need not leave the caret inside the
    /// picture's span. Cleared when a press begins, so a menu raised over prose
    /// can never inherit the last one's attachment.
    private var pressedMediaSource: String?

    /// The document's directory, which a relative `src` in the markup resolves
    /// against. Core does no I/O and knows no paths, so the host supplies this;
    /// nil (an untitled buffer) leaves relative paths unresolvable and their
    /// boxes drawn as labelled chips.
    public var documentDirectory: URL? {
        get { mediaStore.baseURL }
        set {
            guard newValue != mediaStore.baseURL else { return }
            mediaStore.baseURL = newValue
            mediaStore.flush()          // every relative path now points elsewhere
            render(docView, reflow: true)
        }
    }

    /// Gets first refusal on a paste. See `LeafEditorModel.onPaste`.
    public var onPaste: (() -> Bool)?

    /// Reconsider `src` — or every source, for nil — and redraw.
    /// See `LeafEditorModel.reloadMedia`.
    public func reloadMedia(_ src: String?) {
        mediaStore.forget(src)
        render(docView, reflow: true)
    }

    /// A cue shown while the document is empty — "Start writing…" — drawn where
    /// its first character will go. Nil (the default) draws nothing.
    ///
    /// The editor's own, rather than a label a host stacks over the view,
    /// because only the layout knows where the prose starts: the text column is
    /// centred when the theme sets a `measure`, which `theme.padding` is only
    /// the floor for. The system draws the caret on this surface, over
    /// everything painted here, so the caret stands at the cue's first letter
    /// without this having to order the two.
    public var placeholder: String? {
        didSet { if placeholder != oldValue { setNeedsDisplay() } }
    }

    /// What activating a block video or audio does. `.inline` (the default)
    /// installs a real AVKit player over the box; `.host` draws the still and
    /// hands the source to `onOpenMedia` instead. See `MediaPlaybackMode`.
    public var mediaPlayback: MediaPlaybackMode = .inline

    /// Asks the host to resolve a source this view can't read itself — a remote
    /// URL, or a scheme only the host understands — to a local file it can.
    /// See `MediaStore.onResolveMedia`; LeafUI never touches the network.
    public var onResolveMedia: ((String, @escaping (URL?) -> Void) -> Void)? {
        get { mediaStore.onResolveMedia }
        set { mediaStore.onResolveMedia = newValue }
    }

    /// The host's own reading of a source, answered synchronously and read at
    /// once if it names a file that is here. See `MediaStore.onLocateMedia`.
    public var onLocateMedia: ((String) -> URL?)? {
        get { mediaStore.onLocateMedia }
        set { mediaStore.onLocateMedia = newValue }
    }

    /// Loads and caches the stills the media boxes draw.
    private let mediaStore = MediaStore()
    /// The AVKit players currently installed over media boxes.
    private let mediaPlayers = MediaPlayerHost()
    /// The source the reader tapped while the host was still resolving it, so
    /// the answer can start playback rather than land silently.
    private var pendingMediaActivation: String?

    /// Activates a block media box on a plain tap — the box draws a play badge,
    /// so that is what a tap on it should mean. This rides *beside*
    /// `textInteraction` rather than replacing it — it doesn't cancel touches
    /// and recognises simultaneously — so the caret still lands where it always
    /// did, and this only adds playback when the tap was on a media box.
    ///
    /// It used to follow links too. It no longer does: a tap places the caret,
    /// like a tap on any other text, and following moved to the edit menu's
    /// "Open Link" (see `editMenuInteraction(_:menuFor:suggestedActions:)`). The
    /// desktop rule is the same one — the editor is an editor first, and a tap
    /// that navigated made link text the one span you couldn't get a caret into
    /// without leaving the document.
    private lazy var mediaTap: UITapGestureRecognizer = {
        let tap = UITapGestureRecognizer(target: self, action: #selector(handleMediaTap(_:)))
        tap.numberOfTapsRequired = 1
        tap.cancelsTouchesInView = false
        tap.delegate = self
        return tap
    }()

    /// The long press that actually gets the link menu on screen.
    ///
    /// The menu items were there and unreachable. `UITextInteraction` owns the
    /// long press, and what it does with one is raise the loupe and steer the
    /// caret — it does not then present the edit menu, so holding a link moved
    /// the cursor and nothing else, which is exactly what it looked like. The
    /// menu was being *built* correctly and never asked for.
    ///
    /// So this asks for it. It runs beside the system's press rather than instead
    /// of it (`shouldRecognizeSimultaneouslyWith`), which is what keeps the loupe:
    /// a link's tap target is a few characters wide on a phone, and the loupe is
    /// how a reader lands on the right one. Then, on lift, the menu.
    private lazy var linkPress: UILongPressGestureRecognizer = {
        let press = UILongPressGestureRecognizer(target: self, action: #selector(handleLinkPress(_:)))
        // The system's own press drives the caret; this one only watches for the
        // lift, so it must not swallow the touches that press needs.
        press.cancelsTouchesInView = false
        press.delegate = self
        return press
    }()

    /// The edit menu, so the link actions have somewhere to appear.
    ///
    /// Owning one is what makes custom items possible at all on iOS 16+.
    /// `canPerformAction` alone is enough for the *system* items (Cut/Copy/Paste
    /// know their own selectors), but a selector UIKit has never heard of has no
    /// title and no place in the menu until a `UIMenu` names it —
    /// `UIMenuController.menuItems`, which used to do that, is deprecated in
    /// favour of exactly this. `UITextInteraction` presents through the
    /// interaction installed on its view, so adding ours here puts the items in
    /// the menu the long press already raises rather than in a second one.
    private lazy var editMenu = UIEditMenuInteraction(delegate: self)

    /// The system find panel — ⌘F on an iPad keyboard, Edit ▸ Find, the Find
    /// rotor — over this view as its `UITextSearching` client (see "Find"
    /// below). The panel is UIKit's; what it searches, and how a match is lit,
    /// scrolled to and replaced, is this view's.
    public private(set) lazy var findInteraction = UIFindInteraction(sessionDelegate: self)

    /// The matches the find panel has asked to be lit, by source byte range,
    /// with how: every match found, and the current one brighter.
    private var foundDecorations: [FoundRange: UITextSearchFoundTextStyle] = [:]
    /// A re-search is already queued for the text having changed under the
    /// panel — see `invalidateFoundResultsAfterEdit`.
    private var findInvalidationQueued = false
    /// A replace the panel itself asked for is under way: it re-searches on
    /// its own afterwards, and an invalidation on top of that leaves it with
    /// no results.
    private var replacingFoundText = false
    /// The panel's last search, to run again when the text changes under it.
    private var lastFindQuery: (query: String, options: UITextSearchOptions)?

    public init(doc: LeafDoc, theme: EditorTheme = .default) {
        self.doc = doc
        self.hostTheme = theme
        self.renderTheme = theme
        // Every frame after the first is the change since the frame before,
        // spliced into `docView` by `render` — the AppKit peer's rule.
        doc.setIncrementalFrames(on: true)
        // Unwrapped layout (one row per block); the view soft-wraps at pixel width.
        let first = doc.setUnwrapped()
        self.docView = first
        var seed: [Row: ShapedRow] = [:]
        self.layoutEngine = EditorLayout(first, theme: renderTheme, viewWidth: 0, cache: &seed)
        self.shapeCache = seed
        super.init(frame: .zero)
        // A resolved source has a picture to draw, and may be the one the reader
        // tapped while it was still being fetched.
        // Laid out again rather than just repainted: the box keeps the still it
        // was laid out with, so a repaint alone would leave a chip-height row.
        mediaStore.onLoaded = { [weak self] src in
            guard let self else { return }
            self.render(self.docView, reflow: true)
            self.playIfAwaited(src)
        }
        backgroundColor = .clear
        contentMode = .redraw
        addInteraction(textInteraction)
        addGestureRecognizer(mediaTap)
        addGestureRecognizer(linkPress)
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(handlePinch))
        pinch.delegate = self   // alongside the scroll view's pan: a pinch that drifts also scrolls
        addGestureRecognizer(pinch)
        addInteraction(editMenu)
        addInteraction(findInteraction)
        // Seed with the initial caret so the first reflow opens at the top.
        lastCaretOffset = doc.caretOffset()
        applyDynamicType()   // scale type to the current trait environment
    }

    // MARK: reaching a link

    /// Raise the edit menu when a long press ends on something the menu has an
    /// answer for — a link, a footnote reference, a note.
    ///
    /// On the lift rather than at `.began`, for two reasons. The loupe spends the
    /// whole press steering the caret, so where the reader's finger *started* is
    /// not what they chose — the final caret is, and asking any earlier would
    /// offer the menu for whatever character the press happened to begin over.
    /// And a menu raised under a finger that is still down is one the same touch
    /// then dismisses.
    ///
    /// Silent when the caret ends up on ordinary prose: an ordinary long press
    /// keeps doing exactly what it did, which is to place a caret.
    @objc private func handleLinkPress(_ gesture: UILongPressGestureRecognizer) {
        // Where the finger went down is what a media box is judged by — the
        // loupe spends the press dragging the caret out of it, so by the lift
        // the caret may be anywhere, but the box the reader pressed is the one
        // they meant.
        if gesture.state == .began {
            pressedMediaSource = layoutEngine.mediaBox(at: gesture.location(in: self))?.src
        }
        guard gesture.state == .ended else { return }
        // A press that made a selection is the selection's, and the system shows
        // its own menu over one. Presenting a second here would be two menus for
        // one gesture.
        guard !docView.hasSelection, caretHasMenuActions else { return }
        let rc = doc.posForOffset(off: doc.caretOffset())
        // Anchored on the caret rather than on the finger, so the menu points at
        // the link the reader landed on rather than at where they let go.
        let source = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch))?.origin
            ?? gesture.location(in: self)
        editMenu.presentEditMenu(
            with: UIEditMenuConfiguration(identifier: nil, sourcePoint: source))
    }

    /// Whether the caret stands on anything the edit menu would add an entry for.
    ///
    /// Asked of the same two builders the menu itself uses, rather than of the
    /// queries under them: the point is that the press raises a menu exactly when
    /// there will be something in it, and two ways of deciding that would drift
    /// into a press that opens an empty menu or one that opens none at all.
    private var caretHasMenuActions: Bool {
        !footnoteMenuActions().isEmpty
            || showableMediaAtPress != nil
            || !doc.linkActionsAtCaret(wikilinks: recognizesWikilinks,
                                       canEdit: onEditLink != nil,
                                       canPeek: onPeekLink != nil).isEmpty
    }

    /// The attachment the menu about to be raised is about, or nil — the box the
    /// press landed on, else the image the caret ended in, and only when a host
    /// is listening. See `showableMediaSource`.
    private var showableMediaAtPress: String? {
        showableMediaSource(box: pressedMediaSource, caret: doc.mediaSourceAtCaret(),
                            canShow: onShowMedia != nil)
    }

    // MARK: media activation

    /// The tap handler. A tap that landed on no media box does nothing at all —
    /// `textInteraction` has already placed the caret, which is the whole of what
    /// a tap on ordinary prose (link text included) should do.
    @objc private func handleMediaTap(_ gesture: UITapGestureRecognizer) {
        // A tap that lands while text is selected is the *selection's* — the
        // system answers it by showing the edit menu over the selection or by
        // dismissing it — and this handler has no business moving the caret out
        // from under either. Which is also what keeps selection working at all:
        // `mediaTap` recognises simultaneously with `textInteraction`'s own
        // gestures, so the second tap of a double-tap-to-select reaches both, and
        // without this guard whichever ran last collapsed the word the other had
        // just selected. Selecting text was impossible, and the Copy/Paste menu
        // never appeared, because every tap ended as a bare caret.
        guard !docView.hasSelection else { return }
        // A tap ends whatever the last press was about, so the next menu cannot
        // inherit its attachment.
        pressedMediaSource = nil
        let point = gesture.location(in: self)
        // A margin marker outranks everything at its point — it is chrome, and
        // the whole reason it sits in the margin is to be the one tap that
        // opens the annotation while the washed text underneath stays ordinary
        // text (see `onTapHighlight`).
        if let onTapHighlight,
           let hit = markerHits.first(where: { $0.rect.contains(point) }) {
            onTapHighlight(hit.id)
            return
        }
        // A tap on a video or audio box starts it, and a tap on an *empty*
        // picture box asks the host for it.
        if let hit = layoutEngine.mediaBox(at: point) {
            _ = activateMedia(hit)
            return
        }
        // A tap on a task item's box ticks it. `textInteraction` still places
        // its caret for the same tap — at the item's start, which is where core
        // maps the marker's glyphs — and the tick does not move it again: core's
        // `toggle_task_at` goes by the tap's own offset, not the caret's.
        if !isSourceView, let box = layoutEngine.taskBox(at: point) {
            command { $0.toggleTaskAt(offset: UInt64(box)) }
            return
        }
        // A tap under the last block is on nothing: the caret goes onto an
        // empty paragraph under it, opened if the document has none — the way
        // out from under a fence Return cannot leave. `textInteraction` places
        // its caret through `closestPosition(to:)`, which answers the
        // document's end for the same point, so the two land in the same place
        // whichever runs first: at the end that already was, or at the end
        // this just opened.
        if !isSourceView, layoutEngine.isPastEnd(point) {
            command { $0.clickPastEnd() }
        }
    }

    // MARK: link following

    /// Open the link under the caret, if there is one. The host gets first
    /// refusal (`onOpenLink`); otherwise it goes to the system, which needs the
    /// destination to parse as a URL.
    ///
    /// Reached from the edit menu rather than from a tap: with no ⌘ to hold and
    /// no pointer to hover, a long press is the phone's "do something else to
    /// this" gesture, and it is the one that doesn't collide with placing a
    /// caret.
    @discardableResult
    private func openLinkAtCaret() -> Bool {
        guard let dest = targetAtCaret() else { return false }
        // A bare `#v2` is a place in this document, so following it is a scroll
        // rather than a departure — the AppKit peer's rule, and the same one.
        if let landing = doc.selfLanding(of: dest) { reveal(offset: landing); return true }
        if onOpenLink?(dest) == true { return true }
        guard let url = URL(string: dest) else { return false }
        UIApplication.shared.open(url)
        return true
    }

    /// The link the caret stands in, honouring this view's wikilink setting.
    private func targetAtCaret() -> String? {
        doc.activatableTargetAtCaret(wikilinks: recognizesWikilinks)
    }

    @objc func openLink(_ sender: Any?) { openLinkAtCaret() }

    @objc func copyLink(_ sender: Any?) {
        guard let dest = targetAtCaret() else { return }
        UIPasteboard.general.string = dest
    }

    @objc func editLink(_ sender: Any?) {
        guard let dest = doc.linkDestinationAtCaret() else { return }
        onEditLink?(dest)
    }

    // MARK: footnotes

    /// The popover a "Show Note" raises. Owned here for the AppKit peer's reason:
    /// it outlives the tap that raised it, and something has to take it down.
    private let footnotePeek = FootnotePeekPresenter()

    /// Follow the footnote under the caret — down to the note from a reference,
    /// back up to the reference from a note — and say whether there was one.
    @discardableResult
    private func followFootnoteAtCaret() -> Bool {
        guard let jump = doc.footnoteJumpAtCaret() else { return false }
        footnotePeek.hide()
        render(doc.caretMoved(to: jump.offset))
        return true
    }

    /// Show the note the caret's reference names, anchored to the caret.
    ///
    /// The phone's answer to the Mac's hover. A finger has no resting state to
    /// read as "tell me about this", so the peek is something the reader asks for
    /// by name in the menu the long press already raises — and the jump rides
    /// inside the popover rather than beside it in the menu, so the common case
    /// (read the note, carry on reading) costs one tap and never moves the caret.
    private func showFootnotePeekAtCaret() {
        // `docView` is the frame on screen, so the note is drawn from the rows
        // the page below is already drawing it from.
        guard let content = doc.footnotePeekContent(at: doc.caretOffset(), in: docView, theme: renderTheme),
              let parent = owningViewController
        else { return }
        let rc = doc.posForOffset(off: doc.caretOffset())
        guard let caret = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch)) else { return }
        // Nil when the reference names no note: a button that led nowhere would
        // contradict the sentence right above it.
        let follow: (() -> Void)? = doc.footnoteJumpAtCaret() == nil
            ? nil
            : { [weak self] in self?.followFootnoteAtCaret() }
        footnotePeek.show(content, from: caret.insetBy(dx: -6, dy: -2), in: self,
                          presentedBy: parent, onFollow: follow,
                          onTarget: { [weak self] in self?.followPeekTarget($0) })
    }

    /// Show what the link under the caret points at, anchored to the caret — the
    /// phone's answer to the Mac's hover-a-link, reached from the same long-press
    /// menu that offers "Show Note" for a footnote.
    ///
    /// Two sources, as on the Mac: a `#v2` is a place in the document already on
    /// screen, and anything else is a file only the host can read.
    private func showLinkPeekAtCaret() {
        guard let destination = doc.activatableTargetAtCaret(wikilinks: recognizesWikilinks)
        else { return }
        if destination.hasPrefix("#") {
            let locator = String(destination.dropFirst())
            present(FootnotePeekContent(
                peeking: locator, of: doc, in: docView, theme: renderTheme))
            return
        }
        let caret = doc.caretOffset()
        onPeekLink?(destination) { [weak self] fetched in
            guard let self, let fetched, self.doc.caretOffset() == caret else { return }
            self.present(FootnotePeekContent(peeking: fetched, theme: self.renderTheme))
        }
    }

    /// Put a link's peek on screen at the caret. Nothing in it leads anywhere
    /// (see `FootnotePeekContent`'s peeking initializer), so unlike the footnote
    /// peek it needs neither a jump button nor a target handler.
    private func present(_ content: FootnotePeekContent?) {
        guard let content, let parent = owningViewController else { return }
        let rc = doc.posForOffset(off: doc.caretOffset())
        guard let caret = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch)) else { return }
        footnotePeek.show(content, from: caret.insetBy(dx: -6, dy: -2), in: self,
                          presentedBy: parent, onFollow: nil, onTarget: { _ in })
    }

    /// Answer a tap on something followable inside the note — a link, or a
    /// reference to another footnote.
    ///
    /// The same two answers the Mac gives, for the same reasons: a link is the
    /// host's first (a note's `./sibling.md` means what it means everywhere else
    /// in the document), and a nested reference navigates rather than stacking a
    /// second popover on top of the first.
    private func followPeekTarget(_ target: FootnotePeekTarget) {
        switch target.kind {
        case .link(let destination):
            // A `#v2` in a note names a place in the document the note belongs
            // to, so it navigates here rather than going out to the host.
            if let landing = doc.selfLanding(of: destination) { reveal(offset: landing); return }
            if onOpenLink?(destination) == true { return }
            guard let url = URL(string: destination) else { return }
            UIApplication.shared.open(url)
        case .footnote(let offset):
            render(doc.caretMoved(to: offset))
            followFootnoteAtCaret()
        }
    }

    /// The footnote entries the edit menu should show, in the order it shows them.
    ///
    /// Two shapes, because the two ends of a footnote want different things. On a
    /// *reference* the question is almost always "what does it say", so the entry
    /// is the peek — and it is offered even for a `[^99]` nothing defines, since
    /// the popover is the one place that can say so. In a *note* there is nothing
    /// to peek at (the reader is looking at it) and only one useful move, so the
    /// entry is the jump itself.
    private func footnoteMenuActions() -> [UIAction] {
        // "Is the caret on a reference at all" — the raw query, not the rendered
        // content, which is a menu's worth of work too early.
        if doc.footnotePeek(at: doc.caretOffset()) != nil {
            return [UIAction(title: loc("menu.showNote", "Show Note")) { [weak self] _ in
                self?.showFootnotePeekAtCaret()
            }]
        }
        guard doc.footnoteJumpAtCaret()?.action == .backToReference else { return [] }
        return [UIAction(title: loc("menu.backToReference", "Back to Reference")) { [weak self] _ in
            self?.followFootnoteAtCaret()
        }]
    }

    /// Answer a tap on a block media box, returning whether it was handled.
    ///
    /// In `.inline` mode this installs an AVKit player over the box and starts
    /// it; a second tap on a playing one pauses. `.host` mode, a source this
    /// loader can't resolve to a local file, and (on iOS) a view with no owning
    /// view controller to parent the player to all fall through to `onOpenMedia`.
    private func activateMedia(_ media: MediaView) -> Bool {
        // An image plays nothing, so it is worth activating only when its box is
        // empty and the host might yet fill it — a source the host declined, or
        // one whose bytes aren't on this device. A picture that loaded is just
        // text to tap into, and swallowing that tap would be a bug.
        if media.kind == .image {
            guard mediaStore.still(for: media) == nil else { return false }
            if mediaStore.isResolving(media.src) { return true }
            guard let open = onOpenMedia else { return false }
            open(media.src)
            return true
        }
        if mediaPlayback == .inline {
            if let url = mediaStore.playableURL(for: media.src) {
                let rects = layoutEngine.mediaRects()
                if let rect = rects[media.src],
                   mediaPlayers.activate(media, at: rect, in: self, url: url) {
                    setNeedsDisplay()   // the badge under the player must stop drawing
                    return true
                }
            } else if mediaStore.isResolving(media.src) {
                // The host is fetching it. Remember what was asked for, so the
                // answer starts playback instead of leaving the reader to tap a
                // second time on a box that looks unchanged.
                pendingMediaActivation = media.src
                return true
            }
        }
        guard let open = onOpenMedia else { return false }
        open(media.src)
        return true
    }

    /// Play the media the reader tapped, now that the host has resolved it.
    /// A no-op unless this is the source they were waiting on.
    private func playIfAwaited(_ src: String) {
        guard pendingMediaActivation == src else { return }
        pendingMediaActivation = nil
        guard let url = mediaStore.playableURL(for: src),
              let info = layoutEngine.rows.compactMap(\.media).first(where: { $0.media.src == src })
        else { return }
        if let rect = layoutEngine.mediaRects()[src] {
            mediaPlayers.activate(info.media, at: rect, in: self, url: url)
            setNeedsDisplay()
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    public override var canBecomeFirstResponder: Bool { true }
    public override var intrinsicContentSize: CGSize {
        let raw = layoutEngine.contentHeight
        // Once the document already needs to scroll, pad another half screen below
        // the last line — the AppKit peer's `applyContentHeight` mirrors this — so a
        // long entry can be pulled up to a comfortable reading height instead of
        // staying glued to the bottom edge. Content that already fits the viewport
        // (the common short-document case) gets no extra room, so nothing here makes
        // a short document scrollable; `pin(_:into:)`'s own minimum-height
        // constraint still fills the viewport exactly as before in that case.
        //
        // Not in the paginated flow: there the document's height is the stack's,
        // and the blank paper below the last line is already that room.
        //
        // The frame less the bars it runs under, not less the keyboard too. The
        // keyboard is a content inset (`LeafEditorController`), and measuring it
        // here would resize this view on every rise and fall without a render
        // between — and measured, such a resize of a view this tall (2755pt to
        // 2577pt on an iPhone 18 Pro Max: across 8192px at 3×) left the caret's
        // row unpainted until the next scroll, whether made during the keyboard
        // animation or after it, with `draw` handed the whole bounds each time.
        // The compositor's, then, and not worth provoking: the bars change
        // with nothing, the keyboard with every tap.
        //
        // In layout points: the viewport is measured on screen, and this size is
        // scaled up by the zoom on its way to the scroll view (`LeafZoomView`).
        let viewportHeight = (enclosingScrollView().map {
            $0.bounds.height - $0.safeAreaInsets.top - $0.safeAreaInsets.bottom
        } ?? 0) / zoomScale
        let extra = pageSetup == nil && raw > viewportHeight ? viewportHeight * 0.5 : 0
        return CGSize(width: UIView.noIntrinsicMetric, height: raw + extra)
    }

    /// The layout's own width — the stack's, on paper — or `0` in the continuous
    /// flow, which has no width but the one it is given. What the wrapper reports
    /// as its intrinsic width, scaled.
    var layoutContentWidth: CGFloat { layoutEngine.contentWidth }

    /// A custom view shown above the system keyboard while this view is first
    /// responder — a host app's own formatting toolbar, say. `nil` (the
    /// default) shows nothing. This is the raw `UIResponder` hook: SwiftUI's
    /// `.toolbar(placement: .keyboard)` only self-installs for SwiftUI-native
    /// text controls (`TextField`/`TextEditor`), not an arbitrary custom
    /// `UIView`-based text surface like this one, so a host wanting a keyboard
    /// accessory has to set this directly (see `LeafEditor`'s `accessory:`
    /// initializer, which wires a SwiftUI view in here).
    public var accessoryView: UIView? {
        didSet {
            guard accessoryView !== oldValue else { return }
            reloadInputViews()
        }
    }

    public override var inputAccessoryView: UIView? { isReadOnly ? nil : accessoryView }

    /// No keyboard over a reader. `nil` here means "the system keyboard", so
    /// suppressing it takes an explicit empty view — becoming first responder
    /// (which copy and the edit menu still need) must not raise a keyboard the
    /// document would refuse every key of.
    public override var inputView: UIView? { isReadOnly ? UIView() : nil }

    private func off(_ p: UITextPosition) -> Int { (p as? LeafTextPosition)?.offset ?? 0 }

    /// The source offsets bounding the visual line offset `o` sits on — a cell's
    /// line inside a table, a wrapped line of the block elsewhere — and whether
    /// that line continues one above it across a soft wrap, in which case its
    /// `start` is also the line above's `end`. What `LeafTokenizer` answers line
    /// questions from.
    func visualLineBounds(at o: Int) -> (start: Int, end: Int, continues: Bool)? {
        if let cell = layoutEngine.tableLineBounds(src: o) { return cell }
        let rc = doc.posForOffset(off: UInt32(o))
        guard let line = layoutEngine.visualLine(row: Int(rc.row), ch: Int(rc.ch)) else { return nil }
        return (Int(doc.offsetForPos(row: rc.row, ch: UInt32(line.start))),
                Int(doc.offsetForPos(row: rc.row, ch: UInt32(line.end))),
                line.index > 0)
    }

    // MARK: layout / wrap

    public override func layoutSubviews() {
        super.layoutSubviews()
        relayoutForWidth(force: false)
    }

    private func relayoutForWidth(force: Bool) {
        let w = bounds.width
        guard w > renderTheme.padding.left + renderTheme.padding.right else { return }
        if force || abs(w - viewWidth) > 0.5 {
            viewWidth = w
            // Re-wrap the current frame at the new pixel width — no round trip to core.
            render(docView, reflow: true)
        }
    }

    // MARK: applying a frame

    /// Install a fresh `DocView` and repaint. The system re-reads `selectedTextRange`
    /// and re-lays its selection overlays afterward.
    ///
    /// The frame is the change since the frame before, spliced into `docView`
    /// (`DocView.apply`), or the whole document; a change against a frame
    /// this view does not hold is replaced by a whole one asked of core. The
    /// layout is the frame before's wherever the rows are — kept whole when
    /// nothing changed but the caret, rebuilt from the first changed row on
    /// otherwise; `reflow` says the geometry under the rows moved and every row
    /// is laid out again. The AppKit peer's rule; see its `render`.
    private func render(_ frame: DocView, reflow: Bool = false) {
        // A peek is chrome anchored to a position, and the positions are being
        // rebuilt — an edit reflowed the line it points at, or a relayout moved
        // the reference out from under it.
        footnotePeek.hide()
        let frame = frame.isChange && frame.basis != docView.frame ? doc.view() : frame
        let blocksChanged = frame.tables != docView.tables || frame.media != docView.media
            || frame.math != docView.math || frame.directives != docView.directives
        let viewFlipped = frame.view != docView.view
        // A flip shapes every row afresh: the cache is keyed by the row's
        // value, and a line that reads the same in both views — plain prose —
        // is set in the body face in one and the mono face in the other.
        if viewFlipped { shapeCache.removeAll(keepingCapacity: true) }
        // The document changed, as distinct from what is shown of it: the flip
        // rewrites every row and edits nothing, and a selection changes what is
        // shown of the text, not the text — see `Row.sameText`. A change that
        // moved the rows below its span edited the source whatever the span
        // holds — a comment pasted between two paragraphs shows no row — and
        // the host has a document to save.
        let change: RowChange?
        let edited: Bool
        if frame.isChange {
            let shifted = frame.srcShift != 0
            let (span, replaced) = docView.apply(frame)
            change = span
            edited = !viewFlipped && (shifted || !frame.rows.sameText(as: replaced))
        } else {
            change = frame.rows.changedRange(from: docView.rows)
            edited = !viewFlipped && !frame.rows.sameText(as: docView.rows, over: change)
            docView = frame
        }
        let view = docView
        readingLines = nil
        // The input traits answer differently per view (see `isSourceView`), and
        // UIKit reads them when the keyboard is set up — so set it up again.
        if viewFlipped, isFirstResponder { reloadInputViews() }
        // Under this view's own traits, not whichever are current. The text's
        // colours are dynamic and resolve when drawn, where UIKit has made the
        // view's traits current; a formula's ink is resolved *here*, to the
        // bytes the typesetter is handed. Left to the caller's traits, a paper
        // sheet (light, whatever the screen) laid out on a dark phone would set
        // its formulas in white — and print them on white.
        traitCollection.performAsCurrent {
            if reflow {
                layoutEngine = EditorLayout(view, theme: renderTheme, viewWidth: viewWidth, page: pageSetup,
                                            cache: &shapeCache, media: mediaStore)
            } else if change != nil || blocksChanged || layoutEngine.rows.isEmpty {
                layoutEngine = EditorLayout(view, theme: renderTheme, viewWidth: viewWidth, page: pageSetup,
                                            cache: &shapeCache, media: mediaStore, previous: layoutEngine,
                                            change: change)
            }
        }
        let relaid = reflow || change != nil || blocksChanged
        // Installed players follow their boxes; media edited out of the document
        // is absent from the rects, which is what stops its playback.
        if relaid, !mediaPlayers.isEmpty {
            mediaPlayers.reposition(layoutEngine.mediaRects())
        }
        if relaid {
            invalidateIntrinsicContentSize()
            // The scroll view reads the wrapper's size, not this view's.
            zoomHost?.invalidateIntrinsicContentSize()
        }
        setNeedsDisplay()
        // Only follow the caret when it actually moved, not on a passive reflow.
        let caret = doc.caretOffset()
        if caret != lastCaretOffset {
            lastCaretOffset = caret
            scrollCaretToVisible()
        }
        onStateChange?(EditorState(view))
        if relaid { onLayoutChange?() }
        if edited {
            invalidateFoundResultsAfterEdit()
            onEdit?()
        }
    }

    /// Put the caret at `offset` and land the reader on it — how a host arrives
    /// at the place a `#v2` names, with `through` bounding the block to flash.
    /// The AppKit peer's twin; see it and `Landing` for why an arrival is its own
    /// move rather than the least scroll that works.
    public func reveal(offset: UInt32, through end: UInt32? = nil) {
        command { $0.caretMoved(to: offset) }
        lastCaretOffset = offset
        land()
        guard let end, end > offset else { return }
        flash(from: offset, to: end)
    }

    /// Scroll so the caret's block sits a fixed distance below the top of the
    /// viewport, rather than the least distance that brings it into view.
    private func land() {
        guard let scroll = enclosingScrollView(),
              let rect = layoutEngine.caretRect(docView, theme: renderTheme)
        else { return scrollCaretToVisible() }
        let target = convert(rect, to: scroll)
        let visible = scroll.bounds.height - scroll.adjustedContentInset.top
            - scroll.adjustedContentInset.bottom
        let y = Landing.scrollTop(for: target,
                                  visibleHeight: visible,
                                  documentHeight: scroll.contentSize.height)
        scroll.setContentOffset(
            CGPoint(x: scroll.contentOffset.x, y: y - scroll.adjustedContentInset.top),
            animated: false)
    }

    // MARK: the flash a landing leaves

    /// The byte range lit up by the landing in progress, and when it started —
    /// the AppKit peer's pair, drawn the same way and for the same reason.
    private var flashRange: Range<UInt32>?
    private var flashStarted: Date?
    private var flashTimer: Timer?

    private func flash(from start: UInt32, to end: UInt32) {
        flashTimer?.invalidate()
        flashRange = start..<end
        flashStarted = Date()
        setNeedsDisplay()
        flashTimer = Timer.scheduledTimer(withTimeInterval: 1 / 30, repeats: true) { [weak self] t in
            guard let self, let started = self.flashStarted else { return t.invalidate() }
            guard Landing.opacity(elapsed: Date().timeIntervalSince(started)) != nil else {
                t.invalidate()
                self.flashTimer = nil
                self.flashRange = nil
                self.flashStarted = nil
                self.setNeedsDisplay()
                return
            }
            self.setNeedsDisplay()
        }
    }

    /// Paint the landing flash behind the rows its range covers — measured off
    /// `bands`, where a block's own background belongs.
    private func drawLandingFlash(in ctx: CGContext) {
        guard let flashRange, let flashStarted,
              let alpha = Landing.opacity(elapsed: Date().timeIntervalSince(flashStarted))
        else { return }
        // Core says which rows the range covers, as on the AppKit side: a block
        // ending in a link ends inside the hidden destination, and the caret
        // snap `posForOffset` applies carries that byte onto the row below.
        let span = doc.rowRangeFor(start: flashRange.lowerBound, end: flashRange.upperBound)
        let first = Int(span.first), last = Int(span.last)
        guard first <= last, !layoutEngine.rows.isEmpty else { return }
        ctx.saveGState()
        ctx.setFillColor(renderTheme.landingFlashColor.withAlphaComponent(
            renderTheme.landingFlashColor.cgColor.alpha * alpha).cgColor)
        for rl in layoutEngine.rows[max(0, first)...min(last, layoutEngine.rows.count - 1)] {
            for band in rl.bands where band.height > 0 {
                ctx.addPath(CGPath(roundedRect: band.insetBy(dx: -6, dy: 0),
                                   cornerWidth: 4, cornerHeight: 4, transform: nil))
            }
        }
        ctx.fillPath()
        ctx.restoreGState()
    }

    private func scrollCaretToVisible() {
        guard let caret = layoutEngine.caretRect(docView, theme: renderTheme),
              let scroll = enclosingScrollView() else { return }
        scroll.scrollRectToVisible(convert(caret.insetBy(dx: 0, dy: -renderTheme.lineHeight), to: scroll), animated: false)
    }

    /// Bring the caret's line back into the visible part of the scroll view —
    /// for the controller, when the keyboard has just covered it.
    func revealCaret() { scrollCaretToVisible() }

    private func enclosingScrollView() -> UIScrollView? {
        var v: UIView? = superview
        while let cur = v { if let s = cur as? UIScrollView { return s }; v = cur.superview }
        return nil
    }

    // MARK: drawing — text + code panels only; the system draws all selection UI

    public override func draw(_ rect: CGRect) {
        guard let ctx = UIGraphicsGetCurrentContext() else { return }

        // The paper first: every other thing here paints onto a sheet. Not on
        // paper itself, where the sheet is the page and the backdrop between two
        // sheets does not exist.
        if !isPaper { PageChrome.draw(layoutEngine.pages, theme: renderTheme, clip: rect, in: ctx) }
        // Under every other mark: a light behind the words, not over them.
        if !isPaper { drawLandingFlash(in: ctx) }
        if !isPaper { drawFoundText(in: ctx) }
        drawDirectiveBorders(in: ctx, dirtyRect: rect)
        // One pass for the quote bars (a run of quoted rows merges into a single
        // bar), before the rows, exactly as the AppKit surface orders it.
        BlockChrome.drawQuoteBars(layoutEngine.rows, theme: renderTheme, in: ctx)

        for rl in layoutEngine.rows {
            // Cull to the dirty band, so a scroll repaints only the visible rows.
            // Skipped rather than stopped at the first row past the band: rows run
            // top-down only while a sheet has one column, and a second one starts
            // back at the top of the same sheet.
            if rl.top >= rect.maxY || rl.top + rl.height <= rect.minY { continue }
            // A table draws its own grid (once, on its first picture row).
            if let grid = rl.table {
                if rl.tableFirst { drawTable(grid, tableTop: rl.tableTop, in: ctx) }
                continue
            }
            // A media box likewise draws once, on its first placeholder row, in
            // place of core's `🖼 alt` glyphs. Inset by the row's own prefix, so a
            // picture inside a quote or a list sits beside its gutter.
            if let box = rl.media {
                if rl.mediaFirst {
                    BlockChrome.drawMedia(box,
                                          at: box.rect(top: rl.mediaTop, left: rl.originX + rl.shaped.prefixWidth),
                                          theme: renderTheme,
                                          playing: mediaPlayers.isPlaying(box.media.src), in: ctx)
                }
                continue
            }
            // A display formula's picture, once, on its first placeholder row,
            // centred on the column.
            if let box = rl.math {
                if rl.mathFirst {
                    box.draw(in: box.rect(top: rl.mathTop, left: rl.originX + rl.shaped.prefixWidth,
                                          width: rl.columnWidth - rl.shaped.prefixWidth), ctx: ctx)
                }
                continue
            }
            // The row's bands, not one rect over its whole height: a split row has
            // a sheet edge — or a column gutter — through the middle of it, and a
            // code fill drawn over that would tile the backdrop or the gutter too.
            // Each band carries its own column, so this is where the x comes from.
            let bands = rl.bands
            let rowRect = bands.first
                ?? CGRect(x: rl.originX, y: rl.top, width: rl.columnWidth, height: rl.height)
            if rl.row.directive, let label = rl.row.directiveLabel, !label.isEmpty {
                drawDirectiveLabel(label, in: rowRect)
            }
            if rl.row.code {
                ctx.setFillColor(renderTheme.codeBackground.cgColor)
                for b in bands { ctx.fill(b.insetBy(dx: -4, dy: 0)) }
                if let lang = rl.row.codeLang, !lang.isEmpty { drawCodeLang(lang, in: rowRect) }
            }
            // The system paints selection on iOS, so no selection fill here.
            BlockChrome.drawRule(rl, theme: renderTheme, selColor: nil, in: ctx)
            BlockChrome.drawPageBreak(rl, theme: renderTheme, selColor: nil, in: ctx)
            // Draw each wrapped visual line's substring on its own line box, hung
            // at the row's indent (zero on the first line, the prefix width after).
            for (i, wl) in rl.wrapped.enumerated() {
                // `continue`, not `break`: a row's lines run down one column and
                // then back up to the top of the next, so passing the dirty band
                // once says nothing about the lines after it.
                let o = rl.lineOrigin(i)
                if o.y >= rect.maxY || o.y + rl.lineHeight <= rect.minY { continue }
                wl.attributed.draw(with: CGRect(x: o.x + wl.offset, y: o.y,
                                                width: rl.columnWidth - wl.indent, height: rl.lineHeight),
                                   options: [.usesLineFragmentOrigin], context: nil)
            }
        }

        if isPaper { return }

        if let placeholder, let box = layoutEngine.placeholderBox {
            BlockChrome.drawPlaceholder(placeholder, in: box, theme: renderTheme, in: ctx)
        }

        drawHighlightMarkers()
    }

    /// The margin pass: a small glyph beside the first line of every marked
    /// highlight, at the trailing edge — where an annotation says "there is
    /// more here" without sitting in the prose. Draws nothing when no
    /// highlight carries a marker, which is every document outside an
    /// annotating host.
    private func drawHighlightMarkers() {
        markerHits.removeAll()
        let highlights = doc.highlights()
        guard highlights.contains(where: { $0.marker != nil }) else { return }
        let box: CGFloat = 20
        let inset: CGFloat = 4
        let config = UIImage.SymbolConfiguration(pointSize: 13, weight: .regular)
        for highlight in highlights {
            guard let symbol = highlight.marker else { continue }
            let rc = doc.posForOffset(off: UInt32(highlight.start))
            guard let line = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch)) else { continue }
            let frame = CGRect(
                x: bounds.width - box - inset,
                y: line.midY - box / 2,
                width: box, height: box)
            if let image = UIImage(systemName: symbol, withConfiguration: config)?
                .withTintColor(renderTheme.secondaryColor, renderingMode: .alwaysOriginal)
            {
                image.draw(
                    at: CGPoint(
                        x: frame.midX - image.size.width / 2,
                        y: frame.midY - image.size.height / 2))
            }
            markerHits.append((frame.insetBy(dx: -10, dy: -8), highlight.id))
        }
    }

    /// Draw a table as a proportional grid — header fill and body stripes, cell
    /// text, then the grid rules — the UIKit peer of the AppKit `drawTable`.
    private func drawTable(_ grid: TableLayout, tableTop: CGFloat, in ctx: CGContext) {
        let left = layoutEngine.originX
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
        let rows = layoutEngine.rows
        var i = 0
        while i < rows.count {
            guard rows[i].isChromedDirective else { i += 1; continue }
            let start = i
            while i < rows.count, rows[i].isChromedDirective { i += 1 }
            // The run's rows reduced to their vertical bands, merged where they
            // touch. Continuously that always collapses back to a single box.
            // Paginated, a run crossing a sheet edge — between two of its rows, or
            // through the middle of one of them — comes out as one box per sheet,
            // so no outline is ever stroked across the backdrop.
            var spans: [CGRect] = []
            for rl in rows[start..<i] {
                for b in rl.bands {
                    if let last = spans.last, abs(last.maxY - b.minY) < 0.5,
                       abs(last.minX - b.minX) < 0.5 {
                        spans[spans.count - 1].size.height += b.height
                    } else {
                        spans.append(b)
                    }
                }
            }
            for span in spans {
                let rect = span.insetBy(dx: -4, dy: 0)
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
                k("\r", .shift),
                // ⌥↑ / ⌥↓ move the caret's *block* — a paragraph, a picture, a
                // list item with its children — one place, as the Mac's Format
                // menu does. Unmodified arrows stay the text-input system's.
                k(UIKeyCommand.inputUpArrow, .alternate),
                k(UIKeyCommand.inputDownArrow, .alternate)]
    }

    @objc private func handleShortcut(_ cmd: UIKeyCommand) {
        // The arrows are named, not typed, and their names are case-sensitive.
        if cmd.input == UIKeyCommand.inputUpArrow { return command { $0.moveBlockUp() } }
        if cmd.input == UIKeyCommand.inputDownArrow { return command { $0.moveBlockDown() } }
        switch (cmd.input?.lowercased(), cmd.modifierFlags.contains(.shift)) {
        case ("b", _): command { $0.toggleBold() }
        case ("i", _): command { $0.toggleItalic() }
        case ("u", _): command { $0.toggleUnderline() }
        case ("\t", true): command { $0.cellTab(forward: false) ?? $0.outdent() }
        case ("\r", true): command { $0.cellLineBreak() ?? $0.newline() }
        case ("z", false): historyManager.undo()
        case ("z", true): historyManager.redo()
        // ⇧⌘V — plain-flavor escape hatch: paste as leaf source, ignoring rich HTML.
        case ("v", true):
            let text = UIPasteboard.general.string ?? ""
            if !text.isEmpty { command { $0.paste(text: text) } }
        default: break
        }
    }

    // MARK: rich clipboard (edit-menu Cut/Copy/Paste keep twig's HTML flavour)

    public override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        // A reader copies; nothing else from the clipboard family applies.
        if isReadOnly, action == #selector(cut(_:)) || action == #selector(paste(_:)) {
            return false
        }
        switch action {
        case #selector(copy(_:)), #selector(cut(_:)): return docView.hasSelection
        // A host that claims pastes makes an image-only clipboard pasteable, so
        // the item has to be offered for one — `hasStrings` alone would grey out
        // Paste for a screenshot and there would be no way to reach `onPaste`.
        case #selector(paste(_:)):
            let pb = UIPasteboard.general
            return pb.hasStrings || (onPaste != nil && (pb.hasImages || pb.hasURLs))
        case #selector(selectAll(_:)):                return true
        case #selector(find(_:)), #selector(findNext(_:)), #selector(findPrevious(_:)):
            return true
        case #selector(findAndReplace(_:)):           return !isReadOnly
        case #selector(useSelectionForFind(_:)):      return docView.hasSelection
        default: return super.canPerformAction(action, withSender: sender)
        }
    }

    /// The *selection's* edit menu — the one `UITextInteraction` raises over a
    /// selection, as against the caret/link menu this view presents itself
    /// (see `editMenuInteraction(_:menuFor:suggestedActions:)`). This is
    /// `UITextInput`'s own hook for it, which is what finally made host items
    /// possible here: `UIMenuController.menuItems` was deprecated with no
    /// replacement a custom text view could reach until this arrived.
    ///
    /// Host verbs lead, inline, with the system's Copy/Look Up kept after
    /// them: a host adds to the reader's menu, it does not take the menu over.
    /// Find Selection follows them, as it does in a `UITextView`: on a phone
    /// with no keyboard it is the way into the find panel.
    public func editMenu(
        for textRange: UITextRange, suggestedActions: [UIMenuElement]
    ) -> UIMenu? {
        let host = selectionMenuActions?() ?? []
        let findSelection = UIAction(title: loc("menu.findSelection", "Find Selection"),
                                     image: UIImage(systemName: "text.magnifyingglass")) { [weak self] _ in
            self?.useSelectionForFind(nil)
            self?.performFind(.showFind)
        }
        let find = UIMenu(options: .displayInline, children: [findSelection])
        guard !host.isEmpty else { return UIMenu(children: suggestedActions + [find]) }
        return UIMenu(children: [UIMenu(options: .displayInline, children: host)] + suggestedActions + [find])
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
        // Before the text flavors: an image-only clipboard has no text, so a host
        // asked later would never hear about it. See `LeafEditorModel.onPaste`.
        if onPaste?() == true { return }
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
            guard let caret = layoutEngine.caretRect(src: cur, row: Int(rc.row), ch: Int(rc.ch),
                                                     theme: renderTheme) else { break }
            // Probe from the caret's full line band (a table cell's padding is
            // cleared) and resolve the table-aware way, or a probe into a table
            // teleports to its top-left cell. See the AppKit peer's `moveVertical`.
            let band = layoutEngine.caretBand(src: cur)
            var probeY = up ? (band?.minY ?? caret.minY) - 1 : (band?.maxY ?? caret.maxY) + 1
            let probe = CGPoint(x: caret.minX, y: probeY)
            let next: Int
            if let off = layoutEngine.tableHitOffset(probe) {
                next = Int(doc.snapOffset(off: UInt32(off)))
            } else {
                var (row, ch) = layoutEngine.hit(probe)
                // Step over the short blank gap row a block boundary is drawn with:
                // probing one line past the caret lands inside it, where the hit
                // snaps back and the step stalls between a paragraph and the list or
                // code block below. See the AppKit peer's `moveVertical`.
                let rows = layoutEngine.rows
                var guardCount = 0
                while rows.indices.contains(row), rows[row].row.isBlockGap, guardCount < rows.count {
                    let r = rows[row]
                    probeY = up ? r.top - 1 : r.top + r.height + 1
                    (row, ch) = layoutEngine.hit(CGPoint(x: caret.minX, y: probeY))
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
        let o = off(position)
        let rc = doc.posForOffset(off: UInt32(o))
        // Through the table-aware path: a position in a cell is drawn in that
        // cell, not at the grid's top-left (see `EditorLayout.caretRect(src:)`).
        return layoutEngine.caretRect(src: o, row: Int(rc.row), ch: Int(rc.ch), theme: renderTheme) ?? .zero
    }

    public func selectionRects(for range: UITextRange) -> [UITextSelectionRect] {
        guard let r = range as? LeafTextRange else { return [] }
        let s = doc.posForOffset(off: UInt32(r.from.offset))
        let e = doc.posForOffset(off: UInt32(r.to.offset))
        // One rect per visual line the selection touches in each block.
        var rects: [UITextSelectionRect] = layoutEngine
            .rangeRects(from: (Int(s.row), Int(s.ch)), to: (Int(e.row), Int(e.ch)))
            .map { LeafSelectionRect(rect: $0.rect, containsStart: $0.containsStart, containsEnd: $0.containsEnd) }
        // Tables carry no `wrapped` lines, so the row walk above skips them; add
        // the highlight over any table cells the range covers, keyed by source
        // offset (the coordinate a cell is laid out by).
        rects.append(contentsOf: layoutEngine.tableSelectionRects(
            from: r.from.offset, to: r.to.offset
        ).map { LeafSelectionRect(rect: $0.rect, containsStart: $0.containsStart, containsEnd: $0.containsEnd) })
        return rects
    }

    public func firstRect(for range: UITextRange) -> CGRect {
        selectionRects(for: range).first?.rect ?? .zero
    }

    public func closestPosition(to point: CGPoint) -> UITextPosition? {
        // Inside a table, the point maps through the grid straight to a source
        // offset; elsewhere it's the plain row/ch hit-test.
        if let off = layoutEngine.tableHitOffset(point) {
            return LeafTextPosition(off)
        }
        // Under the last block the nearest position is the document's end,
        // wherever the finger is horizontally — see `handleMediaTap`, which
        // opens the paragraph there when a plain tap asks for it.
        if !isSourceView, layoutEngine.isPastEnd(point) {
            return LeafTextPosition(Int(doc.docEndOffset()))
        }
        let (row, ch) = layoutEngine.hit(point)
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

// MARK: - Gesture coexistence

// MARK: - Accessibility — the document as VoiceOver reads it
//
// A `UIView` is invisible to VoiceOver until it says otherwise, and conforming
// to `UITextInput` does not say it. This is the AppKit peer's `NSAccessibility`
// text area, reached the UIKit way: the view is one element whose value is the
// visible text, and `UIAccessibilityReadingContent` lets VoiceOver read it a
// visual line at a time — the Read Page rotor, swipe-to-next-line, and the
// touch-to-hear-this-line exploration all come from these four answers.
extension LeafTextView: UIAccessibilityReadingContent {
    public override var isAccessibilityElement: Bool {
        get { true }
        set {}
    }

    /// A host's label if it set one, else what the surface is.
    public override var accessibilityLabel: String? {
        get { hostAccessibilityLabel ?? loc("a11y.document", "Document") }
        set { hostAccessibilityLabel = newValue }
    }

    /// The visible text — what a sighted reader sees, delimiters hidden.
    public override var accessibilityValue: String? {
        get { doc.textInRange(from: 0, to: doc.docEndOffset()) }
        set {}
    }

    public override var accessibilityTraits: UIAccessibilityTraits {
        get { isReadOnly ? .staticText : super.accessibilityTraits }
        set { super.accessibilityTraits = newValue }
    }

    /// A double-tap puts the caret in the document, as it does in a text field.
    public override func accessibilityActivate() -> Bool {
        guard !isReadOnly else { return false }
        return becomeFirstResponder()
    }

    /// The visual lines, top to bottom — see `readingLines`.
    private func currentReadingLines() -> [(text: String, frame: CGRect)] {
        if let cached = readingLines { return cached }
        var lines: [(text: String, frame: CGRect)] = []
        for rl in layoutEngine.rows {
            if rl.row.isBlockGap { continue }
            // A table or a media block is one thing to a reader, spoken once
            // from the row that carries its box.
            if rl.table != nil || rl.media != nil || rl.math != nil {
                guard rl.tableFirst || rl.mediaFirst || rl.mathFirst, let box = rl.lineBoxes.first else { continue }
                let text = rl.attributed.string.trimmingCharacters(in: .whitespacesAndNewlines)
                if !text.isEmpty { lines.append((text, box)) }
                continue
            }
            let boxes = rl.lineBoxes
            for (i, wl) in rl.wrapped.enumerated() where boxes.indices.contains(i) {
                let text = wl.attributed.string.trimmingCharacters(in: .whitespacesAndNewlines)
                if !text.isEmpty { lines.append((text, boxes[i])) }
            }
        }
        readingLines = lines
        return lines
    }

    public func accessibilityLineNumber(for point: CGPoint) -> Int {
        // `point` is in screen coordinates; the lines are in the view's.
        let inView = screenPointToView(point)
        let lines = currentReadingLines()
        if let i = lines.firstIndex(where: { $0.frame.minY <= inView.y && inView.y < $0.frame.maxY }) {
            return i
        }
        return NSNotFound
    }

    /// Screen coordinates to this view's: the inverse of what
    /// `UIAccessibility.convertToScreenCoordinates` does for a frame.
    private func screenPointToView(_ p: CGPoint) -> CGPoint {
        guard let window else { return p }
        return convert(window.convert(p, from: window.screen.coordinateSpace), from: window)
    }

    public func accessibilityContent(forLineNumber lineNumber: Int) -> String? {
        let lines = currentReadingLines()
        return lines.indices.contains(lineNumber) ? lines[lineNumber].text : nil
    }

    public func accessibilityFrame(forLineNumber lineNumber: Int) -> CGRect {
        let lines = currentReadingLines()
        guard lines.indices.contains(lineNumber) else { return .null }
        return UIAccessibility.convertToScreenCoordinates(lines[lineNumber].frame, in: self)
    }

    public func accessibilityPageContent() -> String? { accessibilityValue }
}

extension LeafTextView: UIGestureRecognizerDelegate {
    /// `mediaTap` never competes with `textInteraction`'s own recognisers — the
    /// system owns caret placement, selection, the loupe and the edit menu, and
    /// activating a media box is strictly additive to whichever of those the
    /// touch was also going to drive. Failing to say so would make the two
    /// exclusive, and the system's would win. The pinch is the same alongside
    /// the scroll view's pan: two fingers that spread and drift both zoom and
    /// scroll, as they do in every zooming scroll view.
    public func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
    ) -> Bool { true }
}

// MARK: - Edit menu

extension LeafTextView: UIEditMenuInteractionDelegate {
    /// Add the footnote and link actions to the menu the long press raises, ahead
    /// of the system's Cut/Copy/Paste. They appear only when the caret stands in
    /// one, so an ordinary press gets exactly the menu it always did.
    ///
    /// This is the phone's whole vocabulary for reaching either now that a tap
    /// places the caret: no ⌘ to hold, no pointer to hover, and a long press is
    /// the one gesture that means "something other than typing here".
    ///
    /// Footnotes lead, and never appear beside a link's entries — the caret is on
    /// a reference, in a note, in a link, or in none of them.
    public func editMenuInteraction(
        _ interaction: UIEditMenuInteraction,
        menuFor configuration: UIEditMenuConfiguration,
        suggestedActions: [UIMenuElement]
    ) -> UIMenu? {
        let media: [UIAction] = showableMediaAtPress.map { src in
            [UIAction(title: loc("menu.showMedia", "Show Attachment")) { [weak self] _ in
                self?.onShowMedia?(src)
            }]
        } ?? []
        let actions: [UIAction] = footnoteMenuActions() + media + doc
            .linkActionsAtCaret(wikilinks: recognizesWikilinks, canEdit: onEditLink != nil,
                                canPeek: onPeekLink != nil)
            .map { action in
                switch action {
                case .peek:
                    return UIAction(title: loc("menu.previewLink", "Preview Link")) { [weak self] _ in
                        self?.showLinkPeekAtCaret()
                    }
                case .open:
                    return UIAction(title: loc("menu.openLink", "Open Link")) { [weak self] _ in
                        self?.openLinkAtCaret()
                    }
                case .edit:
                    return UIAction(title: loc("menu.editLink", "Edit Link…")) { [weak self] _ in
                        self?.editLink(nil)
                    }
                case .copy:
                    return UIAction(title: loc("menu.copyLink", "Copy Link")) { [weak self] _ in
                        self?.copyLink(nil)
                    }
                }
            }
        guard !actions.isEmpty else { return nil }
        // `.displayInline` keeps them as a group in the same menu rather than
        // folding them behind a submenu title.
        return UIMenu(children: [UIMenu(options: .displayInline, children: actions)] + suggestedActions)
    }
}
// MARK: - Find — the system find panel over the visible text

/// A found range's identity, in source bytes: what a decoration is keyed by.
private struct FoundRange: Hashable {
    let from: Int
    let to: Int
    init(_ range: UITextRange) {
        let r = range as? LeafTextRange
        from = r?.from.offset ?? 0
        to = r?.to.offset ?? 0
    }
}

/// What a host's Find menu asks of the iOS view — the AppKit peer takes an
/// `NSTextFinder.Action` for the same items.
public enum LeafFindAction: Sendable {
    /// Find… — the panel, with the search field.
    case showFind
    /// Find and Replace… — the panel with the replace field too.
    case showReplace
    /// Find Next and Find Previous: the next match after, or before, the
    /// current one.
    case next, previous
    /// Use Selection for Find: the selected text becomes the search string.
    case useSelection
    /// Put the panel away.
    case hide
}

extension LeafTextView: UIFindInteractionDelegate {
    public func findInteraction(_ interaction: UIFindInteraction, sessionFor view: UIView) -> UIFindSession? {
        UITextSearchingFindSession(searchableObject: self)
    }

    /// Carry out a Find menu item.
    public func performFind(_ action: LeafFindAction) {
        switch action {
        case .showFind: findInteraction.presentFindNavigator(showingReplace: false)
        case .showReplace: findInteraction.presentFindNavigator(showingReplace: !isReadOnly)
        case .next: findInteraction.findNext()
        case .previous: findInteraction.findPrevious()
        case .useSelection: useSelectionForFind(nil)
        case .hide: findInteraction.dismissFindNavigator()
        }
    }

    // The responder chain's names for the same items — UIKit's own Edit ▸ Find
    // menu, and a hardware keyboard's ⌘F where the host's menu has none.
    public override func find(_ sender: Any?) { performFind(.showFind) }
    public override func findAndReplace(_ sender: Any?) { performFind(.showReplace) }
    public override func findNext(_ sender: Any?) { performFind(.next) }
    public override func findPrevious(_ sender: Any?) { performFind(.previous) }
    public override func useSelectionForFind(_ sender: Any?) {
        let (lo, hi) = (min(Int(doc.anchorOffset()), Int(doc.caretOffset())),
                        max(Int(doc.anchorOffset()), Int(doc.caretOffset())))
        guard hi > lo else { return }
        findInteraction.searchText = doc.textInRange(from: UInt32(lo), to: UInt32(hi))
    }

    /// The text changed under an open panel — typed, pasted, undone, a
    /// toolbar command — so its matches may be stale: run its search again.
    /// Not `invalidateFoundResults`, which empties the panel ("0") and leaves
    /// it empty until the query is edited. Once per turn, however many frames
    /// the change took, and after the change rather than inside it; and not
    /// for a replace the panel asked for, which it follows up itself.
    fileprivate func invalidateFoundResultsAfterEdit() {
        guard !replacingFoundText, findInteraction.activeFindSession != nil, !findInvalidationQueued else { return }
        findInvalidationQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.findInvalidationQueued = false
            guard let session = self.findInteraction.activeFindSession, let last = self.lastFindQuery else { return }
            session.performSearch(query: last.query, options: last.options)
        }
    }

    /// The lit matches, under the text: a wash on every match found, and a
    /// stronger one on the current one — the two a `UITextView` draws.
    fileprivate func drawFoundText(in ctx: CGContext) {
        guard !foundDecorations.isEmpty else { return }
        // The current match last, so a wash on a neighbour never covers it.
        for (style, box) in foundTextRects.sorted(by: { $0.style.rawValue < $1.style.rawValue }) {
            let color = style == .highlighted
                ? UIColor.systemYellow.withAlphaComponent(0.85)
                : UIColor.systemYellow.withAlphaComponent(0.3)
            ctx.setFillColor(color.cgColor)
            ctx.fill(box.insetBy(dx: -1, dy: 0))
        }
    }

    /// The lit boxes of every decorated match, by style — what `draw` paints.
    var foundTextRects: [(style: UITextSearchFoundTextStyle, rect: CGRect)] {
        foundDecorations.flatMap { range, style in
            layoutEngine.rangeRects(fromByte: range.from, toByte: range.to, in: doc)
                .filter { $0.width > 0 }.map { (style, $0) }
        }
    }

    /// A match as the panel's range: the source bytes the UTF-16 match covers
    /// in the visible text.
    private func foundRange(_ utf16: NSRange) -> LeafTextRange {
        let (from, to) = doc.matchBounds(utf16)
        return LeafTextRange(LeafTextPosition(from), LeafTextPosition(to))
    }

    /// Every match of `query` the panel's options allow, first to last.
    func foundRanges(of query: String, options: UITextSearchOptions) -> [LeafTextRange] {
        let word: TextSearch.WordMatch
        switch options.wordMatchMethod {
        case .startsWith: word = .startsWith
        case .fullWord: word = .fullWord
        default: word = .contains
        }
        return TextSearch.matches(of: query, in: doc.visibleText(),
                                  options: options.stringCompareOptions, word: word)
            .map(foundRange)
    }
}

extension LeafTextView: UITextSearching {
    /// One document: the view's own text.
    public typealias DocumentIdentifier = AnyHashable?

    public func compare(_ foundRange: UITextRange, toRange: UITextRange,
                        document: DocumentIdentifier?) -> ComparisonResult {
        let a = FoundRange(foundRange), b = FoundRange(toRange)
        if a.from != b.from { return a.from < b.from ? .orderedAscending : .orderedDescending }
        if a.to != b.to { return a.to < b.to ? .orderedAscending : .orderedDescending }
        return .orderedSame
    }

    /// The search runs over what the reader sees — the words without their
    /// markup in the rendered view, the source in the source view — which is
    /// the text the system's UTF-16 ranges index everywhere else.
    public func performTextSearch(queryString: String, options: UITextSearchOptions,
                                  resultAggregator: UITextSearchAggregator<DocumentIdentifier>) {
        lastFindQuery = (queryString, options)
        for range in foundRanges(of: queryString, options: options) {
            resultAggregator.foundRange(range, searchString: queryString, document: nil)
        }
        resultAggregator.finishedSearching()
    }

    public func decorate(foundTextRange: UITextRange, document: DocumentIdentifier?,
                         usingStyle style: UITextSearchFoundTextStyle) {
        let key = FoundRange(foundTextRange)
        if style == .normal { foundDecorations[key] = nil } else { foundDecorations[key] = style }
        setNeedsDisplay()
    }

    public func clearAllDecoratedFoundText() {
        guard !foundDecorations.isEmpty else { return }
        foundDecorations.removeAll()
        setNeedsDisplay()
    }

    /// The current match is the selection, as the Mac's find bar leaves it: the
    /// panel closes on the word it stopped at, selected.
    public func willHighlight(foundTextRange: UITextRange, document: DocumentIdentifier?) {
        let r = FoundRange(foundTextRange)
        // The exact bytes, snapping neither end: a match inside `**word**` is
        // the word, not one stop short of it.
        command { $0.selectRange(start: UInt32(r.from), end: UInt32(r.to)) }
    }

    public func scrollRangeToVisible(_ range: UITextRange, inDocument: DocumentIdentifier?) {
        let r = FoundRange(range)
        let boxes = layoutEngine.rangeRects(fromByte: r.from, toByte: r.to, in: doc)
        guard let first = boxes.first, let scroll = enclosingScrollView() else { return }
        let union = boxes.dropFirst().reduce(first) { $0.union($1) }
        // A line's worth of room either side, as the caret gets, so the match
        // is not left flush against the panel or the top bar.
        scroll.scrollRectToVisible(convert(union.insetBy(dx: 0, dy: -renderTheme.lineHeight), to: scroll),
                                   animated: true)
    }

    public var supportsTextReplacement: Bool { !isReadOnly }

    public func shouldReplace(foundTextRange: UITextRange, document: DocumentIdentifier?,
                              withText: String) -> Bool { !isReadOnly }

    public func replace(foundTextRange: UITextRange, document: DocumentIdentifier?,
                        withText replacementText: String) {
        let r = FoundRange(foundTextRange)
        replacingFoundText = true
        defer { replacingFoundText = false }
        command { $0.replaceRange(from: UInt32(r.from), to: UInt32(r.to), text: replacementText) }
    }

    /// Every match, last to first, so each replacement leaves the offsets of the
    /// ones still to go where the search found them — inside one core undo
    /// group, so a single ⌘Z puts every match back and a single redo replaces
    /// them all again.
    public func replaceAll(queryString: String, options: UITextSearchOptions, withText replacementText: String) {
        guard !isReadOnly else { return }
        let ranges = foundRanges(of: queryString, options: options)
        guard !ranges.isEmpty else { return }
        replacingFoundText = true
        defer { replacingFoundText = false }
        notifyingDelegate {
            doc.beginUndoGroup()
            defer { doc.endUndoGroup() }
            for r in ranges.reversed() {
                render(doc.replaceRange(from: UInt32(r.from.offset), to: UInt32(r.to.offset), text: replacementText))
            }
        }
    }
}
#endif
