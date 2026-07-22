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

    // ── soft-break flow preference ────────────────────────────────────────────
    // Fold (the default) reflows soft breaks into the paragraph; Preserve renders
    // each where it was written, so a source laid out in semantic line breaks
    // shows that structure. Unlike reveal, the renderer honours this immediately.

    public var lineFlow: LineFlow { doc.lineFlow() }
    public func setLineFlow(_ mode: LineFlow) { run { $0.setLineFlow(mode: mode) } }

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
        let textView = makeTextView()

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
        guard let hosted = scroll.documentView as? LeafTextView else { return }
        // A freshly-swapped model has never been through `makeNSView`, so its
        // `textView` is still nil — that mismatch (rather than comparing docs
        // directly, which `LeafTextView` doesn't expose) is the stale-binding
        // signal. SwiftUI keeps this view's identity across the swap, so without
        // this the cached `hosted` view would go on showing the OLD model's doc
        // forever (the bug this fixes; hosts no longer need `.id(...)`).
        guard model.textView === hosted else {
            let textView = makeTextView()
            scroll.documentView = textView
            textView.autoresizingMask = [.width]
            textView.frame = CGRect(origin: .zero, size: CGSize(width: scroll.contentSize.width, height: 0))
            // `doc.view()` is a read-only snapshot — routing it through `command`
            // forces an immediate render → `onStateChange`, rather than waiting on
            // whatever layout pass happens to come next.
            textView.command { $0.view() }
            DispatchQueue.main.async { scroll.window?.makeFirstResponder(textView) }
            return
        }
        hosted.theme = theme
    }

    /// Build a `LeafTextView` over `model.doc`, wired the way `makeNSView` and the
    /// stale-binding rebuild in `updateNSView` both need it.
    private func makeTextView() -> LeafTextView {
        let textView = LeafTextView(doc: model.doc, theme: theme)
        // Defer the publish: `render()` can fire during a SwiftUI layout pass, and
        // mutating an `@Published` mid-update loops the view system.
        textView.onStateChange = { [weak model] s in
            DispatchQueue.main.async { model?.updateState(s) }
        }
        model.textView = textView
        return textView
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
        let textView = makeTextView()

        let scroll = UIScrollView()
        scroll.alwaysBounceVertical = true
        scroll.keyboardDismissMode = .interactive
        pin(textView, into: scroll)

        DispatchQueue.main.async { _ = textView.becomeFirstResponder() }
        return scroll
    }

    public func updateUIView(_ scroll: UIScrollView, context: Context) {
        guard let hosted = scroll.subviews.first(where: { $0 is LeafTextView }) as? LeafTextView else { return }
        // A freshly-swapped model has never been through `makeUIView`, so its
        // `textView` is still nil — that mismatch (rather than comparing docs
        // directly, which `LeafTextView` doesn't expose) is the stale-binding
        // signal. SwiftUI keeps this view's identity across the swap, so without
        // this the cached `hosted` view would go on showing the OLD model's doc
        // forever (the bug this fixes; hosts no longer need `.id(...)`).
        guard model.textView === hosted else {
            hosted.removeFromSuperview() // also tears down its own constraints
            let textView = makeTextView()
            pin(textView, into: scroll)
            // `doc.view()` is a read-only snapshot — routing it through `command`
            // forces an immediate render → `onStateChange`, rather than waiting on
            // whatever layout pass happens to come next.
            textView.command { $0.view() }
            DispatchQueue.main.async { _ = textView.becomeFirstResponder() }
            return
        }
        hosted.theme = theme
    }

    /// Build a `LeafTextView` over `model.doc`, wired the way `makeUIView` and the
    /// stale-binding rebuild in `updateUIView` both need it.
    private func makeTextView() -> LeafTextView {
        let textView = LeafTextView(doc: model.doc, theme: theme)
        // Defer the publish: `render()` can fire during a SwiftUI layout pass, and
        // mutating an `@Published` mid-update loops the view system.
        textView.onStateChange = { [weak model] s in
            DispatchQueue.main.async { model?.updateState(s) }
        }
        model.textView = textView
        return textView
    }

    /// Add `textView` to `scroll` and pin it to the content/frame layout guides —
    /// the same constraint set `makeUIView` and the stale-binding rebuild both need.
    private func pin(_ textView: LeafTextView, into scroll: UIScrollView) {
        scroll.addSubview(textView)
        textView.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            textView.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            textView.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            textView.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            textView.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            textView.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor),
            // Without this, the text view's height is purely its intrinsic content
            // height — for a short or empty document that's a sliver at the top, and
            // UIKit only routes touches to a view under them, so tapping anywhere in
            // the rest of the visible editor pane hit nothing (no caret, no focus,
            // typing impossible). `EditorLayout.hit` already clamps a point below the
            // last row to it, so filling the viewport just makes that reachable —
            // clicking below the text lands the caret at the document's end, same as
            // most text editors.
            textView.heightAnchor.constraint(greaterThanOrEqualTo: scroll.frameLayoutGuide.heightAnchor),
        ])
    }
}
#endif
