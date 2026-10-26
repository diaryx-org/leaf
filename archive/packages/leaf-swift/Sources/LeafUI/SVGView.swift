//  SVGView.swift
//
//  An SVG drawn as vectors, at its own size or fitted into a smaller box, for
//  a host whose region editor draws a picture: a Mermaid fence's diagram, as
//  leaf-mermaid renders it. The drawing goes through resvg-swift, as a
//  formula's and a media `.svg`'s do, with the system's fonts loaded for SVG
//  `<text>`. leaf's own views do not use it.

import CoreGraphics
import Foundation
import ResvgCoreGraphics
import ResvgFFI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// An SVG document drawn into a view: at its natural size where there is
/// room, scaled down to fit where there is not, and centred.
public final class SVGView: LeafView {
    private var picture: SVGPicture?

    /// The SVG's natural size in points, or zero with nothing to draw.
    public var naturalSize: CGSize { picture?.size ?? .zero }

    /// A view drawing `svg`; it draws nothing if `svg` does not parse.
    public init(svg: String) {
        super.init(frame: .zero)
        setSVG(svg)
        #if canImport(UIKit)
        isOpaque = false
        backgroundColor = .clear
        contentMode = .redraw
        #endif
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    /// Draw `svg` instead. Whether it parsed.
    @discardableResult
    public func setSVG(_ svg: String) -> Bool {
        picture = svg.data(using: .utf8).flatMap { try? SVGPicture(data: $0, options: ParseOptions(loadSystemFonts: true)) }
        invalidateIntrinsicContentSize()
        #if canImport(UIKit)
        setNeedsDisplay()
        #else
        needsDisplay = true
        #endif
        return picture != nil
    }

    public override var intrinsicContentSize: CGSize {
        guard let picture else { return CGSize(width: -1, height: -1) }
        return picture.size
    }

    /// Where the picture goes in `bounds`: its own size, or fitted, centred.
    private func box(in bounds: CGRect) -> CGRect {
        guard let size = picture?.size, size.width > 0, size.height > 0 else { return .zero }
        let scale = min(1, bounds.width / size.width, bounds.height / size.height)
        let w = size.width * scale, h = size.height * scale
        return CGRect(x: bounds.midX - w / 2, y: bounds.midY - h / 2, width: w, height: h)
    }

    #if canImport(UIKit)
    public override func draw(_ rect: CGRect) {
        guard let picture, let ctx = UIGraphicsGetCurrentContext() else { return }
        picture.draw(in: ctx, rect: box(in: bounds), scale: window?.screen.scale ?? 2)
    }
    #else
    public override var isFlipped: Bool { true }

    public override func draw(_ dirtyRect: NSRect) {
        guard let picture, let ctx = NSGraphicsContext.current?.cgContext else { return }
        picture.draw(in: ctx, rect: box(in: bounds), scale: window?.backingScaleFactor ?? 2)
    }
    #endif
}
