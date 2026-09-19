//  ZoomView.swift  (UIKit / iOS)
//
//  The view between a scroll view and a zoomed `LeafTextView`. The text view
//  zooms by its own `transform`, and Auto Layout does not read transforms: a
//  constraint pins the untransformed frame, so a text view scaled to 200% and
//  pinned edge to edge would be laid out at its unscaled size and drawn at twice
//  it, spilling out of the scroll view's content. This wrapper is what the
//  constraints pin instead. Its intrinsic size is the text view's scaled by the
//  zoom, which is what the scroll view has to scroll, and its layout sets the
//  text view's unscaled bounds from its own — so a zoom of 2 is a text view half
//  as wide in layout points, drawn at twice the size, exactly filling it.
//
//  `LeafEditor` puts one of these in its scroll view. A UIKit host that embeds
//  `LeafTextView` in a scroll view of its own does the same: the wrapper, not
//  the text view, is what its constraints reach.

#if canImport(UIKit)
import UIKit

public final class LeafZoomView: UIView {
    public let textView: LeafTextView

    public init(textView: LeafTextView) {
        self.textView = textView
        super.init(frame: .zero)
        // The transform scales about the anchor, and the anchor has to be the
        // top-left for the scaled view to start where the unscaled one does.
        // The position is then the origin, which `layoutSubviews` keeps at zero.
        textView.layer.anchorPoint = .zero
        textView.translatesAutoresizingMaskIntoConstraints = true
        textView.autoresizingMask = []
        addSubview(textView)
        // A sheet is a fixed width: wider than the viewport, it is this view
        // that holds its ground and the scroll view that scrolls sideways.
        setContentCompressionResistancePriority(.required, for: .horizontal)
        setContentHuggingPriority(.defaultLow, for: .horizontal)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    /// The text view's own size, scaled. No width in the continuous flow, which
    /// fills whatever it is given.
    public override var intrinsicContentSize: CGSize {
        let s = textView.zoomScale
        let inner = textView.intrinsicContentSize
        let layoutWidth = textView.layoutContentWidth
        let width = layoutWidth > 0 ? layoutWidth * s : UIView.noIntrinsicMetric
        return CGSize(width: width, height: inner.height * s)
    }

    public override func layoutSubviews() {
        super.layoutSubviews()
        // A fit follows the viewport, and this is where a changed viewport
        // arrives — a rotation, a split view resized.
        textView.refitZoom()
        let s = textView.zoomScale
        let size = CGSize(width: bounds.width / s, height: bounds.height / s)
        if abs(textView.bounds.width - size.width) > 0.01 || abs(textView.bounds.height - size.height) > 0.01 {
            textView.bounds = CGRect(origin: .zero, size: size)
        }
        textView.layer.position = .zero
        let transform = CGAffineTransform(scaleX: s, y: s)
        if textView.transform != transform { textView.transform = transform }
    }
}
#endif
