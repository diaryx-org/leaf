//  LeafEditorView.swift
//
//  The SwiftUI face of the editor. `LeafEditorModel` is an `ObservableObject`
//  that owns the `LeafDoc` and exposes leaf-core's commands + the live toolbar
//  state; `LeafEditor` is the `NSViewRepresentable` that hosts the `LeafTextView`
//  in a scroll view and keeps the model's `state` in step after every repaint.
//
//  Usage:
//      @StateObject private var editor = try! LeafEditorModel(
//          source: "# Hello\n\nSome *text*.", format: "markdown")
//
//      var body: some View {
//          VStack(spacing: 0) {
//              toolbar                       // reads editor.state, calls editor.toggleBold() …
//              LeafEditor(model: editor)
//          }
//      }

#if canImport(AppKit)
import AppKit
import SwiftUI
import LeafFFI

extension EditorState {
    /// Project a full `DocView` down to the chrome-facing state.
    init(_ v: DocView) {
        self.init(view: v.view, dirty: v.dirty, heading: v.heading, active: v.active)
    }
}

/// The observable owner of a document. Hold it with `@StateObject`; bind a
/// toolbar to `state` and call the command methods from buttons.
public final class LeafEditorModel: ObservableObject {
    /// Live toolbar/footer state, refreshed after every edit, motion, and click.
    @Published public private(set) var state: EditorState

    let doc: LeafDoc
    fileprivate weak var textView: LeafTextView?

    /// Parse `source` as `format` (`"markdown"`, `"djot"`, `"html"`, `"xml"`).
    public init(source: String, format: String = "markdown") throws {
        let doc = try LeafDoc(source: source, format: format)
        self.doc = doc
        self.state = EditorState(doc.view())
    }

    // ── host-facing model access ──────────────────────────────────────────────

    /// The current source text — for save / export / a source panel.
    public func source() -> String { doc.source() }

    /// Clear the dirty flag after persisting `source()` the host's own way.
    public func markSaved() { textView?.markSaved() }

    // ── formatting commands (mirror leaf-gpui's EditorCommand) ────────────────

    public func toggleBold()       { run { $0.toggleBold() } }
    public func toggleItalic()     { run { $0.toggleItalic() } }
    public func toggleCode()       { run { $0.toggleCode() } }
    public func toggleMark()       { run { $0.toggleMark() } }
    public func toggleUnderline()  { run { $0.toggleUnderline() } }
    public func toggleStrike()     { run { $0.toggleStrike() } }
    public func setParagraph()     { run { $0.setParagraph() } }
    /// Toggle the block to a heading of `level` (1–6); the active level toggles
    /// off to a paragraph, per core.
    public func setHeading(_ level: UInt32) { run { $0.setHeading(level: level) } }
    public func toggleBlockquote() { run { $0.toggleBlockquote() } }
    public func toggleList(ordered: Bool) { run { $0.toggleList(ordered: ordered) } }
    public func insertLink(_ destination: String) { run { $0.insertLink(destination: destination) } }
    public func undo() { run { $0.undo() } }
    public func redo() { run { $0.redo() } }
    /// Switch between the rendered WYSIWYG surface and the raw source.
    public func toggleView() { run { $0.toggleView() } }

    // ── convenience toolbar queries ───────────────────────────────────────────

    public func isActive(_ mark: String) -> Bool { state.active.contains(mark) }
    public var isSource: Bool { state.view == "source" }

    /// Route a command through the text view so the single repaint path (caret,
    /// scroll, state push) runs. No-op until the view is mounted.
    private func run(_ op: @escaping (LeafDoc) -> DocView) { textView?.command(op) }

    /// The view pushes each repaint's state here (its `state` setter is private).
    fileprivate func updateState(_ s: EditorState) { state = s }
}

/// Hosts the `LeafTextView` in a scrolling viewport and wires its state back to
/// the model. Place it in a SwiftUI hierarchy like any other view.
public struct LeafEditor: NSViewRepresentable {
    @ObservedObject private var model: LeafEditorModel
    private let theme: EditorTheme

    public init(model: LeafEditorModel, theme: EditorTheme = .default) {
        self.model = model
        self.theme = theme
    }

    public func makeNSView(context: Context) -> NSScrollView {
        let textView = LeafTextView(doc: model.doc, theme: theme)
        textView.onStateChange = { [weak model] s in model?.updateState(s) }
        model.textView = textView

        let scroll = NSScrollView()
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.automaticallyAdjustsContentInsets = false

        // The document view fills the clip width and grows in height; horizontal
        // scrolling is off (core rewraps to the width instead).
        textView.translatesAutoresizingMaskIntoConstraints = true
        textView.autoresizingMask = [.width]
        textView.frame = CGRect(origin: .zero, size: CGSize(width: scroll.contentSize.width, height: 0))

        DispatchQueue.main.async { scroll.window?.makeFirstResponder(textView) }
        return scroll
    }

    public func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let textView = scroll.documentView as? LeafTextView else { return }
        textView.theme = theme
    }
}
#endif
