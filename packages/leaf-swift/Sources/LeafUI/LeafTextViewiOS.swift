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
    ///
    /// Which lengths the factor reaches is `EditorTheme.scaled(by:)`'s answer, not
    /// this method's: the same question comes up wherever a theme is resized, and
    /// only the theme knows which of its numbers are typography.
    private func applyDynamicType() {
        let old = renderTheme
        let factor = UIFontMetrics.default.scaledValue(for: 100, compatibleWith: traitCollection) / 100
        renderTheme = hostTheme.scaled(by: factor)
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
            render(docView)
        }
    }

    /// Gets first refusal on a paste. See `LeafEditorModel.onPaste`.
    public var onPaste: (() -> Bool)?

    /// Reconsider `src` — or every source, for nil — and redraw.
    /// See `LeafEditorModel.reloadMedia`.
    public func reloadMedia(_ src: String?) {
        mediaStore.forget(src)
        render(docView)
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

    public init(doc: LeafDoc, theme: EditorTheme = .default) {
        self.doc = doc
        self.hostTheme = theme
        self.renderTheme = theme
        // Unwrapped layout (one row per block); the view soft-wraps at pixel width.
        let first = doc.setUnwrapped()
        self.docView = first
        var seed: [Row: ShapedRow] = [:]
        self.layoutEngine = EditorLayout(first, theme: renderTheme, viewWidth: 0, cache: &seed)
        self.shapeCache = seed
        super.init(frame: .zero)
        // A resolved source has a picture to draw, and may be the one the reader
        // tapped while it was still being fetched.
        mediaStore.onLoaded = { [weak self] src in
            guard let self else { return }
            self.setNeedsDisplay()
            self.playIfAwaited(src)
        }
        backgroundColor = .clear
        contentMode = .redraw
        addInteraction(textInteraction)
        addGestureRecognizer(mediaTap)
        addGestureRecognizer(linkPress)
        addInteraction(editMenu)
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
            || !doc.linkActionsAtCaret(wikilinks: recognizesWikilinks,
                                       canEdit: onEditLink != nil,
                                       canPeek: onPeekLink != nil).isEmpty
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
        let point = gesture.location(in: self)
        // A tap on a video or audio box starts it, and a tap on an *empty*
        // picture box asks the host for it.
        if let hit = layoutEngine.mediaBox(at: point) {
            _ = activateMedia(hit)
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
        let viewportHeight = enclosingScrollView()?.bounds.height ?? 0
        let extra = raw > viewportHeight ? viewportHeight * 0.5 : 0
        return CGSize(width: UIView.noIntrinsicMetric, height: raw + extra)
    }

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
            render(docView)
        }
    }

    // MARK: applying a frame

    /// Install a fresh `DocView` and repaint. The system re-reads `selectedTextRange`
    /// and re-lays its selection overlays afterward.
    private func render(_ view: DocView) {
        // A peek is chrome anchored to a position, and the positions are being
        // rebuilt — an edit reflowed the line it points at, or a relayout moved
        // the reference out from under it.
        footnotePeek.hide()
        docView = view
        layoutEngine = EditorLayout(view, theme: renderTheme, viewWidth: viewWidth, cache: &shapeCache,
                                    media: mediaStore)
        // Installed players follow their boxes; media edited out of the document
        // is absent from the rects, which is what stops its playback.
        if !mediaPlayers.isEmpty {
            mediaPlayers.reposition(layoutEngine.mediaRects())
        }
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

    private func enclosingScrollView() -> UIScrollView? {
        var v: UIView? = superview
        while let cur = v { if let s = cur as? UIScrollView { return s }; v = cur.superview }
        return nil
    }

    // MARK: drawing — text + code panels only; the system draws all selection UI

    public override func draw(_ rect: CGRect) {
        guard let ctx = UIGraphicsGetCurrentContext() else { return }
        let padX = layoutEngine.originX
        let fullWidth = layoutEngine.columnWidth

        // Under every other mark: a light behind the words, not over them.
        drawLandingFlash(in: ctx)
        drawDirectiveBorders(in: ctx, dirtyRect: rect)
        // One pass for the quote bars (a run of quoted rows merges into a single
        // bar), before the rows, exactly as the AppKit surface orders it.
        BlockChrome.drawQuoteBars(layoutEngine.rows, theme: renderTheme, in: ctx)

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
            // A media box likewise draws once, on its first placeholder row, in
            // place of core's `🖼 alt` glyphs. Inset by the row's own prefix, so a
            // picture inside a quote or a list sits beside its gutter.
            if let box = rl.media {
                if rl.mediaFirst {
                    BlockChrome.drawMedia(box,
                                          at: box.rect(top: rl.mediaTop, left: padX + rl.shaped.prefixWidth),
                                          theme: renderTheme,
                                          playing: mediaPlayers.isPlaying(box.media.src), in: ctx)
                }
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
            // The system paints selection on iOS, so no selection fill here.
            BlockChrome.drawRule(rl, theme: renderTheme, selColor: nil, in: ctx)
            // Draw each wrapped visual line's substring on its own line box, hung
            // at the row's indent (zero on the first line, the prefix width after).
            for (i, wl) in rl.wrapped.enumerated() {
                let lineTop = rl.top + rl.labelInset + CGFloat(i) * rl.lineHeight
                if lineTop >= rect.maxY { break }
                if lineTop + rl.lineHeight <= rect.minY { continue }
                wl.attributed.draw(with: CGRect(x: padX + wl.indent, y: lineTop,
                                                width: fullWidth - wl.indent, height: rl.lineHeight),
                                   options: [.usesLineFragmentOrigin], context: nil)
            }
        }

        if let placeholder, let box = layoutEngine.placeholderBox {
            BlockChrome.drawPlaceholder(placeholder, in: box, theme: renderTheme, in: ctx)
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
        let padX = layoutEngine.originX
        let fullWidth = layoutEngine.columnWidth
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
    public func editMenu(
        for textRange: UITextRange, suggestedActions: [UIMenuElement]
    ) -> UIMenu? {
        guard let host = selectionMenuActions?(), !host.isEmpty else { return nil }
        return UIMenu(children: [UIMenu(options: .displayInline, children: host)] + suggestedActions)
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
            guard let caret = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch)) else { break }
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
        let rc = doc.posForOffset(off: UInt32(off(position)))
        return layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch)) ?? .zero
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
                let rect = CGRect(x: layoutEngine.originX + wl.indent + x0,
                                  y: rl.top + rl.labelInset + CGFloat(i) * rl.lineHeight,
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

extension LeafTextView: UIGestureRecognizerDelegate {
    /// `mediaTap` never competes with `textInteraction`'s own recognisers — the
    /// system owns caret placement, selection, the loupe and the edit menu, and
    /// activating a media box is strictly additive to whichever of those the
    /// touch was also going to drive. Failing to say so would make the two
    /// exclusive, and the system's would win.
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
        let actions: [UIAction] = footnoteMenuActions() + doc
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
#endif
