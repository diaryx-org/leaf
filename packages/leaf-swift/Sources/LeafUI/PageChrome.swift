//  PageChrome.swift
//
//  The paper a paginated document is drawn on: the surface behind the stack, each
//  sheet, its shadow, and its hairline edge. The peer of `BlockChrome` — pure
//  CoreGraphics over the frames `EditorLayout.pages` hands back, so it is written
//  once and carries no toolkit of its own.
//
//  Drawn *before* the rows, never after. A sheet is a background, and the text,
//  the selection, the quote bars, and the caret all paint onto it; the only thing
//  that decides what lands on which sheet is where `EditorLayout` put it, which
//  this file has no say in.

import CoreGraphics
import Foundation

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

enum PageChrome {
    /// The shadow under a sheet — soft, straight down, and short enough to read as
    /// paper lifted off a surface rather than as a drop shadow on a card.
    static let shadowRadius: CGFloat = 6
    static let shadowOffset = CGSize(width: 0, height: 2)
    static let shadowAlpha: CGFloat = 0.28

    /// Paint the backdrop over `clip` and every sheet in `pages` that meets it.
    ///
    /// `clip` is the dirty band in *layout* coordinates, so a scroll repaints only
    /// the sheets on screen — the same culling the row loop does, and it matters
    /// more here: a hundred-page document is a hundred shadowed rects, and a
    /// shadowed rect is not cheap.
    static func draw(_ pages: [CGRect], theme: EditorTheme, clip: CGRect, in ctx: CGContext) {
        guard !pages.isEmpty else { return }
        ctx.setFillColor(theme.pageBackdropColor.cgColor)
        ctx.fill(clip)

        for sheet in pages {
            if sheet.minY > clip.maxY { break }          // pages run top-down
            if sheet.maxY < clip.minY { continue }
            ctx.saveGState()
            ctx.setShadow(offset: shadowOffset, blur: shadowRadius,
                          color: LeafColor.black.withAlphaComponent(shadowAlpha).cgColor)
            ctx.setFillColor(theme.pageColor.cgColor)
            ctx.fill(sheet)
            ctx.restoreGState()
            // The hairline goes on after the shadow is off: a stroke inside the
            // shadowed state would cast a second one along the edge and read as a
            // double border on a light backdrop.
            ctx.setStrokeColor(theme.pageBorderColor.cgColor)
            ctx.setLineWidth(1)
            ctx.stroke(sheet.insetBy(dx: 0.5, dy: 0.5))
        }
    }
}
