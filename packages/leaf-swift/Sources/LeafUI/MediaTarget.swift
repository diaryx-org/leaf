//  MediaTarget.swift
//
//  Which attachment a gesture is *about* — the media peer of `LinkTarget`, kept
//  here so the AppKit contextual menu and the UIKit edit menu can't drift apart
//  in what they offer or when.
//
//  Two questions answer it, and both are needed. Core knows the caret stands in
//  an `![](cat.png)` (`imageDestinationAtCaret`), which is what a menu raised
//  from the keyboard or from prose beside a picture has to go on. Geometry knows
//  which box a click landed in (`EditorLayout.mediaBox(at:)`), which is the only
//  thing that answers for a video or an audio box — those are HTML elements with
//  no image node under the caret to find — and the only one that is exact when a
//  click inside a block box puts the caret at the block's edge rather than
//  inside its span.
//
//  So: the box under the pointer wins, the caret answers otherwise, and the
//  whole entry disappears when the host has set no `onShowMedia`. That last
//  clause is `onEditLink`'s rule — an affordance for a hook nobody is listening
//  to is a menu item that does nothing — and it is why this takes `canShow`
//  rather than reading a view's property.

import Foundation
import LeafFFI

extension LeafDoc {
    /// The source of the image the caret stands in, or nil — core's
    /// `image_destination_at_caret`, under the name the rest of LeafUI uses.
    ///
    /// Pictures only, and deliberately so: a `<video>` is not an image node, and
    /// the box it draws in is found by hit-testing rather than by asking core.
    func mediaSourceAtCaret() -> String? { imageDestinationAtCaret() }
}

/// The attachment a menu should offer to show, or nil for no entry at all.
///
/// `box` is the `src` of the media box the gesture landed on (nil when it landed
/// on none), `caret` is core's answer for the caret's own position, and
/// `canShow` is whether a host set `LeafEditorModel.onShowMedia`.
func showableMediaSource(box: String?, caret: String?, canShow: Bool) -> String? {
    guard canShow else { return nil }
    // An empty `src` is a `<video>` that named no source — a broken document,
    // and nothing a host could show.
    return [box, caret].compactMap { $0 }.first { !$0.isEmpty }
}
