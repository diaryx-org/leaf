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
import Foundation

enum BlockChrome {
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
