//  EditorTheme.swift
//
//  The presentation knobs — the AppKit peer of leaf-gpui's `EditorStyle` and
//  leaf-wasm's `DEFAULT_THEME`. Everything here is *look*, never model: it maps a
//  glyph's semantic `Role` (carried on each `Run` as a class id string) to a
//  font, size, weight, and colour. Core decides *what a glyph is*; this decides
//  *how it's painted*, exactly as the TUI maps a role to a terminal colour and
//  the web maps it to a CSS class.
//
//  Headings are told apart by **size and weight alone** (no colour), matching the
//  gpui/web frontends — so `headingScale` is the whole hierarchy.

#if canImport(AppKit)
import AppKit

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
    /// 26 / 22 / 19 / 17 / 16 / 15 pt against a 16pt body — the gpui/web ramp.
    public var headingScale: [CGFloat]
    /// Horizontal text inset from the view's edges.
    public var padding: NSEdgeInsets

    // Colours default to dynamic system colours, so light/dark just works.
    public var textColor: NSColor
    public var secondaryColor: NSColor
    public var linkColor: NSColor
    public var codeColor: NSColor
    public var codeBackground: NSColor
    public var markBackground: NSColor
    public var quoteBarColor: NSColor
    public var ruleColor: NSColor
    public var selectionColor: NSColor
    public var caretColor: NSColor

    public init(
        bodyFontName: String = "Helvetica Neue",
        monoFontName: String = "Menlo",
        fontSize: CGFloat = 16,
        lineHeight: CGFloat = 24,
        headingScale: [CGFloat] = [1.625, 1.375, 1.1875, 1.0625, 1.0, 0.9375],
        padding: NSEdgeInsets = NSEdgeInsets(top: 12, left: 16, bottom: 12, right: 16),
        textColor: NSColor = .labelColor,
        secondaryColor: NSColor = .secondaryLabelColor,
        linkColor: NSColor = .linkColor,
        codeColor: NSColor = .labelColor,
        codeBackground: NSColor = NSColor.secondaryLabelColor.withAlphaComponent(0.08),
        markBackground: NSColor = NSColor.systemYellow.withAlphaComponent(0.28),
        quoteBarColor: NSColor = .tertiaryLabelColor,
        ruleColor: NSColor = .separatorColor,
        selectionColor: NSColor = .selectedTextBackgroundColor,
        caretColor: NSColor = .labelColor
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
    }

    public static let `default` = EditorTheme()

    // ── derived metrics ──────────────────────────────────────────────────────

    /// The ratio the line box grows relative to the font — kept so a heading row
    /// scales its height in proportion to its larger size.
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
    func proportionalFont(size: CGFloat, bold: Bool, italic: Bool) -> NSFont {
        font(name: bodyFontName, size: size, bold: bold, italic: italic)
    }

    /// A monospace font at `size` with the requested emphasis traits — inline
    /// `code` sits at the body size so it aligns with surrounding prose.
    func monospaceFont(size: CGFloat, bold: Bool, italic: Bool) -> NSFont {
        font(name: monoFontName, size: size, bold: bold, italic: italic)
    }

    private func font(name: String, size: CGFloat, bold: Bool, italic: Bool) -> NSFont {
        let base = NSFont(name: name, size: size)
            ?? NSFont.systemFont(ofSize: size)
        var traits: NSFontDescriptor.SymbolicTraits = []
        if bold { traits.insert(.bold) }
        if italic { traits.insert(.italic) }
        guard !traits.isEmpty else { return base }
        let desc = base.fontDescriptor.withSymbolicTraits(traits)
        return NSFont(descriptor: desc, size: size) ?? base
    }
}
#endif
