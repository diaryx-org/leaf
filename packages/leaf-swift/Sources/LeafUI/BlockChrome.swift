//  BlockChrome.swift
//
//  The block decoration both surfaces paint *as graphics* rather than as core's
//  monospace glyphs: a blockquote's gutter bars and a thematic break's line. The
//  same move `TableLayout` makes for a table — core's picture is right on a
//  terminal and sheared in a proportional font, so the GUI reads the structure
//  out of it and draws the real thing.
//
//  Core still emits the glyphs (`│ ` per quote level, a row of `─` for a break)
//  and they still carry the row's caret offsets; `AttributedRow` just draws them
//  clear, and these routines paint over the space they hold. All CoreGraphics, so
//  the AppKit and UIKit views share one implementation.

import CoreGraphics
import CoreText
import Foundation

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

enum BlockChrome {
    /// Paint a block media box: the picture if there is one, else a labelled chip,
    /// plus a play badge for video and audio.
    ///
    /// Shared by both surfaces, and drawn into the text's own context — which is
    /// exactly why playback isn't here. A still is pixels; a player is a subview.
    /// See `MediaLayout` for that division and what `onOpenMedia` does about it.
    /// `playing` means an AVKit player is installed over this box, and nothing is
    /// drawn at all: the box belongs to the player now.
    ///
    /// Not merely "hide the badge". The still is a *stand-in* for a player that
    /// isn't there yet, and a player view doesn't necessarily fill its frame edge
    /// to edge — AVKit lays the picture out inside its own chrome — so a poster
    /// left underneath shows as a mismatched band around the video rather than
    /// hiding behind it.
    static func drawMedia(_ box: MediaLayout, at rect: CGRect, theme: EditorTheme,
                          playing: Bool = false, in ctx: CGContext) {
        guard !playing else { return }
        ctx.saveGState()
        defer { ctx.restoreGState() }

        let rounded = CGPath(roundedRect: rect, cornerWidth: MediaMetrics.corner,
                             cornerHeight: MediaMetrics.corner, transform: nil)

        if let img = box.still {
            // Save/restore around the clip and the flip, so neither leaks into the
            // frame and badge drawn after this — `resetClip` can't undo a clip and
            // undoing a transform by re-applying its inverse accumulates error.
            ctx.saveGState()
            // Clip to the rounded box so the picture's corners match the chrome's
            // rather than poking out square behind it.
            ctx.addPath(rounded)
            ctx.clip()
            // Both surfaces draw into a flipped context (AppKit's `isFlipped`,
            // UIKit's native top-left origin) so rows can be laid out top-down.
            // `CGContext.draw` doesn't know that and would render the picture
            // upside down, so flip back across the box before drawing it — the
            // text paths avoid this only because NSString/NSAttributedString
            // drawing compensates internally.
            ctx.translateBy(x: 0, y: rect.maxY)
            ctx.scaleBy(x: 1, y: -1)
            ctx.draw(img, in: CGRect(x: rect.minX, y: 0, width: rect.width, height: rect.height))
            ctx.restoreGState()
        } else {
            // Nothing to show: a filled chip carrying the media's name, so the row
            // says what stands there instead of going blank.
            ctx.addPath(rounded)
            ctx.setFillColor(theme.codeBackground.cgColor)
            ctx.fillPath()
            drawChipLabel(box, in: rect, theme: theme, ctx: ctx)
        }

        // A frame: solid for something real, dashed for an image that didn't load
        // — the same signal leaf-web's dashed outline gives, for the same reason.
        ctx.addPath(rounded)
        ctx.setStrokeColor((box.isBroken ? theme.secondaryColor : theme.tableBorderColor).cgColor)
        ctx.setLineWidth(1)
        if box.isBroken { ctx.setLineDash(phase: 0, lengths: [4, 3]) }
        ctx.strokePath()
        ctx.setLineDash(phase: 0, lengths: [])

        if box.showsPlayBadge { drawPlayBadge(in: rect, theme: theme, ctx: ctx) }
    }

    /// The media's name, centred in a chip that has no picture. Left-aligned and
    /// inset when a play badge shares the box, so the two don't overlap.
    private static func drawChipLabel(_ box: MediaLayout, in rect: CGRect,
                                      theme: EditorTheme, ctx: CGContext) {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: theme.proportionalFont(size: theme.fontSize * 0.85, bold: false, italic: false),
            .foregroundColor: theme.secondaryColor,
        ]
        let text = box.chipLabel as NSString
        let size = text.size(withAttributes: attrs)
        // Left of the label sits the play badge when there is one; both sides get
        // the same padding otherwise.
        let leftInset = box.showsPlayBadge ? MediaMetrics.badge + 16 : 12
        // A name too long for what's left is clipped rather than spilling out of
        // the chip and over the prose beside it.
        let room = rect.width - leftInset - 12
        guard room > 0 else { return }
        ctx.saveGState()
        ctx.clip(to: CGRect(x: rect.minX + leftInset, y: rect.minY,
                            width: room, height: rect.height))
        text.draw(at: CGPoint(x: rect.minX + leftInset, y: rect.midY - size.height / 2),
                  withAttributes: attrs)
        ctx.restoreGState()
    }

    /// A play badge — a translucent disc with a triangle — so a video or audio box
    /// reads as something to start rather than something to look at. Centred on a
    /// picture, left-aligned on a chip where the label takes the rest of the room.
    private static func drawPlayBadge(in rect: CGRect, theme: EditorTheme, ctx: CGContext) {
        let d = min(MediaMetrics.badge, rect.height - 8, rect.width - 8)
        guard d > 8 else { return }
        // A tall picture centres the badge; a short chip keeps it at the left so
        // the label has the rest of the width.
        let cx = rect.height >= MediaMetrics.badge * 1.5 ? rect.midX : rect.minX + d / 2 + 8
        let centre = CGPoint(x: cx, y: rect.midY)
        let disc = CGRect(x: centre.x - d / 2, y: centre.y - d / 2, width: d, height: d)

        ctx.setFillColor(gray: 0, alpha: 0.45)
        ctx.fillEllipse(in: disc)

        // An equilateral triangle inside the disc, nudged right so it reads as
        // centred (a triangle's visual centre sits left of its bounding box's).
        let s = d * 0.36
        let ox = centre.x + s * 0.12
        ctx.beginPath()
        ctx.move(to: CGPoint(x: ox - s * 0.5, y: centre.y - s * 0.6))
        ctx.addLine(to: CGPoint(x: ox - s * 0.5, y: centre.y + s * 0.6))
        ctx.addLine(to: CGPoint(x: ox + s * 0.6, y: centre.y))
        ctx.closePath()
        ctx.setFillColor(gray: 1, alpha: 0.95)
        ctx.fillPath()
    }

    /// The quote bars of a whole frame, merged down the page: consecutive rows
    /// quoted at the same level yield ONE rect, so a multi-row quote reads as a
    /// single unbroken bar with rounded caps rather than a stack of segments.
    ///
    /// A row that carries no bar at some level (the paragraph after the quote, or
    /// a table's picture rows, which draw a grid that isn't inset) ends the run
    /// there and the next quoted row starts a fresh one.
    static func quoteBarRuns(_ rows: [RowLayout], theme: EditorTheme) -> [CGRect] {
        var open: [CGFloat: CGRect] = [:]   // level x → the run still growing there
        var done: [CGRect] = []
        for rl in rows {
            var seen = Set<CGFloat>()
            for bar in rl.quoteBars(theme: theme) {
                let key = (bar.minX * 2).rounded() / 2   // same level ⇒ same x, to ½pt
                seen.insert(key)
                if var run = open[key], abs(run.maxY - bar.minY) < 0.5 {
                    run.size.height = bar.maxY - run.minY
                    open[key] = run
                } else {
                    if let stale = open[key] { done.append(stale) }
                    open[key] = bar
                }
            }
            for (key, run) in open where !seen.contains(key) {
                done.append(run)
                open[key] = nil
            }
        }
        done.append(contentsOf: open.values)
        return done.filter { $0.height > 0 }
    }

    /// Paint those runs, capsule-capped, in the theme's bar colour.
    static func drawQuoteBars(_ rows: [RowLayout], theme: EditorTheme, in ctx: CGContext) {
        let runs = quoteBarRuns(rows, theme: theme)
        guard !runs.isEmpty else { return }
        ctx.setFillColor(theme.quoteBarColor.cgColor)
        let r = theme.quoteBarWidth / 2
        for run in runs {
            ctx.addPath(CGPath(roundedRect: run, cornerWidth: r, cornerHeight: r, transform: nil))
        }
        ctx.fillPath()
    }

    /// Paint a thematic break: a hairline across the text column, inset past any
    /// gutter the break sits inside. `selColor` (macOS, where the view draws its
    /// own selection) fills the row's box first, so a selection running through a
    /// rule shows on it — the collapsed row has no glyphs of its own to highlight.
    /// No-op on a row that isn't a break.
    static func drawRule(_ rl: RowLayout, theme: EditorTheme, contentWidth: CGFloat,
                         selColor: LeafColor?, in ctx: CGContext) {
        guard let line = rl.ruleLine(theme: theme, contentWidth: contentWidth) else { return }
        if let selColor, rl.row.runs.contains(where: { $0.sel }) {
            ctx.setFillColor(selColor.cgColor)
            ctx.fill(CGRect(x: line.minX, y: rl.top + rl.labelInset,
                            width: line.width, height: rl.height - rl.labelInset))
        }
        ctx.setFillColor(theme.ruleColor.cgColor)
        ctx.fill(line)
    }
}
