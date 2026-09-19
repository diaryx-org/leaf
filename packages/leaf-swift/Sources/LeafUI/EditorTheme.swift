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
    /// Proportional body family — prose and headings shape with this. A family
    /// name, or `systemFontName` for the system's own face (the default).
    public var bodyFontName: String
    /// Monospace family — inline `code` and fenced blocks. A family name, or
    /// `systemMonospacedFontName` (the default).
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
    /// How much smaller a superscript or subscript is set than the text it rides
    /// on, as a fraction of that text's size. Sized off the *run's* size rather
    /// than the body's, so a footnote reference in a heading stays in proportion
    /// to the heading.
    public var baselineScale: CGFloat
    /// How far a superscript is raised, as a fraction of the run's size. A
    /// subscript is lowered by `baselineSubScale` of the same.
    ///
    /// Both are deliberately modest: a row's height is the theme's, not the
    /// measured glyphs' (see `rowHeight(for:)`), so a raised glyph has only the
    /// line box's leading to grow into and a bold shift would clip against the
    /// row above.
    public var baselineSuperShift: CGFloat
    /// How far a subscript is lowered, as a fraction of the run's size. Smaller
    /// than `baselineSuperShift` because a descender has less room under the
    /// baseline than an ascender has above it.
    public var baselineSubShift: CGFloat
    /// How much larger or smaller a run set to one of the presentation
    /// vocabulary's size steps is than the text around it — a multiple of the
    /// size that run would otherwise take, keyed by the step's own name
    /// (`xx-small`…`xxx-large`, the `data-size` token core carries on `Run.size`).
    ///
    /// A multiple rather than a point size, which is the whole reason the
    /// document says `large` and not `18pt`: a step scales whatever it is applied
    /// to, so `large` on a heading is a step up from *that heading* and the same
    /// word in body text is a step up from the body. A name this table has no
    /// entry for is drawn at the plain size, the bargain `markBackground(_:)` and
    /// `syntaxColor(_:)` already make with a vocabulary a newer core has grown.
    public var sizeSteps: [String: CGFloat]
    /// The concrete family each of the vocabulary's four generic faces names,
    /// keyed by the `data-font` token (`Run.font`). `monospace` is not here: it
    /// is `monoFontName`, so a document's monospaced run and its inline `code`
    /// are the one face this theme already names.
    ///
    /// The document names a *generic* and the theme names the face, so a
    /// document never asks for a family the machine hasn't got. The defaults are
    /// the faces every Apple platform has shipped for a decade — see
    /// `defaultFontFamilies` for which and why.
    public var fontFamilies: [String: String]
    /// How far apart a block set to one of the vocabulary's spacings lays its
    /// lines, as a multiple of this theme's own `lineHeight`, keyed by the
    /// `data-line-height` token (`Row.lineHeight`).
    ///
    /// A table rather than the token read as arithmetic, though the tokens
    /// happen to be numbers: a theme whose body already sets at 1.5 may want
    /// "double" to mean something other than twice *its* leading, and a token
    /// this table doesn't know sets at the theme's own spacing rather than at
    /// whatever `Double(name)` makes of it.
    public var lineSpacings: [String: CGFloat]
    /// The ink a run with a `data-color` is painted in (`Run.textColor`), keyed
    /// by the seven names a highlight's colour is also spelled with, each with a
    /// light and a dark version. A name this table has no entry for reads in
    /// `textColor`.
    ///
    /// The same vocabulary as `markBackground(_:)` and deliberately not the same
    /// colours — see `Palette.textInks`, which is where the two part company.
    public var textColors: [String: LeafColor]
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
    /// The ink for a highlighted glyph in a fenced block, keyed by the class
    /// id its `Run.token` carries — see `Palette.syntax` for the eight ids. A
    /// run whose token the table has no entry for, and a run with no token —
    /// an identifier the grammar left plain, a block in a language no grammar
    /// covers, inline code — reads in `codeColor`. Comments are italic as well.
    public var syntaxColors: [String: LeafColor]
    /// A directive container's (`:::name{.class}`) dashed outline colour.
    public var directiveBorderColor: LeafColor
    public var markBackground: LeafColor
    /// The wash behind a host-painted highlight — see
    /// `LeafEditorModel.setHighlights`. A `Highlight.color` hex hint overrides
    /// the hue per highlight (`highlightBackground(_:)`); this is the default.
    public var highlightBackground: LeafColor
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
    /// The cue drawn on an empty document's first line — see `placeholder` on
    /// the text views. Tertiary rather than secondary: it stands where the
    /// reader's own words will, and has to read as an absence rather than as
    /// something already written there.
    public var placeholderColor: LeafColor
    public var selectionColor: LeafColor
    /// The selection fill when the view isn't the focus — window not key, or the
    /// view not first responder. Matches native text: emphasized blue when active,
    /// this unemphasized grey otherwise. Only the macOS surface draws it (iOS lets
    /// the system overlay selection).
    public var inactiveSelectionColor: LeafColor
    public var caretColor: LeafColor
    /// The drag-handle knobs on iOS selection (the loupe-free native peers).
    public var handleColor: LeafColor
    /// The light left on a block a `reveal` landed on, faded out over the moment
    /// after. See `Landing` for why an arrival needs one at all.
    public var landingFlashColor: LeafColor
    /// The paginated view's chrome: the paper, the surface behind the stack, and
    /// the hairline round each sheet. Inert while no `PageSetup` is set — the
    /// continuous flow draws no paper — and pure colour either way, so changing
    /// one repaints rather than re-wrapping (see `metricsDiffer`).
    public var pageColor: LeafColor
    public var pageBackdropColor: LeafColor
    public var pageBorderColor: LeafColor

    /// The name that asks for the system's own text face — San Francisco on
    /// every Apple platform today, and whatever the OS chooses tomorrow, at the
    /// optical size the OS picks for the point size. The default body face, so a
    /// document reads in the same type as the window around it. Any real family
    /// name (`"Helvetica Neue"`, `"Georgia"`) is a fine substitute.
    public static let systemFontName = "system"
    /// The system's monospaced face — SF Mono — sized to sit with the body type.
    public static let systemMonospacedFontName = "system-monospaced"

    /// CSS's own ramp, which is where the vocabulary's step names come from: the
    /// ratios a browser's user-agent stylesheet gives `small`, `large`,
    /// `x-large`… against `medium`, `medium` being absence. A document rendered
    /// from leaf's published stylesheet and one drawn by this renderer therefore
    /// agree about how much bigger `large` is, which is the point of naming a
    /// step rather than a size.
    public static let defaultSizeSteps: [String: CGFloat] = [
        "xx-small": 0.5625,
        "x-small": 0.625,
        "small": 0.8125,
        "large": 1.125,
        "x-large": 1.5,
        "xx-large": 2,
        "xxx-large": 3,
    ]

    /// The face this platform sets each generic family in.
    ///
    /// `sans-serif` is the system's own text face — the body's default, so a run
    /// that asks for sans-serif in a theme that already sets in one is a no-op
    /// rather than a jump to some other grotesque. `serif` is Georgia and
    /// `cursive` is Snell Roundhand because both have shipped on macOS *and* iOS
    /// for as long as either has had fonts, which a document opened on a phone
    /// depends on; a theme with a licence to a better pair should say so here.
    /// `monospace` is absent on purpose — it resolves to `monoFontName`.
    public static let defaultFontFamilies: [String: String] = [
        "serif": "Georgia",
        "sans-serif": EditorTheme.systemFontName,
        "cursive": "Snell Roundhand",
    ]

    /// The word processor's spacing menu, as multiples of the theme's leading:
    /// the tokens are the ratios and the default is to mean them literally. `1`
    /// is not here because `1` is absence.
    public static let defaultLineSpacings: [String: CGFloat] = [
        "1.15": 1.15,
        "1.5": 1.5,
        "2": 2,
    ]

    public init(
        bodyFontName: String = EditorTheme.systemFontName,
        monoFontName: String = EditorTheme.systemMonospacedFontName,
        fontSize: CGFloat = 16,
        lineHeight: CGFloat = 24,
        blockGapScale: CGFloat = 0.5,
        headingGapScale: CGFloat = 1.8,
        headingScale: [CGFloat] = [1.625, 1.375, 1.1875, 1.0625, 1.0, 0.9375],
        headingLineRatio: CGFloat = 1.2,
        baselineScale: CGFloat = 0.72,
        baselineSuperShift: CGFloat = 0.34,
        baselineSubShift: CGFloat = 0.16,
        sizeSteps: [String: CGFloat] = EditorTheme.defaultSizeSteps,
        fontFamilies: [String: String] = EditorTheme.defaultFontFamilies,
        lineSpacings: [String: CGFloat] = EditorTheme.defaultLineSpacings,
        textColors: [String: LeafColor] = Palette.textInks,
        measure: CGFloat? = 68,
        padding: LeafInsets = LeafInsets(top: 12, left: 16, bottom: 12, right: 16),
        textColor: LeafColor = Palette.label,
        secondaryColor: LeafColor = Palette.secondary,
        linkColor: LeafColor = Palette.link,
        codeColor: LeafColor = Palette.label,
        codeBackground: LeafColor = Palette.codeBackground,
        syntaxColors: [String: LeafColor] = Palette.syntax,
        directiveBorderColor: LeafColor = Palette.directiveBorderColor,
        markBackground: LeafColor = Palette.markBackground,
        highlightBackground: LeafColor = Palette.hostHighlight,
        quoteBarColor: LeafColor = Palette.tertiary,
        quoteBarWidth: CGFloat = 3,
        quoteIndent: CGFloat = 22,
        ruleColor: LeafColor = Palette.separator,
        ruleThickness: CGFloat = 1,
        tableBorderColor: LeafColor = Palette.tableBorder,
        tableHeaderColor: LeafColor = Palette.tableHeader,
        tableStripeColor: LeafColor = Palette.tableStripe,
        placeholderColor: LeafColor = Palette.tertiary,
        selectionColor: LeafColor = Palette.selection,
        inactiveSelectionColor: LeafColor = Palette.inactiveSelection,
        caretColor: LeafColor = Palette.caret,
        handleColor: LeafColor = Palette.accent,
        landingFlashColor: LeafColor = Palette.landingFlash,
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
        self.baselineScale = baselineScale
        self.baselineSuperShift = baselineSuperShift
        self.baselineSubShift = baselineSubShift
        self.sizeSteps = sizeSteps
        self.fontFamilies = fontFamilies
        self.lineSpacings = lineSpacings
        self.textColors = textColors
        self.measure = measure
        self.padding = padding
        self.textColor = textColor
        self.secondaryColor = secondaryColor
        self.linkColor = linkColor
        self.codeColor = codeColor
        self.codeBackground = codeBackground
        self.syntaxColors = syntaxColors
        self.directiveBorderColor = directiveBorderColor
        self.markBackground = markBackground
        self.highlightBackground = highlightBackground
        self.quoteBarColor = quoteBarColor
        self.quoteBarWidth = quoteBarWidth
        self.quoteIndent = quoteIndent
        self.ruleColor = ruleColor
        self.ruleThickness = ruleThickness
        self.tableBorderColor = tableBorderColor
        self.tableHeaderColor = tableHeaderColor
        self.tableStripeColor = tableStripeColor
        self.placeholderColor = placeholderColor
        self.selectionColor = selectionColor
        self.inactiveSelectionColor = inactiveSelectionColor
        self.caretColor = caretColor
        self.handleColor = handleColor
        self.landingFlashColor = landingFlashColor
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
            // A raised run is set at its own size, so these change how wide it
            // shapes — and therefore where the line it sits on wraps.
            || baselineScale != other.baselineScale
            || baselineSuperShift != other.baselineSuperShift
            || baselineSubShift != other.baselineSubShift
            // A run's size step and a block's spacing are lengths, not colours:
            // one shapes the run wider, the other opens the row's line box. A
            // face changes both. So all three re-wrap.
            || sizeSteps != other.sizeSteps
            || fontFamilies != other.fontFamilies
            || lineSpacings != other.lineSpacings
            || measure != other.measure
            || padding != other.padding
            // The quote gutter is stretched to `quoteIndent` at shaping time, so
            // it moves every quoted glyph — a geometry change, not a repaint.
            || quoteIndent != other.quoteIndent
    }

    /// This theme with its *typographic* lengths multiplied by `factor` — how a
    /// Dynamic Type content size is applied on iOS (see
    /// `LeafTextView.applyDynamicType`), and a plain multiplier anywhere else.
    ///
    /// What scales is what belongs to the type: the body size, its line box, and
    /// the quote gutter — which is spelled in points but *means* "the indent one
    /// level of quoting costs", so it has to grow with the text it indents, or a
    /// bar at AX5 ends up sitting almost against the glyphs it separates.
    ///
    /// What doesn't scale is what belongs to the *view*: `padding` is a minimum
    /// inset from the window's edge rather than a piece of typography, and the
    /// hairlines (`quoteBarWidth`, `ruleThickness`) are rules, which stay a
    /// rule's thickness at any text size. `measure` needs nothing done to it — it
    /// is already counted in characters, so it tracks the type for free.
    func scaled(by factor: CGFloat) -> EditorTheme {
        guard factor != 1 else { return self }
        var t = self
        t.fontSize = fontSize * factor
        t.lineHeight = lineHeight * factor
        t.quoteIndent = quoteIndent * factor
        return t
    }

    /// This theme with its type set so that `measure` characters of the body
    /// font fill one column of `page` — the theme's own `measure` when none is
    /// given, or the classic 66 when it has none either.
    ///
    /// On paper the column is the sheet's and `measure` has nothing to decide,
    /// so the equation runs the other way: the width is fixed and the type is
    /// what gives. Without this the default 16-point body, chosen for a screen,
    /// sets a one-inch-margin Letter column 58 characters wide — *shorter* than
    /// the 68 the continuous flow wraps to, because that flow's column at 16
    /// points is wider than a sheet of Letter — and a page that came out 13.4
    /// points is a page that reads as the flow does. The result rounds to a
    /// half point, since a size like 13.41 is not one anyone would name.
    ///
    /// A helper, not a rule the page applies on its own: two columns of a
    /// sheet at 68 would drop the type below 7 points, and a page someone wants
    /// at "12 point, full stop" should not have to argue with a formula. The
    /// host decides; this is what the answer is when it wants the flow's.
    ///
    /// What is *on screen* is a separate question with a separate knob — a
    /// Mac's point is a hundred-and-some to the inch, not seventy-two, so a
    /// sheet at actual size is smaller than paper — and that knob is `Zoom`.
    public func fitted(to page: PageSetup, measure: CGFloat? = nil) -> EditorTheme {
        let characters = measure ?? self.measure ?? 66
        let column = page.columnWidth
        guard characters > 0, column > 0, fontSize > 0 else { return self }
        let wanted = column / characters               // the mean advance the column wants
        // A glyph's advance is not quite proportional to its size — the system
        // face swaps optical sizes and tracks its small sizes wider — so the
        // scale is refined by measuring the candidate, twice, rather than read
        // off this theme's advance once. Each pass brings the estimate onto the
        // measurement it stands on; two are within a hundredth of a point.
        var candidate = self
        for _ in 0..<3 {
            let advance = candidate.averageCharWidth
            guard advance > 0 else { return self }
            let raw = candidate.fontSize * wanted / advance
            let size = (raw * 2).rounded() / 2
            guard size > 0 else { return self }
            if size == candidate.fontSize { break }
            candidate = scaled(by: size / fontSize)
        }
        return candidate
    }

    // ── derived metrics ──────────────────────────────────────────────────────

    /// The ratio the line box grows relative to the font — the body's leading.
    var lineRatio: CGFloat { lineHeight / fontSize }

    /// The point size for a heading of `level` (1–6), clamped to the ramp.
    /// The wash for one highlight: its own `#RRGGBB` hint at a readable
    /// alpha, or the theme's default. Strictly hex — an unparseable hint is
    /// the default, not a guess.
    func highlightBackground(_ hint: String?) -> LeafColor {
        guard let hint, let color = leafColor(hex: hint) else { return highlightBackground }
        return color.withAlphaComponent(0.32)
    }

    /// The ink for a code run classed `token`: the palette's entry, or
    /// `codeColor` for no token and for a token the palette doesn't know — so
    /// a class a newer core emits and this table lacks draws as plain code
    /// rather than not at all, the same bargain `markBackground(_:)` makes.
    func syntaxColor(_ token: String?) -> LeafColor {
        guard let token, let color = syntaxColors[token] else { return codeColor }
        return color
    }

    /// The wash behind one `==mark==`: the colour the author named, or the
    /// theme's own highlighter when they named none.
    ///
    /// A *name* from a closed vocabulary, unlike `highlightBackground(_:)`'s
    /// `#RRGGBB` — the document said "red" and the theme decides which red, the
    /// same division of labour a role and a palette already have. A name the
    /// theme doesn't know falls back rather than guessing, so a colour twig
    /// grows later draws as a plain highlight until this list catches up.
    func markBackground(_ name: String?) -> LeafColor {
        guard let name, let color = Palette.markBackground(named: name) else {
            return markBackground
        }
        return color
    }

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
    /// row (empty, holds no caret), otherwise its heading/body line box — opened
    /// by the block's own line spacing and by the largest size step any of its
    /// runs is set at.
    ///
    /// The step matters because a line box is the theme's number rather than the
    /// measured glyphs' (see `rowHeight(heading:)`): a row holding an `x-large`
    /// run in a body-sized box would set those glyphs over the line above it. It
    /// only ever grows — a row whose one `small` run is the only thing on it
    /// keeps the body's leading, because the rows around it do and a document
    /// whose lines drift closer together wherever a word is small reads as broken.
    ///
    /// A boundary row is left at the plain gap: it separates two blocks and
    /// belongs to neither, so the spacing of the one below it is not its business.
    func rowHeight(for row: Row) -> CGFloat {
        guard !row.isBlockGap else { return blockGap }
        let step = row.runs.reduce(CGFloat(1)) { max($0, sizeScale($1.size)) }
        return rowHeight(heading: row.heading) * step * lineSpacing(row.lineHeight)
    }

    /// How much larger a run set to size step `name` is than it would otherwise
    /// be — `1` for no step and for a step this theme's ramp has no entry for.
    func sizeScale(_ name: String?) -> CGFloat {
        guard let name, let scale = sizeSteps[name], scale > 0 else { return 1 }
        return scale
    }

    /// The multiple a block set to line spacing `name` lays its lines at — `1`
    /// for no spacing and for a token this theme has no entry for.
    func lineSpacing(_ name: String?) -> CGFloat {
        guard let name, let ratio = lineSpacings[name], ratio > 0 else { return 1 }
        return ratio
    }

    /// The ink a run coloured `name` is painted in, or `textColor` for no colour
    /// and for a name outside the palette — `markBackground(_:)`'s bargain, and
    /// for its reason: a colour a newer core knows draws as ordinary text rather
    /// than not at all.
    func textColor(_ name: String?) -> LeafColor {
        guard let name, let ink = textColors[name] else { return textColor }
        return ink
    }

    // ── fonts ────────────────────────────────────────────────────────────────

    /// The family this theme sets generic face `name` in — `monoFontName` for
    /// `monospace`, this theme's answer from `fontFamilies` for the rest, and nil
    /// for no face and for a generic it doesn't name, which then reads in the
    /// body face like every run around it.
    func fontName(_ name: String?) -> String? {
        guard let name else { return nil }
        if name == "monospace" { return monoFontName }
        return fontFamilies[name]
    }

    /// A run's font: the generic face `family` names at `size` with the requested
    /// traits, or the body face when the run asks for none.
    func font(family: String?, size: CGFloat, bold: Bool, italic: Bool) -> LeafFont {
        guard let name = fontName(family) else {
            return proportionalFont(size: size, bold: bold, italic: italic)
        }
        return makeFont(name: name, size: size, bold: bold, italic: italic)
    }

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
