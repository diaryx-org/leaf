//  LeafTextView.swift  (AppKit / macOS)
//
//  The macOS editing surface — the peer of leaf-wasm's `LeafEditor`. It owns
//  presentation and input, never the model; `LeafDoc` (leaf-core, over the FFI)
//  stays the single source of truth. Each already-wrapped `Row` is drawn directly
//  (no NSTextView/NSLayoutManager), the caret is placed at `caret_ch`, and every
//  key/mouse intent routes back into core, which edits and returns the next frame.
//  Shared geometry lives in `EditorLayout`; the UIKit peer is `LeafTextView` in
//  `LeafTextViewiOS.swift`.
//
//  ## Native selection on AppKit
//
//  AppKit has no analogue to iOS's `UITextInteraction` — nothing lets the system
//  draw or own a selection over custom-laid-out text, so (like Xcode's own editor)
//  this view paints the selection itself. What makes it *native* is that the OS is
//  told the truth about it: `NSTextInputClient` reports the real `selectedRange` and
//  answers `attributedSubstring`/`firstRect`/`characterIndex`, the view is an
//  `NSServicesMenuRequestor`, and it exposes an `NSAccessibility` text area. So Look
//  Up, the Services menu, dictation, the right-click menu, VoiceOver, and the
//  emphasized/unemphasized (key-window-aware) highlight all behave natively — the
//  same experience the iOS peer gets from `UITextInput`, reached a different way.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import LeafFFI

public final class LeafTextView: NSView, NSTextInputClient, NSServicesMenuRequestor, FootnotePeekDelegate {
    let doc: LeafDoc
    public var theme: EditorTheme {
        didSet {
            // Re-wrap only when the geometry changed; a colour-only (or identical)
            // theme just repaints. Guarding this breaks the relayout⇄state-publish
            // loop that otherwise re-scrolled the view to the caret every frame.
            guard theme.metricsDiffer(from: oldValue) else { needsDisplay = true; return }
            shapeCache.removeAll(keepingCapacity: true)   // shaping is theme-dependent
            relayoutForWidth(force: true)
        }
    }

    /// The sheet this document is laid onto, or `nil` (the default) for the
    /// continuous scrolling flow the editor has always had.
    ///
    /// Setting one is a *mode*, not a style: rows break across a stack of pages
    /// and wrap to the sheet's margins instead of the theme's `measure`, which a
    /// page supersedes. See `PageSetup`.
    public var pageSetup: PageSetup? {
        didSet {
            guard pageSetup != oldValue else { return }
            // The column width changed, and the shape cache is only valid at the
            // width it was built for.
            shapeCache.removeAll(keepingCapacity: true)
            // A paginated document is a fixed width the scroll view may have to
            // scroll sideways to, so it stops tracking the viewport's.
            autoresizingMask = pageSetup == nil ? [.width] : []
            relayoutForWidth(force: true)
        }
    }

    /// The on-screen scale, `1` being one layout point per screen point.
    ///
    /// A view-level transform and nothing more: `EditorLayout` always works in
    /// unzoomed page space, `draw` scales the context, and points coming the other
    /// way (clicks, drags, the IME's screen rects) are divided back out. So a drag
    /// of a zoom slider re-*draws* but never re-shapes — the row cache, which is
    /// keyed by wrap width, survives untouched — and the text stays vector-crisp
    /// at every stop, which is what scaling a rasterized layer would cost.
    public var zoom: CGFloat {
        get { zoomScale }
        set {
            let clamped = min(max(newValue, Self.zoomRange.lowerBound), Self.zoomRange.upperBound)
            guard clamped != zoomScale else { return }
            zoomScale = clamped
            // No re-shaping: the wrap width is the sheet's (or the theme's) and
            // neither moved. This re-runs the layout only because the viewport
            // measured in layout points just changed, which is what decides where
            // the stack centres.
            relayoutForWidth(force: true)
            needsDisplay = true
        }
    }
    private var zoomScale: CGFloat = 1

    /// The scales the view will hold — 25% to 400%, the range a word processor's
    /// zoom control usually offers.
    public static let zoomRange: ClosedRange<CGFloat> = 0.25...4

    /// A point in view coordinates in layout (page) space — the inverse of the
    /// scale `draw` applies. The identity at `zoom == 1`.
    private func layoutPoint(_ p: CGPoint) -> CGPoint { CGPoint(x: p.x / zoom, y: p.y / zoom) }

    /// A rect in layout space back out in view coordinates.
    private func viewRect(_ r: CGRect) -> CGRect {
        r.applying(CGAffineTransform(scaleX: zoom, y: zoom))
    }

    /// A rect in view coordinates back in layout space — how a dirty band becomes
    /// something the row and page loops can cull against.
    private func layoutRect(_ r: CGRect) -> CGRect {
        r.applying(CGAffineTransform(scaleX: 1 / zoom, y: 1 / zoom))
    }
    /// Fired after every repaint so a host can update a toolbar/footer.
    public var onStateChange: ((EditorState) -> Void)?
    /// Host hook for link activation. Called with the link's raw destination
    /// before the view falls back to opening it with the system; return `true`
    /// to claim it. This is how a host resolves destinations only *it* can make
    /// sense of — a note app's `./sibling.md` or `id:6tzwsxg` means a document
    /// in its own workspace, not a file: URL, and handing either to
    /// `NSWorkspace` is at best wrong and at worst opens the raw markdown in
    /// another app. Nil (or a `false` return) keeps the system behaviour.
    public var onOpenLink: ((String) -> Bool)?

    /// Asked to edit the destination of the link under the caret, with its
    /// current destination to seed a field with. See `LeafEditorModel.onEditLink`.
    public var onEditLink: ((String) -> Void)?

    /// Asked what a link points at, so a hover can show it. See
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
    /// With `mediaPlayback == .inline` (the default) a click installs an
    /// `AVPlayerView` over the box and this is never called, *except* for a
    /// source the editor's own loader can't resolve to a local file — a remote
    /// URL, which a host is better placed to handle since it can fetch
    /// asynchronously and this loader cannot. With `.host` it is called for every
    /// activation. Nil in either case leaves a click doing nothing but placing
    /// the caret.
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
    /// The source the reader activated while the host was still resolving it,
    /// so the answer can start playback rather than land silently.
    private var pendingMediaActivation: String?

    private var docView: DocView
    private var layoutEngine: EditorLayout
    /// The view width the current layout was built for. The text column inside it
    /// — where it starts, how wide it wraps — is the theme's to decide (see
    /// `EditorTheme.column(in:)`), and the layout carries the answer.
    private var viewWidth: CGFloat = 0
    /// Per-row shaped-text cache reused across frames; an edit re-shapes only the
    /// changed row(s). Cleared when the theme geometry changes (see `theme`).
    private var shapeCache: [Row: ShapedRow] = [:]
    /// The pixel x that ↑/↓ aim for, so repeated vertical motion rides the visual
    /// wrap without drifting through shorter lines. Nil except mid vertical run.
    private var verticalGoalX: CGFloat?
    /// The byte range of the in-flight IME composition (marked text), drawn with a
    /// composing underline. Nil when not composing. Committed text clears it.
    private var markedByteRange: NSRange?

    private var caretVisible = true
    private var blinkTimer: Timer?
    private var isFocused = false
    /// The caret offset the view last scrolled to reveal. Only a *move* re-scrolls,
    /// so passive reflows (width/theme relayout, state refreshes) leave the reader's
    /// scroll position alone instead of yanking it back to the caret.
    private var lastCaretOffset: UInt32?

    public init(doc: LeafDoc, theme: EditorTheme = .default) {
        self.doc = doc
        self.theme = theme
        // Switch core to unwrapped layout (one row per block); the view soft-wraps
        // each row at its own pixel width.
        let first = doc.setUnwrapped()
        self.docView = first
        var seed: [Row: ShapedRow] = [:]
        self.layoutEngine = EditorLayout(first, theme: theme, viewWidth: 0, cache: &seed)
        self.shapeCache = seed
        super.init(frame: .zero)
        autoresizingMask = [.width]
        // A resolved source has a picture to draw, and may be the one the reader
        // tapped while it was still being fetched.
        mediaStore.onLoaded = { [weak self] src in
            guard let self else { return }
            self.needsDisplay = true
            self.playIfAwaited(src)
        }
        // Seed with the initial caret so the first reflow opens at the top rather
        // than scrolling to wherever the caret happens to start.
        lastCaretOffset = doc.caretOffset()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    public override var isFlipped: Bool { true }   // origin top-left → rows top-down
    public override var acceptsFirstResponder: Bool { true }
    public override var isOpaque: Bool { false }

    // MARK: layout / wrap

    public override func layout() {
        super.layout()
        relayoutForWidth(force: false)
        applyContentSize()
    }

    /// The width the layout is asked to fill, in unzoomed points: the *viewport's*,
    /// not this view's own.
    ///
    /// They are the same thing in the continuous flow, where the document view
    /// tracks the clip view's width. Paginated they part company — the frame grows
    /// to the stack's width so a sheet wider than the window can be scrolled to —
    /// and it is the viewport the stack should centre in, since that is what the
    /// reader is looking through.
    private var layoutWidth: CGFloat {
        (enclosingScrollView?.contentView.bounds.width ?? bounds.width) / zoom
    }

    private func relayoutForWidth(force: Bool) {
        let w = layoutWidth
        guard w > theme.padding.left + theme.padding.right else { return }
        if force || abs(w - viewWidth) > 0.5 {
            viewWidth = w
            // Re-wrap the current frame at the new pixel width — the unwrapped map is
            // width-independent, so no round trip to core is needed.
            render(docView, keepVerticalGoal: true)
        }
    }

    /// Keep the view at least as tall as the enclosing scroll view's visible clip
    /// area, never just the text's own height. `NSScrollView` only routes clicks to
    /// a view under the cursor; a view sized tightly to short/empty content left the
    /// rest of the visible editor pane unclickable — no caret, no focus, typing
    /// impossible. `EditorLayout.hit` already clamps a point below the last row to
    /// it, so filling the clip area just makes that reachable: clicking below the
    /// text lands the caret at the document's end, same as most text editors.
    private func applyContentSize() {
        let clip = enclosingScrollView?.contentView.bounds.size ?? bounds.size
        let raw = layoutEngine.contentHeight * zoom
        // Once the document already needs to scroll, pad another half screen below
        // the last line, so a long entry can be pulled up to a comfortable reading
        // height instead of staying glued to the bottom edge. Content that already
        // fits the clip area (the common short-document case) gets no extra room —
        // `raw > clip.height` is false — so nothing here makes a short document
        // scrollable.
        //
        // Not in the paginated flow. There the document's height is the stack's,
        // and a sheet of blank paper below the last page is already the room this
        // was reaching for — half a screen more would just be backdrop, and would
        // let the reader scroll a whole page past the end of their document.
        let extra = pageSetup == nil && raw > clip.height ? clip.height * 0.5 : 0
        let h = max(raw + extra, clip.height)
        // Continuously the layout has no width of its own: it fills the viewport
        // (and `autoresizingMask` keeps it there). A stack of sheets is a fixed
        // width, so a window narrower than one scrolls sideways to it.
        let w = max(layoutEngine.contentWidth * zoom, clip.width)
        if abs(frame.height - h) > 0.5 || abs(frame.width - w) > 0.5 {
            setFrameSize(NSSize(width: w, height: h))
        }
    }

    // MARK: applying a frame

    private func render(_ view: DocView, keepVerticalGoal: Bool = false) {
        if !keepVerticalGoal { verticalGoalX = nil }
        // Whatever the peek was pointing at has just moved, changed or gone: an
        // edit reflowed the line under it, or a relayout put the reference
        // somewhere else. It is chrome about a position, and the positions are
        // being rebuilt.
        dismissFootnotePeek()
        docView = view
        layoutEngine = EditorLayout(view, theme: theme, viewWidth: viewWidth, page: pageSetup,
                                    cache: &shapeCache, media: mediaStore)
        // Installed players follow their boxes: the layout just moved every one
        // of them, and any media edited out of the document is gone from the
        // rects, which is what stops its playback. Skipped entirely when nothing
        // is installed, which is the overwhelmingly common case.
        if !mediaPlayers.isEmpty {
            mediaPlayers.reposition(layoutEngine.mediaRects())
        }
        applyContentSize()
        needsDisplay = true
        resetBlink()
        // Only follow the caret when it actually moved (typing, motion, click), not
        // on a passive reflow — otherwise every relayout snaps the reader back.
        let caret = doc.caretOffset()
        if caret != lastCaretOffset {
            lastCaretOffset = caret
            scrollCaretToVisible()
        }
        onStateChange?(EditorState(view: view.view, dirty: view.dirty, heading: view.heading, active: view.active))
    }

    // MARK: drawing

    public override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        // Everything below this line is in layout coordinates. Scaling the context
        // once — rather than multiplying every rect on the way out — is what keeps
        // zoom out of the geometry: `EditorLayout` never learns it exists, and the
        // text is re-rendered at the new scale rather than resampled, so it stays
        // crisp. `dirtyRect` comes in already scaled, so the culling band is it
        // divided back out.
        ctx.saveGState()
        defer { ctx.restoreGState() }
        ctx.scaleBy(x: zoom, y: zoom)
        let band = layoutRect(dirtyRect)

        let active = selectionIsActive
        let selColor = active ? theme.selectionColor : theme.inactiveSelectionColor

        // The paper first: every other thing here paints onto a sheet.
        PageChrome.draw(layoutEngine.pages, theme: theme, clip: band, in: ctx)
        // Then the landing flash, under every other mark: it is a light behind
        // the words, not something drawn over them.
        drawLandingFlash(in: ctx)
        drawDirectiveBorders(in: ctx, dirtyRect: band)
        // The quote bars are one pass over the frame (a run of quoted rows merges
        // into a single bar), so they're painted before the rows, like the
        // directive outlines — the text then draws beside them.
        BlockChrome.drawQuoteBars(layoutEngine.rows, theme: theme, in: ctx)

        for rl in layoutEngine.rows {
            // Cull to the dirty band, so a scroll or a caret blink repaints only
            // what's on screen. Skipped rather than stopped at the first row past
            // the band: rows run top-down only while a sheet has one column, and a
            // second one starts back at the top of the same sheet.
            if rl.top >= band.maxY || rl.top + rl.height <= band.minY { continue }
            // A table draws its own grid (once, on its first picture row); its
            // rows carry no text to paint.
            if let grid = rl.table {
                if rl.tableFirst { drawTable(grid, tableTop: rl.tableTop, selColor: selColor, in: ctx) }
                continue
            }
            // A media box likewise draws once, on its first placeholder row, in
            // place of core's `🖼 alt` glyphs. Inset by the row's own prefix, so a
            // picture inside a quote or a list sits beside its gutter rather than
            // under it.
            if let box = rl.media {
                if rl.mediaFirst {
                    BlockChrome.drawMedia(box,
                                          at: box.rect(top: rl.mediaTop,
                                                       left: rl.originX + rl.shaped.prefixWidth),
                                          theme: theme,
                                          playing: mediaPlayers.isPlaying(box.media.src), in: ctx)
                }
                continue
            }
            // The row's bands, not one rect over its whole height: a split row has
            // a sheet edge — or a column gutter — through the middle of it, and a
            // code fill drawn over that would tile the backdrop or the gutter too.
            // Each band carries its own column, so this is where the x comes from.
            let bands = rl.bands
            let rowRect = bands.first
                ?? CGRect(x: rl.originX, y: rl.top, width: rl.columnWidth, height: rl.height)
            if rl.row.directive, let label = rl.row.directiveLabel, !label.isEmpty {
                drawDirectiveLabel(label, in: rowRect)
            }
            if rl.row.code {
                ctx.setFillColor(theme.codeBackground.cgColor)
                for b in bands { ctx.fill(b.insetBy(dx: -4, dy: 0)) }
                if let lang = rl.row.codeLang, !lang.isEmpty { drawCodeLang(lang, in: rowRect) }
            }
            BlockChrome.drawRule(rl, theme: theme, selColor: selColor, in: ctx)
            layoutEngine.fillSelection(row: rl, color: selColor, in: ctx)
            // Draw each wrapped visual line's substring on its own line box, hung
            // at the row's indent (zero on the first line, the prefix width after).
            for (i, wl) in rl.wrapped.enumerated() {
                // `continue`, not `break`: a row's lines run down one column and
                // then back up to the top of the next, so passing the dirty band
                // once says nothing about the lines after it.
                let o = rl.lineOrigin(i)
                if o.y >= band.maxY || o.y + rl.lineHeight <= band.minY { continue }
                wl.attributed.draw(with: CGRect(x: o.x + wl.indent, y: o.y,
                                                width: rl.columnWidth - wl.indent,
                                                height: rl.lineHeight),
                                   options: [.usesLineFragmentOrigin])
            }
        }

        if markedByteRange != nil { drawMarkedUnderline(in: ctx) }

        if active, caretVisible, let rect = layoutEngine.caretRect(docView, theme: theme) {
            ctx.setFillColor(theme.caretColor.cgColor)
            ctx.fill(rect)
        }
    }

    /// Answer a click on a block media box, returning whether it was handled (so
    /// the caller can fall through to plain caret placement if not).
    ///
    /// In `.inline` mode this installs an AVKit player over the box and starts
    /// it; a second click on a playing one pauses. `.host` mode, and any source
    /// this loader can't resolve to a local file, fall through to `onOpenMedia`
    /// — a remote URL is exactly the case a host is better placed to handle,
    /// since it can fetch asynchronously and this cannot.
    private func activateMedia(_ media: MediaView) -> Bool {
        // An image plays nothing, so it is worth activating only when its box is
        // empty and the host might yet fill it — a source the host declined, or
        // one whose bytes aren't on this device. A picture that loaded is just
        // text to click into, and swallowing that click would be a bug.
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
                    needsDisplay = true   // the badge under the player must stop drawing
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
    /// Called from the store's `onLoaded`; a no-op unless this is the source
    /// they were waiting on.
    private func playIfAwaited(_ src: String) {
        guard pendingMediaActivation == src else { return }
        pendingMediaActivation = nil
        guard let url = mediaStore.playableURL(for: src),
              let info = layoutEngine.rows.compactMap(\.media).first(where: { $0.media.src == src })
        else { return }
        if let rect = layoutEngine.mediaRects()[src] {
            mediaPlayers.activate(info.media, at: rect, in: self, url: url)
            needsDisplay = true
        }
    }

    /// Draw a table as a proportional grid — header fill and body stripes, the
    /// cell text, then the grid rules over them — the Apple peer of leaf-gpui's
    /// `table_chrome`. `tableTop` is the grid's top in view coordinates.
    private func drawTable(_ grid: TableLayout, tableTop: CGFloat, selColor: LeafColor, in ctx: CGContext) {
        let left = layoutEngine.originX
        let border = TableMetrics.border
        let x0 = left + (grid.colX.first ?? 0)
        let x1 = left + (grid.colX.last ?? 0)

        // Fills under the text: the header rows, then every other body row.
        var body = 0
        for row in grid.rows {
            let bg: LeafColor?
            if row.head {
                bg = theme.tableHeaderColor
            } else {
                body += 1
                bg = body % 2 == 0 ? theme.tableStripeColor : nil // first body row clear
            }
            if let bg {
                ctx.setFillColor(bg.cgColor)
                ctx.fill(CGRect(x: x0, y: tableTop + row.top, width: x1 - x0, height: row.height))
            }
        }

        // Selection highlight, behind the cell text — the table peer of the row
        // path's `fillSelection`. One rect per selected span, clipped to its cell
        // line; the row backgrounds above it, the text below, exactly as a plain
        // row layers them.
        ctx.setFillColor(selColor.cgColor)
        for row in grid.rows {
            let selTop = tableTop + row.top + TableMetrics.padY
            for cell in row.cells {
                for (i, line) in cell.lines.enumerated() where !line.selRanges.isEmpty {
                    let y = selTop + CGFloat(i) * grid.lineHeight
                    for (s, e) in line.selRanges {
                        let sx = CTLineGetOffsetForStringIndex(line.line, CFIndex(s), nil)
                        let ex = CTLineGetOffsetForStringIndex(line.line, CFIndex(e), nil)
                        ctx.fill(CGRect(x: left + line.textX + sx, y: y,
                                        width: ex - sx, height: grid.lineHeight))
                    }
                }
            }
        }

        // Cell text — each cell line on its own row within the cell's band.
        for row in grid.rows {
            let top = tableTop + row.top + TableMetrics.padY
            for cell in row.cells {
                for (i, line) in cell.lines.enumerated() {
                    line.attributed.draw(
                        with: CGRect(x: left + line.textX,
                                     y: top + CGFloat(i) * grid.lineHeight,
                                     width: .greatestFiniteMagnitude, height: theme.lineHeight),
                        options: [.usesLineFragmentOrigin])
                }
            }
        }

        // Grid rules over the fills and text.
        ctx.setFillColor(theme.tableBorderColor.cgColor)
        for bx in grid.colX { // verticals, outer two included
            ctx.fill(CGRect(x: left + bx, y: tableTop, width: border, height: grid.height))
        }
        var edgeYs = [tableTop] // horizontals: top, each row boundary, bottom
        for row in grid.rows { edgeYs.append(tableTop + row.top + row.height) }
        for ey in edgeYs {
            ctx.fill(CGRect(x: x0, y: min(ey, tableTop + grid.height - border),
                            width: x1 - x0 + border, height: border))
        }
    }

    /// Underline the in-flight IME composition, one segment per visual line — the
    /// native "you're still composing this" affordance.
    private func drawMarkedUnderline(in ctx: CGContext) {
        guard let m = markedByteRange, m.length > 0 else { return }
        let s = doc.posForOffset(off: UInt32(m.location))
        let e = doc.posForOffset(off: UInt32(m.location + m.length))
        ctx.setFillColor(theme.caretColor.cgColor)
        for row in Int(s.row)...Int(e.row) where layoutEngine.rows.indices.contains(row) {
            let rl = layoutEngine.rows[row]
            let rowFrom = (row == Int(s.row)) ? Int(s.ch) : 0
            let rowTo = (row == Int(e.row)) ? Int(e.ch) : rl.attributed.length
            for (i, wl) in rl.wrapped.enumerated() {
                let lineStart = wl.start, lineEnd = wl.start + wl.length
                let cs = max(rowFrom, lineStart), ce = min(rowTo, lineEnd)
                guard cs < ce else { continue }
                let x0 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(cs - lineStart), nil)
                let x1 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(ce - lineStart), nil)
                let o = rl.lineOrigin(i)
                ctx.fill(CGRect(x: o.x + wl.indent + x0, y: o.y + rl.lineHeight - 1.5,
                                width: x1 - x0, height: 1))
            }
        }
    }

    /// Whether this view owns the text focus right now: first responder **and** in the
    /// key window. Drives the emphasized-vs-unemphasized selection fill and whether the
    /// caret shows — matching a native `NSTextView`, which greys its selection and hides
    /// its caret the moment its window stops being key.
    private var selectionIsActive: Bool { isFocused && (window?.isKeyWindow ?? false) }

    private func drawCodeLang(_ lang: String, in rowRect: CGRect) {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: theme.monospaceFont(size: theme.fontSize * 0.75, bold: false, italic: false),
            .foregroundColor: theme.secondaryColor,
        ]
        let s = lang as NSString
        let size = s.size(withAttributes: attrs)
        s.draw(at: CGPoint(x: rowRect.maxX - size.width - 2, y: rowRect.minY + 1), withAttributes: attrs)
    }

    /// A directive container's `.class` label, top-left of its first row — the
    /// mirror of `drawCodeLang`'s top-right fence-language label. Opposite
    /// corners so a directive wrapping a code block never collides the two.
    private func drawDirectiveLabel(_ label: String, in rowRect: CGRect) {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: theme.proportionalFont(size: theme.fontSize * 0.75, bold: false, italic: false),
            .foregroundColor: theme.secondaryColor,
        ]
        (label as NSString).draw(at: CGPoint(x: rowRect.minX + 2, y: rowRect.minY + 1), withAttributes: attrs)
    }

    /// One dashed outline per maximal run of consecutive `directive` rows — a
    /// directive block reads as a single bordered aside, not one filled band per
    /// row. Drawn before the row loop so the label/text paint over it, not the
    /// reverse. Skips a table row (it has no drawable rect of its own here; a
    /// directive-wrapped table isn't chromed today).
    private func drawDirectiveBorders(in ctx: CGContext, dirtyRect: NSRect) {
        let rows = layoutEngine.rows
        var i = 0
        while i < rows.count {
            guard rows[i].row.directive, rows[i].table == nil else { i += 1; continue }
            let start = i
            while i < rows.count, rows[i].row.directive, rows[i].table == nil { i += 1 }
            // The run's rows reduced to their vertical bands, merged where they
            // touch. Continuously that always collapses back to the single box
            // this drew before. Paginated, a run crossing a sheet edge — between
            // two of its rows, or through the middle of one of them — comes out as
            // one box per sheet, so no outline is ever stroked across the backdrop.
            var spans: [CGRect] = []
            for rl in rows[start..<i] {
                for b in rl.bands {
                    if let last = spans.last, abs(last.maxY - b.minY) < 0.5,
                       abs(last.minX - b.minX) < 0.5 {
                        spans[spans.count - 1].size.height += b.height
                    } else {
                        spans.append(b)
                    }
                }
            }
            for span in spans {
                let rect = span.insetBy(dx: -4, dy: 0)
                if rect.maxY < dirtyRect.minY || rect.minY > dirtyRect.maxY { continue }
                ctx.saveGState()
                ctx.setStrokeColor(theme.directiveBorderColor.cgColor)
                ctx.setLineWidth(1)
                ctx.setLineDash(phase: 0, lengths: [3, 3])
                ctx.addPath(CGPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5),
                                   cornerWidth: 6, cornerHeight: 6, transform: nil))
                ctx.strokePath()
                ctx.restoreGState()
            }
        }
    }

    /// Put the caret at `offset` and land the reader on it — how a host arrives
    /// at the place a `#v2` names.
    ///
    /// `through` bounds the block that was named, and gets it flashed. Passing it
    /// is what makes an arrival legible: without it the reader is somewhere new
    /// with nothing saying which words they were sent to.
    ///
    /// A landing rather than the caret-following in `render`, which scrolls the
    /// least it can and only when the caret *moved* — right for typing, wrong for
    /// arriving. See `Landing`.
    public func reveal(offset: UInt32, through end: UInt32? = nil) {
        command { $0.caretMoved(to: offset) }
        lastCaretOffset = offset
        land()
        guard let end, end > offset else { return }
        flash(from: offset, to: end)
    }

    /// Scroll so the caret's block sits a fixed distance below the top of the
    /// viewport. Falls back to the ordinary minimum scroll when there is no clip
    /// view to measure against (a text view not in a scroll view at all).
    private func land() {
        guard let clip = enclosingScrollView?.contentView,
              let rect = layoutEngine.caretRect(docView, theme: theme)
        else { return scrollCaretToVisible() }
        let target = viewRect(rect)
        let y = Landing.scrollTop(for: target,
                                  visibleHeight: clip.bounds.height,
                                  documentHeight: bounds.height)
        clip.scroll(to: CGPoint(x: clip.bounds.origin.x, y: y))
        enclosingScrollView?.reflectScrolledClipView(clip)
    }

    private func scrollCaretToVisible() {
        if let rect = layoutEngine.caretRect(docView, theme: theme) {
            scrollToVisible(viewRect(rect.insetBy(dx: 0, dy: -theme.lineHeight)))
        }
    }

    // MARK: the flash a landing leaves

    /// The byte range lit up by the landing in progress, and when it started.
    /// Both nil between landings, which is what keeps `draw` free of the whole
    /// question on every ordinary repaint.
    private var flashRange: Range<UInt32>?
    private var flashStarted: Date?
    private var flashTimer: Timer?

    /// Light up the block from `start` to `end` and fade it out.
    ///
    /// Redrawn on a timer rather than through Core Animation: the highlight is
    /// painted in `draw` alongside every other band of block chrome, in layout
    /// space, and a layer over the top would have to be re-placed on each scroll
    /// and reflow to stay on the words it belongs to.
    private func flash(from start: UInt32, to end: UInt32) {
        flashTimer?.invalidate()
        flashRange = start..<end
        flashStarted = Date()
        needsDisplay = true
        flashTimer = Timer.scheduledTimer(withTimeInterval: 1 / 30, repeats: true) { [weak self] t in
            guard let self, let started = self.flashStarted else { return t.invalidate() }
            guard Landing.opacity(elapsed: Date().timeIntervalSince(started)) != nil else {
                t.invalidate()
                self.flashTimer = nil
                self.flashRange = nil
                self.flashStarted = nil
                self.needsDisplay = true
                return
            }
            self.needsDisplay = true
        }
    }

    /// Paint the landing flash behind the rows its range covers.
    ///
    /// Behind the text and over the paper, where a code block's fill goes, and
    /// measured off `bands` for that same reason: a row's `height` reaches across
    /// a page break and a column gutter, and a highlight is a thing that must
    /// never appear in either.
    private func drawLandingFlash(in ctx: CGContext) {
        guard let flashRange, let flashStarted,
              let alpha = Landing.opacity(elapsed: Date().timeIntervalSince(flashStarted))
        else { return }
        // Core says which rows the range covers. Mapping `upperBound - 1`
        // through `posForOffset` looked equivalent and wasn't: a block ending in
        // a link ends inside the hidden destination, and the caret snap carries
        // that offset onto the row below — the flash lit the next block too.
        let span = doc.rowRangeFor(start: flashRange.lowerBound, end: flashRange.upperBound)
        let first = Int(span.first), last = Int(span.last)
        guard first <= last else { return }
        ctx.saveGState()
        ctx.setFillColor(theme.landingFlashColor.withAlphaComponent(
            theme.landingFlashColor.alphaComponent * alpha).cgColor)
        for rl in layoutEngine.rows[max(0, first)...min(last, layoutEngine.rows.count - 1)] {
            for band in rl.bands where band.height > 0 {
                let lit = band.insetBy(dx: -6, dy: 0)
                ctx.addPath(CGPath(roundedRect: lit, cornerWidth: 4, cornerHeight: 4, transform: nil))
            }
        }
        ctx.fillPath()
        ctx.restoreGState()
    }

    // MARK: mouse

    /// The `(row, ch)` a click at `p` resolves to. Inside a table the point is
    /// mapped through the grid to a source offset and back to a picture-row
    /// coordinate, so the ordinary `clickCh` path (which snaps to a cell stop)
    /// still applies; elsewhere it's the plain visual hit-test.
    private func hitRowCh(_ p: CGPoint) -> (Int, Int) {
        if let off = layoutEngine.tableHitOffset(p) {
            let rc = doc.posForOffset(off: UInt32(off))
            return (Int(rc.row), Int(rc.ch))
        }
        return layoutEngine.hit(p)
    }

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let p = layoutPoint(convert(event.locationInWindow, from: nil))
        // A plain click on a video or audio box starts it — the box's whole point
        // is the play badge drawn on it — and a click on an *empty* picture box
        // asks the host for it. The caret still moves there first, so a host that
        // ignores the media (no `onOpenMedia`) behaves exactly as before, and the
        // reader can still type around the block afterwards.
        if event.clickCount == 1, !event.modifierFlags.contains(.shift),
           let hit = layoutEngine.mediaBox(at: p),
           activateMedia(hit) {
            let (row, ch) = hitRowCh(p)
            render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: false))
            return
        }
        let (row, ch) = hitRowCh(p)
        // ⌘-click opens a link under the pointer (the native convention), leaving the
        // caret there. A plain click still places the caret to edit the link text.
        //
        // A footnote gets the same gesture and is tried first — the two can't
        // both answer (a reference is not a link node), so the order only decides
        // which query runs, not which wins. Same modifier for both because to a
        // reader they are one idea, "take me to where this points"; that a
        // footnote's destination happens to be further down this page rather than
        // in another document is leaf's distinction, not theirs.
        if event.modifierFlags.contains(.command), event.clickCount == 1 {
            render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: false))
            if !followFootnoteAtCaret() { openLinkAtCaret() }
            return
        }
        let extend = event.modifierFlags.contains(.shift)
        // A plain click places the caret; a double-click selects a word — inside
        // link text exactly as anywhere else. This is a text editor first: the
        // common thing to do to a link you can see is edit its label, and a click
        // that navigated instead made that the one span of text you couldn't put
        // a caret in without leaving the document. Following is ⌘-click (above),
        // the native convention, and the context menu's "Open Link".
        switch event.clickCount {
        case 2:  render(doc.selectWordCh(row: UInt32(row), ch: UInt32(ch)))
        case 3:  render(doc.selectBlockCh(row: UInt32(row), ch: UInt32(ch)))
        default: render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: extend))
        }
    }

    /// Open the link under the caret, if there is one. The host gets first
    /// refusal (`onOpenLink`); otherwise it goes to the default app, which needs
    /// the destination to parse as a URL. Used by ⌘-click and the "Open Link"
    /// menu item — the two gestures that ask to *leave*, rather than to edit.
    @discardableResult
    private func openLinkAtCaret() -> Bool {
        guard let dest = targetAtCaret() else { return false }
        // A bare `#v2` is a place in this document, so following it is a scroll
        // rather than a departure — the host has nothing to resolve and the
        // system has nothing to open.
        if let landing = doc.selfLanding(of: dest) { reveal(offset: landing); return true }
        if onOpenLink?(dest) == true { return true }
        guard let url = URL(string: dest) else { return false }
        NSWorkspace.shared.open(url)
        return true
    }

    /// The link the caret stands in, honouring this view's wikilink setting.
    private func targetAtCaret() -> String? {
        doc.activatableTargetAtCaret(wikilinks: recognizesWikilinks)
    }

    @objc private func openLink(_ sender: Any?) { openLinkAtCaret() }

    /// "Preview Link" — the menu's way to the same popover a resting pointer
    /// raises, for a reader who arrived by right-click or by keyboard rather than
    /// by hovering.
    @objc private func previewLink(_ sender: Any?) {
        let rc = doc.posForOffset(off: doc.caretOffset())
        guard let caret = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch)) else { return }
        peekOffset = doc.caretOffset()
        showLinkPeek(at: doc.caretOffset(), anchor: caret.insetBy(dx: -6, dy: 0))
    }

    @objc private func editLink(_ sender: Any?) {
        // The parsed destination, not `targetAtCaret()`: a wikilink can be
        // followed but has no node to repoint, and `linkActionsAtCaret` has
        // already kept `.edit` off the menu for one.
        guard let dest = doc.linkDestinationAtCaret() else { return }
        onEditLink?(dest)
    }

    // MARK: footnotes

    /// Follow the footnote under the caret — down to the note from a reference,
    /// back up to the reference from a note — and say whether there was one.
    ///
    /// Both directions are one call because to the reader they are one gesture:
    /// ⌘-click a footnote and you go to its other half, whichever half you were
    /// standing on. `FootnoteJump` has already worked out which that is.
    @discardableResult
    private func followFootnoteAtCaret() -> Bool {
        guard let jump = doc.footnoteJumpAtCaret() else { return false }
        // The peek was about where the reader *was*.
        dismissFootnotePeek()
        render(doc.caretMoved(to: jump.offset))
        return true
    }

    @objc private func followFootnote(_ sender: Any?) { followFootnoteAtCaret() }

    // MARK: footnote peek (hover)

    /// The popover a resting pointer raises over a footnote reference, and the
    /// bookkeeping that decides when to raise and drop it.
    ///
    /// `peekAnchor` is the rect that raised the one currently up, in layout
    /// space. The pointer staying inside it means "still the same reference", and
    /// testing that rather than the byte offset is what stops a one-glyph label —
    /// whose two ends are two different offsets — flickering the popover as the
    /// pointer crosses it.
    private lazy var footnotePeek: FootnotePeekPresenter = {
        let presenter = FootnotePeekPresenter()
        presenter.delegate = self
        return presenter
    }()

    /// The peek a *citation inside a note* raises — a second popover over the
    /// first, and the only stacking allowed anywhere here.
    ///
    /// A vault of scripture puts the interesting links exactly one level down: a
    /// verse's footnote is a list of citations, and every one is a link into
    /// another chapter. Refusing to stack meant the reader could see which
    /// citations there were and never what they said without clicking through and
    /// losing the note they were reading. One level answers that; `raisesNestedPeeks`
    /// is what stops a second.
    private lazy var nestedPeek: FootnotePeekPresenter = {
        let presenter = FootnotePeekPresenter()
        presenter.delegate = self
        presenter.raisesNestedPeeks = false
        return presenter
    }()
    private var peekAnchor: CGRect?
    private var peekOffset: UInt32?
    private var peekTimer: Timer?
    private var peekTracking: NSTrackingArea?
    /// Counts down to taking a peek away once the pointer has left the reference,
    /// so the reader has time to travel into the popover. Cancelled when they
    /// arrive; see `footnotePeek(_:pointerIsInside:)`.
    private var peekCloseTimer: Timer?
    /// Whether the pointer is inside the popover right now — the one state that
    /// makes a peek unclosable, because closing it would be closing it under a
    /// reader who is reading or selecting from it.
    private var pointerInPeek = false

    public override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let peekTracking { removeTrackingArea(peekTracking) }
        // `.inVisibleRect` keeps the area in step with scrolling and resizing on
        // its own, so the zero rect is never consulted.
        let area = NSTrackingArea(
            rect: .zero,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self, userInfo: nil)
        addTrackingArea(area)
        peekTracking = area
    }

    public override func mouseMoved(with event: NSEvent) {
        let p = layoutPoint(convert(event.locationInWindow, from: nil))
        // Still over the reference the popover is about: leave it up, and cancel
        // any close the pointer's last excursion started.
        if let peekAnchor, peekAnchor.insetBy(dx: -6, dy: -2).contains(p) {
            peekCloseTimer?.invalidate()
            peekCloseTimer = nil
            return
        }

        let (row, ch) = hitRowCh(p)
        let off = doc.offsetForPos(row: UInt32(row), ch: UInt32(ch))
        guard off != peekOffset else { return }
        // Off the reference and onto something else. A peek that is up gets a
        // moment first: the pointer may be on its way *into* it, and the path
        // from a reference to the popover below it crosses ordinary prose.
        if footnotePeek.isShowing {
            scheduleFootnotePeekClose()
        } else {
            dismissFootnotePeek()
        }
        peekOffset = off

        // Asked only once the pointer has rested. `footnotePeekContent` walks the
        // document's nodes — far too much work to repeat for every point a
        // pointer sweeps across a paragraph — and a popover that appeared the
        // instant the pointer touched a reference would strobe along the line
        // anyway. This is the delay a tooltip has, for the reason a tooltip has
        // it.
        peekTimer = Timer.scheduledTimer(withTimeInterval: 0.4, repeats: false) { [weak self] _ in
            self?.showFootnotePeek(at: off, row: row, ch: ch)
        }
    }

    public override func mouseExited(with event: NSEvent) {
        // The pointer may have left this view *into* the popover, which sits over
        // it in a window of its own — so this is a candidate for closing, not a
        // close. The popover says whether the reader arrived.
        peekOffset = nil
        if footnotePeek.isShowing {
            scheduleFootnotePeekClose()
        } else {
            dismissFootnotePeek()
        }
    }

    /// Take the peek down shortly, unless the reader turns out to be in it.
    ///
    /// The delay is what makes a peek usable at all: with an immediate close the
    /// popover vanished the moment the pointer left the `[1]`, so there was no
    /// way to reach it — and a note you can't reach is a note you can't select,
    /// copy, or follow a link out of.
    private func scheduleFootnotePeekClose() {
        guard peekCloseTimer == nil else { return }
        peekCloseTimer = Timer.scheduledTimer(withTimeInterval: 0.35, repeats: false) { [weak self] _ in
            guard let self else { return }
            self.peekCloseTimer = nil
            guard !self.pointerInPeek else { return }
            self.dismissFootnotePeek()
        }
    }

    private func showFootnotePeek(at off: UInt32, row: Int, ch: Int) {
        peekTimer = nil
        guard let caret = layoutEngine.rect(row: row, ch: ch) else { return }
        // A caret rect is a hairline; widened so the popover's arrow points at the
        // reference rather than balancing on its edge.
        let anchor = caret.insetBy(dx: -6, dy: 0)
        // `docView` is the frame on screen, so the note is drawn from the rows
        // the page below is already drawing it from — the popover and the
        // document can't disagree about the same note.
        if let content = doc.footnotePeekContent(at: off, in: docView, theme: theme) {
            peekAnchor = anchor
            footnotePeek.show(content, from: viewRect(anchor), in: self)
            return
        }
        showLinkPeek(at: off, anchor: anchor)
    }

    /// Show what the link at `off` points *at*, the way a footnote reference
    /// shows its note.
    ///
    /// Two sources, and only the first is leaf's. A `#v2` names a place in this
    /// document, which is already laid out on screen. Anything else names a file,
    /// and reading a file is the host's — `onPeekLink` fetches, and the answer
    /// comes back whenever it comes back.
    private func showLinkPeek(at off: UInt32, anchor: CGRect) {
        guard let destination = doc.linkDestinationAt(off: off) else { return }
        if destination.hasPrefix("#") {
            let locator = String(destination.dropFirst())
            guard let content = FootnotePeekContent(
                peeking: locator, of: doc, in: docView, theme: theme) else { return }
            peekAnchor = anchor
            footnotePeek.show(content, from: viewRect(anchor), in: self)
            return
        }
        guard let onPeekLink else { return }
        onPeekLink(destination) { [weak self] fetched in
            guard let self, let fetched,
                  // The pointer may have moved on, or moved on and come back to a
                  // different link, while the host was reading a file. Answering
                  // the old question over the new one is worse than not answering.
                  self.peekOffset == off,
                  let content = FootnotePeekContent(peeking: fetched, theme: self.theme)
            else { return }
            self.peekAnchor = anchor
            self.footnotePeek.show(content, from: self.viewRect(anchor), in: self)
        }
    }

    /// Take the peek down and forget what it was about. Called for everything
    /// that makes it stale — the pointer leaving, a click, a keystroke, a
    /// relayout — since an answer about a place the reader has left is worse than
    /// no answer.
    private func dismissFootnotePeek() {
        peekTimer?.invalidate()
        peekTimer = nil
        peekCloseTimer?.invalidate()
        peekCloseTimer = nil
        pointerInPeek = false
        peekAnchor = nil
        // The nested one first, and always: it is anchored inside the note's
        // window, so a note taken down under it would leave a popover pointing
        // at nothing.
        nestedPeek.hide()
        footnotePeek.hide()
    }

    // MARK: a peek the reader is using

    /// The pointer arrived in (or left) the popover.
    ///
    /// Arriving cancels the pending close, which is what lets a reader move from
    /// the reference into the note and stay there — reading it, selecting a
    /// sentence, following a link. Leaving starts the countdown again rather than
    /// closing outright, for the same reason arriving needed a grace period: the
    /// pointer may be crossing back to the reference.
    func footnotePeek(_ presenter: FootnotePeekPresenter, pointerIsInside inside: Bool) {
        // The nested peek is a thing to read, not to travel into: it holds no
        // links (a foreign document's are inert) and nothing to select that the
        // note above it doesn't already offer. So the pointer's comings and
        // goings in it say nothing about whether the *note* should stay up, and
        // reading them as the note's own would take the note down the moment the
        // second popover appeared under the pointer.
        guard presenter !== nestedPeek else { return }
        pointerInPeek = inside
        if inside {
            peekCloseTimer?.invalidate()
            peekCloseTimer = nil
        } else {
            scheduleFootnotePeekClose()
        }
    }

    /// A citation inside a note has been rested on: show what it points at, over
    /// the note, without disturbing it.
    ///
    /// Same two sources as a hover in the document itself — a `#v2` is a place in
    /// this document and needs no host, anything else is a file only the host can
    /// read. Unlike that one it anchors to a rect inside the *popover's* text
    /// view, which is a real view in a real window and can carry a popover of its
    /// own.
    func footnotePeek(_ presenter: FootnotePeekPresenter,
                      wantsPeekOf destination: String?, from rect: CGRect, in view: NSView) {
        nestedPeek.hide()
        guard let destination else { return }
        if destination.hasPrefix("#") {
            let content = FootnotePeekContent(peeking: String(destination.dropFirst()),
                                              of: doc, in: docView, theme: theme)
            guard let content else { return }
            nestedPeek.show(content, from: rect, in: view)
            return
        }
        guard let onPeekLink else { return }
        onPeekLink(destination) { [weak self, weak view] fetched in
            guard let self, let view, let fetched,
                  // The note itself may have gone while the host was reading a
                  // file — and a second popover outliving the first would be left
                  // pointing at a window that isn't there.
                  self.footnotePeek.isShowing,
                  let content = FootnotePeekContent(peeking: fetched, theme: self.theme)
            else { return }
            self.nestedPeek.show(content, from: rect, in: view)
        }
    }

    /// The reader clicked something followable inside the note.
    ///
    /// Either way the peek is over: it was about the reference they have now
    /// left, and leaving it up over a document that has just scrolled somewhere
    /// else would point at the wrong words.
    func footnotePeek(_ presenter: FootnotePeekPresenter, didFollow target: FootnotePeekTarget) {
        switch target.kind {
        case .link(let destination):
            dismissFootnotePeek()
            // A `#v2` in a note names a place in the document the note belongs
            // to, so it navigates here rather than going out to the host.
            if let landing = doc.selfLanding(of: destination) { reveal(offset: landing); return }
            // The same division the document draws: the host gets first refusal,
            // because a note's `./sibling.md` means what it means everywhere else
            // in this document.
            if onOpenLink?(destination) == true { return }
            guard let url = URL(string: destination) else { return }
            NSWorkspace.shared.open(url)
        case .footnote(let offset):
            // Navigate rather than stack a second popover: the note it names is a
            // place in this document, and `render` scrolls the caret into view,
            // so the reader lands looking at it.
            dismissFootnotePeek()
            render(doc.caretMoved(to: offset))
            followFootnoteAtCaret()
        }
    }

    @objc private func copyLink(_ sender: Any?) {
        guard let dest = targetAtCaret() else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(dest, forType: .string)
    }

    public override func mouseDragged(with event: NSEvent) {
        let p = layoutPoint(convert(event.locationInWindow, from: nil))
        let (row, ch) = hitRowCh(p)
        render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: true))
    }

    // MARK: drag & drop (destination)

    public override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        window?.makeFirstResponder(self)
        moveCaretToDrop(sender)
        return .copy
    }

    public override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        moveCaretToDrop(sender)   // track the drop point so the caret previews it
        return .copy
    }

    public override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        moveCaretToDrop(sender)
        let pb = sender.draggingPasteboard
        if let html = pb.string(forType: .html) {
            render(doc.pasteRich(html: html, text: pb.string(forType: .string) ?? ""))
            return true
        }
        if let text = pb.string(forType: .string) {
            render(doc.paste(text: text))
            return true
        }
        return false
    }

    private func moveCaretToDrop(_ sender: NSDraggingInfo) {
        let p = layoutPoint(convert(sender.draggingLocation, from: nil))
        let (row, ch) = hitRowCh(p)
        render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: false))
    }

    // MARK: keyboard — text + IME

    public override func keyDown(with event: NSEvent) {
        // Shift+Return is leaf's in-cell line break. AppKit's default key bindings
        // don't distinguish it from a bare Return — both resolve to
        // `insertNewline:` (only Ctrl+Return maps to `insertLineBreak:`) — so
        // without catching it here it would drop to the next cell instead of
        // breaking the line. Route it straight to the line-break command, which
        // no-ops off a table (falling back to an ordinary newline). Skip while an
        // IME composition is live so a Shift+Return that commits marked text still
        // reaches the input system.
        let mods = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let isReturn = event.keyCode == 36 || event.keyCode == 76 // Return, keypad Enter
        if markedByteRange == nil, isReturn, mods == .shift {
            doCommand(by: #selector(insertLineBreak(_:)))
            return
        }
        if !(inputContext?.handleEvent(event) ?? false) { interpretKeyEvents([event]) }
    }

    public func insertText(_ string: Any, replacementRange: NSRange) {
        let text = (string as? String) ?? (string as? NSAttributedString)?.string ?? ""
        // Committing an IME composition: replace the marked bytes with the final text.
        if let m = markedByteRange {
            markedByteRange = nil
            render(doc.replaceRange(from: UInt32(m.location), to: UInt32(m.location + m.length), text: text))
            return
        }
        guard !text.isEmpty else { return }
        render(doc.insert(text: text))
    }

    public override func doCommand(by selector: Selector) {
        switch selector {
        case #selector(moveLeft(_:)):                       render(doc.moveLeft(extend: false))
        case #selector(moveRight(_:)):                      render(doc.moveRight(extend: false))
        // ↑/↓ ride the pixel wrap, not core's paragraph rows (which the unwrapped map
        // exposes) — computed from the visual geometry, then snapped by `clickCh`.
        case #selector(moveUp(_:)):                         moveVertical(up: true, extend: false)
        case #selector(moveDown(_:)):                       moveVertical(up: false, extend: false)
        case #selector(moveLeftAndModifySelection(_:)):     render(doc.moveLeft(extend: true))
        case #selector(moveRightAndModifySelection(_:)):    render(doc.moveRight(extend: true))
        case #selector(moveUpAndModifySelection(_:)):       moveVertical(up: true, extend: true)
        case #selector(moveDownAndModifySelection(_:)):     moveVertical(up: false, extend: true)
        case #selector(moveWordLeft(_:)):                   render(doc.moveWordLeft(extend: false))
        case #selector(moveWordRight(_:)):                  render(doc.moveWordRight(extend: false))
        case #selector(moveWordLeftAndModifySelection(_:)): render(doc.moveWordLeft(extend: true))
        case #selector(moveWordRightAndModifySelection(_:)):render(doc.moveWordRight(extend: true))
        // Home/End go to the *visual* line's ends, not the whole paragraph's.
        case #selector(moveToLeftEndOfLine(_:)),
             #selector(moveToBeginningOfLine(_:)):          moveToVisualLineBoundary(toStart: true, extend: false)
        case #selector(moveToLeftEndOfLineAndModifySelection(_:)),
             #selector(moveToBeginningOfLineAndModifySelection(_:)): moveToVisualLineBoundary(toStart: true, extend: true)
        case #selector(moveToRightEndOfLine(_:)),
             #selector(moveToEndOfLine(_:)):                moveToVisualLineBoundary(toStart: false, extend: false)
        case #selector(moveToRightEndOfLineAndModifySelection(_:)),
             #selector(moveToEndOfLineAndModifySelection(_:)): moveToVisualLineBoundary(toStart: false, extend: true)
        case #selector(moveToBeginningOfDocument(_:)):      render(doc.moveDocStart(extend: false))
        case #selector(moveToEndOfDocument(_:)):            render(doc.moveDocEnd(extend: false))
        case #selector(moveToBeginningOfDocumentAndModifySelection(_:)): render(doc.moveDocStart(extend: true))
        case #selector(moveToEndOfDocumentAndModifySelection(_:)):       render(doc.moveDocEnd(extend: true))
        // In a table these keys take on grid meanings (see the FFI's `cell_*`):
        // Return drops a cell, Shift+Return breaks a line within one, Tab/Shift+Tab
        // walk the cells. Each returns nil off the table, where the key keeps its
        // ordinary job (newline, indent).
        case #selector(insertNewline(_:)):
            render(doc.cellReturn() ?? doc.newline())
        case #selector(insertLineBreak(_:)):
            render(doc.cellLineBreak() ?? doc.newline())
        case #selector(insertTab(_:)):
            render(doc.cellTab(forward: true) ?? doc.indent())
        case #selector(insertBacktab(_:)):
            render(doc.cellTab(forward: false) ?? doc.outdent())
        case #selector(deleteBackward(_:)):                 render(doc.backspace())
        case #selector(deleteForward(_:)):                  render(doc.deleteForward())
        case #selector(deleteWordBackward(_:)):             render(doc.deleteWordBack())
        case #selector(deleteWordForward(_:)):              render(doc.deleteWordForward())
        default: super.doCommand(by: selector)
        }
    }

    // MARK: visual-line motion (the wrap is ours, so core can't do these)

    /// Move the caret one *visual* line up/down, holding the pixel x it started from
    /// so a run of ↑/↓ doesn't drift through shorter lines. Probes one line-height
    /// past the caret and hit-tests, so it crosses block boundaries naturally.
    private func moveVertical(up: Bool, extend: Bool) {
        guard let caret = layoutEngine.caretRect(docView, theme: theme) else {
            render(up ? doc.moveUp(extend: extend) : doc.moveDown(extend: extend))
            return
        }
        // Probe from the caret's full *line band* — inside a table that clears the
        // cell's vertical padding, which the thin caret rect doesn't, so a step
        // actually crosses into the next line/cell instead of stalling. Hit-test
        // the table-aware way (`hitRowCh`), or a probe into a table resolves to the
        // collapsed picture row and teleports the caret to its top-left cell.
        let band = layoutEngine.caretBand(src: Int(docView.caretSrc))
        let goalX = verticalGoalX ?? caret.minX
        var probeY = up ? (band?.minY ?? caret.minY) - 1 : (band?.maxY ?? caret.maxY) + 1
        var (row, ch) = hitRowCh(CGPoint(x: goalX, y: probeY))
        // A block boundary is drawn as a short blank gap row (`blockGap`, half a
        // line) that holds no caret. Probing one line past the caret lands *inside*
        // that gap, where the hit-test snaps back and forth between the block above
        // and below — so a step from a paragraph into the list or code block under
        // it moves only sometimes. Step over any gap row(s) to the next row that can
        // actually hold the caret; the bounded walk can't outrun the row count.
        let rows = layoutEngine.rows
        var guardCount = 0
        while rows.indices.contains(row), rows[row].row.isBlockGap, guardCount < rows.count {
            let r = rows[row]
            probeY = up ? r.top - 1 : r.top + r.height + 1
            (row, ch) = hitRowCh(CGPoint(x: goalX, y: probeY))
            guardCount += 1
        }
        verticalGoalX = goalX
        render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: extend), keepVerticalGoal: true)
    }

    /// Move to the start or end of the caret's *visual* line. At the end of a
    /// soft-wrapped line, stop before the wrap whitespace so the caret stays on this
    /// line rather than jumping to the next line's start.
    private func moveToVisualLineBoundary(toStart: Bool, extend: Bool) {
        let row = Int(docView.caretRow), ch = Int(docView.caretCh)
        guard let vl = layoutEngine.visualLine(row: row, ch: ch) else {
            render(toStart ? doc.moveHome(extend: extend) : doc.moveEnd(extend: extend))
            return
        }
        var target = toStart ? vl.start : vl.end
        if !toStart, vl.index < layoutEngine.rows[row].wrapped.count - 1,
           layoutEngine.rows[row].wrapped[vl.index].attributed.string.hasSuffix(" ") {
            target -= 1
        }
        render(doc.clickCh(row: UInt32(row), ch: UInt32(target), extend: extend))
    }

    public override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard event.modifierFlags.contains(.command) else { return super.performKeyEquivalent(with: event) }
        let shift = event.modifierFlags.contains(.shift)
        switch event.charactersIgnoringModifiers?.lowercased() {
        case "b": render(doc.toggleBold()); return true
        case "i": render(doc.toggleItalic()); return true
        case "u": render(doc.toggleUnderline()); return true
        case "e": render(doc.toggleView()); return true
        case "z": render(shift ? doc.redo() : doc.undo()); return true
        case "a": render(doc.selectAll()); return true
        case "c": copy(nil); return true
        case "x": cut(nil); return true
        case "v": if shift { pasteAsPlainText(nil) } else { paste(nil) }; return true
        default:  return super.performKeyEquivalent(with: event)
        }
    }

    // MARK: rich clipboard

    @objc public func copy(_ sender: Any?) {
        guard let text = doc.selectedText() else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
        if let html = doc.selectionHtml() { pb.setString(html, forType: .html) }
    }

    @objc public func cut(_ sender: Any?) {
        copy(sender)
        if doc.selectedText() != nil { render(doc.backspace()) }
    }

    @objc public func paste(_ sender: Any?) {
        // Asked before the text flavors, and before the guard below: a clipboard
        // holding only an image has no text at all, so by the time this method
        // decided there was nothing to paste, the host would never hear about the
        // one thing that was there. See `LeafEditorModel.onPaste`.
        if onPaste?() == true { return }
        let pb = NSPasteboard.general
        let html = pb.string(forType: .html)
        let text = pb.string(forType: .string) ?? ""
        guard html != nil || !text.isEmpty else { return }
        render(doc.pasteRich(html: html, text: text))
    }

    /// ⇧⌘V — the plain-flavor escape hatch: insert the pasteboard's text as leaf
    /// *source*, ignoring any rich HTML flavor (mirrors leaf-gpui's ⇧⌘V and
    /// leaf-tui's ⌥V). The Edit menu's "Paste and Match Style" routes here too.
    @objc public func pasteAsPlainText(_ sender: Any?) {
        guard let text = NSPasteboard.general.string(forType: .string), !text.isEmpty else { return }
        render(doc.paste(text: text))
    }

    @objc public override func selectAll(_ sender: Any?) { render(doc.selectAll()) }

    // MARK: contextual menu + macOS text services

    public override func menu(for event: NSEvent) -> NSMenu? {
        window?.makeFirstResponder(self)
        // Right-clicking outside the selection moves the caret there first, like a
        // native text view; a click inside an existing selection keeps it.
        if !hasSelection {
            let p = layoutPoint(convert(event.locationInWindow, from: nil))
            let (row, ch) = hitRowCh(p)
            render(doc.clickCh(row: UInt32(row), ch: UInt32(ch), extend: false))
        }

        let menu = NSMenu()
        // A footnote under the click leads the menu, for the reason a link does:
        // since a plain click no longer follows anything, the menu and ⌘-click
        // are the only ways to get there. At most one entry, and never alongside
        // a link's — the caret is either on a reference, in a note, or in neither.
        let footnotes = doc.footnoteActionsAtCaret()
        for action in footnotes {
            switch action {
            case .goToNote:
                menu.addItem(withTitle: loc("menu.goToNote", "Go to Note"),
                             action: #selector(followFootnote(_:)), keyEquivalent: "")
            case .backToReference:
                menu.addItem(withTitle: loc("menu.backToReference", "Back to Reference"),
                             action: #selector(followFootnote(_:)), keyEquivalent: "")
            }
        }
        // A link under the click (the caret was just placed there) leads the menu.
        // Since a plain click no longer follows, this — with ⌘-click — is how a
        // reader gets to the destination at all, so it stays first.
        let links = doc.linkActionsAtCaret(wikilinks: recognizesWikilinks, canEdit: onEditLink != nil,
                                           canPeek: onPeekLink != nil)
        for action in links {
            switch action {
            case .peek:
                menu.addItem(withTitle: loc("menu.previewLink", "Preview Link"), action: #selector(previewLink(_:)), keyEquivalent: "")
            case .open:
                menu.addItem(withTitle: loc("menu.openLink", "Open Link"), action: #selector(openLink(_:)), keyEquivalent: "")
            case .edit:
                menu.addItem(withTitle: loc("menu.editLink", "Edit Link…"), action: #selector(editLink(_:)), keyEquivalent: "")
            case .copy:
                menu.addItem(withTitle: loc("menu.copyLink", "Copy Link"), action: #selector(copyLink(_:)), keyEquivalent: "")
            }
        }
        if !links.isEmpty || !footnotes.isEmpty {
            menu.addItem(.separator())
        }
        if hasSelection {
            menu.addItem(withTitle: loc("menu.cut", "Cut"), action: #selector(cut(_:)), keyEquivalent: "")
            menu.addItem(withTitle: loc("menu.copy", "Copy"), action: #selector(copy(_:)), keyEquivalent: "")
        }
        menu.addItem(withTitle: loc("menu.paste", "Paste"), action: #selector(paste(_:)), keyEquivalent: "")
        menu.addItem(withTitle: loc("menu.pasteMatchStyle", "Paste and Match Style"), action: #selector(pasteAsPlainText(_:)), keyEquivalent: "")
        menu.addItem(withTitle: loc("menu.selectAll", "Select All"), action: #selector(selectAll(_:)), keyEquivalent: "")
        if hasSelection, let text = doc.selectedText(), !text.isEmpty {
            menu.addItem(.separator())
            let shown = text.count > 24 ? text.prefix(24) + "…" : Substring(text)
            menu.addItem(withTitle: String(format: loc("menu.lookUp", "Look Up “%@”"), String(shown)),
                         action: #selector(lookUpSelection(_:)), keyEquivalent: "")
            menu.addItem(withTitle: loc("menu.share", "Share…"), action: #selector(shareSelection(_:)), keyEquivalent: "")
        }
        return menu
    }

    @objc private func lookUpSelection(_ sender: Any?) {
        guard let text = doc.selectedText(), !text.isEmpty else { return }
        let rc = doc.posForOffset(off: UInt32(selLowByte))
        let origin = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch))?.origin ?? .zero
        showDefinition(for: NSAttributedString(string: text),
                       at: NSPoint(x: origin.x, y: origin.y + theme.lineHeight))
    }

    @objc private func shareSelection(_ sender: Any?) {
        guard let text = doc.selectedText(), !text.isEmpty else { return }
        let rc = doc.posForOffset(off: UInt32(selLowByte))
        let anchor = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch)) ?? .zero
        NSSharingServicePicker(items: [text]).show(relativeTo: anchor, of: self, preferredEdge: .minY)
    }

    /// Advertise the selection to the Services system: we can *send* a string when
    /// there's a selection and *receive* one to replace it. Pairs with the
    /// `NSServicesMenuRequestor` methods below.
    public override func validRequestor(forSendType sendType: NSPasteboard.PasteboardType?,
                                        returnType: NSPasteboard.PasteboardType?) -> Any? {
        let sendOK = sendType == nil || (sendType == .string && hasSelection)
        let returnOK = returnType == nil || returnType == .string
        if sendOK, returnOK { return self }
        return super.validRequestor(forSendType: sendType, returnType: returnType)
    }

    public func writeSelection(to pboard: NSPasteboard, types: [NSPasteboard.PasteboardType]) -> Bool {
        guard hasSelection, types.contains(.string), let text = doc.selectedText() else { return false }
        pboard.clearContents()
        return pboard.setString(text, forType: .string)
    }

    public func readSelection(from pboard: NSPasteboard) -> Bool {
        guard let text = pboard.string(forType: .string) else { return false }
        render(doc.pasteRich(html: nil, text: text))
        return true
    }

    // MARK: accessibility — expose the document as a native text area

    public override func isAccessibilityElement() -> Bool { true }
    public override func accessibilityRole() -> NSAccessibility.Role? { .textArea }
    public override func accessibilityValue() -> Any? { fullText() }
    public override func accessibilityNumberOfCharacters() -> Int { (fullText() as NSString).length }
    public override func accessibilityInsertionPointLineNumber() -> Int { Int(docView.caretRow) }

    public override func accessibilitySelectedText() -> String? {
        doc.textInRange(from: UInt32(selLowByte), to: UInt32(selHighByte))
    }

    public override func accessibilitySelectedTextRange() -> NSRange {
        let loc = (doc.textInRange(from: 0, to: UInt32(selLowByte)) as NSString).length
        let len = ((accessibilitySelectedText() ?? "") as NSString).length
        return NSRange(location: loc, length: len)
    }

    public override func setAccessibilitySelectedTextRange(_ range: NSRange) {
        let full = fullText() as NSString
        guard range.location >= 0, range.location + range.length <= full.length else { return }
        let fromByte = full.substring(to: range.location).utf8.count
        let toByte = full.substring(to: range.location + range.length).utf8.count
        render(doc.setSelectionOffsets(anchor: UInt32(fromByte), focus: UInt32(toByte)))
    }

    public override func accessibilityString(for range: NSRange) -> String? {
        let full = fullText() as NSString
        guard range.location >= 0, range.location + range.length <= full.length else { return nil }
        return full.substring(with: range)
    }

    // MARK: focus + caret blink

    public override func becomeFirstResponder() -> Bool { isFocused = true; resetBlink(); needsDisplay = true; return true }
    public override func resignFirstResponder() -> Bool { isFocused = false; blinkTimer?.invalidate(); needsDisplay = true; return true }

    private func resetBlink() {
        blinkTimer?.invalidate()
        caretVisible = true
        guard isFocused else { return }
        blinkTimer = Timer.scheduledTimer(withTimeInterval: 0.53, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.caretVisible.toggle()
            if let r = self.layoutEngine.caretRect(self.docView, theme: self.theme) { self.setNeedsDisplay(r) }
        }
    }

    // MARK: window key state — selection emphasis + caret track the key window

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        let nc = NotificationCenter.default
        nc.removeObserver(self, name: NSWindow.didBecomeKeyNotification, object: nil)
        nc.removeObserver(self, name: NSWindow.didResignKeyNotification, object: nil)
        nc.removeObserver(self, name: NSView.frameDidChangeNotification, object: nil)
        guard let window else { return }
        // Paginated, the frame is the stack's own width rather than the clip
        // view's, so `layout()` no longer fires on a window resize — and where the
        // stack centres is decided by exactly that width. Watch the viewport
        // instead. (Continuously this is redundant with the autoresized frame's
        // own `layout()`, and both paths guard on the width actually changing.)
        if let clip = enclosingScrollView?.contentView {
            clip.postsFrameChangedNotifications = true
            nc.addObserver(self, selector: #selector(viewportResized),
                           name: NSView.frameDidChangeNotification, object: clip)
        }
        // Advertise the selection to the app-wide Services menu (Edit ▸ Services).
        NSApp.registerServicesMenuSendTypes([.string], returnTypes: [.string])
        // Accept text/rich content dropped into the editor.
        registerForDraggedTypes([.string, .html])
        nc.addObserver(self, selector: #selector(keyStateChanged), name: NSWindow.didBecomeKeyNotification, object: window)
        nc.addObserver(self, selector: #selector(keyStateChanged), name: NSWindow.didResignKeyNotification, object: window)
    }

    @objc private func keyStateChanged() { resetBlink(); needsDisplay = true }

    @objc private func viewportResized() {
        relayoutForWidth(force: false)
        applyContentSize()
    }

    deinit { NotificationCenter.default.removeObserver(self) }

    // MARK: selection offsets
    //
    // The character-index space shared by `NSTextInputClient`, Services, and
    // accessibility below is leaf-core's **byte offset** — the same handle the iOS
    // `UITextInput` peer uses. Core owns the offset⇄position mapping (`posForOffset` /
    // `offsetForPos`), so these only have to stay self-consistent, which they do.

    private var caretByte: Int { Int(doc.caretOffset()) }
    private var anchorByte: Int { Int(doc.anchorOffset()) }
    private var selLowByte: Int { min(anchorByte, caretByte) }
    private var selHighByte: Int { max(anchorByte, caretByte) }
    private var hasSelection: Bool { docView.hasSelection }

    /// The document's whole plain text — the buffer those byte offsets count into.
    private func fullText() -> String { doc.textInRange(from: 0, to: doc.docEndOffset()) }

    // MARK: NSTextInputClient — real selection, geometry, and hit-testing
    //
    // With these reporting the true selection, macOS's system text services light up:
    // Look Up, the Services menu, dictation, and IME candidate placement all target
    // the real range. Marked text (the inline IME composition) is inserted as it's
    // composed and drawn with a composing underline; `insertText` commits it.

    public func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let text = (string as? String) ?? (string as? NSAttributedString)?.string ?? ""
        // Bytes to replace: the existing composition, else the proposed replacement,
        // else the current selection.
        let start: Int, end: Int
        if let m = markedByteRange {
            start = m.location; end = m.location + m.length
        } else if replacementRange.location != NSNotFound {
            start = replacementRange.location; end = replacementRange.location + replacementRange.length
        } else {
            start = selLowByte; end = selHighByte
        }
        render(doc.replaceRange(from: UInt32(max(0, start)), to: UInt32(max(start, end)), text: text))
        if text.isEmpty {
            markedByteRange = nil
        } else {
            markedByteRange = NSRange(location: start, length: text.utf8.count)
            // Place the caret within the composition per the IME's selected range.
            let ns = text as NSString
            let uptoUTF16 = min(max(0, selectedRange.location + selectedRange.length), ns.length)
            let caret = start + ns.substring(to: uptoUTF16).utf8.count
            render(doc.setSelectionOffsets(anchor: UInt32(caret), focus: UInt32(caret)))
        }
        needsDisplay = true
    }

    public func unmarkText() { markedByteRange = nil; needsDisplay = true }
    public func hasMarkedText() -> Bool { markedByteRange != nil }
    public func markedRange() -> NSRange { markedByteRange ?? NSRange(location: NSNotFound, length: 0) }
    public func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

    public func selectedRange() -> NSRange {
        NSRange(location: selLowByte, length: selHighByte - selLowByte)
    }

    public func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? {
        let from = max(0, range.location)
        let to = max(from, range.location + range.length)
        actualRange?.pointee = NSRange(location: from, length: to - from)
        return NSAttributedString(string: doc.textInRange(from: UInt32(from), to: UInt32(to)))
    }

    public func characterIndex(for point: NSPoint) -> Int {
        guard let window else { return NSNotFound }
        let local = layoutPoint(convert(window.convertPoint(fromScreen: point), from: nil))
        let (row, ch) = layoutEngine.hit(local)
        return Int(doc.offsetForPos(row: UInt32(row), ch: UInt32(ch)))
    }

    public func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let window else { return .zero }
        actualRange?.pointee = range
        let rc = doc.posForOffset(off: UInt32(max(0, range.location)))
        guard let rect = layoutEngine.rect(row: Int(rc.row), ch: Int(rc.ch)) else { return .zero }
        return window.convertToScreen(convert(viewRect(rect), to: nil))
    }

    // MARK: host access

    public func sourceText() -> String { doc.source() }
    public func markSaved() { render(doc.markSaved()) }
    public func command(_ op: (LeafDoc) -> DocView) { render(op(doc)) }
}
#endif
