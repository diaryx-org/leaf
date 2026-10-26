//  Landing.swift
//
//  Arriving somewhere on purpose, as opposed to scrolling there.
//
//  The editor already keeps the caret in sight, and for typing that is exactly
//  right: it scrolls the *least* it can, so a caret stepping off the bottom edge
//  brings one line into view and leaves the reader's eyes where they were. Follow
//  a link into a verse with the same rule and the verse lands wherever the
//  minimum happened to put it — at the bottom edge coming from above, at the top
//  coming from below, not at all if it was already on screen. The same
//  destination, three different arrivals, and none of them where the reader is
//  looking.
//
//  So a landing is its own move: put the block a fixed distance below the top of
//  the viewport, every time, and flash it so the eye is told where it went. The
//  flash is not decoration — a scroll that lands mid-page is indistinguishable
//  from a scroll that failed, and the reader's next act is to hunt for the verse
//  they were promised.

import Foundation

/// Where a landing puts the thing it landed on, and how it says so.
enum Landing {
    /// How far below the top of the viewport the landed block sits.
    ///
    /// Not zero: a block flush against the top edge reads as a page that got cut
    /// off, and a verse means more with the sentence before it visible. Not a
    /// fraction of the viewport either — that puts a short window's landing at a
    /// different place from a tall one's, which is the inconsistency this exists
    /// to remove.
    static let lead: CGFloat = 72

    /// Where to scroll so that `target` sits `lead` below the top of a viewport
    /// `visibleHeight` tall, within a document `documentHeight` tall.
    ///
    /// Clamped at both ends, so landing on the first block doesn't scroll above
    /// the document and landing on the last doesn't scroll past it. The clamp is
    /// also why a short document may land its block higher than `lead` — there is
    /// nothing below to scroll up into, and stretching to obey the rule would mean
    /// emptiness under the text.
    static func scrollTop(for target: CGRect,
                          visibleHeight: CGFloat,
                          documentHeight: CGFloat) -> CGFloat {
        // Never scroll a block's own top out of view to honour the lead: a block
        // taller than the viewport is read from its beginning.
        let lead = min(lead, max(0, visibleHeight - target.height))
        let bottom = max(0, documentHeight - visibleHeight)
        return min(max(0, target.minY - lead), bottom)
    }

    /// How long the flash stays up, and how long it takes to go.
    ///
    /// Long enough to be seen after the scroll settles, short enough that it is
    /// gone before the reader starts reading — a highlight still sitting under
    /// the words is a highlight they have to dismiss, and there is nothing here
    /// to dismiss it with.
    static let hold: TimeInterval = 0.9
    static let fade: TimeInterval = 0.45

    /// The flash's opacity `elapsed` seconds in: solid while held, easing off
    /// through the fade, gone after. Returns nil once there is nothing to draw,
    /// which is what tells a view to stop animating.
    static func opacity(elapsed: TimeInterval) -> CGFloat? {
        guard elapsed < hold + fade else { return nil }
        guard elapsed > hold else { return 1 }
        return 1 - CGFloat((elapsed - hold) / fade)
    }
}
