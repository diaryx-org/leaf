//  Zoom.swift
//
//  How large the document is on screen. A scale, or a rule that becomes one —
//  "fit the sheet's width to the window" is a zoom that follows a resize, and a
//  number that was read off the window at one size stops being that rule the
//  moment the window changes. So the two are kept apart: `Zoom` is what a host
//  or a menu asks for, and the scale it resolves to on a given viewport is the
//  view's `zoomScale`, which is what a "125%" label shows.
//
//  A view transform and nothing more, on either platform: `EditorLayout` always
//  works in unzoomed page space and the surface scales the result, so a change
//  of zoom re-draws but never re-shapes. See `LeafTextView.zoom`.

import CoreGraphics

public enum Zoom: Equatable, Sendable {
    /// A fixed scale, `1` being one layout point per screen point. Held to
    /// `Zoom.range`.
    case scale(CGFloat)
    /// The scale that sets a sheet — with its backdrop either side — to the
    /// viewport's width, and follows the viewport as it resizes. In the
    /// continuous flow, which already fills the width it is given, this is `1`.
    case fitWidth
    /// The scale that shows a whole sheet at once, width and height. `1` off
    /// paper, as `fitWidth` is.
    case fitPage

    /// One layout point per screen point — ⌘0.
    public static let actualSize = Zoom.scale(1)

    /// The scales a view will hold — 25% to 400%, the range a word processor's
    /// zoom control usually offers. A `.scale` outside it is clamped, and a fit
    /// that would fall outside it stops at the edge.
    public static let range: ClosedRange<CGFloat> = 0.25...4

    /// The stops Zoom In and Zoom Out step between. From a scale between two
    /// (a pinch left the view at 137%) a step lands on the next stop, not on
    /// 137% plus a fixed amount, so a run of ⌘> always walks the same ladder.
    public static let stops: [CGFloat] = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4]

    /// Whether this zoom is a rule the viewport decides rather than a number.
    public var isFit: Bool {
        if case .scale = self { return false }
        return true
    }

    /// The scale this zoom is in a viewport `viewport` wide and tall, over
    /// `page` — or over the continuous flow, when `page` is nil, where the fits
    /// are the identity.
    func resolve(in viewport: CGSize, page: PageSetup?) -> CGFloat {
        switch self {
        case .scale(let s):
            return Self.clamp(s)
        case .fitWidth:
            guard let page, viewport.width > 0, page.stackWidth > 0 else { return 1 }
            return Self.clamp(viewport.width / page.stackWidth)
        case .fitPage:
            guard let page, viewport.width > 0, viewport.height > 0, page.stackWidth > 0 else { return 1 }
            let stackHeight = page.size.height + page.backdrop * 2
            return Self.clamp(min(viewport.width / page.stackWidth, viewport.height / stackHeight))
        }
    }

    static func clamp(_ scale: CGFloat) -> CGFloat {
        guard scale.isFinite else { return 1 }
        return min(max(scale, range.lowerBound), range.upperBound)
    }

    /// The first stop above `scale`, or the top of the range from there.
    public static func stepUp(from scale: CGFloat) -> CGFloat {
        stops.first { $0 > scale + 0.001 } ?? range.upperBound
    }

    /// The first stop below `scale`, or the bottom of the range from there.
    public static func stepDown(from scale: CGFloat) -> CGFloat {
        stops.last { $0 < scale - 0.001 } ?? range.lowerBound
    }
}
