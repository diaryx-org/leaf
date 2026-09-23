//  WritingTools.swift
//
//  What the two views' Writing Tools coordinators share: which text a context
//  holds, and how a range in it crosses into the document.
//
//  Writing Tools reads and rewrites the text as the reader sees it — the
//  visible text, `LeafDoc.visibleText()`, the same string the find bar and the
//  text input system index in UTF-16 — and never the markup. A context is a
//  stretch of that string: its `origin` is where the stretch starts in it, and
//  every range Writing Tools hands back is relative to the stretch. Adding the
//  origin makes it a range of the visible text, and core's `matchBounds` takes
//  it to the exact source bytes, as find does. A rewrite goes back through
//  `replaceRange`, so it is an edit like any other, inside the undo group the
//  session holds open — only where it changed the words, and block by block,
//  so the markup Writing Tools never saw stays (`writingToolsEdits`).

import Foundation
import CoreGraphics
import LeafFFI

/// A stretch of the visible text handed to Writing Tools.
struct WritingToolsSpan: Equatable {
    /// Where the stretch starts in the visible text, in UTF-16 units.
    let origin: Int
    /// How long it is.
    let length: Int
    /// The part of it Writing Tools is to work on, relative to `origin` — the
    /// selection, with the rest of its paragraphs around it as context.
    let range: NSRange
}

enum WritingToolsText {
    /// How much text Writing Tools asked for — the platform scopes' shared shape.
    enum Scope { case selection, document, visible }

    /// The stretch of `text` (the visible text) to hand over for `scope`, given
    /// the selection and the visible range, both UTF-16 ranges of `text`.
    ///
    /// The selection and the visible area are widened to whole paragraphs, as
    /// the coordinator's documentation asks: the words around a sentence are
    /// what a rewrite of it is judged by. An empty selection is the whole
    /// document, which is what a text view proofreads when nothing is selected.
    static func span(for scope: Scope, in text: String, selection: NSRange, visible: NSRange) -> WritingToolsSpan {
        let ns = text as NSString
        let whole = WritingToolsSpan(origin: 0, length: ns.length, range: NSRange(location: 0, length: ns.length))
        let focus: NSRange
        switch scope {
        case .document: return whole
        case .selection:
            guard selection.length > 0 else { return whole }
            focus = selection
        case .visible:
            focus = visible
        }
        guard focus.location != NSNotFound, NSMaxRange(focus) <= ns.length else { return whole }
        let paragraphs = ns.paragraphRange(for: focus)
        return WritingToolsSpan(origin: paragraphs.location, length: paragraphs.length,
                                range: NSRange(location: focus.location - paragraphs.location,
                                               length: focus.length))
    }

    /// `range` of a context that starts at `origin`, as a range of the visible text.
    static func visibleRange(_ range: NSRange, origin: Int) -> NSRange {
        NSRange(location: origin + range.location, length: range.length)
    }

    /// A rewrite as it goes into the source. A line break in the visible text is
    /// where one block ends and the next begins, which the source spells as a
    /// blank line — written as one `\n`, the paragraphs of a rewrite would run
    /// together into one.
    static func sourceText(_ replacement: String) -> String {
        replacement.replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\n", with: "\n\n")
    }
}

extension LeafDoc {
    /// The source bytes a range of a Writing Tools context covers, exactly —
    /// see `matchBounds`.
    func writingToolsBytes(_ range: NSRange, origin: Int) -> (from: Int, to: Int) {
        matchBounds(WritingToolsText.visibleRange(range, origin: origin))
    }

    /// Replace `range` of the context at `origin` with `text`, through
    /// `replaceRange` — see `writingToolsEdits` for how, and when not. The
    /// view after the last edit, and the visible text that went in; nil when
    /// the rewrite was refused and nothing changed.
    func applyWritingTools(_ range: NSRange, origin: Int, text: String) -> (view: DocView, applied: String)? {
        guard let rewrite = writingToolsEdits(range, origin: origin, text: text) else { return nil }
        var view: DocView?
        // Last to first, so each edit leaves the bytes of the ones still to
        // go where they were measured.
        for edit in rewrite.edits.sorted(by: { $0.from > $1.from }) {
            view = replaceRange(from: UInt32(edit.from), to: UInt32(edit.to), text: edit.text)
        }
        return (view ?? self.view(), rewrite.applied)
    }

    /// The source edits that put `text` in place of `range` of the context at
    /// `origin`, and the visible text they leave there — or nil, to refuse a
    /// rewrite that would take structure with it.
    ///
    /// A rewrite is plain text, and the source has markup Writing Tools never
    /// saw: a heading's `##`, a list item's `- `, the `**` around a word. So
    /// it goes in as narrowly as it can:
    ///
    /// - each paragraph of it replaces only what changed in its own paragraph
    ///   — the text the old and the new share at either end stays, with any
    ///   markup inside it;
    /// - over several blocks, paragraph by paragraph, when the rewrite has as
    ///   many as the range: a line break of the visible text is a block
    ///   boundary, and a block's markup lies between two of them, where no
    ///   edit reaches;
    /// - when it has more or fewer, only where the blocks are plain
    ///   paragraphs — the source nothing but the text and the blank lines
    ///   between — since there is no telling which block's markup a paragraph
    ///   of the rewrite would take. Anything else is refused whole.
    ///
    /// The break at the end of the range's last block is the boundary with
    /// the block after it, and a rewrite over several blocks neither removes
    /// it nor adds one.
    func writingToolsEdits(_ range: NSRange, origin: Int, text: String)
        -> (edits: [(from: Int, to: Int, text: String)], applied: String)? {
        let whole = WritingToolsText.visibleRange(range, origin: origin)
        let visible = visibleText() as NSString
        guard whole.location >= 0, NSMaxRange(whole) <= visible.length else { return nil }
        let original = visible.substring(with: whole)
        let rewrite = text.replacingOccurrences(of: "\r\n", with: "\n")
        let trailing = original.hasSuffix("\n")
        let old = (trailing ? String(original.dropLast()) : original).components(separatedBy: "\n")
        guard old.count > 1 else {
            return ([writingToolsEdit(whole.location, original, rewrite)].compactMap { $0 }, rewrite)
        }
        let body = rewrite.hasSuffix("\n") ? String(rewrite.dropLast()) : rewrite
        let new = body.components(separatedBy: "\n")
        if new.count == old.count {
            var edits: [(from: Int, to: Int, text: String)] = []
            var at = whole.location
            for (was, now) in zip(old, new) {
                if let edit = writingToolsEdit(at, was, now) { edits.append(edit) }
                at += (was as NSString).length + 1
            }
            return (edits, body + (trailing ? "\n" : ""))
        }
        let (from, to) = matchBounds(whole)
        let source = Array(self.source().utf8)
        guard from <= to, to <= source.count,
              String(decoding: source[from..<to], as: UTF8.self) == WritingToolsText.sourceText(original)
        else { return nil }
        return ([writingToolsEdit(whole.location, original, rewrite)].compactMap { $0 }, rewrite)
    }

    /// The edit that makes `old`, which starts at `location` in the visible
    /// text, read `new`: the source of what differs between them — what they
    /// share at either end left as it is, markup and all — or nil when they
    /// are the same.
    private func writingToolsEdit(_ location: Int, _ old: String, _ new: String) -> (from: Int, to: Int, text: String)? {
        let a = Array(old), b = Array(new)
        var head = 0
        while head < a.count, head < b.count, a[head] == b[head] { head += 1 }
        var tail = 0
        while tail < a.count - head, tail < b.count - head, a[a.count - 1 - tail] == b[b.count - 1 - tail] { tail += 1 }
        guard head + tail < a.count || head + tail < b.count else { return nil }
        let start = location + String(a[..<head]).utf16.count
        let length = String(a[head..<(a.count - tail)]).utf16.count
        let (from, to) = matchBounds(NSRange(location: start, length: length))
        return (from, max(from, to), WritingToolsText.sourceText(String(b[head..<(b.count - tail)])))
    }
}

/// A view's side of a Writing Tools session, the same on both platforms: the
/// contexts handed out, the text an animation has hidden or dimmed, and the
/// core undo group the session is held in.
final class WritingToolsSession {
    /// Where each context handed out starts in the visible text, by its identifier.
    var origins: [UUID: Int] = [:]
    /// Ranges of the visible text an animation has asked to hide — or, `dim`,
    /// to grey while Writing Tools waits — by the animation that asked.
    var effects: [String: (range: NSRange, dim: Bool)] = [:]
    /// Whether the session holds a core undo group open.
    private(set) var groupOpen = false
    /// Set while a replacement Writing Tools asked for is applied — an edit it
    /// made, which it need not be told of.
    var applying = false

    /// Hold `doc`'s undo group open for the session, so that everything
    /// Writing Tools changes — every accepted suggestion, a whole rewrite — is
    /// one step.
    func openGroup(_ doc: LeafDoc) {
        guard !groupOpen else { return }
        groupOpen = true
        doc.beginUndoGroup()
    }

    func closeGroup(_ doc: LeafDoc) {
        guard groupOpen else { return }
        groupOpen = false
        doc.endUndoGroup()
    }

    /// The key an animation's effect is kept under: what it is, and where.
    static func effectKey(_ animation: Int, _ range: NSRange, _ context: UUID) -> String {
        "\(animation):\(context):\(range.location):\(range.length)"
    }

    /// The boxes the effects cover now, in layout coordinates.
    func effectRects(doc: LeafDoc, layout: EditorLayout) -> (hidden: [CGRect], dimmed: [CGRect]) {
        guard !effects.isEmpty else { return ([], []) }
        let length = (doc.visibleText() as NSString).length
        var hidden: [CGRect] = [], dimmed: [CGRect] = []
        for effect in effects.values {
            let start = min(effect.range.location, length)
            let range = NSRange(location: start, length: min(effect.range.length, length - start))
            let (from, to) = doc.byteBounds(range)
            let boxes = layout.rangeRects(fromByte: from, toByte: to, in: doc)
            if effect.dim { dimmed += boxes } else { hidden += boxes }
        }
        return (hidden, dimmed)
    }

    /// Draw a row's lines with the effects applied: nothing where the text is
    /// hidden, half-strength where it is dimmed. Layout coordinates, each rect
    /// trimmed to the band so the even-odd clip is exact.
    static func drawMasked(hidden: [CGRect], dimmed: [CGRect], band: CGRect, in ctx: CGContext,
                           _ drawLines: () -> Void) {
        let trim = { (rects: [CGRect]) in rects.map { $0.intersection(band) }.filter { !$0.isNull && !$0.isEmpty } }
        let (hidden, dimmed) = (trim(hidden), trim(dimmed))
        ctx.saveGState()
        let visible = CGMutablePath()
        visible.addRect(band)
        for r in hidden + dimmed { visible.addRect(r) }
        ctx.addPath(visible)
        ctx.clip(using: .evenOdd)
        drawLines()
        ctx.restoreGState()
        guard !dimmed.isEmpty else { return }
        ctx.saveGState()
        ctx.clip(to: dimmed)
        ctx.setAlpha(0.5)
        drawLines()
        ctx.restoreGState()
    }
}
