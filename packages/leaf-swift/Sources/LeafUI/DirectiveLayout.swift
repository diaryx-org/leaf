//  DirectiveLayout.swift
//
//  Leaf directives a host draws. Core reads every `::name{…}` a document
//  carries and publishes it in `DocView.directives` — its rows, name, label and
//  attributes — over a `⧉ name` placeholder row it leaves for a frontend that
//  knows the host's vocabulary to paint over. This file is that frontend's half
//  on Apple platforms: the host answers `directiveView` with a platform view per
//  directive, `EditorLayout` collapses the directive's rows onto one box as tall
//  as the view, and `DirectiveHost` keeps the views and places them over their
//  boxes. See `docs/proposals/host-directives.md`.
//
//  ## Sizing
//
//  Points, not rows. Core would reserve rows for a drawing if asked
//  (`set_directive_rows`, which is the terminal's loop), and a proportional GUI
//  does not play that game: like a picture, the directive keeps core's one
//  placeholder row and the layout gives that row the view's height. Its
//  `intrinsicContentSize` when it has one, else its fitting size at the text
//  column's width — so a SwiftUI view in a hosting view, or an Auto Layout view
//  whose height follows from its width, both measure as they are.
//
//  ## What stays leaf's
//
//  `::page-break` is never offered to the host: the name is leaf's, and a page
//  opening there is a layout fact rather than a picture. A nil answer leaves the
//  placeholder drawn exactly as it was, so a host with no hook, or one that
//  knows only some names, changes nothing about the rest.

import CoreGraphics
import Foundation
import LeafFFI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// Which directive a host view stands for: what the directive says — its name,
/// label and attributes — and, for a document that says the same thing twice,
/// which of the two. Rows are not part of it: they renumber on every keystroke
/// above the directive, and the view should ride the edit rather than be built
/// again.
struct DirectiveKey: Hashable {
    let name: String
    let label: String
    let attrs: [String]
    /// How many directives before this one in the document say exactly the
    /// same thing. One view cannot stand in two places.
    let occurrence: Int

    init(_ directive: DirectiveView, occurrence: Int) {
        name = directive.name
        label = directive.label
        // Key and value joined by a character neither can hold — a key is a
        // name, and a value that held a newline would have been refused.
        attrs = directive.attrs.map { "\($0.key)\n\($0.value)" }
        self.occurrence = occurrence
    }
}

/// Asked for the height a directive's view wants at a column `width` points
/// wide, or nil to leave it the placeholder. `DirectiveHost.height` in the
/// views; a closure in the tests.
typealias DirectiveMeasure = (DirectiveView, DirectiveKey, CGFloat) -> CGFloat?

/// The box one host-drawn directive occupies.
struct DirectiveLayout {
    let directive: DirectiveView
    let key: DirectiveKey
    /// The view's own size: the column's width (less the row's prefix), and the
    /// height it asked for.
    let size: CGSize

    /// The height the row reserves: the view plus the breathing room a picture
    /// gets above and below, so a card does not crowd the prose around it.
    var height: CGFloat { size.height + DirectiveMetrics.gap * 2 }

    /// The view's frame in layout coordinates, given the top of the reserved
    /// row and the left edge of its content.
    func rect(top: CGFloat, left: CGFloat) -> CGRect {
        CGRect(x: left, y: top + DirectiveMetrics.gap, width: size.width, height: size.height)
    }
}

enum DirectiveMetrics {
    /// Breathing room above and below a host's view — a picture's.
    static let gap: CGFloat = MediaMetrics.gap
}

/// The host's views for the directives in one text view: asked for once per
/// directive, kept while the directive is in the document, and placed over the
/// box the layout gives it on every frame that lays out.
final class DirectiveHost {
    /// The host's hook — see `LeafTextView.directiveView`.
    var provider: ((DirectiveView) -> LeafView?)? {
        didSet { removeAll() }
    }

    /// Each directive's answer, a nil one included, so a directive the host
    /// declined is not asked about again on every frame.
    private var answers: [DirectiveKey: LeafView?] = [:]

    var isEmpty: Bool { answers.isEmpty }

    /// The height `directive`'s view wants at `width`, asking the host for the
    /// view the first time. Nil when there is no hook, when the directive is
    /// leaf's own, and when the host answered nil.
    func height(for directive: DirectiveView, key: DirectiveKey, width: CGFloat) -> CGFloat? {
        guard let provider, directive.name != EditorLayout.pageBreakDirective else { return nil }
        let view: LeafView?
        if let answer = answers[key] {
            view = answer
        } else {
            view = provider(directive)
            answers[key] = .some(view)
        }
        guard let view else { return nil }
        return Self.measure(view, width: width)
    }

    /// The measure the layout is handed: this host's, or nil when there is no
    /// hook, which is what keeps a host with none from paying anything.
    var measure: DirectiveMeasure? {
        guard provider != nil else { return nil }
        return { [weak self] d, key, width in self?.height(for: d, key: key, width: width) }
    }

    /// Put each view on its rect, and take away every view whose directive has
    /// left the document. `rects` are in layout coordinates; `scale` is how
    /// many view points a layout point is on a surface that zooms by scaling
    /// its drawing rather than itself (the AppKit view), and 1 on one whose
    /// own transform does the scaling (the UIKit view).
    ///
    /// Added behind every other subview, so the system's caret — the AppKit
    /// insertion indicator, UIKit's selection views — stands in front of a view
    /// it is drawn at the edge of.
    func place(_ rects: [DirectiveKey: CGRect], in container: LeafView, scale: CGFloat = 1) {
        for (key, answer) in answers {
            guard let rect = rects[key] else {
                answer?.removeFromSuperview()
                answers[key] = nil
                continue
            }
            guard let view = answer else { continue }
            if view.superview !== container {
                #if canImport(UIKit)
                container.insertSubview(view, at: 0)
                #else
                container.addSubview(view, positioned: .below, relativeTo: nil)
                #endif
            }
            #if canImport(UIKit)
            view.frame = rect
            #else
            view.frame = rect.applying(CGAffineTransform(scaleX: scale, y: scale))
            // Scaled by its bounds, so what it draws zooms with the text around it
            // rather than being re-laid-out at a size it never asked for.
            view.setBoundsSize(rect.size)
            #endif
        }
    }

    /// Take every view away and forget every answer — the hook changed, and
    /// what it said before no longer stands.
    func removeAll() {
        for answer in answers.values { answer?.removeFromSuperview() }
        answers.removeAll()
    }

    /// The height `view` asks for at a column `width` wide: its intrinsic
    /// height where it has one, its fitting height at that width where it has
    /// not, and its frame's height where it has neither — a plain view the host
    /// sized by hand.
    static func measure(_ view: LeafView, width: CGFloat) -> CGFloat {
        #if canImport(UIKit)
        let intrinsic = view.intrinsicContentSize.height
        if intrinsic != UIView.noIntrinsicMetric, intrinsic > 0 { return intrinsic }
        let fitted = view.systemLayoutSizeFitting(
            CGSize(width: width, height: UIView.layoutFittingCompressedSize.height),
            withHorizontalFittingPriority: .required,
            verticalFittingPriority: .fittingSizeLevel).height
        if fitted > 0 { return fitted }
        return max(0, view.frame.height)
        #else
        let intrinsic = view.intrinsicContentSize.height
        if intrinsic != NSView.noIntrinsicMetric, intrinsic > 0 { return intrinsic }
        // A width to fit at, for a view whose height follows from it. Only where
        // the view lays itself out by constraints: one sized by its autoresizing
        // mask has no fitting size to ask for.
        var fitted: CGFloat = 0
        if !view.translatesAutoresizingMaskIntoConstraints {
            let pin = view.widthAnchor.constraint(equalToConstant: width)
            pin.isActive = true
            fitted = view.fittingSize.height
            pin.isActive = false
        } else {
            fitted = view.fittingSize.height
        }
        if fitted > 0 { return fitted }
        return max(0, view.frame.height)
        #endif
    }
}
