//  AttributedRow.swift
//
//  Turns a core `Row` (a list of styled `Run`s) into an `NSAttributedString`.
//  This is the one place a run's `role`/emphasis crosses into AppKit text
//  attributes — the peer of leaf-wasm's `make_run` → CSS class and leaf-tui's
//  `to_ratatui`. The resulting string's UTF-16 indices line up 1:1 with core's
//  `caret_ch` / `click_ch` offsets, because the runs are concatenated in the same
//  order core measured them (and `code_lang` chrome is deliberately excluded, so
//  it never shifts an offset).

import CoreGraphics
import Foundation
import LeafFFI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

enum AttributedRow {
    /// Build the attributed text for one visual row. `theme` supplies fonts and
    /// colours; the row's own `heading` level sizes the *whole* line (so an inline
    /// `` `code` `` run inside a heading still reads at the heading's size),
    /// mirroring how gpui and the web shape a heading line as one unit.
    ///
    /// `source` is the source view: every run in the mono family at the body
    /// size, coloured by what its markup *is* — see `attributes(source:)`.
    static func make(_ row: Row, theme: EditorTheme, math: [UInt32: MathView] = [:],
                     source: Bool = false) -> NSAttributedString {
        make(row.runs, row: row, theme: theme, math: math, source: source)
    }

    /// Build the attributed text for `runs` under `row`'s line-level styling. The
    /// runs are usually the row's own; a thematic break passes only its *prefix*
    /// runs, because its `───` glyphs are replaced by a drawn line (see
    /// `Row.isThematicBreak`).
    ///
    /// `math` is the frame's inline formulas by source offset: a `math` run
    /// whose `src` names one is drawn as its typeset picture — see
    /// `mathPiece` — and one that names none (the frame has not caught up, or
    /// the TeX would not typeset) stays core's own glyph.
    static func make(_ runs: [Run], row: Row, theme: EditorTheme,
                     math: [UInt32: MathView] = [:], source: Bool = false) -> NSAttributedString {
        let result = NSMutableAttributedString()
        let size = row.heading.map { theme.headingSize(Int($0)) } ?? theme.fontSize
        let isHeadingRow = row.heading != nil
        let widened = codeListMarker(row)

        for (index, run) in runs.enumerated() {
            let attrs = attributes(
                run: run,
                size: size,
                headingRow: isHeadingRow,
                codeRow: row.code,
                theme: theme,
                source: source
            )
            if run.role == "math", let mv = math[run.src],
               let piece = mathPiece(run, view: mv, attrs: attrs, theme: theme) {
                result.append(piece)
                continue
            }
            let piece = NSMutableAttributedString(string: run.text, attributes: attrs)
            if run.role == "quote" { kernGutter(piece, theme: theme) }
            if index == widened { widenMarker(piece) }
            result.append(piece)
        }
        return result
    }

    /// How far a code block's tint reaches past its text on either side.
    static let codeFillOutset: CGFloat = 4

    /// The index of the run whose marker a code row widens: its last prefix
    /// run, when that is a list item's marker or indent. The tint starts at
    /// the end of the prefix less `codeFillOutset`, which would bring it up
    /// against the bullet; the marker's gap grows by as much instead, the same
    /// on the item's first row and its later ones, so they still line up.
    /// Nil on any other row — a quote's gutter is wide enough already.
    private static func codeListMarker(_ row: Row) -> Int? {
        guard row.code, let last = row.prefixRuns.last,
              last.role == "list" || last.role == "list-indent" else { return nil }
        return row.prefixRuns.count - 1
    }

    /// Kern the marker's last glyph before its space by `codeFillOutset`. On
    /// the glyph and not the space, for the reason `kernGutter` gives: kern on
    /// the trailing space would stand a caret at the line's start short of the
    /// first letter.
    private static func widenMarker(_ piece: NSMutableAttributedString) {
        let text = piece.string as NSString
        guard text.length >= 2 else { return }
        piece.addAttribute(.kern, value: codeFillOutset, range: NSRange(location: text.length - 2, length: 1))
    }

    /// An inline formula's run as its picture: the one character core drew
    /// replaced by the attachment character — one UTF-16 unit for one, so the
    /// row's offsets are untouched — carrying a `MathAttachment` for TextKit to
    /// draw and a run delegate for Core Text to measure by, both the picture's
    /// size. The picture is set at the run's own size in the theme's ink, so a
    /// formula in a heading is the heading's size. `nil` when the TeX will not
    /// typeset, and the caller keeps core's glyph.
    private static func mathPiece(_ run: Run, view: MathView,
                                  attrs: [NSAttributedString.Key: Any],
                                  theme: EditorTheme) -> NSAttributedString? {
        guard run.text.utf16.count == 1,
              let font = attrs[.font] as? LeafFont,
              let glyph = MathStore.glyph(tex: view.tex, display: view.display,
                                          size: font.pointSize, ink: theme.textColor)
        else { return nil }
        var a = attrs
        a[.attachment] = MathAttachment(glyph: glyph)
        if let delegate = MathRunDelegate.make(glyph) {
            a[NSAttributedString.Key(kCTRunDelegateAttributeName as String)] = delegate
        }
        // The attachment character draws the picture; nothing else on the
        // character should show through it.
        a[.backgroundColor] = nil
        a[.underlineStyle] = nil
        return NSAttributedString(string: "\u{FFFC}", attributes: a)
    }

    /// Build the attributed text for one line of a table cell. A header cell
    /// draws bold (via the same path a heading row takes); everything else — role
    /// colours, inline `code`/`mark` backgrounds, emphasis — is the ordinary run
    /// styling. One line at a time so an in-cell `<br>` shapes as several.
    static func makeCellLine(_ line: TableCellLineView, head: Bool, theme: EditorTheme) -> NSAttributedString {
        let result = NSMutableAttributedString()
        for run in line.runs {
            result.append(
                NSAttributedString(
                    string: run.text,
                    attributes: attributes(
                        run: run,
                        size: theme.fontSize,
                        headingRow: head,
                        codeRow: false,
                        theme: theme
                    )
                )
            )
        }
        return result
    }

    /// The AppKit attributes for a single run.
    ///
    /// Under `source` the run is a piece of the document as written, and this
    /// is a code editor's surface: every run takes the mono family at the
    /// body size, so columns line up and a line is a line. What the markup
    /// *is* still colours it — a delimiter recedes, a link is a link, a fence's
    /// body takes its tokens' inks — and a heading's text is bold, as its row
    /// is in the rendered view. The inline-code pill comes off: a fence's body
    /// arrives as a code run per line here, and a pill per line is a ladder.
    private static func attributes(
        run: Run,
        size: CGFloat,
        headingRow: Bool,
        codeRow: Bool,
        theme: EditorTheme,
        source: Bool = false
    ) -> [NSAttributedString.Key: Any] {
        // A heading's whole line is bold; a run's own `**bold**` adds to that.
        // In the source view the heading is a run, not a row — `# Title` is a
        // delimiter and a heading run on one line — so the role carries it.
        let bold = run.bold || headingRow || (source && isHeadingRole(run.role))
        let isCode = run.role == "code"

        // A raised or lowered run — a footnote reference's `[1]`, an author's
        // `^x^` — is set smaller and shifted off the baseline. Both are measured
        // against the size the run would otherwise have taken, so a reference in
        // a heading scales with the heading rather than with the body.
        //
        // `.baselineOffset` shifts the glyphs without touching the string, so the
        // run's UTF-16 indices still line up 1:1 with core's `caret_ch` — the
        // whole file's contract. Core Text measures the smaller font's advances,
        // so hit-testing and the caret rect follow on their own.
        //
        // The run's own size *step* multiplies the size it would otherwise take,
        // which is what makes `large` mean "a step up from the text around it"
        // rather than a number: on a heading row it is a step up from the
        // heading, in prose a step up from the body. An exact `14pt` replaces
        // that size instead of scaling it — the name scales the ramp and the
        // value replaces it — which `theme.runSize(base:token:)` is the one
        // place deciding. The baseline shift is measured off the result, so a
        // footnote reference inside an x-large run rides that run.
        let runSize = theme.runSize(base: size, token: run.size)
            * (run.sup || run.sub ? theme.baselineScale : 1)

        // A comment in a highlighted block is italic on top of its colour —
        // the one token the palette gives a style as well as a hue.
        let italic = run.italic || (isCode && run.token == "comment")
        var attrs: [NSAttributedString.Key: Any] = [:]
        // A `code` run is monospaced whatever else it says — the role is what the
        // glyphs *are*, and a face named on top of that is a face for the prose
        // around them. Everything else takes the generic family the run names
        // (`serif`, `cursive`, `monospace`…) and the body face when it names none.
        attrs[.font] = isCode || source
            ? theme.monospaceFont(size: runSize, bold: bold, italic: italic)
            : theme.font(family: run.font, size: runSize, bold: bold, italic: run.italic)
        if run.sup {
            attrs[.baselineOffset] = runSize * theme.baselineSuperShift
        } else if run.sub {
            attrs[.baselineOffset] = -runSize * theme.baselineSubShift
        }

        // Foreground colour by role. Headings/body share the text colour — the
        // hierarchy is size + weight, never colour.
        switch run.role {
        case "link": attrs[.foregroundColor] = theme.linkColor
        // A highlighted glyph in a fenced block reads in its token's ink; a
        // plain one — inline code, a block no grammar covers — in `codeColor`.
        case "code": attrs[.foregroundColor] = theme.syntaxColor(run.token)
        case "list": attrs[.foregroundColor] = theme.secondaryColor
        // A list item's marker again, on the item's later rows: spelled with
        // the marker's characters so it takes exactly the marker's width, and
        // drawn clear, so the rows below the bullet line up under its text.
        case "list-indent": attrs[.foregroundColor] = LeafColor.clear
        // A quote's `│ ` gutter is *not* drawn as text: the view paints a real bar
        // down the block's left edge instead. The glyphs stay in the string (they
        // hold the row's UTF-16 offsets in step with core's `caret_ch`) but draw
        // clear, stretched to the themed gutter width so the quoted text is inset
        // by a readable amount rather than by the width of a bar-and-a-space.
        case "quote":
            attrs[.foregroundColor] = LeafColor.clear   // stretched by `kernGutter`
        // Likewise a thematic break's `───`: the row draws a line, not dashes. Any
        // other rule glyph (a table picture's box drawing, when the grid can't be
        // laid out) still paints as text.
        case "rule": attrs[.foregroundColor] = theme.ruleColor
        // Raw markup shown on the caret's line under `MarkupMode.full` — the
        // `*` around an emphasis, a heading's `# `, a link's `](dest)`. Drawn in
        // the secondary colour so the delimiters recede and the line still reads
        // as prose with its scaffolding visible, rather than as source. It keeps
        // the run's own font and emphasis, so a bold run's `**` comes out bold.
        case "delimiter": attrs[.foregroundColor] = theme.secondaryColor
        // A formula's stand-in glyph, where no picture was drawn for it — the
        // `∑` of an inline formula the frame has no view for, or a display
        // block's `∑ tex` placeholder row when its TeX would not typeset.
        // Secondary, as an image's label is.
        case "math": attrs[.foregroundColor] = theme.secondaryColor
        case "mark": attrs[.foregroundColor] = theme.textColor
        default: attrs[.foregroundColor] = theme.textColor
        }

        // A colour the author named on the run wins over the role's own ink: the
        // role says what the glyphs are and the colour is a statement about
        // *these* glyphs, made later. It changes the ink and nothing else, which
        // is the whole of what a foreground colour should do — a coloured run
        // inside a link is still underlined, a coloured `==mark==` keeps its
        // wash, and a coloured word in a heading is still the heading's size.
        //
        // Never over a quote's gutter, which is drawn clear on purpose (the view
        // paints a real bar there): those glyphs hold the row's offsets and must
        // not become visible because the block around them is coloured.
        if let named = run.textColor, run.role != "quote" {
            attrs[.foregroundColor] = theme.textColor(named)
        }

        // Backgrounds honoured by `NSAttributedString.draw(with:)`. Inline `code`
        // gets a faint panel; a code *row* is drawn its own panel by the view, so
        // don't double it there. `==mark==` always gets its highlight.
        if run.role == "code" && !codeRow && !source {
            attrs[.backgroundColor] = theme.codeBackground
        } else if run.role == "mark" {
            attrs[.backgroundColor] = theme.markBackground(run.markColor)
        }
        // A host highlight washes over whatever role background the run had —
        // it is the newer statement about these bytes, and a wash that lost to
        // an author's `==mark==` would make the host's marks vanish exactly
        // where the text is already marked.
        if run.hl != nil {
            attrs[.backgroundColor] = theme.highlightBackground(run.hlColor)
        }

        // A link is underlined; the author's own `{+ins+}` underline adds to it.
        if run.underline || run.role == "link" {
            attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue
        }
        if run.strike {
            attrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
        }
        return attrs
    }

    /// Whether `role` is a heading's — `h1` through `h6`, as core spells them.
    static func isHeadingRole(_ role: String) -> Bool {
        role.count == 2 && role.hasPrefix("h") && ("1"..."6").contains(role.suffix(1))
    }

    /// Kern a `│ `-per-level gutter run to exactly `theme.quoteIndent` per level
    /// — widening it in a font where `│ ` is narrow, tightening it where it's
    /// wide. Pinning the width (rather than only padding it out) is what keeps
    /// every level's bar at the same x down a quote whatever each row's font size
    /// is: a heading inside a quote shapes its gutter at the heading's size, and
    /// would otherwise sit its bar further right than the body rows around it.
    ///
    /// The kern goes on the bar glyphs and never on the spaces. Core Text puts
    /// the caret between two glyphs halfway across whatever kern lies between
    /// them, so kern on the gutter's final space would stand the caret at the
    /// start of a quoted line half a kern short of its first letter — three and a
    /// half points in the system font, whose `│ ` is narrow. A bar is always
    /// followed by its space, so it is never the trailing glyph whose kern Core
    /// Text drops at a line end (a quoted rule's gutter, all that line holds, is
    /// that case for the space). No-op on a run with no bar: there is no level
    /// count to size against.
    private static func kernGutter(_ piece: NSMutableAttributedString, theme: EditorTheme) {
        let text = piece.string as NSString
        let bar = String(Self.quoteBar)
        let barIndices = (0..<text.length).filter { text.substring(with: NSRange(location: $0, length: 1)) == bar }
        guard !barIndices.isEmpty else { return }
        let natural = CGFloat(CTLineGetTypographicBounds(
            CTLineCreateWithAttributedString(piece as CFAttributedString), nil, nil, nil))
        // Never tighten past the painted bar itself — a gutter narrower than the
        // bar would run the quoted text over it.
        let target = max(CGFloat(barIndices.count) * theme.quoteIndent, theme.quoteBarWidth)
        let kern = (target - natural) / CGFloat(barIndices.count)
        for i in barIndices {
            piece.addAttribute(.kern, value: kern, range: NSRange(location: i, length: 1))
        }
    }

    /// The character core spells one blockquote level's gutter with.
    static let quoteBar: Character = "│"
}
