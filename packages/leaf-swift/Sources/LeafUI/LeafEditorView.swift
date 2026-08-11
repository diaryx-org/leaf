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

    /// Called when the reader activates a link — a click or tap on it, ⌘-click,
    /// or "Open Link" — with the link's raw destination exactly as the document
    /// spells it. Return `true` to claim it; `false` (or leaving this nil) lets
    /// the editor open it with the system as before.
    ///
    /// This exists because only the host can resolve the destinations that
    /// aren't URLs. In a note app `[Last week](./2026-07-20.md)` and
    /// `[2026](id:6tzwsxg)` mean *documents in this workspace*, and the right
    /// response is to open one in the same window — something the editor has no
    /// way to do and the system has no way to guess. Claim those, decline
    /// `https:` and `mailto:`, and both kinds behave the way a reader expects.
    ///
    /// Called on the main actor, during the event that activated the link.
    /// Resolution that has to go to disk should return `true` and continue
    /// asynchronously — the destination is the host's now either way.
    public var onOpenLink: ((String) -> Bool)?

    /// Whether a bare `[[target]]` / `[[target|label]]` is a link the reader can
    /// follow. Off by default, because it is a convention rather than a syntax:
    /// neither Markdown nor Djot has it, so twig doesn't parse it and it reaches
    /// the screen as the literal text it is. Turning this on makes it
    /// *activatable* — clicking or tapping inside one calls `onOpenLink` with
    /// the construct verbatim, brackets included — but it does not make it look
    /// like a link, which needs the grammar, not the editor.
    ///
    /// Set it if your documents use the convention (a vault imported from
    /// Obsidian will), leave it off otherwise and `[[…]]` stays inert text.
    public var recognizesWikilinks = false

    /// The document's own directory, which a relative `![](img/cat.png)` or
    /// `<video src="clip.mp4">` resolves against.
    ///
    /// Core does no I/O and holds no path context — a `Doc` is bytes and a
    /// caret — so a relative source is unresolvable until the host says what it
    /// is relative *to*. Leave it nil for an untitled buffer and relative media
    /// draws as a labelled chip rather than a picture. Setting it re-reads every
    /// picture, since the same relative path now points somewhere else.
    public var documentDirectory: URL? {
        didSet { textView?.documentDirectory = documentDirectory }
    }

    /// What activating a block video or audio does. `.inline` (the default)
    /// installs a real AVKit player over the box and plays there; `.host` leaves
    /// the still and the play badge drawn and calls `onOpenMedia` instead, for an
    /// app that wants to present its own player.
    public var mediaPlayback: MediaPlaybackMode = .inline {
        didSet { textView?.mediaPlayback = mediaPlayback }
    }

    /// Asks the app to turn a source the editor can't read itself into a local
    /// file it can — a remote URL, or any scheme only the app understands.
    ///
    /// **LeafUI never touches the network.** A document that silently fetches
    /// from a server on open discloses the reader's address and the moment they
    /// opened it, and that is the app's call, not an editor's. Fetch (or
    /// decline), cache wherever you like, and answer with a file URL — or `nil`,
    /// which is remembered so you are not asked again.
    ///
    /// Answering with a file rather than with bytes is what lets the same answer
    /// serve both uses: the picture decodes from it, and `AVPlayer` streams from
    /// it, so a remote video plays inline like any other. Called on the main
    /// thread; the completion is safe to call from anywhere.
    ///
    /// `data:` sources need none of this — the editor decodes those itself.
    public var onResolveMedia: ((String, @escaping (URL?) -> Void) -> Void)? {
        didSet { textView?.onResolveMedia = onResolveMedia }
    }

    /// Gets first refusal on ⌘V, before the editor looks at the clipboard at all.
    /// Return `true` to say the paste was handled and leave the document alone,
    /// `false` to let the normal rich-then-plain text paste proceed.
    ///
    /// This exists for the flavors a text editor has no answer for. A screenshot
    /// on the clipboard is image bytes and no text, so pasting one *as text* is
    /// nothing — and turning it into something the document can point at means
    /// writing a file somewhere, which is the app's decision and the app's
    /// filesystem. Inspect the pasteboard yourself (the editor has not consumed
    /// it), claim what you can use, decline the rest.
    ///
    /// Called on the main actor, inside the paste. Work that has to go to disk
    /// should return `true` and continue asynchronously — the clipboard is the
    /// host's now either way. The same division `onOpenLink` and `onResolveMedia`
    /// draw.
    public var onPaste: (() -> Bool)?

    /// Have `source` resolved again on the next draw — or every source, when it
    /// is nil.
    ///
    /// A `nil` from `onResolveMedia` is remembered, which is what makes
    /// declining cheap; this is how an app un-declines. Fetch what the reader
    /// asked for, then call this with the same `src` and the editor asks again —
    /// and a video the reader tapped starts playing when the answer lands.
    ///
    /// Sources already in flight are left alone, so calling this is never a way
    /// to send the app after the same bytes twice.
    public func reloadMedia(_ source: String? = nil) {
        textView?.reloadMedia(source)
    }

    /// Called when the reader activates a block media box, with its raw `src`, in
    /// the cases the editor can't answer itself: `.host` playback, a video or
    /// audio whose source its local-file loader can't resolve, or a picture whose
    /// box is empty — a source that was declined, or one whose bytes aren't on
    /// this device. A remote URL is the shape of all three, and only the host can
    /// fetch one asynchronously. Nil leaves those activations doing nothing but
    /// placing the caret.
    ///
    /// This is the reader saying *load it anyway*. Fetch it, then call
    /// `reloadMedia(src)`.
    ///
    /// The same division `onOpenLink` draws: the editor renders the document
    /// surface and leaves what it can't reach to the app around it.
    public var onOpenMedia: ((String) -> Void)?

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

    /// Insert a block image, video, or audio at the caret, pointing at
    /// `destination`. Any selection becomes the alt / fallback text.
    ///
    /// `destination` is written into the document verbatim, so it is spelled the
    /// way the *document* should spell it — a path relative to the document's own
    /// directory, matching `documentDirectory`, not an absolute file URL. What the
    /// editor then does to resolve it is `onResolveMedia`'s business.
    public func insertMedia(_ kind: MediaKind, destination: String, alt: String = "") {
        run { $0.insertMedia(kind: kind, destination: destination, alt: alt) }
    }

    /// Insert a thematic break (`---`) at the caret — the toolbar's Horizontal
    /// Rule button. Splits a paragraph if the caret sits mid-text, and exits a
    /// list or block quote rather than nesting inside it; see
    /// `leaf_core::Doc::insert_thematic_break` for the full behavior.
    public func insertThematicBreak() { run { $0.insertThematicBreak() } }

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

    // ── markup exposure preference ──────────────────────────────────────────
    // A three-rung ladder, not a pair of toggles. `.none` (the default) is the
    // clean surface Diaryx ships: delimiters hidden, and typed syntax kept
    // literal so formatting comes from the toolbar. `.shortcuts` keeps the clean
    // surface but lets typing `*x*` author real emphasis. `.full` additionally
    // shows the caret line's raw markup, for markup-fluent users.
    //
    // The fourth combination — reveal the delimiters but refuse the ones you
    // type — is deliberately absent; source view (`toggleView`) is what serves
    // reading raw markup without authoring it.

    public var markupMode: MarkupMode { doc.markupMode() }
    public func setMarkupMode(_ mode: MarkupMode) { prefer { $0.setMarkupMode(mode: mode) } }

    // ── soft-break flow preference ────────────────────────────────────────────
    // Fold (the default) reflows soft breaks into the paragraph; Preserve renders
    // each where it was written, so a source laid out in semantic line breaks
    // shows that structure.

    public var lineFlow: LineFlow { doc.lineFlow() }
    public func setLineFlow(_ mode: LineFlow) { prefer { $0.setLineFlow(mode: mode) } }

    // ── convenience toolbar queries ───────────────────────────────────────────

    public func isActive(_ mark: String) -> Bool { state.active.contains(mark) }
    public var isSource: Bool { state.view == "source" }

    /// TEMP DEBUG: seed a selection by source offsets, to inspect highlight alignment.
    public func debugSelect(anchor: UInt32, focus: UInt32) {
        run { $0.setSelectionOffsets(anchor: anchor, focus: focus) }
    }

    private func run(_ op: @escaping (LeafDoc) -> DocView) { textView?.command(op) }

    /// Apply a *preference* — one of the rendering modes above — whether or not
    /// the view exists yet. A command dropped because there is nothing on screen
    /// to repaint is no loss (nobody could have issued it), but a preference is
    /// set by the host as it builds the model, one line after `init` and long
    /// before SwiftUI makes the text view: routing it through `run` left it on
    /// the floor, so every freshly opened document rendered at the default mode
    /// no matter what the app had chosen. Set it on the doc regardless; the text
    /// view seeds itself from the doc when it is finally made.
    private func prefer(_ op: @escaping (LeafDoc) -> DocView) {
        guard let textView else { _ = op(doc); return }
        textView.command(op)
    }
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
        // Re-read rather than trusting the copy `makeTextView` took: a host that
        // flips this on the model after the view exists (or per document, for a
        // vault where only some files use the convention) gets it honoured.
        hosted.recognizesWikilinks = model.recognizesWikilinks
        hosted.documentDirectory = model.documentDirectory
        hosted.mediaPlayback = model.mediaPlayback
        hosted.onResolveMedia = model.onResolveMedia
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
        // Read through to the model rather than copying its handler across: a
        // host that sets `onOpenLink` after the editor is on screen (the usual
        // shape — the model is built when a document loads, the handler wired
        // where the view is composed) still gets its links.
        textView.onOpenLink = { [weak model] destination in
            model?.onOpenLink?(destination) ?? false
        }
        // Same read-through, same reason: an app wires its paste handler where
        // the view is composed, after the model was built.
        textView.onPaste = { [weak model] in
            model?.onPaste?() ?? false
        }
        textView.recognizesWikilinks = model.recognizesWikilinks
        textView.documentDirectory = model.documentDirectory
        textView.mediaPlayback = model.mediaPlayback
        textView.onResolveMedia = model.onResolveMedia
        // Weak, like `onOpenLink` above: the closure outlives a host that swaps
        // its model, and a strong capture would keep the old one alive.
        textView.onOpenMedia = { [weak model] src in
            model?.onOpenMedia?(src)
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
    /// Type-erased so `LeafEditor` itself stays a concrete, non-generic type —
    /// existing call sites that don't need an accessory (macOS's peer, every
    /// call before this existed) keep compiling untouched.
    private let accessory: AnyView?

    public init(model: LeafEditorModel, theme: EditorTheme = .default) {
        self.model = model; self.theme = theme; self.accessory = nil
    }

    /// With a custom view shown above the system keyboard while this editor is
    /// first responder — a host app's own formatting toolbar. See
    /// `LeafTextView.accessoryView` for why this has to be threaded through
    /// explicitly rather than SwiftUI's own `.toolbar(placement: .keyboard)`.
    public init<Accessory: View>(
        model: LeafEditorModel, theme: EditorTheme = .default,
        @ViewBuilder accessory: () -> Accessory
    ) {
        self.model = model; self.theme = theme; self.accessory = AnyView(accessory())
    }

    public func makeCoordinator() -> Coordinator { Coordinator() }

    /// Holds the accessory's `UIHostingController` across SwiftUI updates —
    /// `LeafEditor` itself is a value type recreated every update, so this is
    /// the one thing that survives to have its `rootView` refreshed rather
    /// than being torn down and rebuilt each time.
    public final class Coordinator {
        var hosting: UIHostingController<AnyView>?
    }

    public func makeUIView(context: Context) -> UIScrollView {
        let textView = makeTextView()
        attachAccessory(to: textView, context: context)

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
            attachAccessory(to: textView, context: context)
            pin(textView, into: scroll)
            // `doc.view()` is a read-only snapshot — routing it through `command`
            // forces an immediate render → `onStateChange`, rather than waiting on
            // whatever layout pass happens to come next.
            textView.command { $0.view() }
            DispatchQueue.main.async { _ = textView.becomeFirstResponder() }
            return
        }
        hosted.theme = theme
        // Re-read rather than trusting the copy `makeTextView` took: a host that
        // flips this on the model after the view exists (or per document, for a
        // vault where only some files use the convention) gets it honoured.
        hosted.recognizesWikilinks = model.recognizesWikilinks
        hosted.documentDirectory = model.documentDirectory
        hosted.mediaPlayback = model.mediaPlayback
        hosted.onResolveMedia = model.onResolveMedia
        // Refresh the accessory's content in place — its `UIHostingController`
        // persists in the coordinator across updates, so this is a live
        // content swap, not a rebuild (which would drop first-responder focus
        // on whatever's inside the accessory, e.g. a text field mid-edit).
        if let accessory { context.coordinator.hosting?.rootView = accessory }
    }

    /// Wire the accessory (if any) into `textView.accessoryView` as a
    /// `UIHostingController`'s view, sized to a fixed height and left to
    /// stretch to the keyboard's width via `.flexibleWidth` — the standard
    /// shape for a custom `inputAccessoryView`.
    private func attachAccessory(to textView: LeafTextView, context: Context) {
        guard let accessory else { return }
        let hosting = UIHostingController(rootView: accessory)
        hosting.view.backgroundColor = .clear
        hosting.view.autoresizingMask = [.flexibleWidth]
        hosting.view.frame = CGRect(x: 0, y: 0, width: 320, height: 44)
        context.coordinator.hosting = hosting
        textView.accessoryView = hosting.view
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
        // Read through to the model rather than copying its handler across: a
        // host that sets `onOpenLink` after the editor is on screen (the usual
        // shape — the model is built when a document loads, the handler wired
        // where the view is composed) still gets its links.
        textView.onOpenLink = { [weak model] destination in
            model?.onOpenLink?(destination) ?? false
        }
        // Same read-through, same reason: an app wires its paste handler where
        // the view is composed, after the model was built.
        textView.onPaste = { [weak model] in
            model?.onPaste?() ?? false
        }
        textView.recognizesWikilinks = model.recognizesWikilinks
        textView.documentDirectory = model.documentDirectory
        textView.mediaPlayback = model.mediaPlayback
        textView.onResolveMedia = model.onResolveMedia
        // Weak, like `onOpenLink` above: the closure outlives a host that swaps
        // its model, and a strong capture would keep the old one alive.
        textView.onOpenMedia = { [weak model] src in
            model?.onOpenMedia?(src)
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
