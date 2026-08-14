//  PageSetup.swift
//
//  The sheet the paginated view lays a document onto: its size, its margins, and
//  how a stack of them is spaced. Pure geometry, in points at 72 to the inch — the
//  same unit every other measurement in the layout is already in — so US Letter is
//  612×792 and a one-inch margin is 72.
//
//  Deliberately *not* a field on `EditorTheme`. The theme is style: every knob on
//  it says how a glyph is painted, and every one of them applies in both flows. A
//  page is a mode. Switching it on takes the text column away from `measure` (the
//  sheet's margins decide it now) and gives the document a fixed height it has
//  never had, which is a different kind of change from picking a larger font.

import CoreGraphics
import Foundation

public struct PageSetup: Equatable {
    /// The sheet, in points. Portrait; swap the components for landscape.
    public var size: CGSize
    /// The unprinted border around the text column.
    public var margins: LeafInsets
    /// The space between one sheet and the next down the stack.
    public var gap: CGFloat
    /// The space between the stack and the edges of the view.
    public var backdrop: CGFloat

    public init(
        size: CGSize,
        margins: LeafInsets,
        gap: CGFloat = 20,
        backdrop: CGFloat = 24
    ) {
        self.size = size
        self.margins = margins
        self.gap = gap
        self.backdrop = backdrop
    }

    /// One inch on every side — the default a word processor opens with.
    public static let inch = LeafInsets(top: 72, left: 72, bottom: 72, right: 72)

    public static let usLetter = PageSetup(size: CGSize(width: 612, height: 792), margins: inch)
    public static let a4 = PageSetup(size: CGSize(width: 595, height: 842), margins: inch)

    /// The text column's width — what rows wrap to, standing in for the theme's
    /// `measure` while a page is set.
    public var columnWidth: CGFloat { max(0, size.width - margins.left - margins.right) }

    /// The room one sheet has for rows. Floored above zero so a setup whose
    /// margins swallow the sheet can't make the break loop below unable to
    /// advance.
    public var columnHeight: CGFloat { max(1, size.height - margins.top - margins.bottom) }

    /// How far apart two consecutive sheets' tops are.
    var pitch: CGFloat { size.height + gap }

    // ── a sheet's place in the stack ─────────────────────────────────────────

    func sheetTop(_ index: Int) -> CGFloat { backdrop + CGFloat(index) * pitch }
    func contentTop(_ index: Int) -> CGFloat { sheetTop(index) + margins.top }
    func contentBottom(_ index: Int) -> CGFloat { sheetTop(index) + size.height - margins.bottom }

    func sheetRect(_ index: Int, x: CGFloat) -> CGRect {
        CGRect(x: x, y: sheetTop(index), width: size.width, height: size.height)
    }

    /// The sheet a layout `y` falls on — including the ones a block too tall for
    /// any sheet spills across, which is the only way `y` ever runs past the
    /// bottom of the sheet it started on.
    func index(at y: CGFloat) -> Int { max(0, Int(((y - backdrop) / pitch).rounded(.down))) }

    /// Where the stack's left edge sits in a view `viewWidth` wide: centred, but
    /// never nearer the edge than `backdrop`. A window narrower than a sheet
    /// scrolls sideways rather than cropping the margin off — a page is a fixed
    /// width, and reflowing it to the window is precisely what pagination is not.
    func sheetX(in viewWidth: CGFloat) -> CGFloat {
        max(backdrop, ((viewWidth - size.width) / 2).rounded())
    }

    /// The stack's own width: a sheet plus its backdrop either side. The view
    /// takes the wider of this and its viewport, so a narrow window scrolls and a
    /// wide one just centres.
    var stackWidth: CGFloat { size.width + backdrop * 2 }
}
