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

    /// Called when the reader activates a link — ⌘-click, or "Open Link" from the
    /// context menu (macOS) or the long-press edit menu (iOS) — with the link's
    /// raw destination exactly as the document spells it. Return `true` to claim
    /// it; `false` (or leaving this nil) lets the editor open it with the system
    /// as before.
    ///
    /// A plain click or tap deliberately does *not* activate a link: it places
    /// the caret, because the editor is an editor first and link text has to be
    /// as editable as the prose around it.
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

    /// Asked for a link destination, seeded with the current one. Set it by
    /// calling `insertLink(_:)`, which repoints the link the caret stands in —
    /// or, from an empty seed, writes a new one over the selection.
    ///
    /// Two places ask: the context menu's "Edit Link…", which always arrives with
    /// a destination to change, and `LeafFormattingToolbar`'s Link button, which
    /// arrives with `""` when the caret is in no link and is therefore *making*
    /// one. A host offering a field should title it for both — "Link" reads right
    /// either way, where "Edit Link" doesn't.
    ///
    /// A callback rather than a prompt of the editor's own, for the reason
    /// `onOpenLink` is one: asking a question is the host's chrome — its window,
    /// its idiom, its localization — and a note app that resolves `id:6tzwsxg`
    /// needs to offer its own document picker here, not a text field. Leaving
    /// this nil hides the menu item entirely, so no menu ever offers an edit
    /// nothing can carry out; the toolbar button stays (a ready-made bar with a
    /// dead button is worse than one with a plain field) and falls back to a
    /// field of its own.
    ///
    /// The *menu* item is offered only for a *parsed* link (`[t](dest)`, a bare
    /// URL, an autolink). A wikilink is literal text with no node behind it — it
    /// can be followed, but there is nothing to repoint — so it gets no
    /// "Edit Link…".
    public var onEditLink: ((String) -> Void)? {
        didSet { textView?.onEditLink = editBridge }
    }

    /// Asked what a link points *at*, so a reader resting on one can be shown it
    /// without going there — the cross-document half of the popover a footnote
    /// reference already raises.
    ///
    /// Called with the destination exactly as the document spells it
    /// (`./chapter.md#v2`, `id:6tzwsxg`, `[[Some Note]]`). Resolve it however you
    /// resolve it for `onOpenLink`, read the document's *body*, and answer with a
    /// `LinkPeekSource` — or with nil, for a destination you do not claim, cannot
    /// reach, or would rather not disclose you fetched. Nil is also the right
    /// answer for a `https:` URL: the editor will not go to the network, and a
    /// host that wants to is choosing that for its reader.
    ///
    /// Called on the main actor when the pointer has rested on a link (or a
    /// finger has held it); the completion is safe to call from anywhere and may
    /// arrive whenever the read finishes — the peek appears if the reader is
    /// still pointing at the same link, and is dropped if they have moved on. So
    /// a read that has to touch a synced or evicted file should go and do it,
    /// rather than blocking here to keep the popover instant.
    ///
    /// Leaving this nil is the old behaviour: a link shows nothing on hover.
    /// Nothing here is followable in either case — see `FootnotePeekContent`'s
    /// peeking initializer for why a foreign document's links stay inert.
    public var onPeekLink: ((String, @escaping (LinkPeekSource?) -> Void) -> Void)? {
        didSet { textView?.onPeekLink = peekBridge }
    }

    /// The two handlers the views hand *to a menu* rather than merely call, wired
    /// so that "is a host listening?" survives being asked through them.
    ///
    /// Both are read-through closures: they look the handler up on the model at
    /// call time, so a host that wires one after the editor is on screen — the
    /// usual shape, since the model is built when a document loads and the
    /// handlers where the view is composed — still gets its links. But a
    /// read-through closure is never nil, and both views gate a menu item on the
    /// hook being non-nil ("Edit Link…", "Preview Link"). Installed
    /// unconditionally, the wrapper answered *yes* on behalf of a host that had
    /// said nothing, and the menu offered an item that did nothing at all.
    ///
    /// So the bridge is nil when the handler is, and `didSet` re-installs it
    /// whenever that changes. The view gets both properties: read-through when
    /// there is something to read through to, and honestly absent when there
    /// isn't.
    fileprivate var editBridge: ((String) -> Void)? {
        guard onEditLink != nil else { return nil }
        return { [weak self] destination in self?.onEditLink?(destination) }
    }

    fileprivate var peekBridge: ((String, @escaping (LinkPeekSource?) -> Void) -> Void)? {
        guard onPeekLink != nil else { return nil }
        return { [weak self] destination, done in
            guard let peek = self?.onPeekLink else { return done(nil) }
            peek(destination, done)
        }
    }

    /// Whether the document refuses to change — a *reading* surface over the
    /// same rendering, selection and navigation the editor has.
    ///
    /// Set it right after `init` for a document that opens as a reader; it can
    /// also flip at runtime (a lock control, say). Enforcement is leaf-core's —
    /// every splice is refused at the model — and the platform views quiet
    /// their chrome to match: on iOS the interaction swaps to selection-only
    /// and no keyboard rises; on macOS the guarantee currently arrives without
    /// the chrome (see `LeafTextView.isReadOnly`).
    public var isReadOnly: Bool = false {
        didSet {
            let on = isReadOnly
            prefer { $0.setReadOnly(on: on) }
            textView?.isReadOnly = on
        }
    }

    /// The selection as a quote with up to `context` characters of what
    /// surrounded it, cut from the **source** — for a host that cites or
    /// annotates the selected passage. `nil` when nothing is selected.
    public func selectionQuote(context: UInt32 = 30) -> SelectionQuote? {
        doc.selectionQuote(context: context)
    }

    #if canImport(UIKit) && !targetEnvironment(macCatalyst)
    /// Extra actions for the selection's edit menu, ahead of the system's
    /// Copy/Look Up — a host's own verbs where the reader's thumb already is.
    /// Asked each time the menu is built; pair with `selectionQuote` inside an
    /// action to learn what the verbs apply to. iOS-only for now: the macOS
    /// selection menu is a context menu with its own extension point, still to
    /// be wired.
    public var selectionMenuActions: (() -> [UIMenuElement])? {
        didSet { textView?.selectionMenuActions = selectionMenuBridge }
    }

    /// The read-through wrapper `selectionMenuActions` reaches the view as —
    /// nil exactly when the host's is, for the reason `editBridge` exists.
    fileprivate var selectionMenuBridge: (() -> [UIMenuElement])? {
        guard selectionMenuActions != nil else { return nil }
        return { [weak self] in self?.selectionMenuActions?() ?? [] }
    }
    #endif

    /// Paint host ranges over the source — annotation footprints, search
    /// hits. The whole set each time (see `leaf_core::Doc::set_highlights`);
    /// safe to call before the view exists, since the doc holds them and the
    /// first render paints them. Ranges are source bytes, the same coordinate
    /// `selectionQuote` reports — anchor a quote, paint what it found.
    public func setHighlights(_ highlights: [Highlight]) {
        prefer { $0.setHighlights(highlights: highlights) }
    }

    /// Called with a highlight's `id` when the reader taps (iOS) or clicks
    /// (macOS) its **margin marker** — the glyph a `Highlight.marker` puts
    /// beside the wash's first line. The marker is the control and the wash is
    /// ink: text under a wash still selects, copies and (in an editor) takes a
    /// caret like any other. A markerless highlight is purely visual whatever
    /// this is set to.
    public var onTapHighlight: ((String) -> Void)? {
        didSet { textView?.onTapHighlight = tapHighlightBridge }
    }

    /// The read-through wrapper `onTapHighlight` reaches the view as — nil
    /// exactly when the host's is, for the reason `editBridge` exists.
    fileprivate var tapHighlightBridge: ((String) -> Void)? {
        guard onTapHighlight != nil else { return nil }
        return { [weak self] id in self?.onTapHighlight?(id) }
    }

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

    #if canImport(AppKit) && !targetEnvironment(macCatalyst)
    /// Gets first refusal on a drop, handed the drag's own pasteboard — the
    /// `onPaste` of drag and drop. Return `true` to say it was handled.
    ///
    /// Asked only for what the editor cannot use itself: a file or an image.
    /// Dragged text is dropped as text without asking, the way it is pasted.
    /// The caret has been moved to the drop point before this is called, so
    /// `insertMedia` lands where the reader let go.
    ///
    /// A closure over the pasteboard rather than the general one because a drag
    /// carries its own — reading `NSPasteboard.general` here would find
    /// whatever was last copied, not what was dropped.
    public var onDrop: ((NSPasteboard) -> Bool)?
    #endif

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

    /// Land the reader on the place `locator` names — the `#v2` of a
    /// `chapter.dj#v2`, once the host has opened the document that carries it.
    /// `false` when this document answers to no such name, which is a host's cue
    /// to leave the reader at the top rather than pretend the jump worked.
    ///
    /// The half of a located link the editor owns. Resolving `chapter.dj` to a
    /// file is a vault's business and always was; what had no answer until now is
    /// the rest of the destination, so a citation into a chapter dropped the
    /// reader at its first verse to hunt for the twentieth.
    ///
    /// Safe to call the instant a document is opened, before SwiftUI has made the
    /// text view — which is exactly when a host calls it, one line after building
    /// the model. The landing is remembered and applied when the view appears;
    /// see `pendingLanding`.
    @discardableResult
    public func goTo(locator: String) -> Bool {
        guard let landing = doc.locate(id: locator) else { return false }
        reveal(offset: landing.start, through: landing.end)
        return true
    }

    /// Put the caret at `offset` and land the reader on it. `through` bounds the
    /// block that was named, which gets flashed — pass it whenever the arrival is
    /// at a *block* rather than a point, so the reader is told which words they
    /// were sent to.
    public func reveal(offset: UInt32, through end: UInt32? = nil) {
        guard let textView else { pendingLanding = (offset, end); return }
        textView.reveal(offset: offset, through: end)
    }

    /// An offset to land on as soon as there is a view to land in.
    ///
    /// A command dropped for want of a text view is normally no loss — nobody
    /// could have issued it — but this one is issued *by the host*, in the same
    /// breath as opening the document, and the view it needs is made a run loop
    /// later. Worse, dropping it silently is invisible: the reader lands at the
    /// top of the right document, which is precisely what the old behaviour
    /// looked like. `prefer` solves the same problem for rendering modes.
    fileprivate var pendingLanding: (offset: UInt32, end: UInt32?)?

    /// Take the pending landing, if there is one — called once by the view that
    /// has just been made. Taken rather than read, so a later relayout doesn't
    /// yank the reader back to a place they have since scrolled away from.
    fileprivate func takePendingLanding() -> (offset: UInt32, end: UInt32?)? {
        defer { pendingLanding = nil }
        return pendingLanding
    }

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

    /// Write a footnote at the caret — the toolbar's Footnote button.
    ///
    /// Both halves go in as one edit: the `[^1]` where the caret is and the
    /// definition that gives it meaning at the end of the document, so one undo
    /// takes back both and the author never sees a reference rendering as literal
    /// brackets. The label is the lowest number the document has free, and the
    /// caret is left **in the empty note**, ready for the note's first word.
    ///
    /// A selection is marked rather than replaced: the reference lands after it,
    /// so "select the claim, add a footnote" footnotes that claim.
    ///
    /// Formats that can't spell a footnote (HTML) refuse it — see
    /// `leaf_core::Doc::insert_footnote`.
    public func insertFootnote() { run { $0.insertFootnote() } }

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

/// The editing surface, for a SwiftUI host. Hosts the `LeafTextView` in a
/// scrolling viewport and wires its state back to the model; and publishes the
/// model as the scene's focused editor, which is what lets a menu bar built
/// from `LeafEditorCommands` aim Format ▸ Bold at *this* document.
public struct LeafEditor: View {
    private let model: LeafEditorModel
    private let surface: LeafEditorSurface

    /// `placeholder` is the cue shown while the document is empty, drawn where
    /// its first character will go — see `LeafTextView.placeholder` for why the
    /// editor draws it rather than the host stacking a label over the view.
    /// `page` puts the document on paper — a stack of sheets broken at the page
    /// boundaries, wrapping to the sheet's margins rather than the theme's
    /// `measure`. `nil` (the default) is the continuous scrolling flow.
    /// `zoom` scales the surface on screen without re-laying it out; it applies to
    /// both flows, though a zoom control is really a paginated idiom.
    public init(model: LeafEditorModel, theme: EditorTheme = .default,
                placeholder: String? = nil,
                page: PageSetup? = nil, zoom: CGFloat = 1) {
        self.model = model
        self.surface = LeafEditorSurface(model: model, theme: theme, placeholder: placeholder,
                                         page: page, zoom: zoom)
    }

    public var body: some View {
        surface.focusedSceneValue(\.leafEditor, model)
    }
}

/// The `NSViewRepresentable` under `LeafEditor`.
struct LeafEditorSurface: NSViewRepresentable {
    @ObservedObject private var model: LeafEditorModel
    private let theme: EditorTheme
    private let placeholder: String?
    private let page: PageSetup?
    private let zoom: CGFloat

    init(model: LeafEditorModel, theme: EditorTheme, placeholder: String?,
         page: PageSetup?, zoom: CGFloat) {
        self.model = model; self.theme = theme; self.placeholder = placeholder
        self.page = page; self.zoom = zoom
    }

    public func makeNSView(context: Context) -> NSScrollView {
        let textView = makeTextView()

        let scroll = NSScrollView()
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        // A sheet is a fixed width: a window narrower than one scrolls sideways to
        // it rather than reflowing, which is the whole point of setting a page.
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        textView.autoresizingMask = page == nil ? [.width] : []
        textView.frame = CGRect(origin: .zero, size: CGSize(width: scroll.contentSize.width, height: 0))

        // A reader is opened to be read, not typed into — leaving focus where
        // the host put it instead of claiming it for a keyboard that will
        // change nothing.
        if !model.isReadOnly {
            DispatchQueue.main.async { scroll.window?.makeFirstResponder(textView) }
        }
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
            textView.autoresizingMask = page == nil ? [.width] : []
            textView.frame = CGRect(origin: .zero, size: CGSize(width: scroll.contentSize.width, height: 0))
            textView.pageSetup = page
            textView.zoom = zoom
            // `doc.view()` is a read-only snapshot — routing it through `command`
            // forces an immediate render → `onStateChange`, rather than waiting on
            // whatever layout pass happens to come next.
            textView.command { $0.view() }
            if !model.isReadOnly {
                DispatchQueue.main.async { scroll.window?.makeFirstResponder(textView) }
            }
            return
        }
        hosted.theme = theme
        // Both guard themselves against an unchanged value, so re-applying them on
        // every SwiftUI update (which is every state change at all) costs a
        // comparison rather than a relayout.
        hosted.pageSetup = page
        hosted.zoom = zoom
        hosted.placeholder = placeholder
        // Re-read rather than trusting the copy `makeTextView` took: a host that
        // flips this on the model after the view exists (or per document, for a
        // vault where only some files use the convention) gets it honoured.
        hosted.recognizesWikilinks = model.recognizesWikilinks
        hosted.isReadOnly = model.isReadOnly
        hosted.onTapHighlight = model.tapHighlightBridge
        hosted.documentDirectory = model.documentDirectory
        hosted.mediaPlayback = model.mediaPlayback
        hosted.onResolveMedia = model.onResolveMedia
    }

    /// Build a `LeafTextView` over `model.doc`, wired the way `makeNSView` and the
    /// stale-binding rebuild in `updateNSView` both need it.
    private func makeTextView() -> LeafTextView {
        let textView = LeafTextView(doc: model.doc, theme: theme)
        textView.pageSetup = page
        textView.zoom = zoom
        textView.placeholder = placeholder
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
        // The two the menus *gate* on, through the bridges that keep "is a host
        // listening?" answerable — see `editBridge`.
        textView.onEditLink = model.editBridge
        textView.onPeekLink = model.peekBridge
        // Same read-through, same reason: an app wires its paste handler where
        // the view is composed, after the model was built.
        textView.onPaste = { [weak model] in
            model?.onPaste?() ?? false
        }
        textView.onDrop = { [weak model] pasteboard in
            model?.onDrop?(pasteboard) ?? false
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
        textView.isReadOnly = model.isReadOnly
        textView.onTapHighlight = model.tapHighlightBridge
        model.textView = textView
        // A locator the host followed before there was anything to scroll. After
        // the frame lands, not during: the view has no size yet, so a reveal here
        // would measure the caret against a zero-height viewport and scroll
        // nowhere.
        if let landing = model.takePendingLanding() {
            DispatchQueue.main.async { [weak textView] in
                textView?.reveal(offset: landing.offset, through: landing.end)
            }
        }
        return textView
    }
}

#elseif canImport(UIKit)
import UIKit

/// The editing surface, for a SwiftUI host. Hosts the `LeafTextView` in a
/// scrolling viewport and wires its state back to the model; and publishes the
/// model as the scene's focused editor, so a menu bar (an iPad's, under a
/// hardware keyboard) built from `LeafEditorCommands` reaches *this* document.
public struct LeafEditor: View {
    private let model: LeafEditorModel
    private let surface: LeafEditorSurface

    /// `placeholder` is the cue shown while the document is empty, drawn where
    /// its first character will go — see `LeafTextView.placeholder`.
    public init(model: LeafEditorModel, theme: EditorTheme = .default,
                placeholder: String? = nil) {
        self.model = model
        self.surface = LeafEditorSurface(model: model, theme: theme, placeholder: placeholder, accessory: nil)
    }

    /// With a custom view shown above the system keyboard while this editor is
    /// first responder — a host app's own formatting toolbar. See
    /// `LeafTextView.accessoryView` for why this has to be threaded through
    /// explicitly rather than SwiftUI's own `.toolbar(placement: .keyboard)`.
    public init<Accessory: View>(
        model: LeafEditorModel, theme: EditorTheme = .default,
        placeholder: String? = nil,
        @ViewBuilder accessory: () -> Accessory
    ) {
        self.model = model
        self.surface = LeafEditorSurface(model: model, theme: theme, placeholder: placeholder,
                                         accessory: AnyView(accessory()))
    }

    public var body: some View {
        surface.focusedSceneValue(\.leafEditor, model)
    }
}

/// The `UIViewRepresentable` under `LeafEditor`.
struct LeafEditorSurface: UIViewRepresentable {
    @ObservedObject private var model: LeafEditorModel
    private let theme: EditorTheme
    private let placeholder: String?
    /// Type-erased so the surface stays a concrete, non-generic type.
    private let accessory: AnyView?

    init(model: LeafEditorModel, theme: EditorTheme, placeholder: String?, accessory: AnyView?) {
        self.model = model; self.theme = theme; self.placeholder = placeholder
        self.accessory = accessory
    }

    public func makeCoordinator() -> Coordinator { Coordinator() }

    /// Holds the accessory's `UIHostingController` across SwiftUI updates —
    /// `LeafEditor` itself is a value type recreated every update, so this is
    /// the one thing that survives to have its `rootView` refreshed rather
    /// than being torn down and rebuilt each time.
    public final class Coordinator {
        var hosting: UIHostingController<AnyView>?
        /// The view the accessory hangs off, so a resize can ask *it* to re-read
        /// its input views — `reloadInputViews()` is the first responder's call,
        /// and the hosting controller isn't one.
        weak var textView: LeafTextView?
        private var sizeObserver: NSObjectProtocol?

        /// Re-measure the accessory whenever the reader changes their text size.
        ///
        /// The notification rather than `updateUIView`, because SwiftUI never
        /// promises to call that here: the accessory is an `AnyView` built once
        /// in `LeafEditor.init`, so a content-size change re-runs the *toolbar's*
        /// body inside its own hosting environment without re-running the body
        /// that built this representable. The SwiftUI content would resize itself
        /// inside a frame that stayed 44pt tall, and the bottom of the bar would
        /// simply be cut off.
        func observeContentSizeChanges() {
            guard sizeObserver == nil else { return }
            sizeObserver = NotificationCenter.default.addObserver(
                forName: UIContentSizeCategory.didChangeNotification,
                object: nil, queue: .main
            ) { [weak self] _ in self?.resizeAccessory() }
        }

        /// Fit the hosting view's frame to the height its SwiftUI content wants,
        /// and re-present the keyboard's accessory if that moved. Guarded on an
        /// actual change: `reloadInputViews()` on an unchanged bar flickers the
        /// keyboard for nothing.
        func resizeAccessory() {
            guard let hosting, let textView else { return }
            let width = hosting.view.bounds.width > 0 ? hosting.view.bounds.width : 320
            let wanted = hosting.sizeThatFits(
                in: CGSize(width: width, height: CGFloat.greatestFiniteMagnitude)).height
            guard wanted > 0, abs(hosting.view.frame.height - wanted) > 0.5 else { return }
            hosting.view.frame.size.height = wanted
            textView.reloadInputViews()
        }

        deinit {
            if let sizeObserver { NotificationCenter.default.removeObserver(sizeObserver) }
        }
    }

    public func makeUIView(context: Context) -> UIScrollView {
        let textView = makeTextView()
        attachAccessory(to: textView, context: context)

        let scroll = UIScrollView()
        scroll.alwaysBounceVertical = true
        scroll.keyboardDismissMode = .interactive
        pin(textView, into: scroll)

        // A reader is opened to be read — see the AppKit peer.
        if !model.isReadOnly {
            DispatchQueue.main.async { _ = textView.becomeFirstResponder() }
        }
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
            if !model.isReadOnly {
                DispatchQueue.main.async { _ = textView.becomeFirstResponder() }
            }
            return
        }
        hosted.theme = theme
        hosted.placeholder = placeholder
        // Re-read rather than trusting the copy `makeTextView` took: a host that
        // flips this on the model after the view exists (or per document, for a
        // vault where only some files use the convention) gets it honoured.
        hosted.recognizesWikilinks = model.recognizesWikilinks
        hosted.isReadOnly = model.isReadOnly
        hosted.selectionMenuActions = model.selectionMenuBridge
        hosted.onTapHighlight = model.tapHighlightBridge
        hosted.documentDirectory = model.documentDirectory
        hosted.mediaPlayback = model.mediaPlayback
        hosted.onResolveMedia = model.onResolveMedia
        // Refresh the accessory's content in place — its `UIHostingController`
        // persists in the coordinator across updates, so this is a live
        // content swap, not a rebuild (which would drop first-responder focus
        // on whatever's inside the accessory, e.g. a text field mid-edit).
        if let accessory {
            context.coordinator.hosting?.rootView = accessory
            // A content swap can change the bar's height as readily as a text-size
            // change can — a host that shows a taller set of tools for a table, say.
            context.coordinator.resizeAccessory()
        }
    }

    /// Wire the accessory (if any) into `textView.accessoryView` as a
    /// `UIHostingController`'s view, left to stretch to the keyboard's width via
    /// `.flexibleWidth` — the standard shape for a custom `inputAccessoryView`.
    ///
    /// The height is measured off the content rather than fixed at the 44pt a
    /// keyboard accessory usually is, because a bar that respects Dynamic Type
    /// isn't one height: `LeafFormattingToolbar` grows its targets with the
    /// reader's text size (up to its own cap), and a frame nailed to 44 would
    /// crop exactly the readers who asked for something bigger. 44 stays as the
    /// floor for a host whose accessory reports no size at all.
    private func attachAccessory(to textView: LeafTextView, context: Context) {
        guard let accessory else { return }
        let hosting = UIHostingController(rootView: accessory)
        hosting.view.backgroundColor = .clear
        hosting.view.autoresizingMask = [.flexibleWidth]
        let fitted = hosting.sizeThatFits(
            in: CGSize(width: 320, height: CGFloat.greatestFiniteMagnitude)).height
        hosting.view.frame = CGRect(x: 0, y: 0, width: 320, height: max(fitted, 44))
        context.coordinator.hosting = hosting
        context.coordinator.textView = textView
        context.coordinator.observeContentSizeChanges()
        textView.accessoryView = hosting.view
    }

    /// Build a `LeafTextView` over `model.doc`, wired the way `makeUIView` and the
    /// stale-binding rebuild in `updateUIView` both need it.
    private func makeTextView() -> LeafTextView {
        let textView = LeafTextView(doc: model.doc, theme: theme)
        textView.placeholder = placeholder
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
        // The two the menus *gate* on, through the bridges that keep "is a host
        // listening?" answerable — see `editBridge`.
        textView.onEditLink = model.editBridge
        textView.onPeekLink = model.peekBridge
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
        textView.isReadOnly = model.isReadOnly
        textView.selectionMenuActions = model.selectionMenuBridge
        textView.onTapHighlight = model.tapHighlightBridge
        model.textView = textView
        // A locator the host followed before there was anything to scroll — see
        // the AppKit peer for why this waits a turn.
        if let landing = model.takePendingLanding() {
            DispatchQueue.main.async { [weak textView] in
                textView?.reveal(offset: landing.offset, through: landing.end)
            }
        }
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
