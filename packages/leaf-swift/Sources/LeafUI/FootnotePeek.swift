//  FootnotePeek.swift
//
//  The popover that shows a footnote's text where the reference is, so reading a
//  note costs a glance rather than a trip. Following one is still there
//  (`FootnoteJump`) for when a reader wants to *be* at the note — but most of
//  the time the question is "what does that say", and answering it by scrolling
//  the reader to the foot of the document and leaving them there is answering a
//  different question.
//
//  The note is drawn in the *document's* fonts and colours, not the system's:
//  it arrives as the same styled runs the page below is drawing it from (see
//  `FootnotePeekContent`), so a `code` span in a note looks like a `code` span.
//  Only what the popover adds around it — the sentence for a reference nothing
//  defines, the underline under what can be followed — is chrome, and that is
//  what wears system colours.
//
//  A peek is a thing the reader can *use*: the pointer can travel into it, its
//  text selects and copies, its links follow, and a footnote reference inside a
//  note navigates the document to that note rather than stacking a second
//  popover. Keeping it alive long enough to reach is the delegate's job.
//
//  What it *says* is `FootnotePeekContent`'s, shared with the UIKit presenter
//  below it. Only the presentation is per-platform.

#if canImport(AppKit)
import AppKit

/// What the owning text view needs to hear back from a peek the reader is using.
protocol FootnotePeekDelegate: AnyObject {
    /// The pointer entered or left the popover. A peek the reader has moved into
    /// must not vanish under them — the whole point of showing a note in place is
    /// that they can read it, and selecting a line of it or following a link in
    /// it means being inside it.
    func footnotePeek(_ presenter: FootnotePeekPresenter, pointerIsInside: Bool)
    /// The reader clicked something in the note that leads somewhere.
    func footnotePeek(_ presenter: FootnotePeekPresenter, didFollow target: FootnotePeekTarget)
}

/// Presents (and owns) the peek popover on macOS.
///
/// A class rather than a method on the view because the popover outlives the
/// event that raised it: it stays up while the pointer rests, and something has
/// to hold it in the meantime and take it down again.
final class FootnotePeekPresenter {
    private var popover: NSPopover?
    weak var delegate: FootnotePeekDelegate?

    var isShowing: Bool { popover?.isShown ?? false }

    /// Raise a popover for `content`, pointing at `rect` in `view`'s coordinates.
    ///
    /// Replacing rather than reusing any popover already up: the alternative is
    /// resizing a live one to different text, which AppKit animates as a lurch.
    func show(_ content: FootnotePeekContent, from rect: CGRect, in view: NSView) {
        hide()
        let popover = NSPopover()
        popover.contentViewController = controller(for: content)
        // `.applicationDefined` because this view decides when the peek is over —
        // the pointer leaving both the reference and the popover, a click through
        // to the document, a keystroke, a relayout. `.transient` would also close
        // it on the first click *anywhere*, including a click inside it to start
        // selecting the note's text.
        popover.behavior = .applicationDefined
        // A tooltip that fades in is a tooltip that is still fading when the
        // reader has already read it.
        popover.animates = false
        popover.show(relativeTo: rect, of: view, preferredEdge: .maxY)
        self.popover = popover
    }

    func hide() {
        popover?.performClose(nil)
        popover = nil
    }

    private func controller(for content: FootnotePeekContent) -> NSViewController {
        // Measured, not left to auto layout. Wrapping text has an intrinsic width
        // of zero — it will shrink to whatever it is given — so a popover sized
        // from the container's fitting size collapses to a sliver and truncates
        // the note to its first letter.
        let width = min(Self.maxWidth, max(Self.minWidth, ceil(Self.measure(content.body).width)))

        let drawn = NSMutableAttributedString(attributedString: content.body)
        // leaf's own sentence ("no note defined") has no runs behind it, so it is
        // dressed here — in the secondary colour, to say plainly that it is
        // chrome and not the document's words. A real note arrived already
        // wearing the document's fonts and colours and is left alone.
        if !content.isDefined {
            drawn.addAttributes(
                [.font: NSFont.systemFont(ofSize: NSFont.systemFontSize),
                 .foregroundColor: NSColor.secondaryLabelColor],
                range: NSRange(location: 0, length: drawn.length))
        }
        // Underline what leads somewhere. A link a reader can't see is a link
        // they won't try, and inside a popover there is no hover-the-status-bar
        // to fall back on.
        drawn.enumerateAttribute(.footnoteTarget, in: NSRange(location: 0, length: drawn.length)) { value, range, _ in
            guard value != nil else { return }
            drawn.addAttributes([.underlineStyle: NSUnderlineStyle.single.rawValue,
                                 .cursor: NSCursor.pointingHand], range: range)
        }

        let body = FootnotePeekTextView()
        body.presenter = self
        body.textStorage?.setAttributedString(drawn)
        // Selectable, not editable: a reader who wants to quote a note should be
        // able to take the words, and ⌘C comes with the selection for free.
        body.isEditable = false
        body.isSelectable = true
        body.drawsBackground = false
        body.textContainerInset = .zero
        body.textContainer?.lineFragmentPadding = 0
        body.textContainer?.widthTracksTextView = true
        body.textContainer?.size = NSSize(width: width, height: .greatestFiniteMagnitude)
        // A note is prose and can be a paragraph; past a few lines the popover
        // stops being a glance and the reader should go to the note instead.
        body.textContainer?.maximumNumberOfLines = Self.maxLines
        body.textContainer?.lineBreakMode = .byWordWrapping

        let container = FootnotePeekContainer()
        container.presenter = self
        body.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(body)
        NSLayoutConstraint.activate([
            body.topAnchor.constraint(equalTo: container.topAnchor, constant: Self.insets.top),
            body.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -Self.insets.bottom),
            body.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: Self.insets.left),
            body.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -Self.insets.right),
            body.widthAnchor.constraint(equalToConstant: width),
        ])

        // Capped to the same line count the container truncates at, so a long
        // note leaves no empty band under its last visible line.
        let measured = Self.measure(content.body, wrappingAt: width)
        let lineHeight = max(1, Self.measure(content.body, wrappingAt: .greatestFiniteMagnitude).height)
        let bodyHeight = min(ceil(measured.height), ceil(lineHeight) * CGFloat(Self.maxLines))

        let vc = NSViewController()
        vc.view = container
        vc.preferredContentSize = NSSize(width: width + Self.insets.left + Self.insets.right,
                                         height: bodyHeight + Self.insets.top + Self.insets.bottom)
        return vc
    }

    fileprivate func pointer(isInside inside: Bool) {
        delegate?.footnotePeek(self, pointerIsInside: inside)
    }

    fileprivate func follow(_ target: FootnotePeekTarget) {
        delegate?.footnotePeek(self, didFollow: target)
    }

    /// How much room `text` needs — on one line when `wrappingAt` is nil, wrapped
    /// to that width otherwise. Measured from the attributed string itself, so
    /// the note's own fonts (a heading-sized run, inline code) are accounted for.
    private static func measure(_ text: NSAttributedString, wrappingAt width: CGFloat? = nil) -> CGSize {
        text.boundingRect(
            with: NSSize(width: width ?? .greatestFiniteMagnitude, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading]
        ).size
    }

    /// Wide enough for a sentence or two, narrow enough to read in one fixation
    /// and to sit beside the reference rather than over the whole column.
    private static let maxWidth: CGFloat = 320
    /// So a one-word note is still a popover rather than a chip.
    private static let minWidth: CGFloat = 140
    private static let maxLines = 8
    private static let insets = NSEdgeInsets(top: 10, left: 12, bottom: 10, right: 12)
}

/// The popover's content view, which exists only to notice the pointer arriving
/// and leaving.
///
/// The text view inside it can't answer this on its own: it is inset from the
/// edges, so the margin between its text and the popover's border would read as
/// "left" and take the peek down while the pointer is still plainly over it.
private final class FootnotePeekContainer: NSView {
    /// Assigned after construction rather than taken as an init parameter, for
    /// the reason `FootnotePeekTextView` spells out: an AppKit view has to keep
    /// its superclass's designated initializers, and one that adds a required
    /// parameter cannot.
    weak var presenter: FootnotePeekPresenter?
    private var tracking: NSTrackingArea?

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        // `.activeAlways`, not `.activeInKeyWindow`: a popover's window is not
        // the key window, so the usual option would never fire here at all.
        let area = NSTrackingArea(rect: .zero,
                                  options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area)
        tracking = area
    }

    override func mouseEntered(with event: NSEvent) { presenter?.pointer(isInside: true) }
    override func mouseExited(with event: NSEvent) { presenter?.pointer(isInside: false) }
}

/// The note itself: selectable text that follows what it can.
private final class FootnotePeekTextView: NSTextView {
    /// Assigned after construction, not taken as an init parameter.
    ///
    /// `NSTextView`'s designated initializer is `init(frame:textContainer:)`, and
    /// AppKit calls it from inside `init(frame:)` — so a subclass whose only
    /// initializer takes a presenter doesn't *hide* that one, it inherits a
    /// trapping stub for it, and the text view crashes the first time the
    /// popover is built. Keeping the superclass's initializers and setting this
    /// afterwards is the shape that survives contact with AppKit.
    weak var presenter: FootnotePeekPresenter?

    /// A click on a followable run follows it; anything else is an ordinary
    /// click in selectable text, which is what starts a selection.
    ///
    /// Handled here rather than through `clickedOnLink:` because half of these
    /// targets are document offsets rather than URLs, and pretending a footnote
    /// reference is a `file:` URL to get the delegate call would be a lie the
    /// rest of the code would have to keep.
    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let target = target(at: point) {
            presenter?.follow(target)
            return
        }
        super.mouseDown(with: event)
    }

    private func target(at point: CGPoint) -> FootnotePeekTarget? {
        guard let layoutManager, let textContainer, let storage = textStorage, storage.length > 0
        else { return nil }
        // Fraction-checked, because `characterIndex(for:)` answers with the
        // nearest character for a point past the end of a line — which would make
        // the blank space to the right of a link click the link.
        var fraction: CGFloat = 0
        let index = layoutManager.characterIndex(for: point, in: textContainer,
                                                 fractionOfDistanceBetweenInsertionPoints: &fraction)
        guard index < storage.length else { return nil }
        let glyph = layoutManager.glyphIndexForCharacter(at: index)
        let rect = layoutManager.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1),
                                              in: textContainer)
        guard rect.contains(point) else { return nil }
        return storage.attribute(.footnoteTarget, at: index, effectiveRange: nil) as? FootnotePeekTarget
    }
}
#endif

#if canImport(UIKit)
import UIKit

/// Presents (and owns) the peek popover on iOS.
///
/// A popover rather than the edit menu, and a long press rather than a tap,
/// because the phone has neither a pointer to hover nor a ⌘ to hold: the long
/// press is its whole vocabulary for "tell me about this without changing it".
/// The jump is a button inside the popover for the same reason — there is
/// nowhere else to put it that a finger can reach without dismissing this.
final class FootnotePeekPresenter: NSObject, UIPopoverPresentationControllerDelegate {
    private weak var presented: UIViewController?

    var isShowing: Bool { presented?.presentingViewController != nil }

    /// Raise a popover for `content` over `rect` in `view`, presented from
    /// `parent`. `onFollow`, when given, becomes a button that dismisses the
    /// popover and jumps — nil when there is nowhere to jump to (an undefined
    /// reference), so no button ever leads nowhere.
    ///
    /// `onTarget` answers a tap on something followable *inside* the note — a
    /// link, or a reference to another footnote. Its AppKit peer is a delegate
    /// callback; here a closure suffices, because a presented controller already
    /// has an owner and needs no second one.
    func show(_ content: FootnotePeekContent,
              from rect: CGRect,
              in view: UIView,
              presentedBy parent: UIViewController,
              onFollow: (() -> Void)?,
              onTarget: @escaping (FootnotePeekTarget) -> Void) {
        hide()
        let vc = FootnotePeekController(content: content, onFollow: onFollow, onTarget: onTarget)
        vc.modalPresentationStyle = .popover
        guard let popover = vc.popoverPresentationController else { return }
        popover.sourceView = view
        popover.sourceRect = rect
        popover.permittedArrowDirections = [.up, .down]
        // Without this the popover adapts to a full-screen sheet on iPhone,
        // which is the opposite of a glance: the reader loses sight of the
        // sentence the footnote belongs to, which is the whole context.
        popover.delegate = self
        parent.present(vc, animated: true)
        presented = vc
    }

    func hide() {
        presented?.presentingViewController?.dismiss(animated: false)
        presented = nil
    }

    func adaptivePresentationStyle(
        for controller: UIPresentationController,
        traitCollection: UITraitCollection
    ) -> UIModalPresentationStyle { .none }
}

/// The popover's contents: which note, what it says, and the way to it.
private final class FootnotePeekController: UIViewController {
    private let content: FootnotePeekContent
    private let onFollow: (() -> Void)?
    private let onTarget: (FootnotePeekTarget) -> Void

    init(content: FootnotePeekContent,
         onFollow: (() -> Void)?,
         onTarget: @escaping (FootnotePeekTarget) -> Void) {
        self.content = content
        self.onFollow = onFollow
        self.onTarget = onTarget
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func viewDidLoad() {
        super.viewDidLoad()

        let drawn = NSMutableAttributedString(attributedString: content.body)
        // Already rendered by `AttributedRow`, in the document's own fonts and
        // colours. Only leaf's own sentence (no note defined) carries no runs and
        // needs dressing, and it gets the secondary colour to say it is chrome.
        if !content.isDefined {
            drawn.addAttributes(
                [.font: UIFont.preferredFont(forTextStyle: .body),
                 .foregroundColor: UIColor.secondaryLabel],
                range: NSRange(location: 0, length: drawn.length))
        }
        // Underline what leads somewhere — on a phone there is not even a cursor
        // to change shape, so this is the only cue that a word is a door.
        drawn.enumerateAttribute(.footnoteTarget, in: NSRange(location: 0, length: drawn.length)) { value, range, _ in
            guard value != nil else { return }
            drawn.addAttribute(.underlineStyle, value: NSUnderlineStyle.single.rawValue, range: range)
        }

        // A text view rather than a label: it selects, and selection is how a
        // reader takes a quote out of a note. Scrolling is off — the popover is
        // sized to the note, and a note long enough to need scrolling is one the
        // reader should visit rather than squint at.
        let body = UITextView()
        body.attributedText = drawn
        body.isEditable = false
        body.isSelectable = true
        body.isScrollEnabled = false
        body.backgroundColor = .clear
        body.textContainerInset = .zero
        body.textContainer.lineFragmentPadding = 0
        body.textContainer.maximumNumberOfLines = 8
        body.textContainer.lineBreakMode = .byWordWrapping
        let tap = UITapGestureRecognizer(target: self, action: #selector(bodyTapped))
        // Alongside the text view's own recognisers, so a tap that lands on no
        // target still places a selection the way a tap in text should.
        tap.cancelsTouchesInView = false
        body.addGestureRecognizer(tap)
        self.body = body

        var views: [UIView] = [body]
        if onFollow != nil {
            let follow = UIButton(type: .system)
            follow.setTitle(loc("menu.goToNote", "Go to Note"), for: .normal)
            follow.titleLabel?.font = .preferredFont(forTextStyle: .callout)
            follow.contentHorizontalAlignment = .leading
            follow.addTarget(self, action: #selector(followTapped), for: .touchUpInside)
            views.append(follow)
        }

        let stack = UIStackView(arrangedSubviews: views)
        stack.axis = .vertical
        stack.alignment = .leading
        stack.spacing = 4
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)

        let margin: CGFloat = 14
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: view.topAnchor, constant: margin),
            stack.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -margin),
            stack.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: margin),
            stack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -margin),
            stack.widthAnchor.constraint(lessThanOrEqualToConstant: 300),
        ])

        // Lay out once so the popover is sized to the note rather than to a
        // default it would then have to animate away from.
        view.layoutIfNeeded()
        preferredContentSize = CGSize(
            width: min(stack.systemLayoutSizeFitting(UIView.layoutFittingCompressedSize).width, 300) + margin * 2,
            height: stack.systemLayoutSizeFitting(UIView.layoutFittingCompressedSize).height + margin * 2)
    }

    @objc private func followTapped() {
        let follow = onFollow
        presentingViewController?.dismiss(animated: true) { follow?() }
    }

    /// A tap on a link or a nested reference follows it; a tap anywhere else is
    /// the text view's, and places a selection.
    @objc private func bodyTapped(_ gesture: UITapGestureRecognizer) {
        guard let body, let target = body.footnoteTarget(at: gesture.location(in: body)) else { return }
        let handle = onTarget
        presentingViewController?.dismiss(animated: true) { handle(target) }
    }

    private var body: UITextView?
}

private extension UITextView {
    /// The peek target under `point`, or nil for ordinary prose.
    ///
    /// Bounds-checked rather than trusting `characterIndex(for:)`, which answers
    /// with the nearest character for a point past the end of a line — so the
    /// blank space to the right of a link would otherwise be the link.
    func footnoteTarget(at point: CGPoint) -> FootnotePeekTarget? {
        guard let storage = textStorage as NSTextStorage?, storage.length > 0 else { return nil }
        var fraction: CGFloat = 0
        let index = layoutManager.characterIndex(for: point, in: textContainer,
                                                 fractionOfDistanceBetweenInsertionPoints: &fraction)
        guard index < storage.length else { return nil }
        let glyph = layoutManager.glyphIndexForCharacter(at: index)
        let rect = layoutManager.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1),
                                              in: textContainer)
        guard rect.contains(point) else { return nil }
        return storage.attribute(.footnoteTarget, at: index, effectiveRange: nil) as? FootnotePeekTarget
    }
}
#endif
