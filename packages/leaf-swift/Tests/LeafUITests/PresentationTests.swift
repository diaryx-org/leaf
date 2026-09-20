//  PresentationTests.swift
//
//  The presentation vocabulary as this renderer draws it: the theme's ramp,
//  faces, spacings and inks; the attributes a sized/faced/coloured run takes;
//  the geometry an aligned or opened-up block lays out in; and what a page break
//  does to each of the two flows.
//
//  Structural assertions against the theme's own numbers rather than pixel
//  counts, so they stay font-independent the way `EditorLayoutTests` does — a
//  centred line is asserted to sit *in the middle of the room it leaves*, not at
//  some x this machine's Helvetica happens to produce.

import XCTest
import LeafFFI
@testable import LeafUI

final class PresentationTests: XCTestCase {
    private let theme = EditorTheme.default

    private func laid(_ rows: [Row], directives: [DirectiveView] = [],
                      wrapWidth: CGFloat = 400) -> EditorLayout {
        EditorLayout(docView(rows, directives: directives), theme: theme, wrapWidth: wrapWidth)
    }

    // MARK: the theme

    func testTheSizeRampIsCssOwnRatiosAndAnUnknownStepIsPlainText() {
        // The default ramp is CSS's `<absolute-size>` keywords, so a document
        // rendered from leaf's stylesheet and one drawn here agree about how much
        // bigger `large` is. `medium` is absent because `medium` is absence.
        XCTAssertEqual(theme.sizeScale("xx-small"), 0.5625)
        XCTAssertEqual(theme.sizeScale("x-small"), 0.625)
        XCTAssertEqual(theme.sizeScale("small"), 0.8125)
        XCTAssertEqual(theme.sizeScale("large"), 1.125)
        XCTAssertEqual(theme.sizeScale("x-large"), 1.5)
        XCTAssertEqual(theme.sizeScale("xx-large"), 2)
        XCTAssertEqual(theme.sizeScale("xxx-large"), 3)
        XCTAssertEqual(theme.sizeScale("medium"), 1, "medium is absence, not a step")
        XCTAssertEqual(theme.sizeScale(nil), 1)
        XCTAssertEqual(theme.sizeScale("enormous"), 1, "a step a newer core grows reads plain")
    }

    func testEveryGenericFaceIsNamedAndMonospaceIsTheThemesOwn() {
        // The document names a generic; the theme names the face. `monospace` is
        // deliberately not in the table — it *is* `monoFontName`, so a document's
        // monospaced run and its inline code are one face.
        for face in FontFamily.all {
            XCTAssertNotNil(theme.fontName(face.name), "\(face.name) has no face")
        }
        XCTAssertEqual(theme.fontName("monospace"), theme.monoFontName)
        XCTAssertEqual(theme.fontName("sans-serif"), theme.bodyFontName,
                       "sans-serif in a theme set in the system face is that face")
        XCTAssertNil(theme.fontName(nil))
        XCTAssertNil(theme.fontName("blackletter"))
        XCTAssertEqual(theme.font(family: nil, size: 16, bold: false, italic: false),
                       theme.proportionalFont(size: 16, bold: false, italic: false))
        XCTAssertEqual(theme.font(family: "serif", size: 16, bold: false, italic: false).familyName,
                       "Georgia")
    }

    func testEveryColourNameHasAnInkAndItIsNotTheOrdinaryOne() {
        // The seven names a highlight's wash is spelled with, as ink — the same
        // vocabulary and deliberately not the same colours.
        for colour in MarkColor.palette {
            let ink = theme.textColor(colour.name)
            XCTAssertNotNil(theme.textColors[colour.name], "\(colour.name) has no ink")
            XCTAssertNotEqual(ink, theme.textColor, "\(colour.name) reads as ordinary text")
            XCTAssertNotEqual(ink, theme.markBackground(colour.name),
                              "\(colour.name)'s ink is its wash, which is unreadable as letters")
        }
        XCTAssertEqual(theme.textColor(nil), theme.textColor)
        XCTAssertEqual(theme.textColor("chartreuse"), theme.textColor)
    }

    #if canImport(AppKit)
    func testEveryInkIsWrittenTwiceOverForTheTwoAppearances() {
        // The dark side of the palette is the whole argument for naming a colour
        // rather than writing `#c00` into the document: a name has a dark
        // appearance and a hex does not. Each ink has to resolve differently in
        // the two, or half the reason for the vocabulary is unhonoured here.
        for colour in MarkColor.palette {
            let ink = theme.textColor(colour.name)
            var light: LeafColor?
            var dark: LeafColor?
            NSAppearance(named: .aqua)?.performAsCurrentDrawingAppearance {
                light = ink.usingColorSpace(.sRGB)
            }
            NSAppearance(named: .darkAqua)?.performAsCurrentDrawingAppearance {
                dark = ink.usingColorSpace(.sRGB)
            }
            XCTAssertNotNil(light)
            XCTAssertNotEqual(light, dark, "\(colour.name) is the same ink in both appearances")
        }
    }
    #endif

    func testLineSpacingOpensTheRowAndALargerRunOpensItToo() {
        let plain = row([mkRun("hello")])
        XCTAssertEqual(theme.rowHeight(for: plain), theme.lineHeight)
        XCTAssertEqual(theme.rowHeight(for: row([mkRun("hello")], lineHeight: "1.5")),
                       theme.lineHeight * 1.5, accuracy: 0.01)
        XCTAssertEqual(theme.rowHeight(for: row([mkRun("hello")], lineHeight: "2")),
                       theme.lineHeight * 2, accuracy: 0.01)
        // A row is opened by its largest run: an x-large word in a body line box
        // would set its glyphs over the line above.
        XCTAssertEqual(theme.rowHeight(for: row([mkRun("a"), mkRun("b", size: "x-large")])),
                       theme.lineHeight * 1.5, accuracy: 0.01)
        // …and never closed by its smallest: rows whose leading tightened
        // wherever a word was small would read as a broken column.
        XCTAssertEqual(theme.rowHeight(for: row([mkRun("a", size: "x-small")])), theme.lineHeight)
        // A boundary row keeps the plain gap — it belongs to neither block.
        XCTAssertEqual(theme.rowHeight(for: gapRow(.paragraph, .paragraph)), theme.blockGap)
    }

    func testAChangedRampOrSpacingReWrapsRatherThanRepaints() {
        // A size step and a line spacing are lengths: one shapes the run wider,
        // the other opens the line box. Both have to invalidate the cache.
        var bigger = theme
        bigger.sizeSteps["large"] = 2
        XCTAssertTrue(bigger.metricsDiffer(from: theme))
        var looser = theme
        looser.lineSpacings["2"] = 3
        XCTAssertTrue(looser.metricsDiffer(from: theme))
        var recoloured = theme
        recoloured.textColors["red"] = .black
        XCTAssertFalse(recoloured.metricsDiffer(from: theme), "an ink is a repaint")
    }

    // MARK: the run's attributes

    private func attributes(_ run: Run, row r: Row? = nil) -> [NSAttributedString.Key: Any] {
        let line = AttributedRow.make(r ?? row([run]), theme: theme)
        return line.attributes(at: 0, effectiveRange: nil)
    }

    func testASizeStepScalesTheTextAroundItAndAHeadingScalesItsOwn() throws {
        let plain = try XCTUnwrap(attributes(mkRun("x"))[.font] as? LeafFont)
        XCTAssertEqual(plain.pointSize, theme.fontSize, accuracy: 0.01)
        let large = try XCTUnwrap(attributes(mkRun("x", size: "large"))[.font] as? LeafFont)
        XCTAssertEqual(large.pointSize, theme.fontSize * 1.125, accuracy: 0.01)
        // On a heading the same word is a step up from *the heading* — which is
        // what makes `large` a name rather than a number.
        let run = mkRun("x", size: "large")
        let inHeading = try XCTUnwrap(
            attributes(run, row: row([run], heading: 1))[.font] as? LeafFont)
        XCTAssertEqual(inHeading.pointSize, theme.headingSize(1) * 1.125, accuracy: 0.01)
    }

    func testAFaceChangesTheFamilyAndCodeKeepsItsOwn() throws {
        let serif = try XCTUnwrap(attributes(mkRun("x", font: "serif"))[.font] as? LeafFont)
        XCTAssertEqual(serif.familyName, "Georgia")
        let mono = try XCTUnwrap(attributes(mkRun("x", font: "monospace"))[.font] as? LeafFont)
        XCTAssertEqual(mono, theme.monospaceFont(size: theme.fontSize, bold: false, italic: false))
        // A `code` run is monospaced whatever face it names: the role is what the
        // glyphs are, and a face on top of it is for the prose around them.
        let code = try XCTUnwrap(
            attributes(mkRun("x", role: "code", font: "cursive"))[.font] as? LeafFont)
        XCTAssertEqual(code, theme.monospaceFont(size: theme.fontSize, bold: false, italic: false))
    }

    func testAColouredRunIsInkedAndKeepsWhatItsRoleGaveIt() {
        let red = attributes(mkRun("x", textColor: "red"))
        XCTAssertEqual(red[.foregroundColor] as? LeafColor, theme.textColor("red"))
        // A colour inside a link colours the letters; the link's own role still
        // underlines them. Roles and the vocabulary compose rather than compete.
        let link = attributes(mkRun("x", role: "link", textColor: "green"))
        XCTAssertEqual(link[.foregroundColor] as? LeafColor, theme.textColor("green"))
        XCTAssertEqual(link[.underlineStyle] as? Int, NSUnderlineStyle.single.rawValue)
        // A coloured `==mark==` keeps its wash: the ink is the letters, the wash
        // is behind them.
        let mark = attributes(mkRun("x", role: "mark", markColor: "yellow", textColor: "purple"))
        XCTAssertEqual(mark[.foregroundColor] as? LeafColor, theme.textColor("purple"))
        XCTAssertEqual(mark[.backgroundColor] as? LeafColor, theme.markBackground("yellow"))
        // A quote's gutter stays invisible however the block around it is
        // coloured — the view paints a real bar in its place.
        let gutter = attributes(mkRun("│ ", role: "quote", textColor: "red"))
        XCTAssertEqual(gutter[.foregroundColor] as? LeafColor, LeafColor.clear)
    }

    // MARK: alignment

    /// The room a line leaves in its column — what centring halves and right
    /// alignment spends whole.
    private func room(_ line: WrappedLine, in width: CGFloat) -> CGFloat {
        width - line.width + CGFloat(CTLineGetTrailingWhitespaceWidth(line.line))
    }

    func testACentredRowSitsInTheMiddleOfTheRoomItLeaves() throws {
        let width: CGFloat = 400
        let layout = laid([row([mkRun("a short line")], align: "center")], wrapWidth: width)
        let line = try XCTUnwrap(layout.rows.first?.wrapped.first)
        XCTAssertEqual(line.align, room(line, in: width) / 2, accuracy: 0.01)
        XCTAssertGreaterThan(line.align, 0)
        // And the caret goes with it: geometry and glyphs read the same offset,
        // which is the whole reason alignment is an offset here and not a
        // paragraph style on the drawn string.
        let caret = try XCTUnwrap(layout.rect(row: 0, ch: 0))
        XCTAssertEqual(caret.minX, layout.rows[0].lineOrigin(0).x + line.align, accuracy: 0.01)
    }

    func testARightAlignedRowEndsAtTheMarginAndAPlainOneStartsAtIt() throws {
        let width: CGFloat = 400
        let right = laid([row([mkRun("a short line")], align: "right")], wrapWidth: width)
        let line = try XCTUnwrap(right.rows.first?.wrapped.first)
        XCTAssertEqual(line.align, room(line, in: width), accuracy: 0.01)
        let plain = laid([row([mkRun("a short line")])], wrapWidth: width)
        XCTAssertEqual(try XCTUnwrap(plain.rows.first?.wrapped.first).align, 0)
        // A token this renderer doesn't know is the theme's default, not a guess.
        let odd = laid([row([mkRun("a short line")], align: "inside")], wrapWidth: width)
        XCTAssertEqual(try XCTUnwrap(odd.rows.first?.wrapped.first).align, 0)
    }

    func testJustifyFillsEveryLineButTheLast() throws {
        let width: CGFloat = 200
        let text = String(repeating: "lorem ipsum dolor sit amet ", count: 6)
        let layout = laid([row([mkRun(text)], align: "justify")], wrapWidth: width)
        let lines = try XCTUnwrap(layout.rows.first?.wrapped)
        XCTAssertGreaterThan(lines.count, 2, "the fixture has to wrap for this to mean anything")
        for line in lines.dropLast() {
            // Spread to the column, and by kerning the string itself — so the
            // caret, the hit test and the glyphs all measure the same line.
            XCTAssertEqual(room(line, in: width), 0, accuracy: 0.5)
            XCTAssertEqual(line.align, 0, "a justified line fills the room instead of moving")
        }
        XCTAssertGreaterThan(room(try XCTUnwrap(lines.last), in: width), 0,
                             "the last line of a paragraph stays ragged")
    }

    func testAnAlignedQuoteKeepsItsGutterWhereTheBarIs() throws {
        // The gutter is never justified and never moved: the view paints a real
        // bar at `quoteBarXs`, and spreading or shifting the prefix would leave
        // the bar beside nothing.
        let gutter = mkRun("│ ", role: "quote")
        let text = String(repeating: "lorem ipsum dolor sit amet ", count: 6)
        let layout = laid([row([gutter, mkRun(text)], align: "justify")], wrapWidth: 200)
        let first = try XCTUnwrap(layout.rows.first?.wrapped.first)
        XCTAssertEqual(first.indent, 0)
        XCTAssertEqual(first.align, 0)
        let prefix = first.attributed.attributedSubstring(from: NSRange(location: 0, length: 2))
        XCTAssertNil(prefix.attribute(.kern, at: 1, effectiveRange: nil),
                     "the gutter's own space was spread")
    }

    // MARK: the page break

    private let page = PageSetup(
        size: CGSize(width: 400, height: 200),
        margins: LeafInsets(top: 20, left: 20, bottom: 20, right: 20),
        gap: 20, backdrop: 24)

    private func paginated(_ rows: [Row], directives: [DirectiveView]) -> EditorLayout {
        var cache: [Row: ShapedRow] = [:]
        return EditorLayout(docView(rows, directives: directives), theme: theme,
                            viewWidth: 800, page: page, cache: &cache)
    }

    func testAPageBreakTurnsThePageAndTakesNoHeight() throws {
        let rows = [row([mkRun("before")]), pageBreakRow(), row([mkRun("after")])]
        let layout = paginated(rows, directives: [mkDirective("page-break", startRow: 1, endRow: 2)])
        XCTAssertEqual(layout.rows[0].page, 0)
        XCTAssertEqual(layout.rows[1].height, 0, "a spent break occupies nothing")
        XCTAssertTrue(layout.rows[1].pageBreak)
        XCTAssertEqual(layout.rows[2].page, 1, "the block after the break is on the next sheet")
        XCTAssertEqual(layout.rows[2].top, page.contentTop(1), accuracy: 0.5,
                       "and starts at its top margin, not a paragraph's spacing below it")
        XCTAssertEqual(layout.pages.count, 2)
        // The caret still has a home on the break — at the head of the sheet it
        // opened, which is where the next character will land.
        let caret = try XCTUnwrap(layout.rect(row: 1, ch: 0))
        XCTAssertEqual(caret.minY, page.contentTop(1), accuracy: 0.5)
    }

    func testABreakAtTheHeadOfADocumentOpensNoBlankPage() {
        let rows = [pageBreakRow(), row([mkRun("first")])]
        let layout = paginated(rows, directives: [mkDirective("page-break", startRow: 0, endRow: 1)])
        XCTAssertEqual(layout.pages.count, 1, "the first page is never blank")
        XCTAssertEqual(layout.rows[1].page, 0)
        XCTAssertEqual(layout.rows[1].top, page.contentTop(0), accuracy: 0.5)
    }

    func testTwoBreaksInARowSkipAPageRatherThanCollapsing() {
        let rows = [row([mkRun("before")]), pageBreakRow(), pageBreakRow(), row([mkRun("after")])]
        let layout = paginated(rows, directives: [
            mkDirective("page-break", startRow: 1, endRow: 2),
            mkDirective("page-break", startRow: 2, endRow: 3),
        ])
        XCTAssertEqual(layout.rows[3].page, 2, "each break is a page the author asked for")
        XCTAssertEqual(layout.pages.count, 3)
    }

    func testAnotherDirectiveIsNotAPageBreak() {
        // Only the frame's `directives` say which directive a row is — a host's
        // own `::toc` draws as the placeholder row it always did.
        let rows = [row([mkRun("before")]),
                    row([mkRun("⧉ toc")], directive: true),
                    row([mkRun("after")])]
        let layout = paginated(rows, directives: [mkDirective("toc", startRow: 1, endRow: 2)])
        XCTAssertFalse(layout.rows[1].pageBreak)
        XCTAssertEqual(layout.rows[2].page, 0)
    }

    func testTheContinuousFlowDrawsADashedLineWhereThePaperWouldEnd() throws {
        let rows = [row([mkRun("before")]), pageBreakRow(), row([mkRun("after")])]
        let layout = laid(rows, directives: [mkDirective("page-break", startRow: 1, endRow: 2)])
        let rl = layout.rows[1]
        XCTAssertTrue(rl.pageBreak)
        XCTAssertEqual(rl.height, theme.lineHeight, "it keeps a line box to draw in")
        XCTAssertEqual(rl.attributed.string, "", "core's ⧉ placeholder glyphs are not drawn")
        let line = try XCTUnwrap(rl.pageBreakLine(theme: theme))
        XCTAssertEqual(line.height, theme.ruleThickness)
        XCTAssertEqual(line.minX, rl.lineOrigin(0).x, accuracy: 0.5)
        XCTAssertEqual(line.width, rl.columnWidth, accuracy: 0.5)
        // And no dashed *box* around it: a page break is a directive, but the
        // panel an aside is drawn in would say a block of content stands here.
        XCTAssertFalse(rl.isChromedDirective)
        XCTAssertTrue(laid([row([mkRun("⧉ toc")], directive: true)],
                           directives: [mkDirective("toc", startRow: 0, endRow: 1)])
            .rows[0].isChromedDirective, "every other directive keeps its outline")
        // Spent, in the paginated flow, there is nothing left to draw.
        let onPaper = paginated(rows, directives: [mkDirective("page-break", startRow: 1, endRow: 2)])
        XCTAssertNil(onPaper.rows[1].pageBreakLine(theme: theme))
    }

    // MARK: the vocabulary a control offers

    func testEveryTokenAControlOffersIsOneTheRendererReadsBack() {
        // `name` is the join between the closed set a menu offers and the open
        // string a row carries: every value a control can write has to come back
        // as itself, or a menu would tick nothing after applying it.
        for align in Align.all { XCTAssertEqual(Align(name: align.name), align) }
        for step in SizeStep.ramp { XCTAssertEqual(SizeStep(name: step.name), step) }
        for face in FontFamily.all { XCTAssertEqual(FontFamily(name: face.name), face) }
        for spacing in LineSpacing.all { XCTAssertEqual(LineSpacing(name: spacing.name), spacing) }
        XCTAssertNil(Align(name: "left"), "left is absence, not a token")
        XCTAssertNil(LineSpacing(name: "1"), "single is absence too")
        XCTAssertNil(SizeStep(name: "medium"), "and so is medium")
        // Every step the ramp offers is one the theme can size, and every face
        // one it can name — the check `highlightColorPaletteIsWholeAndDrawable`
        // makes for the colours.
        XCTAssertEqual(SizeStep.ramp.count, 7)
        for step in SizeStep.ramp {
            XCTAssertNotEqual(theme.sizeScale(step.name), 1, "\(step.name) is not on the ramp")
        }
        for spacing in LineSpacing.all {
            XCTAssertNotEqual(theme.lineSpacing(spacing.name), 1, "\(spacing.name) opens nothing")
        }
    }

    // MARK: the exact half of the vocabulary

    func testTheGrammarReadsWhatCoreWritesAndRefusesWhatItWouldNot() {
        // The one parse on this side, and core's own grammar: CSS's `<number>`
        // with digits on whichever side of the point there is, `pt` and no
        // other unit, and 0.01…655.35 — which is what core's hundredths of a
        // UInt16 can hold and therefore what a document can carry.
        XCTAssertEqual(PresentationValue.points(size: "14pt"), 14)
        XCTAssertEqual(PresentationValue.points(size: "13.5pt"), 13.5)
        XCTAssertEqual(PresentationValue.points(size: "  14PT "), 14, "CSS reads a unit either way")
        XCTAssertEqual(PresentationValue.points(size: ".5pt"), 0.5, "digits on one side is a number")
        XCTAssertNil(PresentationValue.points(size: "large"), "a step is the other half")
        XCTAssertNil(PresentationValue.points(size: "14px"), "a document is not a screen")
        XCTAssertNil(PresentationValue.points(size: "14"), "points are the unit, and are spelled")
        XCTAssertNil(PresentationValue.points(size: "14.pt"), "a trailing bare point is a typo")
        XCTAssertNil(PresentationValue.points(size: "700pt"), "past what a sheet holds")
        XCTAssertNil(PresentationValue.points(size: "0pt"))
        XCTAssertNil(PresentationValue.points(size: "-3pt"))
        XCTAssertNil(PresentationValue.points(size: "1e2pt"))

        XCTAssertEqual(PresentationValue.ratio(spacing: "1.3"), 1.3)
        XCTAssertEqual(PresentationValue.ratio(spacing: "2"), 2)
        XCTAssertNil(PresentationValue.ratio(spacing: "huge"))
        XCTAssertNil(PresentationValue.ratio(spacing: "1.3em"))

        XCTAssertEqual(PresentationValue.ink(color: "#c03030"), leafColor(hex: "#c03030"))
        XCTAssertEqual(PresentationValue.ink(color: "#f00"), leafColor(hex: "#ff0000"),
                       "the shorthand doubles each digit, as CSS expands it")
        XCTAssertNil(PresentationValue.ink(color: "red"), "a name is the other half")
        XCTAssertNil(PresentationValue.ink(color: "rgb(1, 2, 3)"))

        // And back: the shortest decimal, which is the spelling core writes, so
        // a menu row and the document's token read the same.
        XCTAssertEqual(PresentationValue.spell(14), "14")
        XCTAssertEqual(PresentationValue.spell(13.5), "13.5")
        XCTAssertEqual(PresentationValue.spell(1.3), "1.3")
    }

    func testAThirdDecimalRoundsOnTheDigitsTheWayCoreRoundsIt() {
        // Core reads the third decimal digit and adds a hundredth for a 5; the
        // same number assembled as a binary float and rounded does not, because
        // 1.005 is not a number a `Double` holds — it is a hair under, and
        // `(1.005 * 100).rounded()` is 100 where `Hundredths::parse` is 101.
        // The two have to agree, or the spacing field reads a 1 where core
        // would write 1.01, calls it *absence*, and clears the key instead.
        XCTAssertEqual(PresentationValue.number("1.005"), 1.01)
        XCTAssertEqual(PresentationValue.number("13.455"), 13.46)
        XCTAssertEqual(PresentationValue.number("13.454"), 13.45)
        XCTAssertEqual(PresentationValue.number("1.0049"), 1)
        XCTAssertEqual(PresentationEntry.spacing("1.005"), .value(.ratio(1.01)),
                       "a rounded-up third decimal is a value, not a clearing")
        // Rounding up is still held inside what a document can carry, and the
        // arithmetic is done in hundredths, so a long number is a refusal and
        // never an overflow.
        XCTAssertEqual(PresentationValue.number("655.354"), 655.35)
        XCTAssertNil(PresentationValue.number("655.355"), "rounded past the last hundredth")
        XCTAssertEqual(PresentationValue.number("0.005"), 0.01, "and up into the first one")
        XCTAssertNil(PresentationValue.number("0.004"))
        XCTAssertNil(PresentationValue.number("99999999999999999999"))
    }

    func testAnExactSizeReplacesTheRampWhereAStepScalesIt() throws {
        // The whole of the trade the two halves make: a *name* is a multiple of
        // whatever the run would otherwise be, and a *value* is the number.
        XCTAssertEqual(theme.runSize(base: theme.fontSize, token: "large"),
                       theme.fontSize * 1.125, accuracy: 0.01)
        XCTAssertEqual(theme.runSize(base: theme.fontSize, token: "14pt"), 14)
        XCTAssertEqual(theme.runSize(base: theme.headingSize(1), token: "14pt"), 14,
                       "a heading set to an exact size is that size, not its ramp scaled")
        XCTAssertEqual(theme.runSize(base: theme.fontSize, token: nil), theme.fontSize)
        XCTAssertEqual(theme.runSize(base: theme.fontSize, token: "enormous"), theme.fontSize)

        // Down to the glyphs: the attributed run is set at the number.
        let exact = try XCTUnwrap(attributes(mkRun("x", size: "14pt"))[.font] as? LeafFont)
        XCTAssertEqual(exact.pointSize, 14, accuracy: 0.01)
        let run = mkRun("x", size: "14pt")
        let inHeading = try XCTUnwrap(
            attributes(run, row: row([run], heading: 1))[.font] as? LeafFont)
        XCTAssertEqual(inHeading.pointSize, 14, accuracy: 0.01)
    }

    func testAnExactSpacingOpensTheRowAndAnExactSizeGrowsIt() {
        // A ratio the theme's table has no entry for is read as the arithmetic
        // it is, rather than falling back to the theme's own leading.
        XCTAssertEqual(theme.lineSpacing("1.3"), 1.3)
        XCTAssertEqual(theme.lineSpacing("huge"), 1)
        XCTAssertEqual(theme.rowHeight(for: row([mkRun("hello")], lineHeight: "1.3")),
                       theme.lineHeight * 1.3, accuracy: 0.01)
        // And a row grows to an exact size the way it grows to a step — by the
        // multiple of *its own* base size that size happens to be.
        XCTAssertEqual(theme.rowHeight(for: row([mkRun("a", size: "48pt")])),
                       theme.lineHeight * 48 / theme.fontSize, accuracy: 0.01)
        // …and never shrinks to one, for the reason it never shrinks to a step:
        // a column whose lines drift closer wherever a word is small is broken.
        XCTAssertEqual(theme.rowHeight(for: row([mkRun("a", size: "4pt")])), theme.lineHeight)
    }

    func testARowIsNeverLaidCloserThanHalfALineHoweverSmallTheRatio() {
        // The grammar carries a ratio down to 0.01, and a row laid at a
        // hundredth of its line box is not tight leading: it is a row drawn
        // through the three above it, with a caret and a hit test that land on
        // none of them. So the *drawn* multiple has a floor, where the document
        // keeps its token and the menu keeps ticking it — the same rule
        // `rowHeight(for:)` already keeps for a small run, which opens a line
        // box and never closes one.
        XCTAssertEqual(theme.lineSpacing("0.2"), EditorTheme.tightestLineSpacing)
        XCTAssertEqual(theme.lineSpacing("0.01"), EditorTheme.tightestLineSpacing)
        XCTAssertEqual(theme.lineSpacing("0.5"), 0.5, "the floor itself is a ratio like any other")
        XCTAssertEqual(theme.lineSpacing("0.9"), 0.9, "and nothing above it moves")
        XCTAssertEqual(theme.lineSpacing("1.3"), 1.3)
        XCTAssertEqual(theme.rowHeight(for: row([mkRun("hello")], lineHeight: "0.1")),
                       theme.lineHeight * EditorTheme.tightestLineSpacing, accuracy: 0.01)
        // A theme whose own table names a spacing that tight is floored too: the
        // floor is about the geometry rows are laid in, not about who asked.
        var cramped = theme
        cramped.lineSpacings["2"] = 0.1
        XCTAssertEqual(cramped.lineSpacing("2"), EditorTheme.tightestLineSpacing)
    }

    func testAnExactColourIsPaintedAsWrittenInBothAppearances() {
        XCTAssertEqual(theme.textColor("#c03030"), leafColor(hex: "#c03030"))
        XCTAssertEqual(theme.textColor("#f00"), leafColor(hex: "#ff0000"))
        // A name the theme knows still wins — the table is the theme's answer,
        // and only a token it has no entry for is parsed.
        XCTAssertEqual(theme.textColor("red"), theme.textColors["red"])
        XCTAssertNotEqual(theme.textColor("#c03030"), theme.textColor, "and it is not plain ink")
        #if canImport(AppKit)
        // The cost of exactness, asserted: one ink in both appearances, where
        // `testEveryInkIsWrittenTwiceOverForTheTwoAppearances` demands two of
        // every *name*.
        let exact = theme.textColor("#c03030")
        var light: LeafColor?
        var dark: LeafColor?
        NSAppearance(named: .aqua)?.performAsCurrentDrawingAppearance {
            light = exact.usingColorSpace(.sRGB)
        }
        NSAppearance(named: .darkAqua)?.performAsCurrentDrawingAppearance {
            dark = exact.usingColorSpace(.sRGB)
        }
        XCTAssertEqual(light, dark)
        #endif
    }

    func testANamedFaceResolvesThroughTheRegistryAndFallsBackToTheBody() throws {
        // A family the machine has is that family; one it hasn't is nil, so the
        // run reads in the theme's body face like the prose around it rather
        // than in whatever the system would substitute.
        XCTAssertEqual(theme.fontName("Georgia"), "Georgia")
        XCTAssertNil(theme.fontName("A Face Nobody Has Installed"))
        XCTAssertEqual(PresentationValue.family(face: "  Georgia "), "Georgia")
        XCTAssertNil(PresentationValue.family(face: "   "))
        let named = try XCTUnwrap(attributes(mkRun("x", font: "Georgia"))[.font] as? LeafFont)
        XCTAssertEqual(named.familyName, "Georgia")
        let missing = try XCTUnwrap(
            attributes(mkRun("x", font: "A Face Nobody Has Installed"))[.font] as? LeafFont)
        XCTAssertEqual(missing, theme.proportionalFont(size: theme.fontSize, bold: false,
                                                       italic: false))
    }

    // MARK: the model's API, over a real document

    func testAnExactValueCrossesTheBindingAsItselfAndComesBackWhole() throws {
        // The associated-value enums, out through the generated binding and
        // back — `.points(14)` in, `.points(14)` at the caret, and `"14pt"` on
        // the run the renderer is handed, which is the token the theme's
        // fall-through then reads.
        //
        // Driven through `LeafDoc` rather than through the model's own
        // gestures, which route through the platform text view (see `run(_:)`)
        // and are therefore no-ops in a test that stands none up — the state
        // `EditorModelTests` is about. The model's side of this is its queries,
        // asserted below off the same document.
        let doc = try LeafDoc(source: "hello world\n", format: "markdown")
        let at = UInt32(try XCTUnwrap(doc.source().range(of: "hello")).lowerBound
            .utf16Offset(in: doc.source()))
        let inText = { doc.setSelectionOffsets(anchor: at, focus: at) }

        _ = doc.setFontSize(size: .points(14))
        inText()
        XCTAssertEqual(doc.fontSizeAtCaret(), .points(14))
        _ = doc.setFontFamily(font: .named("Garamond"))
        inText()
        XCTAssertEqual(doc.fontFamilyAtCaret(), .named("Garamond"))
        _ = doc.setTextColor(color: .rgb(r: 0xC0, g: 0x30, b: 0x30))
        inText()
        XCTAssertEqual(doc.textColorAtCaret(), .rgb(r: 0xC0, g: 0x30, b: 0x30))
        let view = doc.setLineSpacing(spacing: .ratio(1.3))
        XCTAssertEqual(doc.lineSpacingAtCaret(), .ratio(1.3))

        // And the *view* carries each value's own canonical spelling, where it
        // carried nothing before the types opened.
        let styled = view.rows.flatMap(\.runs)
            .filter { !$0.text.trimmingCharacters(in: .whitespaces).isEmpty }
            .map { [$0.size, $0.font, $0.textColor] }
        XCTAssertEqual(styled, [["14pt", "Garamond", "#c03030"]])
        XCTAssertEqual(view.rows.compactMap(\.lineHeight), ["1.3"])

        // A name still crosses as a name: the two halves are one type, and the
        // menus tick the same property either way.
        _ = doc.setFontSize(size: .step(.large))
        inText()
        XCTAssertEqual(doc.fontSizeAtCaret(), .step(.large))
        XCTAssertEqual(doc.fontSizeAtCaret()?.stepOnly, .large)
        XCTAssertNil(doc.fontSizeAtCaret()?.pointsOnly)
    }

    func testTheModelReadsBothHalvesBackForTheMenus() throws {
        // The model's four queries, which is what every menu row ticks off.
        let editor = try LeafEditorModel(source: "hello world\n", format: "markdown")
        _ = editor.doc.setFontSize(size: .points(14))
        _ = editor.doc.setLineSpacing(spacing: .ratio(1.3))
        let at = UInt32(try XCTUnwrap(editor.source().range(of: "hello")).lowerBound
            .utf16Offset(in: editor.source()))
        editor.doc.setSelectionOffsets(anchor: at, focus: at)

        XCTAssertEqual(editor.fontSize, .points(14))
        XCTAssertEqual(editor.lineSpacing, .ratio(1.3))
        XCTAssertEqual(editor.fontSize?.exactTitle, "14 pt", "and the menu has a row for it")
        XCTAssertEqual(editor.lineSpacing?.exactTitle, "1.3")
        // Nothing is pending until a row asks for something.
        XCTAssertNil(editor.pendingOther)
    }

    func testBiggerFromAnExactSizeMovesByAPointAndFromAStepWalksTheRamp() {
        // From an exact size the ramp has no rung to walk to, so the step is a
        // point — the stepper beside a font panel's own field.
        XCTAssertEqual(FontSize.stepped(from: .points(14), up: true), .points(15))
        XCTAssertEqual(FontSize.stepped(from: .points(14), up: false), .points(13))
        XCTAssertEqual(FontSize.stepped(from: .points(13.5), up: true), .points(14.5),
                       "a half point stays a half point")
        XCTAssertEqual(FontSize.stepped(from: .points(1), up: false), .points(1),
                       "type under a point is not a size anyone means")
        // From a step, and from no size at all, it is still the ramp — with the
        // theme's own size in the middle of it.
        XCTAssertEqual(FontSize.stepped(from: nil, up: true), .step(.large))
        XCTAssertEqual(FontSize.stepped(from: .step(.small), up: true), nil)
        XCTAssertEqual(FontSize.stepped(from: .step(.large), up: true), .step(.xLarge))
        XCTAssertEqual(FontSize.stepped(from: .step(.xxxLarge), up: true), .step(.xxxLarge))
    }

    func testTheOtherFieldTellsAClearingFromARefusal() {
        // The field is where a typo is caught, because past it a refusal is a
        // gesture that quietly writes nothing and looks exactly like a clear.
        XCTAssertEqual(PresentationEntry.size("14"), .value(.points(14)))
        XCTAssertEqual(PresentationEntry.size(" 13.5 "), .value(.points(13.5)))
        XCTAssertEqual(PresentationEntry.size("0"), .refused)
        XCTAssertEqual(PresentationEntry.size("700"), .refused)
        XCTAssertEqual(PresentationEntry.size("abc"), .refused)
        XCTAssertEqual(PresentationEntry.size(""), .refused)
        XCTAssertEqual(PresentationEntry.size("14."), .refused)

        XCTAssertEqual(PresentationEntry.spacing("1.3"), .value(.ratio(1.3)))
        XCTAssertEqual(PresentationEntry.spacing("1"), .absence,
                       "single spacing is the theme's own, and clears the key")
        XCTAssertEqual(PresentationEntry.spacing("1.00"), .absence)
        XCTAssertEqual(PresentationEntry.spacing("0"), .refused)
        XCTAssertEqual(PresentationEntry.spacing("700"), .refused)
        XCTAssertEqual(PresentationEntry.spacing("abc"), .refused)

        // The colour picker is the one of the four with nothing typed in it, so
        // what it has to tell apart is a colour *chosen* from a picker that was
        // merely seeded: Set on an untouched picker would write the seed, and
        // the seed over a document with no exact colour is the primary ink.
        let red = TextColor.rgb(r: 0xC0, g: 0x30, b: 0x30)
        let black = TextColor.rgb(r: 0, g: 0, b: 0)
        XCTAssertEqual(PresentationEntry.colour(black, touched: false, inForce: nil), .refused,
                       "Return over a picker nobody touched writes no ink")
        XCTAssertEqual(PresentationEntry.colour(black, touched: false, inForce: .named(.red)),
                       .refused, "and turns no named colour into a hex")
        XCTAssertEqual(PresentationEntry.colour(red, touched: true, inForce: nil), .value(red))
        XCTAssertEqual(PresentationEntry.colour(red, touched: true, inForce: .named(.red)),
                       .value(red), "an exact colour over a named one is a change")
        XCTAssertEqual(PresentationEntry.colour(red, touched: true, inForce: red), .refused,
                       "and a picker walked around the wheel and back asks for nothing")
        XCTAssertEqual(PresentationEntry.colour(nil, touched: true, inForce: nil), .refused,
                       "a colour with no place in sRGB is one no #rrggbb spells")
    }

    func testEveryValueHasARowToShowItselfIn() {
        // The row a menu draws above *Other…* exists exactly when the value at
        // the caret is an exact one — a permanent "14 pt" over a document set
        // in none of it would be offering a size nobody chose.
        XCTAssertEqual(FontSize.points(14).exactTitle, "14 pt")
        XCTAssertEqual(FontSize.points(13.5).exactTitle, "13.5 pt")
        XCTAssertNil(FontSize.step(.large).exactTitle)
        XCTAssertEqual(FontSize.step(.large).title, SizeStep.large.title)
        XCTAssertEqual(LineHeight.ratio(1.3).exactTitle, "1.3")
        XCTAssertNil(LineHeight.step(.double).exactTitle)
        XCTAssertEqual(FontFace.named("Garamond").exactTitle, "Garamond")
        XCTAssertNil(FontFace.generic(.serif).exactTitle)
        XCTAssertEqual(TextColor.rgb(r: 0xC0, g: 0x30, b: 0x30).exactTitle, "#c03030",
                       "the hex is the swatch: it is what the document will carry")
        XCTAssertNil(TextColor.named(.red).exactTitle)
        XCTAssertEqual(TextColor.named(.red).title, MarkColor.red.menuTitle)
        // And all four rows are named, so none of them opens an untitled field.
        for field in PresentationOther.allCases {
            XCTAssertFalse(field.label.isEmpty)
            XCTAssertFalse(field.fieldTitle.isEmpty)
        }
    }

    func testSteppingTheSizeWalksThroughTheThemesOwnSize() {
        // The ramp has absence in the middle of it, so an author can walk from
        // small text through ordinary text to large without the document ever
        // carrying a key that says nothing.
        XCTAssertEqual(SizeStep.stepped(from: nil, up: true), .large)
        XCTAssertEqual(SizeStep.stepped(from: nil, up: false), .small)
        XCTAssertNil(SizeStep.stepped(from: .small, up: true))
        XCTAssertNil(SizeStep.stepped(from: .large, up: false))
        XCTAssertEqual(SizeStep.stepped(from: .large, up: true), .xLarge)
        // The ends hold rather than wrapping: one press past the top is a
        // surprise another press cannot undo.
        XCTAssertEqual(SizeStep.stepped(from: .xxxLarge, up: true), .xxxLarge)
        XCTAssertEqual(SizeStep.stepped(from: .xxSmall, up: false), .xxSmall)
    }

    func testTheGesturesReachTheLayoutThroughARealDocument() throws {
        // The one test here that drives a real `LeafDoc`: everything above works
        // from fixtures, which proves the renderer reads what core says and not
        // that core says it. This is the whole chain — a press, the Markdown it
        // writes, the frame it parses back, and the geometry that comes out.
        let doc = try LeafDoc(source: "hello world\n", format: "markdown")
        XCTAssertTrue(doc.capabilities().alignment, "markdown spells the div this needs")
        _ = doc.setAlignment(align: .center)
        _ = doc.setLineSpacing(spacing: .step(.oneHalf))
        XCTAssertTrue(doc.source().contains("center"), "the gesture wrote something")
        XCTAssertEqual(doc.alignmentAtCaret(), .center)

        var cache: [Row: ShapedRow] = [:]
        let layout = EditorLayout(doc.view(), theme: theme, wrapWidth: 400, cache: &cache)
        let rl = try XCTUnwrap(layout.rows.first { !$0.row.runs.isEmpty })
        XCTAssertEqual(rl.row.align, "center")
        XCTAssertEqual(rl.lineHeight, theme.lineHeight * 1.5, accuracy: 0.01)
        XCTAssertGreaterThan(try XCTUnwrap(rl.wrapped.first).align, 0, "and the line moved")
    }

    func testTheCaretsAlignmentRidesThePublishedFrame() {
        // Walking from a centred paragraph into a plain one moves no mark, no
        // heading and no dirty flag — so the alignment has to be part of what
        // makes two states unequal, or a lit segment would never go out.
        let centred = EditorState(docView([row([mkRun("x")], align: "center")], caretRow: 0))
        let plain = EditorState(docView([row([mkRun("x")])], caretRow: 0))
        XCTAssertEqual(centred.align, .center)
        XCTAssertNil(plain.align)
        XCTAssertNotEqual(centred, plain)
        // The caret's own row, not the document's first.
        let second = EditorState(docView([row([mkRun("x")]), row([mkRun("y")], align: "right")],
                                         caretRow: 1))
        XCTAssertEqual(second.align, .right)
    }
}
