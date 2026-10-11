import Foundation
import LeafFFI
import LeafUI
import WebKit

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// The demo's Mermaid editor, on leaf's `regionEditors`: every ```mermaid
/// fence is drawn as its diagram, and → in front of it, ← behind it or a click
/// opens its source beside a live preview, written back to the document a
/// keystroke at a time. The web demo's `mermaid-editor.js`, in AppKit and
/// UIKit. leaf knows nothing of Mermaid: the diagrams are drawn by mermaid.js in
/// a web view, fetched from a CDN. See docs/proposals/embedded-editors.md.
enum MermaidFence {
    static let kind = "fence:mermaid"

    static let editor = RegionEditor(
        view: { region in MermaidDrawing(source: region.content, passesClicks: true) },
        open: { region, entry, host in MermaidSession(region, entry: entry, host: host) })

    /// How tall a closed diagram is drawn: fixed, so the layout never waits on
    /// a web view to measure itself. The diagram is fitted inside.
    static let drawingHeight: CGFloat = 200
    /// How tall the open editor is.
    static let editorHeight: CGFloat = 260

    // A fence's content is whole lines, each with its newline; the text view
    // shows it without the last one, and an empty text view is an empty fence.
    static func text(_ content: String) -> String {
        content.hasSuffix("\n") ? String(content.dropLast()) : content
    }

    static func content(_ text: String) -> String { text.isEmpty ? "" : text + "\n" }
}

/// One diagram, drawn by mermaid.js in a web view.
final class MermaidDrawing: WKWebView {
    private static let page = """
    <!doctype html><meta name="viewport" content="width=device-width">
    <style>
      html, body { margin: 0; height: 100%; background: transparent; }
      #d { display: flex; align-items: center; justify-content: center; height: 100%; }
      #d svg { max-width: 100%; max-height: 100%; }
      #d.fault { display: block; color: #c0392b; font: 12px ui-monospace, Menlo, monospace;
        white-space: pre-wrap; padding: 8px; }
    </style>
    <div id="d"></div>
    <script type="module">
      import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";
      const dark = matchMedia("(prefers-color-scheme: dark)").matches;
      mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: dark ? "dark" : "default" });
      let n = 0;
      window.draw = async (src) => {
        const d = document.getElementById("d");
        const ticket = ++n;
        try {
          const { svg } = src.trim() ? await mermaid.render("m" + ticket, src) : { svg: "" };
          if (ticket === n) { d.className = ""; d.innerHTML = svg; }
        } catch (e) {
          if (ticket === n) { d.className = "fault"; d.textContent = String(e?.message ?? e); }
        }
      };
      window.ready = true;
      if (window.pending != null) draw(window.pending);
    </script>
    """

    private let passesClicks: Bool

    /// `passesClicks`: a closed diagram lets every click through to the
    /// editor, which opens the fence; the open editor's preview does not.
    init(source: String, passesClicks: Bool) {
        self.passesClicks = passesClicks
        let config = WKWebViewConfiguration()
        let seed = (try? JSONSerialization.data(withJSONObject: source, options: .fragmentsAllowed))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "\"\""
        config.userContentController.addUserScript(WKUserScript(
            source: "window.pending = \(seed);", injectionTime: .atDocumentStart, forMainFrameOnly: true))
        super.init(frame: CGRect(x: 0, y: 0, width: 300, height: MermaidFence.drawingHeight), configuration: config)
        #if canImport(AppKit)
        setValue(false, forKey: "drawsBackground")
        #else
        isOpaque = false
        backgroundColor = .clear
        scrollView.isScrollEnabled = false
        isUserInteractionEnabled = !passesClicks
        #endif
        loadHTMLString(Self.page, baseURL: URL(string: "https://cdn.jsdelivr.net/"))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    /// Draw `source` in place of what is there.
    func show(_ source: String) {
        callAsyncJavaScript("window.ready ? draw(s) : (window.pending = s)", arguments: ["s": source],
                            in: nil, in: .page, completionHandler: nil)
    }

    override var intrinsicContentSize: CGSize {
        #if canImport(AppKit)
        CGSize(width: NSView.noIntrinsicMetric, height: MermaidFence.drawingHeight)
        #else
        CGSize(width: UIView.noIntrinsicMetric, height: MermaidFence.drawingHeight)
        #endif
    }

    #if canImport(AppKit)
    override func hitTest(_ point: NSPoint) -> NSView? { passesClicks ? nil : super.hitTest(point) }
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
        preview = MermaidDrawing(source: region.content, passesClicks: false)
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
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work)
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
    private let source = UITextView()
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
#endif
