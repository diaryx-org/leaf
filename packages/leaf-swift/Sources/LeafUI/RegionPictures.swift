//  RegionPictures.swift
//
//  A region's picture, and the renderers that draw them. Each region kind —
//  `math`, `fence:<lang>`, `directive:<name>` — has a renderer that turns the
//  region's content into a `RegionPicture`: vectors with a size and a
//  baseline. The layout places the picture itself, inline on the baseline or
//  as a block over the region's rows, and draws it with the text, so a
//  region's picture pages, prints and exports the way the text around it
//  does. Leaf registers the `math` renderer (leaf-math, through the binding);
//  a host registers any other kind, and may replace `math`'s. A kind with no
//  renderer keeps leaf's placeholder row. See docs/proposals/embedded-editors.md.
//
//  ## Inline and block
//
//  An **inline** picture — a formula in a line of prose — becomes an
//  `NSTextAttachment` standing in for the run's one character, sized to the
//  picture and dropped by its depth so the text baseline runs through it at
//  its baseline. The character count of the row is unchanged — the attachment
//  character is one UTF-16 unit, as core's `∑` is — so every `caret_ch` still
//  lines up. TextKit draws the attachment; Core Text, which the views measure
//  and hit-test with, is told the same width and rise by a run delegate on the
//  same character, so the two agree on where the caret's homes either side of
//  the picture are.
//
//  A **block** picture is a `PictureLayout`, the peer of `MediaLayout`: one
//  box on the placeholder row, the fillers collapsed under it, centred on the
//  column.
//
//  ## Caching
//
//  Each view keeps the pictures its renderers drew (`RegionPictures`), by
//  everything that changes one — the kind, the content, the size, the ink and
//  the appearance — so a frame that shows the same regions as the last one
//  draws nothing new. A renderer whose picture changes for a reason leaf cannot
//  see (its data loaded) asks for `redrawRegions(kind:)`.

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

/// A region drawn: vectors with a size and a baseline, in points.
public struct RegionPicture {
    /// Its width, or nil to take the whole column a block stands in — a card
    /// that spans the text. An inline picture needs a width.
    public var width: CGFloat?
    public var height: CGFloat
    /// Where the text baseline runs through it, down from its top: the rise
    /// above the baseline of an inline picture. A block ignores it.
    public var baseline: CGFloat
    /// Draw into `rect` of a y-down context, at whatever size `rect` is — a
    /// block too wide for the column is drawn fitted to it.
    public let draw: (CGContext, CGRect) -> Void

    public init(width: CGFloat?, height: CGFloat, baseline: CGFloat? = nil,
                draw: @escaping (CGContext, CGRect) -> Void) {
        self.width = width
        self.height = height
        self.baseline = baseline ?? height
        self.draw = draw
    }

    /// An SVG document drawn as vectors through resvg, at `width` × `height`
    /// points — its own size where those are nil. `systemFonts` loads the
    /// system's fonts for SVG `<text>`; an SVG of outlines alone (a formula)
    /// needs none. Nil when it does not parse.
    public init?(svg: String, width: CGFloat? = nil, height: CGFloat? = nil, baseline: CGFloat? = nil,
                 systemFonts: Bool = true) {
        guard let data = svg.data(using: .utf8),
              let picture = try? SVGPicture(data: data, options: ParseOptions(loadSystemFonts: systemFonts))
        else { return nil }
        let w = width ?? picture.size.width, h = height ?? picture.size.height
        self.init(width: w, height: h, baseline: baseline) { ctx, rect in
            picture.draw(in: ctx, rect: rect, scale: hypot(ctx.ctm.a, ctx.ctm.b))
        }
    }

    /// A bitmap, drawn at `size` points.
    public init(image: CGImage, size: CGSize, baseline: CGFloat? = nil) {
        self.init(width: size.width, height: size.height, baseline: baseline) { ctx, rect in
            // A y-down context: flip about the rect, so the image is upright.
            ctx.saveGState()
            ctx.translateBy(x: rect.minX, y: rect.maxY)
            ctx.scaleBy(x: 1, y: -1)
            ctx.draw(image, in: CGRect(origin: .zero, size: rect.size))
            ctx.restoreGState()
        }
    }

    /// The reach below the baseline.
    var depth: CGFloat { height - baseline }

    /// The largest a picture may measure either way, in points — dozens of
    /// screens. TeX will typeset `\rule{1e20em}{1em}` without complaint, and a
    /// box that size is an integer overflow when it is turned into pixels and
    /// an allocation nothing can make well before that.
    static let maxExtent: CGFloat = 16_384

    /// Whether the metrics describe a box a surface can lay out and draw:
    /// finite, not inside out, and within `maxExtent`. One that isn't is
    /// treated as a picture the renderer could not draw, and leaf's placeholder
    /// stands in for it.
    var isDrawable: Bool {
        let w = width ?? 0
        return [w, height, baseline].allSatisfy { $0.isFinite && abs($0) <= Self.maxExtent }
            && w >= 0 && height >= 0
    }
}

/// What a renderer draws a region for.
public struct RegionRenderContext {
    /// The text size around the region, in points per em: the run's for an
    /// inline formula, the body's for a block.
    public let size: CGFloat
    /// The text colour, for a picture drawn in the text's ink.
    public let ink: LeafColor
    /// Whether the surface is dark — a printed sheet never is.
    public let dark: Bool

    public init(size: CGFloat, ink: LeafColor, dark: Bool) {
        self.size = size
        self.ink = ink
        self.dark = dark
    }
}

/// Draws the regions of one kind: the region's picture, or nil for leaf's
/// placeholder. Called on the main thread, as the layout needs the picture;
/// the answer is kept while the region's content and the context stand.
public typealias RegionRenderer = (RegionView, RegionRenderContext) -> RegionPicture?

/// The renderers a view draws its regions with — the host's, over leaf's own —
/// and the pictures they drew.
final class RegionPictures {
    /// Leaf's own renderers: `math`, through leaf-math.
    static let leafRenderers: [String: RegionRenderer] = [MathRenderer.kind: MathRenderer.render]

    /// The host's renderers, by kind. Each replaces leaf's of the same kind.
    var renderers: [String: RegionRenderer] = [:] {
        didSet { invalidate() }
    }

    /// Bumped whenever a picture drawn before may no longer stand, so a layout
    /// knows not to keep rows laid out under the old one.
    private(set) var generation = 0

    /// The ink is resolved to its bytes when the key is made: a dynamic
    /// colour reads differently under each appearance.
    private struct Key: Hashable {
        let kind: String
        let content: String
        let display: Bool
        let inline: Bool
        let size: CGFloat
        let ink: [UInt8]
        let dark: Bool
    }

    private var cache: [Key: RegionPicture?] = [:]
    private static let cap = 512

    /// The pictures of leaf's own renderers, for a layout with no view behind
    /// it — the tests'.
    static let leaf = RegionPictures()

    func renderer(for kind: String) -> RegionRenderer? {
        renderers[kind] ?? Self.leafRenderers[kind]
    }

    /// The kinds with a renderer, leaf's included.
    var kinds: Set<String> { Set(renderers.keys).union(Self.leafRenderers.keys) }

    /// `region`'s picture in `context`, drawn on first use; nil for a kind
    /// with no renderer, or a picture its renderer declined or could not draw.
    func picture(_ region: RegionView, _ context: RegionRenderContext) -> RegionPicture? {
        guard let render = renderer(for: region.kind) else { return nil }
        let c = context.ink.rgbaBytes
        let key = Key(kind: region.kind, content: region.content, display: region.display,
                      inline: region.inline, size: context.size, ink: [c.0, c.1, c.2, c.3], dark: context.dark)
        if let hit = cache[key] { return hit }
        if cache.count >= Self.cap { cache.removeAll(keepingCapacity: true) }
        let picture = render(region, context).flatMap { p -> RegionPicture? in
            guard p.isDrawable, !region.inline || p.width != nil else { return nil }
            return p
        }
        cache[key] = .some(picture)
        return picture
    }

    /// Forget the pictures of `kind`, or of every kind.
    func invalidate(kind: String? = nil) {
        if let kind { cache = cache.filter { $0.key.kind != kind } } else { cache.removeAll() }
        generation += 1
    }
}

/// What the layout asks of a view's regions: their pictures, the open one's
/// from its session, in the view's appearance.
struct RegionDrawing {
    let pictures: RegionPictures
    let open: OpenRegionLayout?
    let dark: Bool
    /// The pictures' generation when this drawing was taken.
    private let generation: Int

    init(pictures: RegionPictures = .leaf, open: OpenRegionLayout? = nil, dark: Bool = false) {
        self.pictures = pictures
        self.open = open
        self.dark = dark
        generation = pictures.generation
    }

    /// The picture for `region` at `size` points per em in `ink`: the open
    /// region's from its session, when leaf draws it; every other's from its
    /// kind's renderer.
    func picture(_ region: RegionView, size: CGFloat, ink: LeafColor) -> RegionPicture? {
        if let open, open.kind == region.kind, open.src == region.src, let picture = open.picture {
            return picture(size, ink)
        }
        return pictures.picture(region, RegionRenderContext(size: size, ink: ink, dark: dark))
    }

    /// Whether `kind` has a renderer.
    func draws(_ kind: String) -> Bool { pictures.renderer(for: kind) != nil }

    /// Whether every closed region draws as it did in `other`: the same
    /// renderers, with nothing redrawn since, in the same appearance. The open
    /// region is `OpenRegionLayout.same`'s to compare.
    func sameClosed(as other: RegionDrawing) -> Bool {
        pictures === other.pictures && generation == other.generation && dark == other.dark
    }
}

extension DocView {
    /// The region formula `math` is: the frame's own, where the view owns
    /// formulas, or a stand-in for it.
    func region(for math: MathView) -> RegionView {
        regions.first { $0.kind == MathRenderer.kind && $0.src == math.src } ?? Self.standIn(for: math)
    }

    /// A region standing for formula `math` where the view does not own
    /// formulas: what a renderer reads — its kind, its TeX, inline or not, its
    /// style — and no ranges to edit.
    static func standIn(for math: MathView) -> RegionView {
        RegionView(kind: MathRenderer.kind, inline: math.inline, display: math.display,
                   name: "", label: "", attrs: [],
                   startRow: math.startRow, endRow: math.endRow, src: math.src, end: math.src,
                   contentStart: 0, contentEnd: 0, content: math.tex)
    }
}

/// Leaf's renderer for formulas: the TeX typeset by leaf-math, through the
/// binding, as an SVG of outlines with its baseline metrics.
enum MathRenderer {
    /// The region kind every formula is.
    static let kind = "math"

    static func render(_ region: RegionView, _ context: RegionRenderContext) -> RegionPicture? {
        MathStore.picture(tex: region.content, display: region.display, size: context.size, ink: context.ink)
    }
}

/// The typeset-formula cache, shared by every view in the process: a formula
/// is the same picture whichever surface asks. A formula that failed to
/// typeset is remembered as `nil`, so the fault is paid once and the surface
/// falls back to core's own glyphs for it.
enum MathStore {
    private static var cache: [String: RegionPicture?] = [:]
    private static let cap = 512

    /// The picture for `tex` at `size` points in `ink`, typeset on first use.
    static func picture(tex: String, display: Bool, size: CGFloat, ink: LeafColor) -> RegionPicture? {
        let rgba = ink.rgbaBytes
        let key = "\(display ? "D" : "T")\(size)|\(rgba)|\(tex)"
        if let hit = cache[key] { return hit }
        if cache.count >= cap { cache.removeAll(keepingCapacity: true) }
        let picture = (try? typesetMath(tex: tex, display: display, size: Double(size),
                                        r: rgba.0, g: rgba.1, b: rgba.2, a: rgba.3))
            .flatMap { picture(from: $0, size: size) }
        cache[key] = picture
        return picture
    }

    /// A typeset picture parsed and measured in points at `size`; `nil` when
    /// its SVG does not parse or its box is not one a surface can draw.
    static func picture(from p: MathPicture, size: CGFloat) -> RegionPicture? {
        let ascent = CGFloat(p.height) * size
        guard let picture = RegionPicture(svg: p.svg, width: CGFloat(p.width) * size,
                                          height: ascent + CGFloat(p.depth) * size,
                                          baseline: ascent, systemFonts: false),
              picture.isDrawable
        else { return nil }
        return picture
    }
}

/// The box one block region's picture occupies — the peer of `MediaLayout`,
/// drawn centred on the column, with the media box's breathing room.
struct PictureLayout {
    let region: RegionView
    let picture: RegionPicture
    /// The picture's drawn size in points, fitted to the column if it is wider.
    let size: CGSize
    /// The total height the row reserves: the picture plus the gap above and
    /// below that keeps it from crowding the prose, as a media box has.
    var height: CGFloat { size.height + MediaMetrics.gap * 2 }

    init(_ region: RegionView, picture: RegionPicture, contentWidth: CGFloat) {
        self.region = region
        self.picture = picture
        // Never scaled up, and fitted to the column when it would run off it;
        // a picture with no width of its own takes the column's.
        let maxW = max(MediaMetrics.minWidth, contentWidth)
        let natural = CGSize(width: picture.width ?? maxW, height: picture.height)
        let scale = natural.width > maxW && natural.width > 0 ? maxW / natural.width : 1
        self.size = CGSize(width: (natural.width * scale).rounded(),
                           height: (natural.height * scale).rounded())
    }

    /// How much smaller than its own size the picture is drawn.
    var scale: CGFloat {
        guard let w = picture.width, w > 0 else { return 1 }
        return size.width / w
    }

    /// The box in view coordinates, centred in a column `width` wide whose left
    /// edge is `left`, given the top of the region's reserved row.
    func rect(top: CGFloat, left: CGFloat, width: CGFloat) -> CGRect {
        let x = left + max(0, (width - size.width) / 2)
        return CGRect(x: x, y: top + MediaMetrics.gap, width: size.width, height: size.height)
    }

    /// Paint the picture into `rect` of a flipped context — see `MediaStill.draw`.
    func draw(in rect: CGRect, ctx: CGContext) {
        ctx.saveGState()
        picture.draw(ctx, rect)
        ctx.restoreGState()
    }
}

/// The attachment an inline picture's character carries: TextKit sizes the
/// character to `bounds` and draws the picture there.
final class PictureAttachment: NSTextAttachment {
    let picture: RegionPicture

    /// `picture` must have a width.
    init(picture: RegionPicture) {
        self.picture = picture
        super.init(data: nil, ofType: nil)
        // Origin below the baseline by the depth, so the baseline runs
        // through the picture at its baseline from the top.
        bounds = CGRect(x: 0, y: -picture.depth, width: picture.width ?? 0, height: picture.height)
        image = PictureAttachment.render(picture)
    }

    required init?(coder: NSCoder) { fatalError("PictureAttachment is never archived") }

    /// The most pixels an inline picture's bitmap holds: 16 MB of RGBA, a
    /// picture some 650 points square at 3×.
    private static let maxPixels: CGFloat = 4_000_000

    /// The picture as a bitmap at 3× — crisp on every Retina density, and
    /// small: a formula is a few hundred points across at most. TextKit scales
    /// it into `bounds`, and an inline picture drawn through this path in a PDF
    /// is a raster at that density; the block path draws vectors.
    ///
    /// A picture that is not small is drawn at less than 3×, down to whatever
    /// keeps the bitmap within `maxPixels` — soft rather than a bitmap of
    /// hundreds of megabytes for one character.
    private static func render(_ picture: RegionPicture) -> LeafImage? {
        let size = CGSize(width: picture.width ?? 0, height: picture.height)
        let area = size.width * size.height
        let scale = area > 0 ? min(3, (maxPixels / area).squareRoot()) : 3
        let w = Int((size.width * scale).rounded(.up)), h = Int((size.height * scale).rounded(.up))
        guard w > 0, h > 0,
              let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return nil }
        // A bitmap context is y-up and a picture draws y-down: flip once, then
        // draw at the picture's own size scaled to the density.
        ctx.translateBy(x: 0, y: CGFloat(h))
        ctx.scaleBy(x: scale, y: -scale)
        picture.draw(ctx, CGRect(origin: .zero, size: size))
        guard let cg = ctx.makeImage() else { return nil }
        #if canImport(UIKit)
        return UIImage(cgImage: cg, scale: scale, orientation: .up)
        #else
        return NSImage(cgImage: cg, size: size)
        #endif
    }
}

/// A Core Text run delegate reporting the attachment's geometry, so a `CTLine`
/// over the same string measures the picture's character as the picture's
/// width and rise — what the caret rect and hit-testing read.
enum PictureRunDelegate {
    static func make(_ picture: RegionPicture) -> CTRunDelegate? {
        struct Metrics { let ascent, descent, width: CGFloat }
        final class Box { let m: Metrics; init(_ m: Metrics) { self.m = m } }
        var callbacks = CTRunDelegateCallbacks(
            version: kCTRunDelegateCurrentVersion,
            dealloc: { p in Unmanaged<Box>.fromOpaque(p).release() },
            getAscent: { p in Unmanaged<Box>.fromOpaque(p).takeUnretainedValue().m.ascent },
            getDescent: { p in Unmanaged<Box>.fromOpaque(p).takeUnretainedValue().m.descent },
            getWidth: { p in Unmanaged<Box>.fromOpaque(p).takeUnretainedValue().m.width }
        )
        let metrics = Metrics(ascent: picture.baseline, descent: picture.depth, width: picture.width ?? 0)
        let box = Unmanaged.passRetained(Box(metrics)).toOpaque()
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
