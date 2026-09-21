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

/// What the document adds up to: the numbers behind a word-count pill in a
/// status bar, or a statistics panel's table.
///
/// Published by `LeafEditorModel.counts` while `countsEnabled` is on, and
/// `Equatable` so the model can republish only when a number actually moved —
/// `EditorState`'s rule, and here for a sharper form of its reason. The tally
/// is redone after the typing settles, and plenty of settles move nothing in
/// it at all: a caret step, a re-wrap, a picture arriving, a mark toggled over
/// words already counted.
public struct EditorCounts: Equatable {
    /// The whole document. See `LeafDoc.counts()` for what is text and what is
    /// furniture the renderer drew — a bullet, a quote's gutter, a picture.
    public var document: TextCounts

    /// The same statistics over the selection, and nil without one — an empty
    /// selection is no selection, which is what lets a panel show these
    /// *instead of* the document's numbers exactly when there are some.
    public var selection: TextCounts?

    /// How many sheets the document lays out to, and nil in the continuous
    /// flow, where the question has no answer until something prints it.
    ///
    /// The one field that is the *surface's* rather than the document's: a page
    /// count is a fact about a layout at a width and a paper size, so it is nil
    /// until there is a view to have laid the document out, where `document`
    /// and `selection` hold from the moment a model exists.
    public var pages: Int?

    public init(document: TextCounts, selection: TextCounts? = nil, pages: Int? = nil) {
        self.document = document
        self.selection = selection
        self.pages = pages
    }
}

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

    /// The app's own reading of a media source, answered on the spot.
    ///
    /// Set this when the app spells a reference in a way the editor would
    /// misread — a leading `/` that means the app's own root rather than the
    /// machine's, say. It is asked first, synchronously, with the source as
    /// written; a readable file it names draws in the same layout pass that
    /// asked, with no trip through `onResolveMedia`. Answer `nil` to have no
    /// opinion and leave the editor's own resolution — the document's
    /// directory for a relative path — in force.
    public var onLocateMedia: ((String) -> URL?)? {
        didSet { textView?.onLocateMedia = onLocateMedia }
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

    /// Called after every edit that changed the document's text — a keystroke,
    /// a paste, a mark toggled, an undo — and not after a caret step, a reflow,
    /// or a switch between the source and rendered views.
    ///
    /// For the host that owns a *file*. `state.dirty` says whether the text
    /// differs from what was last `markSaved()`, which is the right question for
    /// a "● modified" in a title bar and the wrong one for a document system:
    /// `NSDocument` and `UIDocument` autosave on each change they are told of,
    /// and a flag that stays up from the first keystroke tells them of one. A
    /// SwiftUI `ReferenceFileDocument` says it changed by registering with the
    /// scene's undo manager, and this is where to do that.
    ///
    /// The editor's own history is twig's, reached through the responder chain's
    /// undo manager (see `UndoBridge.swift`); nothing here registers with it.
    /// Called on the main actor, after the surface has repainted.
    public var onEdit: (() -> Void)?

    /// Called when the reader asks to be taken to an attachment itself, with its
    /// raw `src` — the contextual menu's "Show Attachment" (macOS) or the edit
    /// menu's (iOS), and ⌘-click on a picture that has loaded.
    ///
    /// The other half of the pair `onOpenMedia` starts. That one means *load it
    /// anyway*: the box is empty, or the editor cannot play what is in it, and
    /// the answer is to fetch the bytes and call `reloadMedia(src)` so the
    /// picture appears here. This one means *go to it*: an app where an
    /// attachment is a thing in its own right — a node in a vault, a row in a
    /// file inspector, a page of its own — can show that, and a reader looking
    /// at the picture in the body is exactly who asks. An app with no such place
    /// leaves this nil.
    ///
    /// Offered for pictures, video, and audio alike: the question is about the
    /// attachment, not about what the editor can draw of it.
    ///
    /// Nil is the default and shows no affordance at all — no menu item, and
    /// ⌘-click on a picture goes on placing the caret, which is what it did
    /// before this existed. The rule `onEditLink` follows, for its reason: a
    /// menu item that calls nobody is worse than no menu item.
    public var onShowMedia: ((String) -> Void)? {
        didSet { textView?.onShowMedia = showMediaBridge }
    }

    /// `onShowMedia` as the views want it: nil while the host has set nothing, so
    /// `onShowMedia != nil` inside a view is still "is a host listening?" after
    /// the closure is read through to the model. Same shape as `editBridge`.
    var showMediaBridge: ((String) -> Void)? {
        guard onShowMedia != nil else { return nil }
        return { [weak self] src in self?.onShowMedia?(src) }
    }

    let doc: LeafDoc
    /// The surface this model drives, once SwiftUI has made one. Internal
    /// rather than private to the file, like `doc` above it: it is the model's
    /// half of a pair, and a caller inside the module may stand the two up
    /// without going through a representable.
    weak var textView: LeafTextView?

    /// Parse `source` as `format` (`"markdown"`, `"djot"`, `"html"`, `"xml"`).
    public init(source: String, format: String = "markdown") throws {
        let doc = try LeafDoc(source: source, format: format)
        // The views paint a picture in a line, so an inline formula arrives as
        // one `math` run to draw its typeset picture over — see `MathLayout`.
        _ = doc.setInlinePictures(on: true)
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

    // ── the highlight's colour ────────────────────────────────────────────────
    // `==🔴 text==`, Obsidian's spelling and the one core reads and writes. A
    // colour is a property of a highlight that already exists — core has no
    // "highlight this in red", because that is two splices and would be one
    // press a single undo could not take back — so the pair below is the whole
    // surface: the exact gesture, and the one a toolbar actually presses.

    /// Whether the caret stands in a highlight. What a colour palette enables
    /// itself by, together with `capabilities().markColor`: djot spells the
    /// highlight and no colour for it, so the format has to answer as well as
    /// the caret.
    ///
    /// Asked of the document rather than read off `state`, the way
    /// `caretInTable` is — a menu is built when it opens, so it sees the caret
    /// where it is now.
    public var caretInMark: Bool { doc.caretInMark() }

    /// The colour of the highlight at the caret, or nil — the swatch a menu
    /// ticks. Rides the published state, because moving between two coloured
    /// highlights changes nothing else about the frame (see `EditorState`).
    public var markColor: MarkColor? { state.markColor }

    /// Colour the highlight at the caret, or clear its colour with nil. Writes
    /// nothing where there is no highlight — see `highlight(_:)`, which is what
    /// a toolbar swatch should call.
    public func setMarkColor(_ color: MarkColor?) { run { $0.setMarkColor(color: color) } }

    /// Whether a colour swatch would land: this document spells a colour on a
    /// highlight (Markdown does, djot doesn't), and there is a highlight to
    /// colour — one the caret is in, or one `highlight(_:)` would make out of the
    /// selection. What a palette dims itself by.
    public var canColourHighlight: Bool {
        capabilities.markColor && (caretInMark || state.hasSelection)
    }

    /// Which formatting controls this document's format can spell — one flag per
    /// button, for chrome that dims rather than fails. Read per use rather than
    /// held: it is a static table lookup per flag, and a document can be handed a
    /// new source (and so a new format) under the same model.
    public var capabilities: Capabilities { doc.capabilities() }

    /// One press of a colour swatch: colour the highlight the caret is in, or —
    /// with a selection and no highlight yet — make one and colour it, as **one**
    /// undo step.
    ///
    /// Core's own compound (`Doc::highlight`), not two calls from here: what a
    /// swatch means over plain text is one answer for every frontend, and only
    /// core can fold the two splices into a single history step. Without that
    /// fold the way back from a red highlight would pass through an uncoloured
    /// one the author never asked for.
    ///
    /// A bare caret in no highlight is left alone — `toggleMark` there arms a
    /// mark for text not yet typed, and a colour cannot be armed with it. Gate
    /// the control on `canColourHighlight`, which is what
    /// `LeafFormattingToolbar` does.
    public func highlight(_ color: MarkColor?) { run { $0.highlight(color: color) } }

    // ── the presentation vocabulary ───────────────────────────────────────────
    // Alignment and line spacing are the block's, size, face and colour the
    // run's (and the block's with no selection — core decides which, not this).
    // Every one of them takes an `Option`: passing nil removes the key, which is
    // how a control spells "left", "single", and "the theme's own". See
    // `docs/proposals/presentation-vocabulary.md`.
    //
    // The five queries are asked of the *document* rather than read off `state`,
    // the way `caretInTable` is: each is offered in a menu, and a menu is built
    // when it opens, so it sees the caret where it is now. Alignment is the
    // exception — a segmented control is on screen the whole time and cannot ask
    // — so that one rides the published frame (`EditorState.align`).

    /// Align the caret's block, or clear its alignment with nil — which is left,
    /// the theme's default, and the reason there is no `.left` to pass.
    public func setAlignment(_ align: Align?) { run { $0.setAlignment(align: align) } }
    /// How the caret's block is aligned, from the published frame.
    public var alignment: Align? { state.align }

    // Each of the four below takes and answers the binding's *open* type — a
    // name or a value, `.step(.large)` or `.points(14)` — rather than the
    // closed enum a menu's named rows offer. One method per property rather
    // than an overload per half, because `setFontSize(nil)` has to go on
    // meaning one thing: clear the key. A named row spells itself at the call
    // site (`editor.setFontSize(.step(step))`), which is a word longer and says
    // which half it is asking for.

    /// Set the caret's block's line spacing — `.step(.oneHalf)` from the menu's
    /// three, or `.ratio(1.3)` from *Other…* — or clear it with nil (single).
    ///
    /// A ratio of 1 clears too: single spacing is the theme's own and has no
    /// token, so a document should not carry a key that says nothing. A ratio
    /// outside 0.01…655.35 writes nothing at all and the block's own spacing
    /// stands — validate the field before calling, which `PresentationEntry`
    /// is what the *Other…* rows do it with.
    public func setLineSpacing(_ spacing: LineHeight?) {
        run { $0.setLineSpacing(spacing: spacing) }
    }
    public var lineSpacing: LineHeight? { doc.lineSpacingAtCaret() }

    /// Set the size of the selection — or of the caret's whole block, with
    /// nothing selected — or clear it with nil.
    ///
    /// `.step(.large)` is a step up from whatever the text around it is set at,
    /// under every theme; `.points(14)` is fourteen points of the sheet and is
    /// all it is. The name is portable and the value is exact, and the menu row
    /// that offers the second says so.
    public func setFontSize(_ size: FontSize?) {
        run { $0.setFontSize(size: size) }
    }
    public var fontSize: FontSize? { doc.fontSizeAtCaret() }

    /// One rung up or down from whatever is at the caret — ⌘⇧+ and ⌘⇧-, the
    /// pair every Mac text editor binds. The same single gesture (and the same
    /// single undo) as picking the next size from the menu.
    ///
    /// From a step, or from no size at all, this walks the ramp, which has the
    /// theme's own size in the middle of it. **From an exact size it moves by a
    /// point**, because the ramp has no rung to walk to from fourteen and the
    /// author who typed a number is working in numbers — which is what a font
    /// panel's stepper does beside its own field. Which rung is
    /// `FontSize.stepped(from:up:)`, beside the ramp it walks.
    public func stepFontSize(up: Bool) {
        let next = FontSize.stepped(from: fontSize, up: up)
        run { $0.setFontSize(size: next) }
    }

    /// Set the face of the selection — or of the caret's whole block — by
    /// generic (`.generic(.serif)`) or by family (`.named("Garamond")`), or
    /// clear it with nil (the theme's body face).
    ///
    /// A generic opens on every machine and a family name does not: a run set
    /// in a family this machine hasn't got draws in the theme's body face. A
    /// name that names nothing writes nothing, and the run's own face stands.
    public func setFontFamily(_ font: FontFace?) {
        run { $0.setFontFamily(font: font) }
    }
    public var fontFamily: FontFace? { doc.fontFamilyAtCaret() }

    /// Colour the selection's text — or the caret's whole block — by name
    /// (`.named(.red)`) or by triple (`.rgb(r:g:b:)`), or clear it with nil
    /// (the theme's ink).
    ///
    /// The *foreground*, not `highlight(_:)`'s wash, though the two share the
    /// seven names on purpose: a frontend with a red for a highlight has a red
    /// for text, and both should be that red. A triple is painted as written in
    /// both appearances, where a name is two inks and the theme owns both.
    public func setTextColor(_ color: TextColor?) {
        run { $0.setTextColor(color: color) }
    }
    public var textColor: TextColor? { doc.textColorAtCaret() }

    /// Which *Other…* row a menu has just pressed, and so which field or picker
    /// the editing surface should raise — nil when none is open.
    ///
    /// On the model rather than in the bar, because a menu row cannot present
    /// anything: a `Menu`'s rows are torn down the instant one is chosen, so a
    /// `.popover` hung on one would be dismissed by the press that asked for
    /// it. The rows set this, `LeafEditor` watches it, and both surfaces that
    /// show the rows — the formatting bar's menus and the app's Format menu —
    /// therefore open the same field over the same document.
    @Published public var pendingOther: PresentationOther?

    /// Write a page break at the caret — `::page-break`, a leaf directive with no
    /// label. The paginated view opens a new sheet there and the continuous one
    /// draws a dashed hairline; a break at the very start of a document is a row
    /// and no page, so the first sheet is never blank.
    public func insertPageBreak() { run { $0.insertPageBreak() } }

    public func setParagraph()     { run { $0.setParagraph() } }
    public func setHeading(_ level: UInt32) { run { $0.setHeading(level: level) } }
    public func toggleBlockquote() { run { $0.toggleBlockquote() } }
    public func toggleList(ordered: Bool) { run { $0.toggleList(ordered: ordered) } }
    /// Toggle a fenced code block over the selection or the block at the caret
    /// — the toolbar's Code Block button, and the rich view's one way to open
    /// one, since a typed backtick is escaped there. On a blank line it opens an
    /// empty block with the caret inside, ready for the first line of code;
    /// `state.codeBlock` lights the button while the caret is in one. See
    /// `leaf_core::Doc::toggle_code_block`.
    public func toggleCodeBlock() { run { $0.toggleCodeBlock() } }
    /// Give the list item at the caret a checkbox, or take its checkbox away —
    /// the toolbar's Checklist button and the Format menu's item of the same
    /// name. A new box arrives unticked; `state.task` is non-nil while the caret
    /// stands in one. Gate on `capabilities.task`. See
    /// `leaf_core::Doc::toggle_task_item`.
    public func toggleTaskItem() { run { $0.toggleTaskItem() } }
    /// Tick or untick the task item at the caret — the keyboard half of the
    /// click a rendered box already answers in the views. A no-op outside a
    /// task item; `state.task` says which way the box faces.
    public func toggleTaskChecked() { run { $0.toggleTaskChecked() } }
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
    /// A fresh table at the caret — `rows` body rows under a header, `cols`
    /// wide — with the caret left in its first header cell. Needs no table
    /// under the caret; `capabilities.table` is the whole gate.
    public func insertTable(rows: Int = 2, cols: Int = 2) {
        run { $0.insertTable(rows: UInt32(max(rows, 1)), cols: UInt32(max(cols, 1))) }
    }

    public func undo() { run { $0.undo() } }
    public func redo() { run { $0.redo() } }

    #if canImport(AppKit) && !targetEnvironment(macCatalyst)
    /// Drive the system find bar: show it, find next/previous, replace, use the
    /// selection. What Edit ▸ Find's items do, for a menu built in SwiftUI.
    public func find(_ action: NSTextFinder.Action) { textView?.performFind(action) }
    /// Paste the clipboard's plain text as source, ignoring any rich flavour —
    /// Edit ▸ Paste and Match Style.
    public func pasteAsPlainText() { textView?.pasteAsPlainText(nil) }

    /// Whether the macOS editor checks prose with the system dictionaries as it
    /// is typed.
    public var isContinuousSpellCheckingEnabled: Bool {
        textView?.isContinuousSpellCheckingEnabled ?? true
    }

    public func toggleContinuousSpellChecking() {
        textView?.toggleContinuousSpellChecking(nil)
        objectWillChange.send()
    }

    public func checkSpelling() { textView?.checkSpelling(nil) }
    #endif
    public func toggleView() { run { $0.toggleView() } }

    // ── zoom ──────────────────────────────────────────────────────────────────
    // The model's, not the host's, because the surface changes it too: a pinch
    // lands here, and View ▸ Zoom In reaches the focused document through here.
    // The default is a fit, which is the identity off paper and, on it, the
    // sheet filling the window — the readable size on a screen whose point is
    // not a printer's, whatever the type on the sheet is set at.

    /// How large the document is on screen — see `Zoom`. Set it to move the
    /// view; read `zoomScale` for the number it currently is.
    @Published public var zoom: Zoom = .fitWidth {
        didSet { textView?.zoom = zoom }
    }

    /// The scale `zoom` resolves to on the current viewport, `1` being actual
    /// size. Published, so a "125%" label follows a pinch and a resize.
    @Published public private(set) var zoomScale: CGFloat = 1

    /// To the next stop up `Zoom.stops` from wherever the view is — View ▸ Zoom In.
    public func zoomIn() { zoom = .scale(Zoom.stepUp(from: zoomScale)) }
    /// To the next stop down — View ▸ Zoom Out.
    public func zoomOut() { zoom = .scale(Zoom.stepDown(from: zoomScale)) }
    /// One layout point per screen point — View ▸ Actual Size.
    public func actualSize() { zoom = .actualSize }

    /// What the surface reports after a pinch, a resize under a fit, or a page
    /// set or cleared. The mode is written at once when a gesture moved it — a
    /// gesture is never inside a SwiftUI update — and the scale a turn later,
    /// because a fit can re-resolve inside `updateNSView` (the host changed the
    /// page) and a publish from inside an update is what SwiftUI forbids.
    fileprivate func zoomChanged(_ mode: Zoom, _ scale: CGFloat) {
        if zoom != mode { zoom = mode }
        if zoomScale != scale {
            DispatchQueue.main.async { [weak self] in
                guard let self, self.zoomScale != scale else { return }
                self.zoomScale = scale
            }
        }
    }

    // ── text statistics ───────────────────────────────────────────────────────
    // Off unless a host asks for them. Counting is O(document) — a reparse and
    // an unwrapped layout of the whole thing, about 4 ms on 45 KB — and most
    // windows have nowhere to put the answer, so a model that counted by
    // default would spend that on nobody's behalf. What a host that *does* ask
    // gets is a number that follows the typing without riding it: one count per
    // settle, not one per keystroke.

    /// Whether the model keeps `counts` up to date. Off by default, for the
    /// price above.
    ///
    /// Turning it on counts at once, so a panel the reader just opened has
    /// something in it rather than a blank waiting out the debounce; turning it
    /// off drops `counts` to nil and cancels whatever was on the clock.
    public var countsEnabled: Bool = false {
        didSet {
            guard countsEnabled != oldValue else { return }
            countsWork?.cancel()
            countsWork = nil
            // Deferred, because a host flips this from inside its own view code
            // — a panel's disclosure, an `.onChange` on a menu item — and that
            // is a SwiftUI update as readily as the one `zoomChanged` dodges.
            recount(deferred: true)
        }
    }

    /// The live statistics: nil while `countsEnabled` is off, and until the
    /// first count lands. Republished only when a number moved — see
    /// `EditorCounts`.
    @Published public private(set) var counts: EditorCounts?

    /// The recount waiting on the typing to settle, cancelled and replaced by
    /// each repaint — which is what makes a burst of keystrokes one count.
    private var countsWork: DispatchWorkItem?

    /// How long "settled" is. Long enough that a fast typist's word is one
    /// count rather than six, short enough that the pill has caught up before
    /// they look up at it.
    private static let countsDebounce: TimeInterval = 0.15

    /// Put a recount on the clock. Called after every repaint that laid the
    /// frame out again (`onLayoutChange`), whatever the chrome state did: the
    /// state is no guide here, since a paragraph that has gained three words
    /// moves no mark, no heading, and no dirty flag after its first character
    /// — the very case a word count exists for. A repaint that only moved the
    /// caret changed no count and is not on the clock: the document's tally
    /// is the rows', the selection's is the selection's and the selection is
    /// in the rows, and the page count is the layout's.
    private func scheduleCounts() {
        guard countsEnabled else { return }
        countsWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            self?.countsWork = nil
            // A turn on the clock already, so there is no update to publish
            // from inside of.
            self?.recount(deferred: false)
        }
        countsWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.countsDebounce, execute: work)
    }

    /// Count, and hand the answer to `publishCounts`. Asks `countsEnabled`
    /// itself rather than trusting the caller, so the disable path spells "no
    /// counts" with the same call the enable path spells "these ones" with.
    private func recount(deferred: Bool) {
        guard countsEnabled else { return publishCounts(nil, deferred: deferred) }
        publishCounts(
            EditorCounts(document: doc.counts(),
                         selection: doc.selectionCounts(),
                         pages: pageCount),
            deferred: deferred)
    }

    /// The sheets the surface laid the document onto, and nil off paper or
    /// before there is a surface — see `EditorCounts.pages`.
    private var pageCount: Int? {
        guard let textView, textView.pageSetup != nil else { return nil }
        return textView.pages.count
    }

    /// Publish `value`, if it differs from what is published. `deferred` waits
    /// a turn first, which is what a count triggered from inside a SwiftUI
    /// update needs: publishing there is what the view system forbids, and
    /// `zoomChanged` steps around it exactly this way.
    private func publishCounts(_ value: EditorCounts?, deferred: Bool) {
        guard deferred else {
            if counts != value { counts = value }
            return
        }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.counts != value else { return }
            self.counts = value
        }
    }

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
    ///
    /// Through `prefer` rather than `run`, for that method's reason: this one is
    /// issued by the *host*, which may well not have put the document on screen
    /// yet, and a selection dropped on the floor there is indistinguishable from
    /// one that did not take.
    public func debugSelect(anchor: UInt32, focus: UInt32) {
        prefer { $0.setSelectionOffsets(anchor: anchor, focus: focus) }
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
        guard let textView else {
            _ = op(doc)
            // Nothing is going to repaint, and a repaint is what normally puts
            // the statistics back on the clock. This is the only path a
            // view-less document changes by, so it has to do that job too, or
            // a host counting a document it has not yet put on screen would be
            // shown the tally from before its own `debugSelect`.
            scheduleCounts()
            return
        }
        textView.command(op)
    }

    /// The surface has repainted: take the chrome state it reports. The
    /// statistics are the other half of a repaint, and go back on the clock
    /// from `relaid` — a separate closure on each platform, because a repaint
    /// that only moved the caret reports its state and changed no count.
    fileprivate func repainted(_ s: EditorState) {
        updateState(s)
    }

    /// The surface laid its frame out again — see `scheduleCounts`.
    fileprivate func relaid() {
        scheduleCounts()
    }

    private func updateState(_ s: EditorState) { if s != state { state = s } }
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
    /// `measure`. `nil` (the default) is the continuous scrolling flow. How
    /// large it is on screen is the model's `zoom`, which a pinch on the
    /// surface and View ▸ Zoom both move.
    public init(model: LeafEditorModel, theme: EditorTheme = .default,
                placeholder: String? = nil,
                page: PageSetup? = nil) {
        self.model = model
        self.surface = LeafEditorSurface(model: model, theme: theme, placeholder: placeholder,
                                         page: page)
    }

    public var body: some View {
        surface
            .focusedSceneValue(\.leafEditor, model)
            // The one place an *Other…* field is raised, from either surface
            // that offers the row — see `PresentationOther.swift`.
            .presentingOtherValues(model)
    }
}

/// The `NSViewRepresentable` under `LeafEditor`.
struct LeafEditorSurface: NSViewRepresentable {
    @ObservedObject private var model: LeafEditorModel
    private let theme: EditorTheme
    private let placeholder: String?
    private let page: PageSetup?

    init(model: LeafEditorModel, theme: EditorTheme, placeholder: String?,
         page: PageSetup?) {
        self.model = model; self.theme = theme; self.placeholder = placeholder
        self.page = page
    }

    public func makeNSView(context: Context) -> NSScrollView {
        let textView = makeTextView()

        let scroll = NSScrollView()
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        Self.configureScrollers(scroll, page: page)
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
        Self.configureScrollers(scroll, page: page)
        // Both guard themselves against an unchanged value, so re-applying them on
        // every SwiftUI update (which is every state change at all) costs a
        // comparison rather than a relayout.
        hosted.pageSetup = page
        hosted.placeholder = placeholder
        // Re-read rather than trusting the copy `makeTextView` took: a host that
        // flips this on the model after the view exists (or per document, for a
        // vault where only some files use the convention) gets it honoured.
        hosted.recognizesWikilinks = model.recognizesWikilinks
        hosted.isReadOnly = model.isReadOnly
        hosted.onTapHighlight = model.tapHighlightBridge
        hosted.mediaPlayback = model.mediaPlayback
        hosted.onResolveMedia = model.onResolveMedia
        hosted.onLocateMedia = model.onLocateMedia
        // Last, because setting it lays the document out, and that layout asks
        // for every picture: with the hooks not yet wired, a `/`-rooted
        // reference missed, and the miss was what the reader saw until they
        // double-clicked the box.
        hosted.documentDirectory = model.documentDirectory
    }

    /// The editor fills whatever it is given, so say so rather than have SwiftUI
    /// measure the scroll view for a size it does not have.
    ///
    /// Measuring is not free of side effects: SwiftUI reads an AppKit view's
    /// size through its constraint engine, every read marks the window's
    /// constraints dirty, and a window whose constraints stay dirty through one
    /// display cycle is one AppKit throws in — an uncatchable crash, not a
    /// warning — which a document of five column-fitted pictures managed.
    ///
    /// Every proposal is answered, not only one with both axes given. The
    /// hosting view under a split-view column asks for the column's *minimum*
    /// and *maximum* too, with nothing and with infinity proposed, and an
    /// unanswered ask is the measurement above: the scroll view's fitting
    /// size, which is the document's own height at whatever width it last
    /// wrapped to. That number moves with every re-wrap and every picture that
    /// arrives, the column reports a new maximum on every constraints pass,
    /// and the pass never settles — the five-picture crash again, reached
    /// through the inspector rather than a scroll bar. See `surfaceSize`.
    @available(macOS 13.0, *)
    public func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView,
                             context: Context) -> CGSize? {
        Self.surfaceSize(for: proposal)
    }

    /// The size the surface reports for `proposal`: the proposal itself, with
    /// an axis left open taken as zero.
    ///
    /// That is the shape of a view with no size of its own — the answer
    /// `Color` gives — and it is what makes the editor fully flexible to the
    /// stack it sits in: nothing at the minimum, the whole of what is offered
    /// at the maximum, and never a number read off the document. Infinity
    /// passes through, as it does for any fill.
    @available(macOS 13.0, *)
    static func surfaceSize(for proposal: ProposedViewSize) -> CGSize {
        CGSize(width: proposal.width ?? 0, height: proposal.height ?? 0)
    }

    /// Which scrollers the view has, and whether they may come and go.
    ///
    /// A scroller that takes up room — scroll bars set to "Always", or a mouse
    /// attached — changes the viewport's width when it appears, and the
    /// continuous flow re-wraps to the viewport's width, which changes the
    /// document's height, which is what decides whether the scroller appears.
    /// Near the threshold that is a loop AppKit ends by throwing. So the
    /// continuous flow keeps its vertical scroller on (an overlay scroller is
    /// unaffected: it hides itself when idle either way) and has no horizontal
    /// one, its width being the viewport's by construction. A stack of sheets
    /// is a fixed width that a narrower window scrolls sideways to, so there
    /// both scrollers are wanted and neither feeds back into the wrap.
    private static func configureScrollers(_ scroll: NSScrollView, page: PageSetup?) {
        let paged = page != nil
        if scroll.hasHorizontalScroller != paged { scroll.hasHorizontalScroller = paged }
        if scroll.autohidesScrollers != paged { scroll.autohidesScrollers = paged }
    }

    /// Build a `LeafTextView` over `model.doc`, wired the way `makeNSView` and the
    /// stale-binding rebuild in `updateNSView` both need it.
    private func makeTextView() -> LeafTextView {
        let textView = LeafTextView(doc: model.doc, theme: theme)
        textView.pageSetup = page
        textView.zoom = model.zoom
        textView.onZoomChange = { [weak model] mode, scale in model?.zoomChanged(mode, scale) }
        textView.placeholder = placeholder
        // Defer the publish: `render()` can fire during a SwiftUI layout pass, and
        // mutating an `@Published` mid-update loops the view system.
        textView.onStateChange = { [weak model] s in
            DispatchQueue.main.async { model?.repainted(s) }
        }
        textView.onLayoutChange = { [weak model] in
            DispatchQueue.main.async { model?.relaid() }
        }
        // Read through to the model rather than copying its handler across: a
        // host that sets `onOpenLink` after the editor is on screen (the usual
        // shape — the model is built when a document loads, the handler wired
        // where the view is composed) still gets its links.
        textView.onOpenLink = { [weak model] destination in
            model?.onOpenLink?(destination) ?? false
        }
        textView.onEdit = { [weak model] in
            DispatchQueue.main.async { model?.onEdit?() }
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
        textView.mediaPlayback = model.mediaPlayback
        textView.onResolveMedia = model.onResolveMedia
        textView.onLocateMedia = model.onLocateMedia
        // Last, because setting it lays the document out, and that layout asks
        // for every picture: with the hooks not yet wired, a `/`-rooted
        // reference missed, and the miss was what the reader saw until they
        // double-clicked the box.
        textView.documentDirectory = model.documentDirectory
        // Weak, like `onOpenLink` above: the closure outlives a host that swaps
        // its model, and a strong capture would keep the old one alive.
        textView.onOpenMedia = { [weak model] src in
            model?.onOpenMedia?(src)
        }
        // Through the bridge, not read through: the menu gates on it. See
        // `showMediaBridge`.
        textView.onShowMedia = model.showMediaBridge
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
    private let theme: EditorTheme
    private let placeholder: String?
    private let page: PageSetup?
    private let accessory: AnyView?
    private var header: AnyView?

    /// `placeholder` is the cue shown while the document is empty, drawn where
    /// its first character will go — see `LeafTextView.placeholder`. `page`
    /// puts the document on paper, as the AppKit peer's does: a stack of sheets
    /// the reader pinches to a comfortable size, or that the model's `zoom` fits
    /// to the screen's width. `nil` (the default) is the continuous flow.
    public init(model: LeafEditorModel, theme: EditorTheme = .default,
                placeholder: String? = nil, page: PageSetup? = nil) {
        self.model = model; self.theme = theme; self.placeholder = placeholder
        self.page = page
        self.accessory = nil
    }

    /// With a custom view shown above the system keyboard while this editor is
    /// first responder — a host app's own formatting toolbar. See
    /// `LeafTextView.accessoryView` for why this has to be threaded through
    /// explicitly rather than SwiftUI's own `.toolbar(placement: .keyboard)`.
    public init<Accessory: View>(
        model: LeafEditorModel, theme: EditorTheme = .default,
        placeholder: String? = nil, page: PageSetup? = nil,
        @ViewBuilder accessory: () -> Accessory
    ) {
        self.model = model; self.theme = theme; self.placeholder = placeholder
        self.page = page
        self.accessory = AnyView(accessory())
    }

    /// With a view laid above the first line, *inside* the scroll: a row of
    /// chips about the document, a title, a byline — anything that belongs to
    /// the top of the document rather than to the screen. It scrolls away with
    /// the prose and, where the host lets the editor run under a bar, goes
    /// under it the same way.
    ///
    /// A modifier rather than a fourth initializer because the header and the
    /// accessory are independent, and every combination as an `init` is a
    /// grid. Stacking the editor under the header in the host's own `VStack`
    /// is not the same thing: then the editor's top edge is a hard line the
    /// text vanishes behind, and a `safeAreaInset` does not reach the scroll
    /// view — SwiftUI's safe area stops at the platform view, so the text
    /// would rest under the chips rather than below them.
    ///
    /// Whether there *is* a header is decided when the surface is made; the
    /// view inside it is live, and a header whose content has nothing to say
    /// can lay out to zero height.
    public func header<Header: View>(@ViewBuilder _ content: () -> Header) -> LeafEditor {
        var copy = self
        copy.header = AnyView(content())
        return copy
    }

    public var body: some View {
        LeafEditorSurface(model: model, theme: theme, placeholder: placeholder, page: page,
                          accessory: accessory, header: header)
            .focusedSceneValue(\.leafEditor, model)
            // The one place an *Other…* field is raised, from either surface
            // that offers the row — see `PresentationOther.swift`.
            .presentingOtherValues(model)
    }
}

/// The controller under `LeafEditorSurface`: the scroll view is its view, and
/// the header's `UIHostingController`, when there is one, is its child.
///
/// A controller rather than the bare scroll view because of the header. Its
/// content is SwiftUI, and SwiftUI content presents — a chip opens a sheet —
/// which needs a controller in the hierarchy to present from, and takes
/// appearance callbacks, which UIKit forwards only down a parent chain. A
/// hosting controller whose view is simply added to the scroll view is in
/// the window but nobody's child, and gets neither. Made a child here, it
/// gets both the ordinary way.
///
/// It also keeps the keyboard off the prose itself, by content inset, rather
/// than leaving that to the host's layout. Left to SwiftUI, the keyboard is
/// safe area: the host's frame for the editor shrinks to the keyboard's top,
/// or should — measured on iOS 27 with a hosted accessory bar, it lands at
/// the keyboard's top plus or minus the accessory's height, changing sign
/// between layout passes and settling on a band of blank paper the height
/// of the bar above it. Insetting by the keyboard's real overlap with the
/// scroll view works in either host: one that runs the editor under the
/// keyboard (`.ignoresSafeArea(.keyboard)`) gets the full inset, and one
/// that still shrinks the frame gets an overlap of zero and nothing changes.
final class LeafEditorController: UIViewController {
    let scroll = UIScrollView()
    /// `content.height >= frame.height - insets`, the fill `pin(_:into:header:)`
    /// installs. The constant is the adjusted insets, so a short document fills
    /// what is *visible* — under a bar, above a keyboard — and no further: at
    /// the frame's full height it could be pulled up into blank paper by the
    /// height of whatever covers the edges.
    var fill: NSLayoutConstraint?
    private var keyboardObserver: Any?

    override func loadView() { view = scroll }

    override func viewDidLoad() {
        super.viewDidLoad()
        // One notification covers a rise, a drop, a height change (predictions,
        // a different layout) and every frame of an interactive dismissal.
        keyboardObserver = NotificationCenter.default.addObserver(
            forName: UIResponder.keyboardWillChangeFrameNotification, object: nil, queue: .main
        ) { [weak self] note in self?.keyboardWillChangeFrame(note) }
    }

    deinit {
        if let keyboardObserver { NotificationCenter.default.removeObserver(keyboardObserver) }
    }

    override func viewSafeAreaInsetsDidChange() {
        super.viewSafeAreaInsetsDidChange()
        updateFill()
    }

    private func keyboardWillChangeFrame(_ note: Notification) {
        guard let info = note.userInfo, let window = view.window,
              let end = (info[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue)?.cgRectValue
        else { return }
        // A keyboard raised in another scene of this app is not over this one.
        if let local = info[UIResponder.keyboardIsLocalUserInfoKey] as? Bool, !local { return }
        // Screen coordinates, to the window's, to the scroll view's. A hidden
        // keyboard's frame sits below the screen, so its overlap is nothing.
        let inView = view.convert(window.convert(end, from: nil), from: window)
        let overlap = max(0, view.bounds.maxY - inView.minY)
        // The safe area under the frame is already an inset (`adjustedContentInset`
        // adds it); the keyboard covers that band too, so count it once.
        let inset = max(0, overlap - view.safeAreaInsets.bottom)
        guard abs(scroll.contentInset.bottom - inset) > 0.5 else { return }
        let duration = info[UIResponder.keyboardAnimationDurationUserInfoKey] as? Double ?? 0.25
        let curve = info[UIResponder.keyboardAnimationCurveUserInfoKey] as? UInt ?? 7
        UIView.animate(withDuration: duration, delay: 0,
                       options: UIView.AnimationOptions(rawValue: curve << 16)) {
            self.scroll.contentInset.bottom = inset
            self.scroll.verticalScrollIndicatorInsets.bottom = inset
            self.updateFill()
            self.scroll.layoutIfNeeded()
        }
        // The keyboard rose over the caret, or the room above it changed: bring
        // the caret back into what is visible, as a text view would. Nothing
        // here resizes the text view — see `LeafTextView.intrinsicContentSize`
        // for why its tail deliberately does not follow the keyboard.
        if inset > 0, let textView, textView.isFirstResponder {
            textView.revealCaret()
        }
    }

    /// The text view in the scroll, through the wrapper that carries its zoom.
    var textView: LeafTextView? {
        (scroll.subviews.first { $0 is LeafZoomView } as? LeafZoomView)?.textView
    }

    private func updateFill() {
        guard let fill else { return }
        let insets = scroll.adjustedContentInset
        let constant = -(insets.top + insets.bottom)
        if abs(fill.constant - constant) > 0.5 { fill.constant = constant }
    }
}

/// The `UIViewControllerRepresentable` under `LeafEditor`.
struct LeafEditorSurface: UIViewControllerRepresentable {
    @ObservedObject private var model: LeafEditorModel
    private let theme: EditorTheme
    private let placeholder: String?
    private let page: PageSetup?
    /// Type-erased so the surface stays a concrete, non-generic type.
    private let accessory: AnyView?
    /// The view above the first line, inside the scroll — see `LeafEditor.header`.
    private let header: AnyView?

    init(model: LeafEditorModel, theme: EditorTheme, placeholder: String?, page: PageSetup?,
         accessory: AnyView?, header: AnyView?) {
        self.model = model; self.theme = theme; self.placeholder = placeholder; self.page = page
        self.accessory = accessory; self.header = header
    }

    public func makeCoordinator() -> Coordinator { Coordinator() }

    /// Holds the accessory's `UIHostingController` across SwiftUI updates —
    /// `LeafEditor` itself is a value type recreated every update, so this is
    /// the one thing that survives to have its `rootView` refreshed rather
    /// than being torn down and rebuilt each time.
    public final class Coordinator {
        var hosting: UIHostingController<AnyView>?
        /// The header's, kept for the same reason `hosting` is: its `rootView`
        /// is refreshed in place on each update rather than rebuilt, which
        /// would drop whatever the header had in flight — a sheet it presents,
        /// a field mid-edit.
        var headerHosting: UIHostingController<AnyView>?
        /// The view the accessory hangs off, so a resize can ask *it* to re-read
        /// its input views — `reloadInputViews()` is the first responder's call,
        /// and the hosting controller isn't one.
        weak var textView: LeafTextView?
        private var sizeObserver: NSObjectProtocol?

        /// Re-measure the accessory whenever the reader changes their text size.
        ///
        /// The notification rather than `updateUIViewController`, because SwiftUI never
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

    public func makeUIViewController(context: Context) -> LeafEditorController {
        let textView = makeTextView()
        attachAccessory(to: textView, context: context)

        let controller = LeafEditorController()
        let scroll = controller.scroll
        scroll.alwaysBounceVertical = true
        scroll.keyboardDismissMode = .interactive
        // One finger scrolls; two are the text view's pinch and nothing else's.
        // Left at its default the scroll view's pan takes a second finger too,
        // and a two-finger drag that is not quite a pinch scrolls and scales at
        // once. (A pan already under way on the first finger is the pinch's
        // to cancel — see `handlePinch`.)
        scroll.panGestureRecognizer.maximumNumberOfTouches = 1
        pin(textView, into: controller, header: makeHeader(context: context))

        // A reader is opened to be read — see the AppKit peer.
        if !model.isReadOnly {
            DispatchQueue.main.async { _ = textView.becomeFirstResponder() }
        }
        return controller
    }

    public func updateUIViewController(_ controller: LeafEditorController, context: Context) {
        guard let hosted = controller.textView else { return }
        // A freshly-swapped model has never been through `makeUIViewController`, so its
        // `textView` is still nil — that mismatch (rather than comparing docs
        // directly, which `LeafTextView` doesn't expose) is the stale-binding
        // signal. SwiftUI keeps this view's identity across the swap, so without
        // this the cached `hosted` view would go on showing the OLD model's doc
        // forever (the bug this fixes; hosts no longer need `.id(...)`).
        guard model.textView === hosted else {
            hosted.zoomHost?.removeFromSuperview() // also tears down its own constraints
            if let header = context.coordinator.headerHosting {
                header.willMove(toParent: nil)
                header.view.removeFromSuperview()
                header.removeFromParent()
            }
            let textView = makeTextView()
            attachAccessory(to: textView, context: context)
            pin(textView, into: controller, header: makeHeader(context: context))
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
        // Guards itself against an unchanged value, so re-applying it on every
        // SwiftUI update costs a comparison rather than a relayout.
        hosted.pageSetup = page
        // Re-read rather than trusting the copy `makeTextView` took: a host that
        // flips this on the model after the view exists (or per document, for a
        // vault where only some files use the convention) gets it honoured.
        hosted.recognizesWikilinks = model.recognizesWikilinks
        hosted.isReadOnly = model.isReadOnly
        hosted.selectionMenuActions = model.selectionMenuBridge
        hosted.onTapHighlight = model.tapHighlightBridge
        hosted.mediaPlayback = model.mediaPlayback
        hosted.onResolveMedia = model.onResolveMedia
        hosted.onLocateMedia = model.onLocateMedia
        // Last, because setting it lays the document out, and that layout asks
        // for every picture: with the hooks not yet wired, a `/`-rooted
        // reference missed, and the miss was what the reader saw until they
        // double-clicked the box.
        hosted.documentDirectory = model.documentDirectory
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
        // The header's height is its content's (`sizingOptions`), so a swap that
        // changes it re-lays the scroll's content out by itself.
        if let header { context.coordinator.headerHosting?.rootView = header }
    }

    /// The header as a controller to pin over the text, or nil when the host
    /// set none.
    ///
    /// Sized by its SwiftUI content through `intrinsicContentSize`, so the text
    /// view's top follows the chips' bottom without anyone measuring the strip —
    /// unlike the accessory, whose frame a keyboard reads and which therefore
    /// has to be measured by hand.
    private func makeHeader(context: Context) -> UIHostingController<AnyView>? {
        guard let header else { return nil }
        let hosting = UIHostingController(rootView: header)
        hosting.view.backgroundColor = .clear
        hosting.sizingOptions = .intrinsicContentSize
        context.coordinator.headerHosting = hosting
        return hosting
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

    /// Build a `LeafTextView` over `model.doc`, wired the way `makeUIViewController` and the
    /// stale-binding rebuild in `updateUIViewController` both need it.
    private func makeTextView() -> LeafTextView {
        let textView = LeafTextView(doc: model.doc, theme: theme)
        textView.placeholder = placeholder
        textView.pageSetup = page
        textView.zoom = model.zoom
        textView.onZoomChange = { [weak model] mode, scale in model?.zoomChanged(mode, scale) }
        // Defer the publish: `render()` can fire during a SwiftUI layout pass, and
        // mutating an `@Published` mid-update loops the view system.
        textView.onStateChange = { [weak model] s in
            DispatchQueue.main.async { model?.repainted(s) }
        }
        textView.onLayoutChange = { [weak model] in
            DispatchQueue.main.async { model?.relaid() }
        }
        // Read through to the model rather than copying its handler across: a
        // host that sets `onOpenLink` after the editor is on screen (the usual
        // shape — the model is built when a document loads, the handler wired
        // where the view is composed) still gets its links.
        textView.onOpenLink = { [weak model] destination in
            model?.onOpenLink?(destination) ?? false
        }
        textView.onEdit = { [weak model] in
            DispatchQueue.main.async { model?.onEdit?() }
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
        textView.mediaPlayback = model.mediaPlayback
        textView.onResolveMedia = model.onResolveMedia
        textView.onLocateMedia = model.onLocateMedia
        // Last, because setting it lays the document out, and that layout asks
        // for every picture: with the hooks not yet wired, a `/`-rooted
        // reference missed, and the miss was what the reader saw until they
        // double-clicked the box.
        textView.documentDirectory = model.documentDirectory
        // Weak, like `onOpenLink` above: the closure outlives a host that swaps
        // its model, and a strong capture would keep the old one alive.
        textView.onOpenMedia = { [weak model] src in
            model?.onOpenMedia?(src)
        }
        // Through the bridge, not read through: the menu gates on it. See
        // `showMediaBridge`.
        textView.onShowMedia = model.showMediaBridge
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

    /// Add `textView` to the controller's scroll view, under `header` when there
    /// is one, and pin them to the content/frame layout guides — the same
    /// constraint set `makeUIViewController` and the stale-binding rebuild both
    /// need. The header joins as a child controller, for the reasons on
    /// `LeafEditorController`.
    ///
    /// What is pinned is the text view's `LeafZoomView`, never the text view:
    /// the zoom is the text view's transform, which constraints do not see, and
    /// the wrapper is what carries the scaled size to the scroll view.
    private func pin(_ textView: LeafTextView, into controller: LeafEditorController,
                     header: UIHostingController<AnyView>?) {
        let scroll = controller.scroll
        let zoomed = LeafZoomView(textView: textView)
        scroll.addSubview(zoomed)
        zoomed.translatesAutoresizingMaskIntoConstraints = false
        // Without this, the content's height is purely the text view's intrinsic
        // height — for a short or empty document that's a sliver at the top, and
        // UIKit only routes touches to a view under them, so tapping anywhere in
        // the rest of the visible editor pane hit nothing (no caret, no focus,
        // typing impossible). `EditorLayout.hit` already clamps a point below the
        // last row to it, so filling the viewport just makes that reachable —
        // clicking below the text lands the caret at the document's end, same as
        // most text editors. On the content guide rather than the text view so
        // that a header counts toward the fill: with it on the text view a short
        // document scrolled by exactly the header's height into blank paper. The
        // constant is the controller's to keep — the visible height, not the
        // frame's, see `LeafEditorController.fill`.
        let fill = scroll.contentLayoutGuide.heightAnchor.constraint(
            greaterThanOrEqualTo: scroll.frameLayoutGuide.heightAnchor)
        controller.fill = fill
        // The width is the viewport's — except on paper, where a sheet wider
        // than the screen keeps its width (the wrapper's intrinsic width, at a
        // required compression resistance) and the scroll view scrolls sideways
        // to it. So the equality yields, and only the floor is required.
        let width = zoomed.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor)
        width.priority = .defaultHigh
        var constraints = [
            zoomed.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            zoomed.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            zoomed.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            zoomed.widthAnchor.constraint(greaterThanOrEqualTo: scroll.frameLayoutGuide.widthAnchor),
            width,
            fill,
        ]
        if let header {
            controller.addChild(header)
            scroll.addSubview(header.view)
            header.didMove(toParent: controller)
            header.view.translatesAutoresizingMaskIntoConstraints = false
            // The fill above has to go to one of the two, and with both at the
            // default hugging it went to the header: a short document put the
            // chips halfway down a tall blank strip, the first line at the
            // bottom of the screen, and a tap in the strip landed on the
            // header's view, not the text — no caret. The header hugs its
            // content, so the slack is the text view's, where a tap below the
            // last row lands the caret at the end as it does without a header.
            header.view.setContentHuggingPriority(.required, for: .vertical)
            zoomed.setContentHuggingPriority(.defaultLow, for: .vertical)
            constraints += [
                header.view.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
                header.view.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
                header.view.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
                zoomed.topAnchor.constraint(equalTo: header.view.bottomAnchor),
            ]
        } else {
            constraints.append(zoomed.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor))
        }
        NSLayoutConstraint.activate(constraints)
    }
}
#endif
