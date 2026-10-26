//  RegionEditing.swift
//
//  Regions an editor of their own edits. A formula, or a fence of a language
//  the host has an editor for, is a region: the view tells core which kinds it
//  owns (`setOwnedRegions`), core keeps each one a picture or a placeholder row,
//  and the frame lists them in `DocView.regions` with the content range their
//  editor splices. One region at a time is open in a view, and the view routes
//  keys, typing, clicks, undo and the caret through the protocol here, written
//  once for every kind. See docs/proposals/embedded-editors.md.
//
//  A closed region is its kind's renderer's picture (`RegionPictures.swift`).
//  Two kinds of session differ in who owns the focus while a region is open.
//  Leaf runs one that draws its own picture — a formula, whose session is
//  a `LeafRegionSession` over the Rust editor — passing it the keys and laying
//  out what it draws. A host's editor runs one with a view of its own, which
//  the view places over the region's rows and gives the focus; that view
//  writes back through its `RegionHost`. Only an open region is ever a live
//  view.

import CoreGraphics
import Foundation
import LeafFFI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// Where the caret enters a region being opened.
public enum RegionEntry {
    /// Its first stop, as → into it from in front.
    case home
    /// Its last stop, as ← into it from behind.
    case end
    /// Nearest a point, in points from its drawing's top left, set at `size`
    /// points per em.
    case point(CGPoint, size: CGFloat)
}

/// One splice on a region's content: replace the UTF-8 bytes `[from, to)` —
/// offsets into the content as the splice before this one left it — with
/// `text`, written exactly.
public struct RegionSplice: Equatable {
    public var from: UInt32
    public var to: UInt32
    public var text: String

    public init(from: UInt32, to: UInt32, text: String) {
        self.from = from
        self.to = to
        self.text = text
    }

    /// The one splice that turns `before` into `after`: what the two do not
    /// share at either end, on character boundaries. Empty when they are the
    /// same.
    public static func diff(_ before: String, _ after: String) -> [RegionSplice] {
        guard !before.utf8.elementsEqual(after.utf8) else { return [] }
        let a = Array(before.utf8)
        let b = Array(after.utf8)
        let continuation = { (x: [UInt8], i: Int) in i < x.count && x[i] & 0xC0 == 0x80 }
        var p = 0
        while p < a.count, p < b.count, a[p] == b[p] { p += 1 }
        while p > 0, continuation(a, p) || continuation(b, p) { p -= 1 }
        var q = 0
        while q < a.count - p, q < b.count - p, a[a.count - 1 - q] == b[b.count - 1 - q] { q += 1 }
        while q > 0, continuation(a, a.count - q) || continuation(b, b.count - q) { q -= 1 }
        let text = String(decoding: b[p..<(b.count - q)], as: UTF8.self)
        return [RegionSplice(from: UInt32(p), to: UInt32(a.count - q), text: text)]
    }
}

/// A key a view hands the open region, when leaf runs its session: one of a
/// set of named keys that grows. Not an enum, so a key added later breaks no
/// host: a session matches the keys it knows (`case .left:`) and answers
/// every other with `.pass` (`default:`), which closes the region and lets
/// the key act on the document.
public struct RegionInput: Hashable, CustomStringConvertible {
    /// The key's name, as the web editor names it too.
    public let name: String

    public init(_ name: String) { self.name = name }

    public var description: String { name }

    public static let left = RegionInput("ArrowLeft")
    public static let right = RegionInput("ArrowRight")
    public static let up = RegionInput("ArrowUp")
    public static let down = RegionInput("ArrowDown")
    /// ⇧← ⇧→ ⇧↑ ⇧↓: extend the selection.
    public static let extendLeft = RegionInput("ExtendLeft")
    public static let extendRight = RegionInput("ExtendRight")
    public static let extendUp = RegionInput("ExtendUp")
    public static let extendDown = RegionInput("ExtendDown")
    /// ⌥← ⌥→: a word back or on.
    public static let wordLeft = RegionInput("WordLeft")
    public static let wordRight = RegionInput("WordRight")
    /// ⌘← ⌘→: the line's start or end.
    public static let lineStart = RegionInput("LineStart")
    public static let lineEnd = RegionInput("LineEnd")
    /// Home and End, ⌘↑ ⌘↓: the region's first stop or last.
    public static let home = RegionInput("Home")
    public static let end = RegionInput("End")
    public static let backspace = RegionInput("Backspace")
    /// Forward delete — and what deletes a selection a cut copied.
    public static let deleteForward = RegionInput("Delete")
    /// Select All, of the region alone.
    public static let selectAll = RegionInput("SelectAll")
    /// Escape.
    public static let cancel = RegionInput("Escape")
    /// Return.
    public static let newline = RegionInput("Enter")
    /// Tab, or Shift-Tab.
    public static let tab = RegionInput("Tab")
}

/// What a session did with a key, typing or a click.
public enum RegionOutcome {
    /// It wrote these splices on its content, as one undo step.
    case edited([RegionSplice])
    /// Only what it draws changed — its caret moved.
    case moved
    /// Close it, the caret in front of the region or behind it.
    case leave(atEnd: Bool)
    /// Not its to take: close it where the caret is, and let the key run.
    case pass
}

/// A region open for editing. A session that leaf runs answers `input`,
/// `type` and `click`, and is drawn by leaf; one a host runs has a `view`,
/// placed over the region and given the focus, and writes back through the
/// `RegionHost` it was opened with. Every member has a default, so a host's
/// session states only what it does.
public protocol RegionSession: AnyObject {
    /// The view placed over the region's rows while it is open, in place of
    /// its drawing. `nil` for a session leaf draws.
    var view: LeafView? { get }
    /// Take the focus, once `view` is in place.
    func focus()
    /// A key, when leaf has the focus.
    func input(_ input: RegionInput) -> RegionOutcome
    /// Typed text, when leaf has the focus.
    func type(_ text: String) -> RegionOutcome
    /// A click on the open region's drawing, in points from its top left.
    func click(at point: CGPoint, size: CGFloat) -> RegionOutcome
    /// What a copy takes from the region — its selection, as the content's
    /// own text — or nil when nothing is selected.
    func copy() -> String?
    /// Text pasted into the region.
    func paste(_ text: String) -> RegionOutcome
    /// The content changed under the session — an undo, another writer.
    /// `false` closes it.
    func reload(content: String) -> Bool
    /// The region closed.
    func close()
    /// What assistive technology reads for the open region; nil for its
    /// content as written.
    var accessibilityValue: String? { get }
}

public extension RegionSession {
    var accessibilityValue: String? { nil }
    var view: LeafView? { nil }
    func focus() {}
    func input(_ input: RegionInput) -> RegionOutcome { .pass }
    func type(_ text: String) -> RegionOutcome { .pass }
    func click(at point: CGPoint, size: CGFloat) -> RegionOutcome { .moved }
    func copy() -> String? { nil }
    func paste(_ text: String) -> RegionOutcome { .pass }
    func reload(content: String) -> Bool { true }
    func close() {}
}

/// What a session leaf draws shows: its picture, and where its caret and
/// selection stand on it, in points from its top left. Leaf draws the caret
/// and the selection, with its own blink and colours, over the picture —
/// the picture has neither.
public struct RegionSessionPicture {
    public var picture: RegionPicture
    /// The caret: a rect at the stop, as tall as the line it is on; its
    /// width is not read. Nil hides it.
    public var caret: CGRect?
    /// The selection, as rects.
    public var selection: [CGRect]

    public init(picture: RegionPicture, caret: CGRect?, selection: [CGRect] = []) {
        self.picture = picture
        self.caret = caret
        self.selection = selection
    }
}

/// A session leaf drives with keys and draws as a picture — the only kind
/// that can stand in a line of text, and leaf's own formula editor's kind.
/// `RegionEditor.open` may answer with one for any region, inline or block.
/// Leaf asks for its picture on every keystroke, on the main thread, so it
/// must be cheap; a kind whose drawing is slow edits through a view. Typed
/// text arrives through `type`, committed: leaf shows an input method's
/// composition itself, at the caret.
public protocol RegionPictureSession: RegionSession {
    /// The region as it stands, at `size` points per em in `ink`.
    func picture(size: CGFloat, ink: LeafColor) -> RegionSessionPicture?
}

/// The way back from a host's session to the view that opened it. Every call
/// is ignored once the region has closed.
public final class RegionHost {
    fileprivate var onEdit: ([RegionSplice]) -> Void = { _ in }
    fileprivate var onContent: () -> String? = { nil }
    fileprivate var onLeave: (Bool) -> Void = { _ in }
    fileprivate var onHistory: (Bool) -> Void = { _ in }
    fileprivate var onResize: () -> Void = {}
    fileprivate var onCaretRect: () -> CGRect? = { nil }
    fileprivate var onRegionRect: () -> CGRect? = { nil }

    init() {}

    /// The text view the region is open in: what `caretRect()` and
    /// `regionRect()` are in the coordinates of, and what a popover is
    /// anchored to.
    public internal(set) weak var view: LeafView?

    /// Where the caret is drawn, in the text view's coordinates — for a
    /// popover the session anchors at it. Nil while it is not drawn.
    public func caretRect() -> CGRect? { onCaretRect() }

    /// Where the region is drawn, in the text view's coordinates.
    public func regionRect() -> CGRect? { onRegionRect() }

    /// Write splices on the content, as one undo step.
    public func edit(_ splices: [RegionSplice]) { onEdit(splices) }

    /// Write the whole content anew: the view splices only what differs, as
    /// one undo step. A fence's content is whole lines, each ending in a
    /// newline (empty for an empty fence) — keep it so.
    public func replace(_ content: String) {
        guard let before = onContent() else { return }
        onEdit(RegionSplice.diff(before, content))
    }

    /// Close the region, the caret in front of it or behind it, and give the
    /// focus back to the document.
    public func leave(atEnd: Bool) { onLeave(atEnd) }

    /// Undo or redo in the document. The region stays open, and a change to its
    /// content reaches the session as `reload`.
    public func undo() { onHistory(false) }
    public func redo() { onHistory(true) }

    /// The session's view wants a different height: lay the document out again.
    public func resized() { onResize() }
}

/// An editor for one kind of region — a fence of one language, a directive of
/// one name, or formulas, which leaf edits with an editor of its own a host
/// may replace. Its session either stands a view of its own over a block, or
/// is a `RegionPictureSession` leaf drives and draws, inline or block. While
/// the region is closed it is its kind's renderer's picture
/// (`regionRenderers`), or leaf's placeholder row where it has none.
public struct RegionEditor {
    /// Open `region`, or answer nil to leave it closed.
    public var open: (RegionView, RegionEntry, RegionHost) -> RegionSession?

    public init(open: @escaping (RegionView, RegionEntry, RegionHost) -> RegionSession?) {
        self.open = open
    }
}

/// The box the open region's view occupies when a host's session has one:
/// the column's width less the row's prefix, and the height the view asks for,
/// with a picture's breathing room above and below.
struct HostedLayout {
    let size: CGSize

    /// Breathing room above and below — a picture's.
    static let gap: CGFloat = MediaMetrics.gap

    /// The height the row reserves.
    var height: CGFloat { size.height + Self.gap * 2 }

    /// The view's frame in layout coordinates, given the top of the reserved
    /// row and the left edge of its content.
    func rect(top: CGFloat, left: CGFloat) -> CGRect {
        CGRect(x: left, y: top + Self.gap, width: size.width, height: size.height)
    }
}

/// Asked for the height the open region's view wants at a column `width`
/// points wide, or nil to leave the region as it stands closed.
/// `RegionViews.height` in the views.
typealias RegionMeasure = (RegionView, CGFloat) -> CGFloat?

/// What the layout needs of the open region: which it is, a revision that
/// changes with whatever it draws, and — for a session leaf draws — its
/// picture.
struct OpenRegionLayout {
    let kind: String
    let src: UInt32
    let revision: Int
    /// The session's picture, for a session leaf draws; nil for one whose
    /// view stands over the region.
    let picture: ((CGFloat, LeafColor) -> RegionPicture?)?

    /// Whether two layouts of the open region draw the same.
    func same(as other: OpenRegionLayout?) -> Bool {
        guard let other else { return false }
        return kind == other.kind && src == other.src && revision == other.revision
    }
}

/// The region one view has open: its session, and where the region is,
/// followed from frame to frame.
final class OpenRegion {
    let kind: String
    let session: RegionSession
    let host: RegionHost
    /// Core's caret waits here, in front of the region, while it is open.
    /// Followed from frame to frame, since an undo above the region moves it.
    private(set) var src: UInt32
    /// Which region of its kind this is, counting from the top: how it is
    /// found again when an edit above it has moved its start.
    private var ordinal: Int
    /// How many regions of its kind the frame had.
    private var siblings: Int
    private(set) var end: UInt32
    private(set) var contentStart: UInt32
    /// The content as the view last saw it, or wrote it.
    private(set) var content: String
    /// Bumped by every change to what the region shows.
    private(set) var revision = 1

    init(_ region: RegionView, in frame: DocView, session: RegionSession, host: RegionHost) {
        kind = region.kind
        let alike = frame.regions.filter { $0.kind == region.kind }
        ordinal = alike.firstIndex { $0.src == region.src } ?? 0
        siblings = alike.count
        self.session = session
        self.host = host
        src = region.src
        end = region.end
        contentStart = region.contentStart
        content = region.content
    }

    /// The session's own view, for a host's session.
    var view: LeafView? { session.view }

    /// The session's last picture, at the size it was asked for and the
    /// revision it showed — where the caret and selection are drawn from.
    private(set) var shown: (size: CGFloat, revision: Int, picture: RegionSessionPicture)?

    var layout: OpenRegionLayout {
        guard let s = session as? RegionPictureSession else {
            return OpenRegionLayout(kind: kind, src: src, revision: revision, picture: nil)
        }
        let revision = self.revision
        return OpenRegionLayout(kind: kind, src: src, revision: revision) { [weak self] size, ink in
            guard let drawn = s.picture(size: size, ink: ink) else { return nil }
            self?.shown = (size, revision, drawn)
            return drawn.picture
        }
    }

    /// The caret and selection of the picture last laid out, if it showed the
    /// region as it stands.
    var marks: RegionSessionPicture? {
        guard let shown, shown.revision == revision else { return nil }
        return shown.picture
    }

    /// Follow the region into `frame`. `false` when the frame has no region
    /// here any more, or one whose new content the session refuses.
    func follow(_ frame: DocView) -> Bool {
        // By its place among regions of its kind while there are as many of
        // them as before — an edit above moves its start, not its place — and
        // by its start once one came or went.
        let alike = frame.regions.filter { $0.kind == kind }
        let found = alike.count == siblings && alike.indices.contains(ordinal)
            ? alike[ordinal] : alike.first { $0.src == src }
        guard let r = found else { return false }
        ordinal = alike.firstIndex { $0.src == r.src } ?? ordinal
        siblings = alike.count
        src = r.src
        end = r.end
        contentStart = r.contentStart
        if !r.content.utf8.elementsEqual(content.utf8) {
            content = r.content
            revision += 1
            return session.reload(content: r.content)
        }
        return true
    }

    /// Apply `splices` to `doc` as one undo step, and answer the frame with
    /// core's caret back in front of the region.
    func apply(_ splices: [RegionSplice], in doc: LeafDoc) -> DocView {
        revision += 1
        // Every splice is against the content as the one before left it, and
        // none moves the content's start.
        doc.beginUndoGroup()
        for s in splices {
            _ = doc.editSource(from: contentStart + s.from, to: contentStart + s.to, text: s.text)
            content = content.replacingUTF8(Int(s.from)..<Int(s.to), with: s.text)
        }
        doc.endUndoGroup()
        return doc.selectRange(start: src, end: src)
    }

    /// Something only the session draws changed.
    func moved() { revision += 1 }
}

extension String {
    /// This string with its UTF-8 bytes in `range` replaced by `text`.
    func replacingUTF8(_ range: Range<Int>, with text: String) -> String {
        let b = Array(utf8)
        let lo = min(range.lowerBound, b.count)
        let hi = min(max(range.upperBound, lo), b.count)
        return String(decoding: b[..<lo], as: UTF8.self) + text + String(decoding: b[hi...], as: UTF8.self)
    }
}

extension DocView {
    /// The region of `kind` starting at `src`.
    func region(_ kind: String, at src: UInt32) -> RegionView? {
        regions.first { $0.kind == kind && $0.src == src }
    }
}

/// The open session's view, when a host's session has one: placed over its
/// region's box on every frame that lays out.
final class RegionViews {
    /// The open session's view, which stands where its region's picture was.
    var open: LeafView?

    var isEmpty: Bool { open == nil }

    /// The height the open session's view wants at `width`.
    func height(width: CGFloat) -> CGFloat? {
        open.map { Self.measure($0, width: width) }
    }

    /// The measure the layout is handed, or nil when no view is open.
    var measure: RegionMeasure? {
        guard open != nil else { return nil }
        return { [weak self] _, width in self?.height(width: width) }
    }

    /// Put the open view on its rect, or take it away when it has none.
    func place(_ rect: CGRect?, in container: LeafView, scale: CGFloat = 1) {
        guard let open else { return }
        if let rect {
            Self.put(open, on: rect, in: container, scale: scale)
        } else {
            open.removeFromSuperview()
        }
    }

    /// Take the open session's view away.
    func closeOpen() {
        open?.removeFromSuperview()
        open = nil
    }

    /// Put `view` on `rect`, in layout coordinates, in `container` — behind
    /// its other subviews the first time, so the system's caret stands in
    /// front of a view it is drawn at the edge of. `scale` is how many view
    /// points a layout point is on a surface that zooms by scaling its drawing
    /// rather than itself (the AppKit view), and 1 on one whose own transform
    /// does the scaling (the UIKit view).
    static func put(_ view: LeafView, on rect: CGRect, in container: LeafView, scale: CGFloat) {
        if view.superview !== container {
            #if canImport(UIKit)
            container.insertSubview(view, at: 0)
            #else
            container.addSubview(view, positioned: .below, relativeTo: nil)
            #endif
        }
        #if canImport(UIKit)
        view.frame = rect
        #else
        view.frame = rect.applying(CGAffineTransform(scaleX: scale, y: scale))
        // Scaled by its bounds, so what it draws zooms with the text around it
        // rather than being re-laid-out at a size it never asked for.
        view.setBoundsSize(rect.size)
        #endif
    }

    /// The height `view` asks for at a column `width` wide: its intrinsic
    /// height where it has one, its fitting height at that width where it has
    /// not, and its frame's height where it has neither — a plain view the host
    /// sized by hand.
    static func measure(_ view: LeafView, width: CGFloat) -> CGFloat {
        #if canImport(UIKit)
        let intrinsic = view.intrinsicContentSize.height
        if intrinsic != UIView.noIntrinsicMetric, intrinsic > 0 { return intrinsic }
        let fitted = view.systemLayoutSizeFitting(
            CGSize(width: width, height: UIView.layoutFittingCompressedSize.height),
            withHorizontalFittingPriority: .required,
            verticalFittingPriority: .fittingSizeLevel).height
        if fitted > 0 { return fitted }
        return max(0, view.frame.height)
        #else
        let intrinsic = view.intrinsicContentSize.height
        if intrinsic != NSView.noIntrinsicMetric, intrinsic > 0 { return intrinsic }
        // A width to fit at, for a view whose height follows from it. Only where
        // the view lays itself out by constraints: one sized by its autoresizing
        // mask has no fitting size to ask for.
        var fitted: CGFloat = 0
        if !view.translatesAutoresizingMaskIntoConstraints {
            let pin = view.widthAnchor.constraint(equalToConstant: width)
            pin.isActive = true
            fitted = view.fittingSize.height
            pin.isActive = false
        } else {
            fitted = view.fittingSize.height
        }
        if fitted > 0 { return fitted }
        return max(0, view.frame.height)
        #endif
    }
}

extension RegionHost {
    /// Wire a host to the view that opened `region`: each call reaches the
    /// view only while `isCurrent` says this region is still the open one.
    func wire(isCurrent: @escaping () -> Bool,
              content: @escaping () -> String?,
              edit: @escaping ([RegionSplice]) -> Void,
              leave: @escaping (Bool) -> Void,
              history: @escaping (Bool) -> Void,
              resize: @escaping () -> Void,
              caretRect: @escaping () -> CGRect?,
              regionRect: @escaping () -> CGRect?) {
        onContent = { isCurrent() ? content() : nil }
        onEdit = { if isCurrent() { edit($0) } }
        onLeave = { if isCurrent() { leave($0) } }
        onHistory = { if isCurrent() { history($0) } }
        onResize = { if isCurrent() { resize() } }
        onCaretRect = { isCurrent() ? caretRect() : nil }
        onRegionRect = { isCurrent() ? regionRect() : nil }
    }
}

extension EditorLayout {
    /// The open region's view's rect, when a host's view stands over it.
    func openRegionRect() -> CGRect? {
        rows.lazy.compactMap(\.hostedRect).first
    }

    /// Every block region's picture rect, by the region's kind and start.
    func pictureRects() -> [String: CGRect] {
        var out: [String: CGRect] = [:]
        for rl in rows where rl.pictureFirst {
            guard let box = rl.picture else { continue }
            out["\(box.region.kind)@\(box.region.src)"] = box.rect(
                top: rl.pictureTop, left: rl.originX + rl.shaped.prefixWidth,
                width: rl.columnWidth - rl.shaped.prefixWidth)
        }
        return out
    }

    /// Every region's picture as drawn — a block's box, or an inline
    /// region's attachment, a formula's or a directive's — with its rect and
    /// the size it is set at, in points per em, and how much smaller than its
    /// own size it is drawn (a block fitted to a narrow column).
    func pictureFrames(theme: EditorTheme) -> [(region: RegionView, rect: CGRect, size: CGFloat, scale: CGFloat)] {
        var out: [(RegionView, CGRect, CGFloat, CGFloat)] = []
        var inline: [UInt32: RegionView] = [:]
        for m in math where m.inline {
            inline[m.src] = regions.first { $0.kind == MathRenderer.kind && $0.src == m.src } ?? DocView.standIn(for: m)
        }
        for r in regions where r.inline && r.kind != MathRenderer.kind { inline[r.src] = r }
        for (i, rl) in rows.enumerated() {
            if rl.pictureFirst, let box = rl.picture {
                let r = box.rect(top: rl.pictureTop, left: rl.originX + rl.shaped.prefixWidth,
                                 width: rl.columnWidth - rl.shaped.prefixWidth)
                out.append((box.region, r, theme.fontSize * box.scale, box.scale))
                continue
            }
            guard rl.drawsItsGlyphs, !inline.isEmpty else { continue }
            let size = rl.row.heading.map { theme.headingSize(Int($0)) } ?? theme.fontSize
            var ch = 0
            for run in rl.row.runs {
                defer { ch += run.text.utf16.count }
                guard run.role == "math" || run.role == "image", run.text.utf16.count == 1,
                      let region = inline[run.src],
                      let picture = drawing.picture(region, size: size, ink: theme.textColor),
                      let base = baselineOrigin(row: i, ch: ch)
                else { continue }
                out.append((region, CGRect(x: base.x, y: base.y - picture.baseline, width: picture.width ?? 0,
                                           height: picture.height), size, 1))
            }
        }
        return out
    }

    /// The region whose picture is at `point`, with that picture's rect and
    /// the size it is set at, so a click can be handed to a session.
    func regionPicture(at point: CGPoint, theme: EditorTheme) -> (region: RegionView, rect: CGRect, size: CGFloat)? {
        pictureFrames(theme: theme).first { $0.rect.contains(point) }.map { ($0.region, $0.rect, $0.size) }
    }

    /// The picture of region `kind` at `src` as drawn.
    func pictureFrame(kind: String, src: UInt32, theme: EditorTheme)
        -> (region: RegionView, rect: CGRect, size: CGFloat, scale: CGFloat)? {
        pictureFrames(theme: theme).first { $0.region.kind == kind && $0.region.src == src }
    }
}

/// What a text view lends its `RegionSlot`: the document, the frame it shows,
/// and the few things only the view can do.
protocol RegionSlotOwner: AnyObject {
    var doc: LeafDoc { get }
    /// The frame on screen.
    var shownFrame: DocView { get }
    /// The layout on screen, and the theme it was laid out in.
    var regionLayout: EditorLayout { get }
    var regionTheme: EditorTheme { get }
    /// A rect in layout coordinates in the view's own.
    func regionViewRect(_ rect: CGRect) -> CGRect
    /// The open region's caret, selection or composition moved: draw them
    /// again, and tell the input system.
    func regionMarksChanged()
    /// Say `value` to assistive technology: the open region's, after it
    /// opened or changed.
    func regionAnnounce(_ value: String)
    func regionRender(_ frame: DocView)
    /// Give the focus back to the document, from a host's view.
    func focusDocument()
    /// Undo or redo through the view's history.
    func regionHistory(redo: Bool)
    /// Lay the document out again, at the same frame.
    func relayout()
    /// Pictures of these kinds, drawn later, have arrived: lay their regions
    /// out again.
    func regionPicturesArrived(_ kinds: Set<String>)
}

/// One view's open region and its routing — the slot the AppKit and UIKit
/// views each own, so that opening, leaving, keys, typing, clicks, undo and
/// following a region from frame to frame are written once for every kind.
final class RegionSlot {
    weak var owner: RegionSlotOwner?
    /// The host's region editors, by kind. One for `math` replaces leaf's.
    var editors: [String: RegionEditor] = [:] {
        didSet { close(repaint: false) }
    }

    /// Leaf's own editors: one for each kind its Rust editors take — `math`,
    /// through the same door as a host's.
    static let leafEditors: [String: RegionEditor] = Dictionary(
        uniqueKeysWithValues: LeafRegionSession.kinds.map { kind in
            (kind, RegionEditor { region, entry, _ in LeafRegionSession(region, entry: entry) })
        })

    /// The editor for `kind`: the host's, else leaf's — leaf's `math` editor
    /// only while formulas are edited in place.
    func editor(for kind: String) -> RegionEditor? {
        if let host = editors[kind] { return host }
        if kind == MathRenderer.kind, !editsMath { return nil }
        return Self.leafEditors[kind]
    }
    /// The renderers the view draws its regions with, and their pictures —
    /// a sheet over the same document shares its view's.
    private(set) var pictures = RegionPictures()
    /// Whether to wait for pictures drawn later rather than stand in for
    /// them: a paper sheet's.
    var waitsForPictures = false
    /// Whether formulas are edited in place, by leaf's own session.
    var editsMath = true {
        didSet { if !editsMath, open?.kind == MathRenderer.kind { close(repaint: false) } }
    }
    let views = RegionViews()
    private(set) var open: OpenRegion?

    /// The kinds the view owns, for `setOwnedRegions`: `math` while formulas
    /// are edited in place — owning it keeps a formula on the caret's line a
    /// picture — and every other kind with a renderer or an editor.
    var ownedKinds: [String] {
        let others = Set(pictures.renderers.keys).union(editors.keys).subtracting([MathRenderer.kind])
        return (editsMath ? [MathRenderer.kind] : []) + others.sorted()
    }

    /// Whether regions of `kind` open for editing: whether it has an editor.
    func edits(_ kind: String) -> Bool { editor(for: kind) != nil }

    /// Whether a region is open, so the view's own caret stands aside.
    var isOpen: Bool { open != nil }

    /// Open `region` with the caret at `entry`. `false` when there is no
    /// editor for it, or the editor declined.
    @discardableResult
    func open(_ region: RegionView, entry: RegionEntry) -> Bool {
        guard let owner, let editor = editor(for: region.kind), owner.shownFrame.view != "source"
        else { return false }
        close(repaint: false)
        let host = RegionHost()
        host.view = owner as? LeafView
        guard let session = editor.open(region, entry, host) else { return false }
        // A view cannot stand in a line of text: an inline region is edited
        // only by a session leaf draws.
        if region.inline, session.view != nil || !(session is RegionPictureSession) {
            session.close()
            return false
        }
        let opened = OpenRegion(region, in: owner.shownFrame, session: session, host: host)
        host.wire(
            isCurrent: { [weak self, weak opened] in opened != nil && self?.open === opened },
            content: { [weak opened] in opened?.content },
            edit: { [weak self] in
                // A host's own write is a step of its own, not more typing.
                self?.endTyping()
                self?.edit($0)
            },
            leave: { [weak self] in self?.leave(atEnd: $0) },
            history: { [weak self] in
                self?.endTyping()
                self?.owner?.regionHistory(redo: $0)
            },
            resize: { [weak self] in self?.owner?.relayout() },
            caretRect: { [weak self] in self?.caretRect().flatMap { r in self?.owner?.regionViewRect(r) } },
            regionRect: { [weak self] in self?.regionRect().flatMap { r in self?.owner?.regionViewRect(r) } })
        open = opened
        views.open = session.view
        owner.regionRender(owner.doc.selectRange(start: region.src, end: region.src))
        session.focus()
        announce()
        return true
    }

    /// → in front of a region, or ← behind one, opens it at its first stop or
    /// its last. Whether it did.
    func enter(right: Bool) -> Bool {
        guard let owner, open == nil else { return false }
        let frame = owner.shownFrame
        guard !frame.hasSelection, frame.view != "source" else { return false }
        let caret = owner.doc.caretOffset()
        guard let r = frame.regions.first(where: { edits($0.kind) && (right ? $0.src == caret : $0.end == caret) })
        else { return false }
        return open(r, entry: right ? .home : .end)
    }

    /// Whether `right` (→) or ← at the caret would open a region — for a view
    /// that takes the arrows from the system only where it must.
    func wouldEnter(right: Bool) -> Bool {
        guard let owner, open == nil else { return false }
        let frame = owner.shownFrame
        guard !frame.hasSelection, frame.view != "source" else { return false }
        let caret = owner.doc.caretOffset()
        return frame.regions.contains { edits($0.kind) && (right ? $0.src == caret : $0.end == caret) }
    }

    /// A key for the open region, when leaf runs its session. Whether it was
    /// taken; a key it passes on closes the region first.
    /// A key reaching the view while a host's view is open — its own focus
    /// was lost — closes the region before it acts on the document.
    func input(_ input: RegionInput) -> Bool {
        guard let open else { return false }
        guard open.view == nil else {
            close()
            return false
        }
        endTyping()
        return act(open.session.input(input))
    }

    /// Typed text for the open region, when leaf runs its session; text
    /// reaching the view while a host's view is open closes it, as a key does.
    func type(_ text: String) -> Bool {
        guard let open else { return false }
        guard open.view == nil else {
            close()
            return false
        }
        marked = nil
        let outcome = open.session.type(text)
        // Typing runs on as one undo step, as typing in the document does:
        // the group opened by the first edit stays open until another key,
        // a click or leaving closes it.
        if case .edited = outcome, !typing, let doc = owner?.doc {
            doc.beginUndoGroup()
            typing = true
        }
        return act(outcome)
    }

    /// Whether a run of typing is open as one undo step.
    private var typing = false

    /// Close the run of typing, if one is open: the next edit is a step of
    /// its own.
    func endTyping() {
        guard typing else { return }
        typing = false
        owner?.doc.endUndoGroup()
    }

    // MARK: composition

    /// An input method's composition in the open region leaf draws: shown by
    /// leaf at the caret, over the picture, and handed to the session only
    /// once committed, through `type`.
    private(set) var marked: (text: String, selected: NSRange)? {
        didSet { owner?.regionMarksChanged() }
    }

    /// Whether the open region takes a composition: it is one leaf draws.
    var composes: Bool { open.map { $0.view == nil } ?? false }

    /// Set the composition, or clear it with empty text.
    func setMarked(_ text: String, selected: NSRange) {
        guard composes else { return }
        marked = text.isEmpty ? nil : (text, selected)
    }

    /// Commit the composition as typed, as an input method does when the
    /// focus moves.
    func commitMarked() {
        guard let text = marked?.text else { return }
        marked = nil
        _ = type(text)
    }

    // MARK: where it is drawn

    /// The open region's picture as drawn, in layout coordinates.
    func regionRect() -> CGRect? {
        guard let owner, let open else { return nil }
        if open.view != nil { return owner.regionLayout.openRegionRect() }
        return owner.regionLayout.pictureFrame(kind: open.kind, src: open.src, theme: owner.regionTheme)?.rect
    }

    /// The open picture session's caret, in layout coordinates: its rect on
    /// the picture, where the picture is drawn, two points wide. Nil for a
    /// session with a view, or one that hides its caret.
    func caretRect() -> CGRect? {
        guard let (frame, marks) = placedMarks(), let caret = marks.caret else { return nil }
        let w: CGFloat = 2
        let x = frame.rect.minX + caret.minX * frame.scale
        // Inside the picture, so a caret at either end is not clipped.
        let clamped = min(max(x - w / 2, frame.rect.minX), max(frame.rect.minX, frame.rect.maxX - w))
        return CGRect(x: clamped, y: frame.rect.minY + caret.minY * frame.scale,
                      width: w, height: caret.height * frame.scale)
    }

    /// The open picture session's selection, in layout coordinates.
    func selectionRects() -> [CGRect] {
        guard let (frame, marks) = placedMarks() else { return [] }
        return marks.selection.map { r in
            CGRect(x: frame.rect.minX + r.minX * frame.scale, y: frame.rect.minY + r.minY * frame.scale,
                   width: r.width * frame.scale, height: r.height * frame.scale)
        }
    }

    private func placedMarks() -> ((rect: CGRect, scale: CGFloat), RegionSessionPicture)? {
        guard let owner, let open, open.view == nil, let marks = open.marks,
              let frame = owner.regionLayout.pictureFrame(kind: open.kind, src: open.src, theme: owner.regionTheme)
        else { return nil }
        return ((frame.rect, frame.scale), marks)
    }

    // MARK: accessibility

    /// What assistive technology reads for the open region: its session's
    /// value, else its content.
    var accessibilityValue: String? {
        guard let open else { return nil }
        return open.session.accessibilityValue ?? open.content
    }

    private func announce() {
        if let value = accessibilityValue { owner?.regionAnnounce(value) }
    }

    /// A click at `point` in layout coordinates: on a region's picture it
    /// opens that region, or moves the caret in the one open; anywhere else it
    /// closes the open region and is left to run. Whether the click was a
    /// region's.
    func click(at point: CGPoint, in layout: EditorLayout, theme: EditorTheme) -> Bool {
        guard let owner, owner.shownFrame.view != "source" else { return false }
        endTyping()
        commitMarked()
        var hit: (RegionView, RegionEntry, CGPoint, CGFloat)?
        if let p = layout.regionPicture(at: point, theme: theme), edits(p.region.kind),
           let r = owner.shownFrame.region(p.region.kind, at: p.region.src) {
            let local = CGPoint(x: point.x - p.rect.minX, y: point.y - p.rect.minY)
            hit = (r, .point(local, size: p.size), local, p.size)
        }
        guard let (region, entry, local, size) = hit else {
            close()
            return false
        }
        if let open, open.kind == region.kind, open.src == region.src {
            return act(open.session.click(at: local, size: size))
        }
        return self.open(region, entry: entry)
    }

    /// What Copy takes from the open region leaf runs, or nil — the view's
    /// own Copy acts then.
    var copyText: String? {
        guard let open, open.view == nil else { return nil }
        return open.session.copy()
    }

    /// Cut from the open region leaf runs: what it copied, after deleting it,
    /// or nil when there was nothing selected there.
    func cut() -> String? {
        guard let text = copyText else { return nil }
        _ = input(.deleteForward)
        return text
    }

    /// Paste into the open region leaf runs. Whether it was taken.
    func paste(_ text: String) -> Bool {
        guard let open, open.view == nil else { return false }
        endTyping()
        return act(open.session.paste(text))
    }

    /// Follow the open region into `frame`, closing it when the frame has
    /// none at its place. Called by the view before it lays a frame out.
    func follow(_ frame: DocView) {
        guard let open, !open.follow(frame) else { return }
        close(repaint: false)
    }

    /// What the layout needs of the open region.
    var layout: OpenRegionLayout? { open?.layout }

    /// What the layout draws the regions with, in an appearance dark or not,
    /// on a screen of `scale`.
    func drawing(dark: Bool, scale: CGFloat) -> RegionDrawing {
        RegionDrawing(pictures: pictures, open: layout, dark: dark, waits: waitsForPictures, scale: scale)
    }

    /// Close the open region with the caret in front of it or behind it.
    func leave(atEnd: Bool) {
        guard let owner, let open else { return }
        close(repaint: false)
        let at = atEnd ? open.end : open.src
        owner.regionRender(owner.doc.selectRange(start: at, end: at))
    }

    /// Close the open region where the caret is, for a command or a click
    /// that goes on to act elsewhere.
    func close(repaint: Bool = true) {
        guard let open else { return }
        endTyping()
        if marked != nil { marked = nil }
        self.open = nil
        let hadView = open.view != nil
        views.closeOpen()
        open.session.close()
        if hadView { owner?.focusDocument() }
        if repaint, let owner { owner.regionRender(owner.shownFrame) }
    }

    private func edit(_ splices: [RegionSplice]) {
        guard let owner, let open else { return }
        // A fence's content cannot hold a line that would close the fence: it
        // would end the region early and turn the rest of the document into
        // code. Such an edit is refused, and the session shown the content as
        // it stands.
        if open.kind.hasPrefix("fence:") {
            let next = splices.reduce(open.content) { $0.replacingUTF8(Int($1.from)..<Int($1.to), with: $1.text) }
            if Self.closesFence(next) {
                _ = open.session.reload(content: open.content)
                return
            }
        }
        if splices.isEmpty {
            open.moved()
            owner.regionRender(owner.shownFrame)
        } else {
            owner.regionRender(open.apply(splices, in: owner.doc))
            announce()
        }
        owner.regionMarksChanged()
    }

    /// Whether fence content holds a line that would close a fence: three or
    /// more backticks or tildes, indented at most three spaces.
    static func closesFence(_ content: String) -> Bool {
        content.split(separator: "\n", omittingEmptySubsequences: false).contains { line in
            let indent = line.prefix { $0 == " " }.count
            let rest = line.dropFirst(indent)
            return indent <= 3 && (rest.hasPrefix("```") || rest.hasPrefix("~~~"))
        }
    }

    /// Do what the session answered. Whether the key or click was taken.
    @discardableResult
    private func act(_ outcome: RegionOutcome) -> Bool {
        switch outcome {
        case let .edited(splices): edit(splices)
        case .moved: edit([])
        case let .leave(atEnd): leave(atEnd: atEnd)
        case .pass:
            close()
            return false
        }
        return true
    }
}

/// The region kinds a host may register a renderer or an editor for.
enum RegionKinds {
    /// A kind a host may register a renderer for: `math`, `fence:` and a
    /// language, or `directive:` and a name — but not `directive:page-break`,
    /// whose name is leaf's.
    static func hostable(_ kind: String) -> Bool {
        kind == MathRenderer.kind || block(kind)
    }

    /// A kind a host may register an editor for: any it may draw, `math`
    /// included, which replaces leaf's own formula editor.
    static func editable(_ kind: String) -> Bool { hostable(kind) }

    private static func block(_ kind: String) -> Bool {
        for prefix in ["fence:", "directive:"] where kind.hasPrefix(prefix) {
            let rest = kind.dropFirst(prefix.count)
            return !rest.isEmpty && !rest.contains(where: \.isWhitespace)
                && kind != "directive:" + EditorLayout.pageBreakDirective
        }
        return false
    }
}

/// What a view draws and edits its regions with: handed to a second view over
/// the same document — a paper sheet — which must own the same kinds, since
/// the kinds are the document's to know.
struct RegionSetup {
    var renderers: [String: RegionRenderer] = [:]
    var editors: [String: RegionEditor] = [:]
    var editsMath = true
    /// The pictures already drawn, and the renderers: a sheet shares its
    /// view's, so what the screen drew is not drawn again.
    var pictures: RegionPictures?
    /// Wait for pictures drawn later: paper never draws a placeholder for one.
    var waits = false

    /// This setup for a paper sheet.
    var onPaper: RegionSetup {
        var setup = self
        setup.waits = true
        return setup
    }
}

extension RegionSlot {
    /// This slot's setup, for a sheet over the same document.
    var setup: RegionSetup {
        RegionSetup(renderers: pictures.renderers, editors: editors, editsMath: editsMath, pictures: pictures)
    }

    /// Take `setup` on, before the view tells core what it owns.
    func adopt(_ setup: RegionSetup) {
        if let shared = setup.pictures {
            pictures = shared
        } else {
            pictures.renderers = setup.renderers
        }
        editors = setup.editors
        editsMath = setup.editsMath
        waitsForPictures = setup.waits
    }

    /// Have arrived pictures reach `owner`. A paper sheet sharing a screen
    /// view's pictures leaves them to that view: it waits for its own and
    /// never needs telling. A screen view sharing them takes them over — it
    /// is the view a host made for the model again, and the one it replaces
    /// is on its way out, so a picture that arrived would otherwise be laid
    /// out nowhere and the region stay a placeholder.
    func listen() {
        if waitsForPictures, pictures.onArrive != nil { return }
        pictures.onArrive = { [weak self] kinds in self?.owner?.regionPicturesArrived(kinds) }
    }
}
