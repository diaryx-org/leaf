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
//  session holds open.

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
    /// `replaceRange`.
    func applyWritingTools(_ range: NSRange, origin: Int, text: String) -> DocView {
        let (from, to) = writingToolsBytes(range, origin: origin)
        return replaceRange(from: UInt32(from), to: UInt32(max(from, to)),
                            text: WritingToolsText.sourceText(text))
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
