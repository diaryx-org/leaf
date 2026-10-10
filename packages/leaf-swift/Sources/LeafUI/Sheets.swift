//  Sheets.swift
//
//  Paper read a sheet at a time, as the leaves of a book are. On paper the
//  surface reports which sheet is in view and how many there are, can be
//  sent to a sheet, and can be made to *hold* one: scrolling then stops at
//  the held sheet's edges, so a pinch zooms in on it and a drag looks round
//  it without drifting on to the next. The host turns from one to the next
//  (`LeafTextView.showSheet`), and so does the caret, when typing or a motion
//  carries it on to another sheet — the held sheet is always the one being
//  written on.
//
//  Writing and reading are one surface here, as everywhere in leaf: a held
//  sheet is as editable as any other, and the keyboard rising over it is a
//  bottom inset the sheet scrolls up within, like the document scrolls above
//  it anywhere else.
//
//  The geometry is shared; each platform's view applies it to its own scroll
//  view (`LeafTextView.heldOffset` on UIKit, `heldBounds` on AppKit).

import CoreGraphics

enum Sheets {
    /// The sheet a layout `y` falls on, or the nearest one to it — a `y` in
    /// the gap between two sheets belongs to whichever it is closer to.
    static func index(at y: CGFloat, in pages: [CGRect]) -> Int {
        var best = 0
        var bestDistance = CGFloat.infinity
        for (i, page) in pages.enumerated() {
            let distance = y < page.minY ? page.minY - y : (y > page.maxY ? y - page.maxY : 0)
            if distance < bestDistance { best = i; bestDistance = distance }
            if distance == 0 || page.minY > y { break }
        }
        return best
    }

    /// The layout band a held sheet `index` scrolls within: the sheet with
    /// its backdrop either side, which is exactly what `Zoom.fitPage` sets to
    /// the viewport — so a held sheet at that zoom does not scroll at all.
    /// The first band runs up to the top of the document and the last down
    /// to its end, so a header above the first sheet and a footer below the
    /// last stay reachable.
    static func band(_ index: Int, pages: [CGRect], backdrop: CGFloat,
                     contentHeight: CGFloat) -> ClosedRange<CGFloat> {
        guard pages.indices.contains(index) else { return 0...max(0, contentHeight) }
        let page = pages[index]
        let top = index == 0 ? 0 : max(0, page.minY - backdrop)
        let bottom = index == pages.count - 1 ? max(contentHeight, page.maxY) : page.maxY + backdrop
        return top...max(top, bottom)
    }

    /// `offset` held to `range`, which a scroll view's own clamp has already
    /// kept to the document: a band shorter than the viewport pins the
    /// offset to its top.
    static func clamp(_ offset: CGFloat, to range: ClosedRange<CGFloat>) -> CGFloat {
        min(max(offset, range.lowerBound), range.upperBound)
    }
}
