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
import CoreText
import Foundation
import LeafFFI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

public struct EditorTheme {
    /// Proportional body family — prose and headings shape with this.
    public var bodyFontName: String
    /// Monospace family — inline `code` and fenced blocks.
    public var monoFontName: String
    /// Body font size in points. A heading is this scaled by `headingScale`.
    public var fontSize: CGFloat
    /// Body line height in points. Heading rows scale taller in proportion.
    public var lineHeight: CGFloat
    /// The height of a between-blocks gap row, as a fraction of `lineHeight`.
    /// Core spells a block boundary with a blank decoration row; drawn at a full
    /// line box it reads as an empty line the user didn't type. A fraction turns
    /// it into ordinary paragraph spacing. `1.0` restores the old full-line gap.
    public var blockGapScale: CGFloat
    /// The gap *above* a heading, as a multiple of the ordinary block gap. A
    /// heading belongs to the text under it, so the space that separates it from
    /// the block above should be the wider of its two margins — otherwise it
    /// floats between its neighbours and reads as belonging to neither.
    public var headingGapScale: CGFloat
    /// How much larger than the body each heading level is, `[h1…h6]`.
    public var headingScale: [CGFloat]
    /// The leading ratio (line box ÷ font size) at the *largest* heading on the
    /// ramp. Display type wants tighter leading than body text — set a two-line
    /// h1 at the body's ratio and its lines drift apart — so heading rows
    /// interpolate from `lineRatio` at body size down to this at the top of the
    /// ramp. See `lineRatio(forHeadingScale:)`.
    public var headingLineRatio: CGFloat
    /// The widest the text column may run, **in characters of the body font** —
    /// the classic typographic "measure". Nil fills whatever `padding` leaves.
    ///
    /// Counted in characters rather than points because that is the quantity
    /// legibility actually depends on (45–75 is the usual range, ~66 the classic
    /// target) and because it then survives a change of font or text size, which
    /// a point width doesn't. `padding.left`/`.right` become *minimum* insets: a
    /// column narrower than the room they leave is centred in it, and one wider
    /// is clamped down to it, so a narrow window simply reflows instead of
    /// scrolling sideways. See `column(in:)`.
    public var measure: CGFloat?
    /// Minimum horizontal/vertical text inset from the view's edges.
    public var padding: LeafInsets

    // Colours default to dynamic system colours (light/dark aware) per platform.
    public var textColor: LeafColor
    public var secondaryColor: LeafColor
    public var linkColor: LeafColor
    public var codeColor: LeafColor
    public var codeBackground: LeafColor
    /// A directive container's (`:::name{.class}`) dashed outline colour.
    public var directiveBorderColor: LeafColor
    public var markBackground: LeafColor
    /// The painted bar down a blockquote's left edge — one per nesting level.
    public var quoteBarColor: LeafColor
    /// The bar's thickness in points.
    public var quoteBarWidth: CGFloat
    /// The gutter one quote level occupies: the bar plus the space between it and
    /// the quoted text. Core spells the gutter `│ `, whose width is whatever the
    /// body font makes of it; the gutter run is sized to this instead, so the
    /// inset is the theme's and every level's bar lines up down the block.
    public var quoteIndent: CGFloat
    /// A thematic break's drawn line — colour and thickness.
    public var ruleColor: LeafColor
    public var ruleThickness: CGFloat
    /// Table chrome: the grid lines, the header row fill, and the body stripe.
    public var tableBorderColor: LeafColor
    public var tableHeaderColor: LeafColor
    public var tableStripeColor: LeafColor
    public var selectionColor: LeafColor
    /// The selection fill when the view isn't the focus — window not key, or the
    /// view not first responder. Matches native text: emphasized blue when active,
    /// this unemphasized grey otherwise. Only the macOS surface draws it (iOS lets
    /// the system overlay selection).
    public var inactiveSelectionColor: LeafColor
    public var caretColor: LeafColor
    /// The drag-handle knobs on iOS selection (the loupe-free native peers).
    public var handleColor: LeafColor
    /// The paginated view's chrome: the paper, the surface behind the stack, and
    /// the hairline round each sheet. Inert while no `PageSetup` is set — the
    /// continuous flow draws no paper — and pure colour either way, so changing
    /// one repaints rather than re-wrapping (see `metricsDiffer`).
    public var pageColor: LeafColor
    public var pageBackdropColor: LeafColor
    public var pageBorderColor: LeafColor

    public init(
        bodyFontName: String = "Helvetica Neue",
        monoFontName: String = "Menlo",
        fontSize: CGFloat = 16,
        lineHeight: CGFloat = 24,
        blockGapScale: CGFloat = 0.5,
        headingGapScale: CGFloat = 1.8,
        headingScale: [CGFloat] = [1.625, 1.375, 1.1875, 1.0625, 1.0, 0.9375],
        headingLineRatio: CGFloat = 1.2,
        measure: CGFloat? = 68,
        padding: LeafInsets = LeafInsets(top: 12, left: 16, bottom: 12, right: 16),
        textColor: LeafColor = Palette.label,
        secondaryColor: LeafColor = Palette.secondary,
        linkColor: LeafColor = Palette.link,
        codeColor: LeafColor = Palette.label,
        codeBackground: LeafColor = Palette.codeBackground,
        directiveBorderColor: LeafColor = Palette.directiveBorderColor,
        markBackground: LeafColor = Palette.markBackground,
        quoteBarColor: LeafColor = Palette.tertiary,
        quoteBarWidth: CGFloat = 3,
        quoteIndent: CGFloat = 22,
        ruleColor: LeafColor = Palette.separator,
        ruleThickness: CGFloat = 1,
        tableBorderColor: LeafColor = Palette.tableBorder,
        tableHeaderColor: LeafColor = Palette.tableHeader,
        tableStripeColor: LeafColor = Palette.tableStripe,
        selectionColor: LeafColor = Palette.selection,
        inactiveSelectionColor: LeafColor = Palette.inactiveSelection,
        caretColor: LeafColor = Palette.label,
        handleColor: LeafColor = Palette.accent,
        pageColor: LeafColor = Palette.page,
        pageBackdropColor: LeafColor = Palette.pageBackdrop,
        pageBorderColor: LeafColor = Palette.separator
    ) {
        self.bodyFontName = bodyFontName
        self.monoFontName = monoFontName
        self.fontSize = fontSize
        self.lineHeight = lineHeight
        self.blockGapScale = blockGapScale
        self.headingGapScale = headingGapScale
        self.headingScale = headingScale
        self.headingLineRatio = headingLineRatio
        self.measure = measure
        self.padding = padding
        self.textColor = textColor
        self.secondaryColor = secondaryColor
        self.linkColor = linkColor
        self.codeColor = codeColor
        self.codeBackground = codeBackground
        self.directiveBorderColor = directiveBorderColor
        self.markBackground = markBackground
        self.quoteBarColor = quoteBarColor
        self.quoteBarWidth = quoteBarWidth
        self.quoteIndent = quoteIndent
        self.ruleColor = ruleColor
        self.ruleThickness = ruleThickness
        self.tableBorderColor = tableBorderColor
        self.tableHeaderColor = tableHeaderColor
        self.tableStripeColor = tableStripeColor
        self.selectionColor = selectionColor
        self.inactiveSelectionColor = inactiveSelectionColor
        self.caretColor = caretColor
        self.handleColor = handleColor
        self.pageColor = pageColor
        self.pageBackdropColor = pageBackdropColor
        self.pageBorderColor = pageBorderColor
    }

    public static let `default` = EditorTheme()

    /// Whether moving from `other` to `self` changes the *geometry* — the only kind
    /// of theme change that needs a re-wrap/re-layout. A pure colour change just
    /// repaints. Lets a host re-apply an equal theme (SwiftUI re-runs `updateNSView`
    /// on every state change) without forcing a relayout, which would otherwise loop
    /// with the state publish and re-scroll the view to the caret every frame.
    func metricsDiffer(from other: EditorTheme) -> Bool {
        bodyFontName != other.bodyFontName
            || monoFontName != other.monoFontName
            || fontSize != other.fontSize
            || lineHeight != other.lineHeight
            || blockGapScale != other.blockGapScale
            || headingGapScale != other.headingGapScale
            || headingScale != other.headingScale
            || headingLineRatio != other.headingLineRatio
            || measure != other.measure
            || padding != other.padding
            // The quote gutter is stretched to `quoteIndent` at shaping time, so
            // it moves every quoted glyph — a geometry change, not a repaint.
            || quoteIndent != other.quoteIndent
    }

    // ── derived metrics ──────────────────────────────────────────────────────

    /// The ratio the line box grows relative to the font — the body's leading.
    var lineRatio: CGFloat { lineHeight / fontSize }

    /// The point size for a heading of `level` (1–6), clamped to the ramp.
    func headingSize(_ level: Int) -> CGFloat {
        let i = min(max(level, 1), 6) - 1
        return fontSize * headingScale[i]
    }

    /// The leading ratio for a heading set at `scale` times the body size.
    ///
    /// Leading and type size don't scale together: the bigger the type, the less
    /// space its lines need between them to stay distinct, and a display line set
    /// at body leading reads as two stranded lines rather than one heading. So
    /// this walks from `lineRatio` at body size to `headingLineRatio` at the
    /// largest scale on the ramp, which leaves h5/h6 (at or under body size) on
    /// the body's own leading and tightens only what's actually large.
    func lineRatio(forHeadingScale scale: CGFloat) -> CGFloat {
        let top = headingScale.max() ?? 1
        guard scale > 1, top > 1 else { return lineRatio }
        let t = min(1, (scale - 1) / (top - 1))
        return lineRatio + (headingLineRatio - lineRatio) * t
    }

    /// The height of a row: the body line box, or a heading's scaled line box at
    /// its own (tighter) leading.
    func rowHeight(heading: UInt8?) -> CGFloat {
        guard let h = heading else { return lineHeight }
        let scale = headingScale[min(max(Int(h), 1), 6) - 1]
        return fontSize * scale * lineRatio(forHeadingScale: scale)
    }

    /// The height a between-blocks gap row occupies — a fraction of the body line
    /// box, so a paragraph boundary reads as spacing rather than a blank line.
    var blockGap: CGFloat { lineHeight * blockGapScale }

    /// The height of the boundary core labelled `boundary` — nil for a gap row
    /// core didn't label (nothing does today, and the plain gap is the right
    /// answer if anything ever does).
    ///
    /// One gap for every boundary is what makes a document read as an undivided
    /// column of paragraphs. Real typography spaces a boundary by what it
    /// separates: a heading takes the wider of its two margins above, so it
    /// groups with the text it introduces rather than floating between two
    /// blocks. Which pair a gap falls between is core's answer (`Row.boundary`),
    /// not something re-derived here from glyph roles — see `leaf_core::Boundary`
    /// for why that division is where it is.
    func blockGap(_ boundary: Boundary?) -> CGFloat {
        boundary?.below == .heading ? blockGap * headingGapScale : blockGap
    }

    // ── the text column ──────────────────────────────────────────────────────

    /// The mean advance of a lowercase body character — what turns a
    /// character-count `measure` into points. Measured off the real font rather
    /// than assumed (the usual 0.5em guess is fine for some families and badly
    /// off for others), on the alphabet plus a space so the space's narrowness
    /// counts the way it does in prose.
    var averageCharWidth: CGFloat {
        let sample = "abcdefghijklmnopqrstuvwxyz "
        let attributed = NSAttributedString(
            string: sample,
            attributes: [.font: proportionalFont(size: fontSize, bold: false, italic: false)])
        let line = CTLineCreateWithAttributedString(attributed as CFAttributedString)
        let width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        return width / CGFloat(sample.count)
    }

    /// The text column inside a view `viewWidth` points wide: where it starts and
    /// how wide it is.
    ///
    /// `padding` is the floor and `measure` the ceiling. When the measure is the
    /// narrower of the two the column is centred in the room the padding leaves —
    /// which is what keeps a maximised window from setting 200-character lines —
    /// and when the window is the narrower one the column just shrinks to it. The
    /// origin is rounded so glyphs land on the same subpixel phase every frame.
    func column(in viewWidth: CGFloat) -> (originX: CGFloat, width: CGFloat) {
        let available = viewWidth - padding.left - padding.right
        guard let measure, measure > 0, available > 0 else {
            return (padding.left, max(0, available))
        }
        let width = min(available, measure * averageCharWidth)
        return ((padding.left + (available - width) / 2).rounded(), width)
    }

    /// The header strip reserved above a directive block's first row for its
    /// audience-name label, sized to the small font `drawDirectiveLabel` paints it
    /// with (see `LeafTextView`/`LeafTextViewiOS`) — so the label sits in its own
    /// space instead of over that row's real text.
    var directiveLabelHeight: CGFloat { fontSize * 0.75 + 4 }

    /// The laid-out height of `row`: a shrunk gap for a block-boundary decoration
    /// row (empty, holds no caret), otherwise its heading/body line box.
    func rowHeight(for row: Row) -> CGFloat {
        row.isBlockGap ? blockGap : rowHeight(heading: row.heading)
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
