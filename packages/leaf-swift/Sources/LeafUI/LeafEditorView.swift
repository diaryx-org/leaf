//  LeafEditorView.swift
//
//  The SwiftUI face of the editor, shared across macOS and iOS. `LeafEditorModel`
//  is a platform-neutral `ObservableObject` that owns the `LeafDoc` and exposes
//  leaf-core's commands + the live toolbar state. `LeafEditor` is the
//  representable that hosts the platform `LeafTextView` and keeps the model's
//  `state` in step after every repaint.
//
//  Usage:
//      @StateObject private var editor = try! LeafEditorModel(
//          source: "# Hello\n\nSome *text*.", format: "markdown")
//
//      var body: some View {
//          VStack(spacing: 0) { toolbar; LeafEditor(model: editor) }
//      }

import SwiftUI
import LeafFFI

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

    public func source() -> String { doc.source() }
    public func markSaved() { textView?.markSaved() }

    // ── formatting commands (mirror leaf-gpui's EditorCommand) ────────────────

    public func toggleBold()       { run { $0.toggleBold() } }
    public func toggleItalic()     { run { $0.toggleItalic() } }
    public func toggleCode()       { run { $0.toggleCode() } }
    public func toggleMark()       { run { $0.toggleMark() } }
    public func toggleUnderline()  { run { $0.toggleUnderline() } }
    public func toggleStrike()     { run { $0.toggleStrike() } }
    public func setParagraph()     { run { $0.setParagraph() } }
    public func setHeading(_ level: UInt32) { run { $0.setHeading(level: level) } }
    public func toggleBlockquote() { run { $0.toggleBlockquote() } }
    public func toggleList(ordered: Bool) { run { $0.toggleList(ordered: ordered) } }
    public func indent()  { run { $0.indent() } }
    public func outdent() { run { $0.outdent() } }
    public func insertLink(_ destination: String) { run { $0.insertLink(destination: destination) } }

    // ── table editing ─────────────────────────────────────────────────────────

    public var caretInTable: Bool { doc.caretInTable() }
    public func tableInsertRow(below: Bool = true) { run { $0.tableInsertRow(below: below) } }
    public func tableDeleteRow() { run { $0.tableDeleteRow() } }
    public func tableInsertColumn(right: Bool = true) { run { $0.tableInsertColumn(right: right) } }
    public func tableDeleteColumn() { run { $0.tableDeleteColumn() } }
    public func tableSetAlignment(_ alignment: TableAlignment) { run { $0.tableSetAlignment(alignment: alignment) } }
    public func tableMoveRow(down: Bool) { run { $0.tableMoveRow(down: down) } }
    public func tableMoveColumn(right: Bool) { run { $0.tableMoveColumn(right: right) } }

    public func undo() { run { $0.undo() } }
    public func redo() { run { $0.redo() } }
    public func toggleView() { run { $0.toggleView() } }

    // ── inline reveal preference ──────────────────────────────────────────────
    // Hidden (the default) is the clean surface Diaryx ships; CaretLine reveals
    // the caret line's raw markdown for Markdown-fluent users. Stored today;
    // honoured by the renderer in a later phase.

    public var revealMode: RevealMode { doc.revealMode() }
    public func setRevealMode(_ mode: RevealMode) { run { $0.setRevealMode(mode: mode) } }

    // ── convenience toolbar queries ───────────────────────────────────────────

    public func isActive(_ mark: String) -> Bool { state.active.contains(mark) }
    public var isSource: Bool { state.view == "source" }

    /// TEMP DEBUG: seed a selection by source offsets, to inspect highlight alignment.
    public func debugSelect(anchor: UInt32, focus: UInt32) {
        run { $0.setSelectionOffsets(anchor: anchor, focus: focus) }
    }

    private func run(_ op: @escaping (LeafDoc) -> DocView) { textView?.command(op) }
    fileprivate func updateState(_ s: EditorState) { if s != state { state = s } }
}

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit

/// Hosts the `LeafTextView` in a scrolling viewport (macOS) and wires its state
/// back to the model.
public struct LeafEditor: NSViewRepresentable {
    @ObservedObject private var model: LeafEditorModel
    private let theme: EditorTheme

    public init(model: LeafEditorModel, theme: EditorTheme = .default) {
        self.model = model; self.theme = theme
    }

    public func makeNSView(context: Context) -> NSScrollView {
        let textView = LeafTextView(doc: model.doc, theme: theme)
        // Defer the publish: `render()` can fire during a SwiftUI layout pass, and
        // mutating an `@Published` mid-update loops the view system.
        textView.onStateChange = { [weak model] s in
            DispatchQueue.main.async { model?.updateState(s) }
        }
        model.textView = textView

        let scroll = NSScrollView()
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        textView.autoresizingMask = [.width]
        textView.frame = CGRect(origin: .zero, size: CGSize(width: scroll.contentSize.width, height: 0))

        DispatchQueue.main.async { scroll.window?.makeFirstResponder(textView) }
        return scroll
    }

    public func updateNSView(_ scroll: NSScrollView, context: Context) {
        (scroll.documentView as? LeafTextView)?.theme = theme
    }
}

#elseif canImport(UIKit)
import UIKit

/// Hosts the `LeafTextView` in a scrolling viewport (iOS) and wires its state
/// back to the model.
public struct LeafEditor: UIViewRepresentable {
    @ObservedObject private var model: LeafEditorModel
    private let theme: EditorTheme

    public init(model: LeafEditorModel, theme: EditorTheme = .default) {
        self.model = model; self.theme = theme
    }

    public func makeUIView(context: Context) -> UIScrollView {
        let textView = LeafTextView(doc: model.doc, theme: theme)
        // Defer the publish: `render()` can fire during a SwiftUI layout pass, and
        // mutating an `@Published` mid-update loops the view system.
        textView.onStateChange = { [weak model] s in
            DispatchQueue.main.async { model?.updateState(s) }
        }
        model.textView = textView

        let scroll = UIScrollView()
        scroll.alwaysBounceVertical = true
        scroll.keyboardDismissMode = .interactive
        scroll.addSubview(textView)
        textView.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            textView.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            textView.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            textView.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            textView.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            textView.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor),
        ])

        DispatchQueue.main.async { _ = textView.becomeFirstResponder() }
        return scroll
    }

    public func updateUIView(_ scroll: UIScrollView, context: Context) {
        (scroll.subviews.first { $0 is LeafTextView } as? LeafTextView)?.theme = theme
    }
}
#endif
