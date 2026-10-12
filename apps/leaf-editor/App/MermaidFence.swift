import CoreText
import Foundation
import LeafFFI
import LeafMermaidFFI
import LeafUI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// The demo's Mermaid renderer and editor, on leaf's `regionRenderers` and
/// `regionEditors`: every ```mermaid fence is drawn as its diagram — a picture
/// the layout places, pages and prints with the text — and → in front of it,
/// ← behind it or a click opens its source beside a live preview, written back
/// to the document a keystroke at a time. The web demo's `mermaid-editor.js`,
/// in AppKit and UIKit. The diagrams are rendered by leaf-mermaid (mmdr, in
/// Rust) through `renderMermaid`, with labels measured by Core Text, and drawn
/// as vectors through resvg. leaf's own views draw no diagrams. See
/// docs/proposals/embedded-editors.md.
enum MermaidFence {
    static let kind = "fence:mermaid"

    /// A closed fence's picture: its diagram, at most `maxDrawingHeight`
    /// tall, or the renderer's complaint about the source in its place.
    static let renderer: RegionRenderer = { region, context in
        switch MermaidDrawing.render(region.content, dark: context.dark) {
        case let .svg(svg):
            guard var picture = RegionPicture(svg: svg) else { return nil }
            // Drawn into whatever box it is given, so a smaller box is all it
            // takes to scale it.
            let scale = min(1, maxDrawingHeight / max(1, picture.height))
            picture.width = picture.width.map { $0 * scale }
            picture.height *= scale
            picture.baseline = picture.height
            return picture
        case let .fault(message):
            return faultPicture(message)
        }
    }

    static let editor = RegionEditor(
        open: { region, entry, host in MermaidSession(region, entry: entry, host: host) })

    /// How tall the open editor is.
    static let editorHeight: CGFloat = 280
    /// The tallest a closed diagram is drawn; a taller one is scaled to fit.
    static let maxDrawingHeight: CGFloat = 600

    // A fence's content is whole lines, each with its newline; the text view
    // shows it without the last one, and an empty text view is an empty fence.
    static func text(_ content: String) -> String {
        content.hasSuffix("\n") ? String(content.dropLast()) : content
    }

    static func content(_ text: String) -> String { text.isEmpty ? "" : text + "\n" }

    /// Where diagrams are rendered when nothing waits on them: off the main
    /// thread, one at a time.
    static let queue = DispatchQueue(label: "org.diaryx.leaf.mermaid", qos: .userInitiated)

    /// `message` set in red monospace, as a picture: what a fence whose
    /// source does not parse shows where its diagram would be.
    static func faultPicture(_ message: String) -> RegionPicture {
        let font = CTFontCreateUIFontForLanguage(.userFixedPitch, 11, nil)
            ?? CTFontCreateWithName("Menlo" as CFString, 11, nil)
        let text = NSAttributedString(string: message, attributes: [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): CGColor(red: 0.85, green: 0.2, blue: 0.2, alpha: 1),
        ])
        let setter = CTFramesetterCreateWithAttributedString(text)
        let size = CTFramesetterSuggestFrameSizeWithConstraints(
            setter, CFRange(location: 0, length: 0), nil, CGSize(width: 480, height: CGFloat.greatestFiniteMagnitude), nil)
        let w = size.width.rounded(.up), h = size.height.rounded(.up)
        return RegionPicture(width: w, height: h) { ctx, rect in
            // Core Text sets its lines y-up; the picture is drawn y-down.
            ctx.saveGState()
            ctx.textMatrix = .identity
            ctx.translateBy(x: rect.minX, y: rect.maxY)
            ctx.scaleBy(x: rect.width / max(1, w), y: -rect.height / max(1, h))
            let frame = CTFramesetterCreateFrame(setter, CFRange(location: 0, length: 0),
                                                 CGPath(rect: CGRect(x: 0, y: 0, width: w, height: h), transform: nil), nil)
            CTFrameDraw(frame, ctx)
            ctx.restoreGState()
        }
    }
}

/// Label widths for `renderMermaid`, from Core Text: the first face of the
/// theme's family list that is installed, which is the face resvg will draw
/// the picture's text in.
final class CoreTextMeasure: MermaidMeasure, @unchecked Sendable {
    static let shared = CoreTextMeasure()

    func width(text: String, size: Float, family: String) -> Float? {
        let font = Self.font(family, size: CGFloat(size))
        let attributed = NSAttributedString(string: text,
                                            attributes: [NSAttributedString.Key(kCTFontAttributeName as String): font])
        let line = CTLineCreateWithAttributedString(attributed)
        return Float(CTLineGetTypographicBounds(line, nil, nil, nil))
    }

    /// The first family in a CSS family list that Core Text has, else
    /// Helvetica — the face a generic `sans-serif` comes to.
    static func font(_ list: String, size: CGFloat) -> CTFont {
        for part in list.split(separator: ",") {
            let name = part.trimmingCharacters(in: CharacterSet(charactersIn: " '\""))
            guard !name.isEmpty else { continue }
            if ["sans-serif", "system-ui", "-apple-system", "ui-sans-serif"].contains(name.lowercased()) { break }
            let descriptor = CTFontDescriptorCreateWithAttributes(
                [kCTFontFamilyNameAttribute: name] as CFDictionary)
            let font = CTFontCreateWithFontDescriptor(descriptor, size, nil)
            if (CTFontCopyFamilyName(font) as String).caseInsensitiveCompare(name) == .orderedSame { return font }
        }
        return CTFontCreateWithName("Helvetica" as CFString, size, nil)
    }
}

/// What rendering a diagram came to: its SVG, or the fault in its place.
enum Rendered {
    case svg(String)
    case fault(String)
}

/// The open editor's preview of one diagram: its picture, or what the
/// renderer said about the source in its place, rendered off the main thread
/// as the source is typed. A closed fence is no view: it is the renderer's
/// picture (`MermaidFence.renderer`).
final class MermaidDrawing: LeafView {
    private let picture = SVGView(svg: "")
    private let fault = MermaidDrawing.label()
    /// The source last asked for, so an older render that finishes late is
    /// dropped.
    private var asked = ""

    init(source: String) {
        super.init(frame: CGRect(x: 0, y: 0, width: 300, height: 120))
        addSubview(picture)
        addSubview(fault)
        show(source)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    /// Render `source` off the main thread and draw it when it is done, if
    /// nothing newer was asked for meanwhile.
    func show(_ source: String) {
        asked = source
        let dark = isDark
        MermaidFence.queue.async { [weak self] in
            let result = Self.render(source, dark: dark)
            DispatchQueue.main.async {
                guard let self, self.asked == source else { return }
                self.apply(result)
            }
        }
    }

    private var isDark: Bool {
        #if canImport(AppKit)
        effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        #else
        traitCollection.userInterfaceStyle == .dark
        #endif
    }

    /// The picture's SVG, or the fault.
    static func render(_ source: String, dark: Bool) -> Rendered {
        guard !source.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return .fault("(empty diagram)") }
        do {
            return .svg(try renderMermaid(source: source, dark: dark, measure: CoreTextMeasure.shared).svg)
        } catch MermaidError.Parse(let message) {
            return .fault(message)
        } catch {
            return .fault(String(describing: error))
        }
    }

    private func apply(_ result: Rendered) {
        switch result {
        case let .svg(svg):
            picture.setSVG(svg)
            picture.isHidden = false
            fault.isHidden = true
        case let .fault(message):
            picture.isHidden = true
            fault.isHidden = false
            #if canImport(AppKit)
            fault.stringValue = message
            #else
            fault.text = message
            #endif
        }
        invalidateIntrinsicContentSize()
        layoutPictures()
    }

    override var intrinsicContentSize: CGSize {
        let height = picture.isHidden ? 44 : min(MermaidFence.maxDrawingHeight, max(24, picture.naturalSize.height))
        return CGSize(width: -1, height: height + 16)
    }

    private func layoutPictures() {
        let inner = bounds.insetBy(dx: 8, dy: 8)
        picture.frame = inner
        fault.frame = inner
    }

    #if canImport(AppKit)
    override var isFlipped: Bool { true }
    override func layout() {
        super.layout()
        layoutPictures()
    }

    private static func label() -> NSTextField {
        let l = NSTextField(wrappingLabelWithString: "")
        l.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        l.textColor = .systemRed
        l.isHidden = true
        return l
    }
    #else
    override func layoutSubviews() {
        super.layoutSubviews()
        layoutPictures()
    }

    private static func label() -> UILabel {
        let l = UILabel()
        l.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        l.textColor = .systemRed
        l.numberOfLines = 0
        l.isHidden = true
        return l
    }
    #endif
}

/// A fence open in the editor: its source in a text view, the diagram beside
/// it (below it on iOS), redrawn as it is typed.
final class MermaidSession: NSObject, RegionSession {
    private let host: RegionHost
    private let entry: RegionEntry
    private let preview: MermaidDrawing
    private let box: MermaidEditorBox
    private var redraw: DispatchWorkItem?

    var view: LeafView? { box }

    init(_ region: RegionView, entry: RegionEntry, host: RegionHost) {
        self.host = host
        self.entry = entry
        preview = MermaidDrawing(source: region.content)
        box = MermaidEditorBox(text: MermaidFence.text(region.content), preview: preview)
        super.init()
        box.session = self
    }

    func focus() {
        box.focus(atEnd: { if case .home = entry { return false } else { return true } }())
    }

    func reload(content: String) -> Bool {
        box.setText(MermaidFence.text(content))
        schedule()
        return true
    }

    func close() { redraw?.cancel() }

    /// The text view's text changed: write it back, and redraw in a moment.
    fileprivate func textChanged(_ text: String) {
        host.replace(MermaidFence.content(text))
        schedule()
    }

    fileprivate func leave(atEnd: Bool) { host.leave(atEnd: atEnd) }
    fileprivate func undo() { host.undo() }
    fileprivate func redo() { host.redo() }

    private func schedule() {
        redraw?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            preview.show(MermaidFence.content(box.text))
        }
        redraw = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08, execute: work)
    }
}

#if canImport(AppKit)
/// The open editor on the Mac: the source on the left, the preview on the
/// right.
final class MermaidEditorBox: NSView, NSTextViewDelegate {
    weak var session: MermaidSession?
    private let source = MermaidSourceView()
    private let scroll = NSScrollView()

    init(text: String, preview: MermaidDrawing) {
        super.init(frame: CGRect(x: 0, y: 0, width: 600, height: MermaidFence.editorHeight))
        source.string = text
        source.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        source.isRichText = false
        source.allowsUndo = false
        source.isAutomaticQuoteSubstitutionEnabled = false
        source.isAutomaticDashSubstitutionEnabled = false
        source.isAutomaticTextReplacementEnabled = false
        source.textContainerInset = CGSize(width: 6, height: 6)
        source.delegate = self
        source.box = self
        scroll.documentView = source
        scroll.hasVerticalScroller = true
        scroll.borderType = .lineBorder
        source.autoresizingMask = [.width]
        source.isVerticallyResizable = true
        source.textContainer?.widthTracksTextView = true
        addSubview(scroll)
        addSubview(preview)
        setAccessibilityIdentifier("mermaid-editor")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var text: String { source.string }

    override var intrinsicContentSize: CGSize {
        CGSize(width: NSView.noIntrinsicMetric, height: MermaidFence.editorHeight)
    }

    override func layout() {
        super.layout()
        let half = (bounds.width - 8) / 2
        scroll.frame = CGRect(x: 0, y: 0, width: half, height: bounds.height)
        subviews.last?.frame = CGRect(x: half + 8, y: 0, width: half, height: bounds.height)
    }

    func focus(atEnd: Bool) {
        window?.makeFirstResponder(source)
        let at = atEnd ? (source.string as NSString).length : 0
        source.setSelectedRange(NSRange(location: at, length: 0))
    }

    func setText(_ text: String) {
        guard text != source.string else { return }
        let at = min(source.selectedRange().location, (text as NSString).length)
        source.string = text
        source.setSelectedRange(NSRange(location: at, length: 0))
    }

    func textDidChange(_ notification: Notification) { session?.textChanged(source.string) }

    /// The keys that leave the fence: Escape, and ↑ on its first line or ↓ on
    /// its last.
    func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        let s = textView.string as NSString
        let r = textView.selectedRange()
        switch selector {
        case #selector(NSResponder.cancelOperation(_:)):
            session?.leave(atEnd: true)
        case #selector(NSResponder.moveUp(_:)) where r.length == 0
            && s.substring(to: r.location).range(of: "\n") == nil:
            session?.leave(atEnd: false)
        case #selector(NSResponder.moveDown(_:)) where r.length == 0
            && s.substring(from: r.location).range(of: "\n") == nil:
            session?.leave(atEnd: true)
        default:
            return false
        }
        return true
    }

    fileprivate func undo() { session?.undo() }
    fileprivate func redo() { session?.redo() }
}

/// The source text view, whose Undo and Redo are the document's.
final class MermaidSourceView: NSTextView {
    weak var box: MermaidEditorBox?

    @objc func undo(_ sender: Any?) { box?.undo() }
    @objc func redo(_ sender: Any?) { box?.redo() }

    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(undo(_:)) || item.action == #selector(redo(_:)) { return true }
        return super.validateUserInterfaceItem(item)
    }
}
#elseif canImport(UIKit)
/// The open editor on iOS: the source above the preview.
final class MermaidEditorBox: UIView, UITextViewDelegate {
    weak var session: MermaidSession?
    private let source = MermaidSourceView()
    private let preview: MermaidDrawing

    init(text: String, preview: MermaidDrawing) {
        self.preview = preview
        super.init(frame: CGRect(x: 0, y: 0, width: 360, height: MermaidFence.editorHeight))
        source.text = text
        source.font = .monospacedSystemFont(ofSize: 14, weight: .regular)
        source.autocorrectionType = .no
        source.autocapitalizationType = .none
        source.smartQuotesType = .no
        source.smartDashesType = .no
        source.layer.borderWidth = 1
        source.layer.borderColor = UIColor.separator.cgColor
        source.layer.cornerRadius = 6
        source.delegate = self
        addSubview(source)
        addSubview(preview)
        accessibilityIdentifier = "mermaid-editor"
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var text: String { source.text }

    override var intrinsicContentSize: CGSize {
        CGSize(width: UIView.noIntrinsicMetric, height: MermaidFence.editorHeight)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let half = (bounds.height - 8) / 2
        source.frame = CGRect(x: 0, y: 0, width: bounds.width, height: half)
        preview.frame = CGRect(x: 0, y: half + 8, width: bounds.width, height: half)
    }

    func focus(atEnd: Bool) {
        source.becomeFirstResponder()
        let at = atEnd ? (source.text as NSString).length : 0
        source.selectedRange = NSRange(location: at, length: 0)
    }

    func setText(_ text: String) {
        guard text != source.text else { return }
        let at = min(source.selectedRange.location, (text as NSString).length)
        source.text = text
        source.selectedRange = NSRange(location: at, length: 0)
    }

    func textViewDidChange(_ textView: UITextView) { session?.textChanged(source.text) }

    override var keyCommands: [UIKeyCommand]? {
        [UIKeyCommand(input: UIKeyCommand.inputEscape, modifierFlags: [], action: #selector(escape)),
         UIKeyCommand(input: "z", modifierFlags: .command, action: #selector(undoDocument)),
         UIKeyCommand(input: "z", modifierFlags: [.command, .shift], action: #selector(redoDocument))]
    }

    @objc private func escape() { session?.leave(atEnd: true) }
    @objc private func undoDocument() { session?.undo() }
    @objc private func redoDocument() { session?.redo() }
}

/// The source text view, whose undo is the document's: a shake or the
/// keyboard's undo reaches the editor's history, not one of its own.
final class MermaidSourceView: UITextView {
    override var undoManager: UndoManager? { superview?.superview?.undoManager }
}
#endif
