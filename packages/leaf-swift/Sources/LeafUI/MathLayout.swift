//  MathLayout.swift
//
//  Formulas as pictures. Core hands the frame a `MathView` per formula standing
//  as a picture — inline, one `math` run on its row; block, a placeholder row
//  and its fillers — and `typesetMath` (leaf-math, through the binding) turns
//  its TeX into a standalone SVG with its baseline metrics. This file is what
//  the views do with the two:
//
//  - An **inline** formula becomes an `NSTextAttachment` standing in for the
//    run's one character, sized to the picture and dropped by its depth so the
//    text baseline runs through it where TeX put it. The character count of the
//    row is unchanged — the attachment character is one UTF-16 unit, as core's
//    `∑` is — so every `caret_ch` still lines up. TextKit draws the attachment;
//    Core Text, which the views measure and hit-test with, is told the same
//    width and rise by a run delegate on the same character, so the two agree
//    on where the caret's homes either side of the picture are.
//
//  - A **block** formula is a `MathLayout`, the peer of `MediaLayout`: one box
//    on the placeholder row, the fillers collapsed under it, drawn centred on
//    the column as vectors through resvg-swift.
//
//  Typesetting is pure layout over fonts embedded in the binary, but not free,
//  so `MathStore` remembers each picture by everything that changes it — the
//  TeX, the style, the size and the ink — and a frame that shows the same
//  formulas as the last one typesets nothing.

import CoreGraphics
import CoreText
import Foundation
import LeafFFI
import ResvgCoreGraphics
import ResvgFFI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// A typeset formula: the parsed picture and its metrics in points at the size
/// it was set.
struct MathGlyph {
    let picture: SVGPicture
    /// The advance, in points.
    let width: CGFloat
    /// The rise above the baseline, in points — the attachment's ascent.
    let ascent: CGFloat
    /// The reach below it, in points — the descent.
    let descent: CGFloat
    var size: CGSize { CGSize(width: width, height: ascent + descent) }
}

/// The typeset-formula cache, shared by every view in the process: a formula
/// is the same picture whichever surface asks. A formula that failed to
/// typeset is remembered as `nil`, so the fault is paid once and the surface
/// falls back to core's own glyphs for it.
enum MathStore {
    private static var cache: [String: MathGlyph?] = [:]
    private static let cap = 512

    /// The picture for `tex` at `size` points in `ink`, typeset on first use.
    static func glyph(tex: String, display: Bool, size: CGFloat, ink: LeafColor) -> MathGlyph? {
        let rgba = ink.rgbaBytes
        let key = "\(display ? "D" : "T")\(size)|\(rgba)|\(tex)"
        if let hit = cache[key] { return hit }
        if cache.count >= cap { cache.removeAll(keepingCapacity: true) }
        let glyph: MathGlyph? = {
            guard let p = try? typesetMath(tex: tex, display: display, size: Double(size),
                                           r: rgba.0, g: rgba.1, b: rgba.2, a: rgba.3),
                  let data = p.svg.data(using: .utf8),
                  let picture = try? SVGPicture(data: data)
            else { return nil }
            return MathGlyph(picture: picture,
                             width: CGFloat(p.width) * size,
                             ascent: CGFloat(p.height) * size,
                             descent: CGFloat(p.depth) * size)
        }()
        cache[key] = glyph
        return glyph
    }
}

/// The box one display formula occupies — the peer of `MediaLayout`, drawn
/// centred on the column as vectors, with the media box's breathing room.
struct MathLayout {
    let math: MathView
    let glyph: MathGlyph
    /// The picture's drawn size in points, fitted to the column if it is wider.
    let size: CGSize
    /// The total height the row reserves: the picture plus the gap above and
    /// below that keeps it from crowding the prose, as a media box has.
    var height: CGFloat { size.height + MediaMetrics.gap * 2 }

    init?(_ math: MathView, contentWidth: CGFloat, theme: EditorTheme) {
        guard let glyph = MathStore.glyph(tex: math.tex, display: math.display,
                                          size: theme.fontSize, ink: theme.textColor)
        else { return nil }
        self.math = math
        self.glyph = glyph
        let natural = glyph.size
        // Never scaled up — the picture is already at the body size — and
        // fitted to the column when a long equation would run off it.
        let maxW = max(MediaMetrics.minWidth, contentWidth)
        let scale = natural.width > maxW && natural.width > 0 ? maxW / natural.width : 1
        self.size = CGSize(width: (natural.width * scale).rounded(),
                           height: (natural.height * scale).rounded())
    }

    /// The box in view coordinates, centred in a column `width` wide whose left
    /// edge is `left`, given the top of the formula's reserved row.
    func rect(top: CGFloat, left: CGFloat, width: CGFloat) -> CGRect {
        let x = left + max(0, (width - size.width) / 2)
        return CGRect(x: x, y: top + MediaMetrics.gap, width: size.width, height: size.height)
    }

    /// Paint the picture into `rect` of a flipped context — see `MediaStill.draw`.
    func draw(in rect: CGRect, ctx: CGContext) {
        let scale = hypot(ctx.ctm.a, ctx.ctm.b)
        glyph.picture.draw(in: ctx, rect: rect, scale: scale)
    }
}

/// The attachment an inline formula's character carries: TextKit sizes the
/// character to `bounds` and draws the picture there.
final class MathAttachment: NSTextAttachment {
    let glyph: MathGlyph

    init(glyph: MathGlyph) {
        self.glyph = glyph
        super.init(data: nil, ofType: nil)
        // Origin below the baseline by the descent, so the baseline runs
        // through the picture at its ascent from the top.
        bounds = CGRect(x: 0, y: -glyph.descent, width: glyph.width, height: glyph.ascent + glyph.descent)
        image = MathAttachment.render(glyph)
    }

    required init?(coder: NSCoder) { fatalError("MathAttachment is never archived") }

    /// The picture as a bitmap at 3× — crisp on every Retina density, and
    /// small: a formula is a few hundred points across at most. TextKit scales
    /// it into `bounds`, and a formula drawn through this path in a PDF is a
    /// raster at that density; the block path draws vectors.
    private static func render(_ glyph: MathGlyph) -> LeafImage? {
        let scale: CGFloat = 3
        let w = Int((glyph.width * scale).rounded(.up)), h = Int(((glyph.ascent + glyph.descent) * scale).rounded(.up))
        guard w > 0, h > 0,
              let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return nil }
        // SVG is y-down and a bitmap context is y-up: flip once, then draw at
        // the picture's own size scaled to the density.
        ctx.translateBy(x: 0, y: CGFloat(h))
        ctx.scaleBy(x: scale, y: -scale)
        glyph.picture.draw(in: ctx, rect: CGRect(origin: .zero, size: glyph.size), scale: scale)
        guard let cg = ctx.makeImage() else { return nil }
        #if canImport(UIKit)
        return UIImage(cgImage: cg, scale: scale, orientation: .up)
        #else
        return NSImage(cgImage: cg, size: glyph.size)
        #endif
    }
}

/// A Core Text run delegate reporting the attachment's geometry, so a `CTLine`
/// over the same string measures the formula's character as the picture's
/// width and rise — what the caret rect and hit-testing read.
enum MathRunDelegate {
    static func make(_ glyph: MathGlyph) -> CTRunDelegate? {
        final class Box { let g: MathGlyph; init(_ g: MathGlyph) { self.g = g } }
        var callbacks = CTRunDelegateCallbacks(
            version: kCTRunDelegateCurrentVersion,
            dealloc: { p in Unmanaged<Box>.fromOpaque(p).release() },
            getAscent: { p in Unmanaged<Box>.fromOpaque(p).takeUnretainedValue().g.ascent },
            getDescent: { p in Unmanaged<Box>.fromOpaque(p).takeUnretainedValue().g.descent },
            getWidth: { p in Unmanaged<Box>.fromOpaque(p).takeUnretainedValue().g.width }
        )
        let box = Unmanaged.passRetained(Box(glyph)).toOpaque()
        return CTRunDelegateCreate(&callbacks, box)
    }
}

extension LeafColor {
    /// The colour as sRGB bytes — the ink `typesetMath` takes.
    var rgbaBytes: (UInt8, UInt8, UInt8, UInt8) {
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 1
        #if canImport(UIKit)
        getRed(&r, green: &g, blue: &b, alpha: &a)
        #else
        let c = usingColorSpace(.sRGB) ?? self
        c.getRed(&r, green: &g, blue: &b, alpha: &a)
        #endif
        let byte = { (v: CGFloat) in UInt8(max(0, min(255, (v * 255).rounded()))) }
        return (byte(r), byte(g), byte(b), byte(a))
    }
}
