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
//  Two kinds of session differ in who owns the focus while a region is open.
//  Leaf runs one that draws its own picture — a formula, whose session is
//  `MathRegionSession` over the Rust editor — passing it the keys and laying
//  out what it draws. A host's editor runs one with a view of its own, which
//  the view places over the region's rows, as it places a directive's view,
//  and gives the focus; that view writes back through its `RegionHost`.

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
        guard before != after else { return [] }
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

/// A key a view hands the open region, when leaf runs its session.
public enum RegionInput {
    case left, right, up, down
    /// ⇧← and ⇧→: extend the selection.
    case extendLeft, extendRight
    case backspace
    /// Forward delete — and what deletes a selection a cut copied.
    case deleteForward
    /// Select All, of the region alone.
    case selectAll
    /// Escape.
    case cancel
    /// Return.
    case newline
    /// Tab, or Shift-Tab.
    case tab
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
}

public extension RegionSession {
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

/// A session leaf draws itself: its picture is laid out where the region's
/// would be.
protocol RegionPictureSession: RegionSession {
    func picture(size: CGFloat, ink: LeafColor) -> MathGlyph?
}

/// The way back from a host's session to the view that opened it. Every call
/// is ignored once the region has closed.
public final class RegionHost {
    fileprivate var onEdit: ([RegionSplice]) -> Void = { _ in }
    fileprivate var onContent: () -> String? = { nil }
    fileprivate var onLeave: (Bool) -> Void = { _ in }
    fileprivate var onHistory: (Bool) -> Void = { _ in }
    fileprivate var onResize: () -> Void = {}

    init() {}

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

/// A host's editor for one kind of region — a fence of one language.
public struct RegionEditor {
    /// The region's drawing while it is closed, or nil for leaf's placeholder
    /// row. Asked once per region and kept while its content stands, as a
    /// directive's view is; sized the same way.
    public var view: ((RegionView) -> LeafView?)?
    /// Open `region`, or answer nil to leave it closed.
    public var open: (RegionView, RegionEntry, RegionHost) -> RegionSession?

    public init(view: ((RegionView) -> LeafView?)? = nil,
                open: @escaping (RegionView, RegionEntry, RegionHost) -> RegionSession?) {
        self.view = view
        self.open = open
    }
}

/// Which host view a box stands for: a directive's, or a region's.
enum HostedKey: Hashable {
    case directive(DirectiveKey)
    case region(RegionKey)
}

/// Which region a host's drawing stands for: its kind and content, and which
/// of several alike — or the open region, whose view rides its edits.
struct RegionKey: Hashable {
    let kind: String
    let content: String
    let occurrence: Int

    static let open = RegionKey(kind: "", content: "", occurrence: -1)
}

/// Asked for the height a region's view wants at a column `width` points wide,
/// or nil to leave it the placeholder. `RegionViews.height` in the views.
typealias RegionMeasure = (RegionView, RegionKey, CGFloat) -> CGFloat?

/// What the layout needs of the open region: which it is, a revision that
/// changes with whatever it draws, and — for a session leaf draws — its
/// picture.
struct OpenRegionLayout {
    let kind: String
    let src: UInt32
    let revision: Int
    let picture: ((CGFloat, LeafColor) -> MathGlyph?)?

    /// The picture for formula `math`: the open one's from its session, every
    /// other's from the typesetter.
    static func glyph(_ math: MathView, size: CGFloat, ink: LeafColor, open: OpenRegionLayout?) -> MathGlyph? {
        if let open, open.kind == MathRegionSession.kind, open.src == math.src, let picture = open.picture {
            return picture(size, ink)
        }
        return MathStore.glyph(tex: math.tex, display: math.display, size: size, ink: ink)
    }

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
    let src: UInt32
    private(set) var end: UInt32
    private(set) var contentStart: UInt32
    /// The content as the view last saw it, or wrote it.
    private(set) var content: String
    /// Bumped by every change to what the region shows.
    private(set) var revision = 1

    init(_ region: RegionView, session: RegionSession, host: RegionHost) {
        kind = region.kind
        self.session = session
        self.host = host
        src = region.src
        end = region.end
        contentStart = region.contentStart
        content = region.content
    }

    /// The session's own view, for a host's session.
    var view: LeafView? { session.view }

    var layout: OpenRegionLayout {
        let picture = (session as? RegionPictureSession).map { s in { (size: CGFloat, ink: LeafColor) in s.picture(size: size, ink: ink) } }
        return OpenRegionLayout(kind: kind, src: src, revision: revision, picture: picture)
    }

    /// Follow the region into `frame`. `false` when the frame has no region
    /// here any more, or one whose new content the session refuses.
    func follow(_ frame: DocView) -> Bool {
        guard let r = frame.regions.first(where: { $0.kind == kind && $0.src == src }) else { return false }
        end = r.end
        contentStart = r.contentStart
        if r.content != content {
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

/// The host's views for the regions in one text view: each editor's drawing of
/// a closed region, asked for once and kept while its content stands, and the
/// open session's view — placed over their boxes on every frame that lays out,
/// as `DirectiveHost` places a directive's.
final class RegionViews {
    /// The host's editors, by kind.
    var editors: [String: RegionEditor] = [:] {
        didSet { removeAll() }
    }
    /// The open session's view, which stands where its region's drawing was.
    var open: LeafView?

    private var answers: [RegionKey: LeafView?] = [:]

    var isEmpty: Bool { answers.isEmpty && open == nil }

    /// The height `region`'s view wants at `width`: the open session's, or the
    /// editor's drawing, asked for the first time it is wanted. Nil for a
    /// region with no editor, or a drawing the editor declined.
    func height(for region: RegionView, key: RegionKey, width: CGFloat) -> CGFloat? {
        if key == .open { return open.map { DirectiveHost.measure($0, width: width) } }
        guard let editor = editors[region.kind] else { return nil }
        let view: LeafView?
        if let answer = answers[key] {
            view = answer
        } else {
            view = editor.view?(region)
            answers[key] = .some(view)
        }
        return view.map { DirectiveHost.measure($0, width: width) }
    }

    /// The measure the layout is handed, or nil when the host has no editors.
    var measure: RegionMeasure? {
        guard !editors.isEmpty else { return nil }
        return { [weak self] r, key, width in self?.height(for: r, key: key, width: width) }
    }

    /// Put each view on its rect and take away every drawing whose region has
    /// gone — `DirectiveHost.place`'s rule.
    func place(_ rects: [RegionKey: CGRect], in container: LeafView, scale: CGFloat = 1) {
        for (key, answer) in answers {
            guard let rect = rects[key] else {
                answer?.removeFromSuperview()
                // The open region's own drawing is kept for when it closes on
                // the content it was drawn from.
                if open == nil { answers[key] = nil }
                continue
            }
            if let view = answer { DirectiveHost.put(view, on: rect, in: container, scale: scale) }
        }
        if let open {
            if let rect = rects[.open] {
                DirectiveHost.put(open, on: rect, in: container, scale: scale)
            } else {
                open.removeFromSuperview()
            }
        }
    }

    /// Take the open session's view away.
    func closeOpen() {
        open?.removeFromSuperview()
        open = nil
    }

    func removeAll() {
        for answer in answers.values { answer?.removeFromSuperview() }
        answers.removeAll()
        closeOpen()
    }
}

/// The region keys of a frame's block regions that have editors, in the
/// layout's terms: the open one under `.open`, every other by kind, content and
/// occurrence.
func regionKeys(_ docView: DocView, editors: (String) -> Bool, open: OpenRegionLayout?)
    -> [Int: (RegionView, RegionKey)] {
    var out: [Int: (RegionView, RegionKey)] = [:]
    var seen: [RegionKey: Int] = [:]
    for r in docView.regions where !r.inline && r.kind != MathRegionSession.kind && editors(r.kind) {
        let bare = RegionKey(kind: r.kind, content: r.content, occurrence: 0)
        let n = seen[bare, default: 0]
        seen[bare] = n + 1
        let isOpen = open.map { $0.kind == r.kind && $0.src == r.src } ?? false
        out[Int(r.startRow)] = (r, isOpen ? .open : RegionKey(kind: r.kind, content: r.content, occurrence: n))
    }
    return out
}

extension RegionHost {
    /// Wire a host to the view that opened `region`: each call reaches the
    /// view only while `isCurrent` says this region is still the open one.
    func wire(isCurrent: @escaping () -> Bool,
              content: @escaping () -> String?,
              edit: @escaping ([RegionSplice]) -> Void,
              leave: @escaping (Bool) -> Void,
              history: @escaping (Bool) -> Void,
              resize: @escaping () -> Void) {
        onContent = { isCurrent() ? content() : nil }
        onEdit = { if isCurrent() { edit($0) } }
        onLeave = { if isCurrent() { leave($0) } }
        onHistory = { if isCurrent() { history($0) } }
        onResize = { if isCurrent() { resize() } }
    }
}

extension EditorLayout {
    /// The block region a host's view stands for at `point`, with the view's
    /// rect — what a click on a closed region's drawing opens.
    func regionBox(at point: CGPoint) -> (region: RegionView, rect: CGRect)? {
        for (i, rl) in rows.enumerated() {
            guard let d = rl.directive, case .region = d.key, let r = rl.directiveRect, r.contains(point),
                  let region = regions.first(where: { Int($0.startRow) == i })
            else { continue }
            return (region, r)
        }
        return nil
    }
}

/// What a text view lends its `RegionSlot`: the document, the frame it shows,
/// and the few things only the view can do.
protocol RegionSlotOwner: AnyObject {
    var doc: LeafDoc { get }
    /// The frame on screen.
    var shownFrame: DocView { get }
    /// The open formula's caret colour.
    var regionAccent: LeafColor { get }
    func regionRender(_ frame: DocView)
    /// Give the focus back to the document, from a host's view.
    func focusDocument()
    /// Undo or redo through the view's history.
    func regionHistory(redo: Bool)
    /// Lay the document out again, at the same frame.
    func relayout()
}

/// One view's open region and its routing — the slot the AppKit and UIKit
/// views each own, so that opening, leaving, keys, typing, clicks, undo and
/// following a region from frame to frame are written once for every kind.
final class RegionSlot {
    weak var owner: RegionSlotOwner?
    /// The host's region editors, by kind; their drawings live in `views`.
    var editors: [String: RegionEditor] {
        get { views.editors }
        set {
            close(repaint: false)
            views.editors = newValue
        }
    }
    /// Whether formulas are edited as structure, through leaf's own session.
    var editsMath = true {
        didSet { if !editsMath, open?.kind == MathRegionSession.kind { close(repaint: false) } }
    }
    let views = RegionViews()
    private(set) var open: OpenRegion?

    /// The kinds the view owns, for `setOwnedRegions`.
    var ownedKinds: [String] {
        (editsMath ? [MathRegionSession.kind] : []) + views.editors.keys.sorted()
    }

    func edits(_ kind: String) -> Bool {
        kind == MathRegionSession.kind ? editsMath : views.editors[kind] != nil
    }

    /// Whether a region is open, so the view's own caret stands aside.
    var isOpen: Bool { open != nil }

    /// Open `region` with the caret at `entry`. `false` when there is no
    /// editor for it, or the editor declined.
    @discardableResult
    func open(_ region: RegionView, entry: RegionEntry) -> Bool {
        guard let owner, edits(region.kind), owner.shownFrame.view != "source" else { return false }
        close(repaint: false)
        let host = RegionHost()
        let session: RegionSession?
        if region.kind == MathRegionSession.kind {
            let display = owner.shownFrame.math.first { $0.src == region.src }?.display ?? false
            session = MathRegionSession(region, display: display, entry: entry, accent: owner.regionAccent)
        } else {
            session = views.editors[region.kind]?.open(region, entry, host)
        }
        guard let session else { return false }
        let opened = OpenRegion(region, session: session, host: host)
        host.wire(
            isCurrent: { [weak self, weak opened] in opened != nil && self?.open === opened },
            content: { [weak opened] in opened?.content },
            edit: { [weak self] in self?.edit($0) },
            leave: { [weak self] in self?.leave(atEnd: $0) },
            history: { [weak self] in self?.owner?.regionHistory(redo: $0) },
            resize: { [weak self] in self?.owner?.relayout() })
        open = opened
        views.open = session.view
        owner.regionRender(owner.doc.selectRange(start: region.src, end: region.src))
        session.focus()
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
    func input(_ input: RegionInput) -> Bool {
        guard let open, open.view == nil else { return false }
        return act(open.session.input(input))
    }

    /// Typed text for the open region, when leaf runs its session.
    func type(_ text: String) -> Bool {
        guard let open, open.view == nil else { return false }
        return act(open.session.type(text))
    }

    /// A click at `point` in layout coordinates: on a formula's picture or a
    /// region's drawing it opens that region, or moves the caret in the one
    /// open; anywhere else it closes the open region and is left to run.
    /// Whether the click was a region's.
    func click(at point: CGPoint, in layout: EditorLayout, theme: EditorTheme) -> Bool {
        guard let owner, owner.shownFrame.view != "source" else { return false }
        var hit: (RegionView, RegionEntry, CGPoint, CGFloat)?
        if editsMath, let m = layout.mathPicture(at: point, theme: theme),
           let r = owner.shownFrame.region(MathRegionSession.kind, at: m.math.src) {
            let local = CGPoint(x: point.x - m.rect.minX, y: point.y - m.rect.minY)
            hit = (r, .point(local, size: m.size), local, m.size)
        } else if let b = layout.regionBox(at: point), edits(b.region.kind) {
            let local = CGPoint(x: point.x - b.rect.minX, y: point.y - b.rect.minY)
            hit = (b.region, .point(local, size: theme.fontSize), local, theme.fontSize)
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
        self.open = nil
        let hadView = open.view != nil
        views.closeOpen()
        open.session.close()
        if hadView { owner?.focusDocument() }
        if repaint, let owner { owner.regionRender(owner.shownFrame) }
    }

    private func edit(_ splices: [RegionSplice]) {
        guard let owner, let open else { return }
        if splices.isEmpty {
            open.moved()
            owner.regionRender(owner.shownFrame)
        } else {
            owner.regionRender(open.apply(splices, in: owner.doc))
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
