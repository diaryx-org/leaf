//  EditorTheme.swift
//
//  The presentation knobs — the peer of leaf-gpui's `EditorStyle` and
//  leaf-wasm's `DEFAULT_THEME`. Everything here is *look*, never model: it maps a
//  glyph's semantic `Role` (carried on each `Run` as a class id) to a font, size,
//  weight, and colour. Core decides *what a glyph is*; this decides *how it's
//  painted*. Cross-platform via the aliases in `Platform.swift`.
//
//  Headings are told apart by **size and weight alone** (no colour), matching the
//  gpui/web frontends — so `headingScale` is the whole hierarchy.

import CoreGraphics
import Foundation

public struct EditorTheme {
    /// Proportional body family — prose and headings shape with this.
    public var bodyFontName: String
    /// Monospace family — inline `code` and fenced blocks.
    public var monoFontName: String
    /// Body font size in points. A heading is this scaled by `headingScale`.
    public var fontSize: CGFloat
    /// Body line height in points. Heading rows scale taller in proportion.
    public var lineHeight: CGFloat
    /// How much larger than the body each heading level is, `[h1…h6]`.
    public var headingScale: [CGFloat]
    /// Horizontal/vertical text inset from the view's edges.
    public var padding: LeafInsets

    // Colours default to dynamic system colours (light/dark aware) per platform.
    public var textColor: LeafColor
    public var secondaryColor: LeafColor
    public var linkColor: LeafColor
    public var codeColor: LeafColor
    public var codeBackground: LeafColor
    public var markBackground: LeafColor
    public var quoteBarColor: LeafColor
    public var ruleColor: LeafColor
    public var selectionColor: LeafColor
    public var caretColor: LeafColor
    /// The drag-handle knobs on iOS selection (the loupe-free native peers).
    public var handleColor: LeafColor

    public init(
        bodyFontName: String = "Helvetica Neue",
        monoFontName: String = "Menlo",
        fontSize: CGFloat = 16,
        lineHeight: CGFloat = 24,
        headingScale: [CGFloat] = [1.625, 1.375, 1.1875, 1.0625, 1.0, 0.9375],
        padding: LeafInsets = LeafInsets(top: 12, left: 16, bottom: 12, right: 16),
        textColor: LeafColor = Palette.label,
        secondaryColor: LeafColor = Palette.secondary,
        linkColor: LeafColor = Palette.link,
        codeColor: LeafColor = Palette.label,
        codeBackground: LeafColor = Palette.codeBackground,
        markBackground: LeafColor = Palette.markBackground,
        quoteBarColor: LeafColor = Palette.tertiary,
        ruleColor: LeafColor = Palette.separator,
        selectionColor: LeafColor = Palette.selection,
        caretColor: LeafColor = Palette.label,
        handleColor: LeafColor = Palette.accent
    ) {
        self.bodyFontName = bodyFontName
        self.monoFontName = monoFontName
        self.fontSize = fontSize
        self.lineHeight = lineHeight
        self.headingScale = headingScale
        self.padding = padding
        self.textColor = textColor
        self.secondaryColor = secondaryColor
        self.linkColor = linkColor
        self.codeColor = codeColor
        self.codeBackground = codeBackground
        self.markBackground = markBackground
        self.quoteBarColor = quoteBarColor
        self.ruleColor = ruleColor
        self.selectionColor = selectionColor
        self.caretColor = caretColor
        self.handleColor = handleColor
    }

    public static let `default` = EditorTheme()

    // ── derived metrics ──────────────────────────────────────────────────────

    /// The ratio the line box grows relative to the font — a heading row scales
    /// its height in proportion to its larger size.
    var lineRatio: CGFloat { lineHeight / fontSize }

    /// The point size for a heading of `level` (1–6), clamped to the ramp.
    func headingSize(_ level: Int) -> CGFloat {
        let i = min(max(level, 1), 6) - 1
        return fontSize * headingScale[i]
    }

    /// The height of a row: the body line box, or a heading's scaled line box.
    func rowHeight(heading: UInt8?) -> CGFloat {
        guard let h = heading else { return lineHeight }
        return headingSize(Int(h)) * lineRatio
    }

    // ── fonts ────────────────────────────────────────────────────────────────

    /// A body/heading font at `size` with the requested emphasis traits.
    func proportionalFont(size: CGFloat, bold: Bool, italic: Bool) -> LeafFont {
        makeFont(name: bodyFontName, size: size, bold: bold, italic: italic)
    }

    /// A monospace font at `size` — inline `code` sits at the body size so it
    /// aligns with surrounding prose.
    func monospaceFont(size: CGFloat, bold: Bool, italic: Bool) -> LeafFont {
        makeFont(name: monoFontName, size: size, bold: bold, italic: italic)
    }
}
