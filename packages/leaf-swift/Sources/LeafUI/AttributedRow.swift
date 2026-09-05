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
    static func make(_ row: Row, theme: EditorTheme) -> NSAttributedString {
        make(row.runs, row: row, theme: theme)
    }

    /// Build the attributed text for `runs` under `row`'s line-level styling. The
    /// runs are usually the row's own; a thematic break passes only its *prefix*
    /// runs, because its `───` glyphs are replaced by a drawn line (see
    /// `Row.isThematicBreak`).
    static func make(_ runs: [Run], row: Row, theme: EditorTheme) -> NSAttributedString {
        let result = NSMutableAttributedString()
        let size = row.heading.map { theme.headingSize(Int($0)) } ?? theme.fontSize
        let isHeadingRow = row.heading != nil

        for run in runs {
            let piece = NSMutableAttributedString(
                string: run.text,
                attributes: attributes(
                    run: run,
                    size: size,
                    headingRow: isHeadingRow,
                    codeRow: row.code,
                    theme: theme
                )
            )
            if run.role == "quote" { kernGutter(piece, theme: theme) }
            result.append(piece)
        }
        return result
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
    private static func attributes(
        run: Run,
        size: CGFloat,
        headingRow: Bool,
        codeRow: Bool,
        theme: EditorTheme
    ) -> [NSAttributedString.Key: Any] {
        // A heading's whole line is bold; a run's own `**bold**` adds to that.
        let bold = run.bold || headingRow
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
        let runSize = size * (run.sup || run.sub ? theme.baselineScale : 1)

        var attrs: [NSAttributedString.Key: Any] = [:]
        attrs[.font] = isCode
            ? theme.monospaceFont(size: runSize, bold: bold, italic: run.italic)
            : theme.proportionalFont(size: runSize, bold: bold, italic: run.italic)
        if run.sup {
            attrs[.baselineOffset] = runSize * theme.baselineSuperShift
        } else if run.sub {
            attrs[.baselineOffset] = -runSize * theme.baselineSubShift
        }

        // Foreground colour by role. Headings/body share the text colour — the
        // hierarchy is size + weight, never colour.
        switch run.role {
        case "link": attrs[.foregroundColor] = theme.linkColor
        case "code": attrs[.foregroundColor] = theme.codeColor
        case "list": attrs[.foregroundColor] = theme.secondaryColor
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
        case "mark": attrs[.foregroundColor] = theme.textColor
        default: attrs[.foregroundColor] = theme.textColor
        }

        // Backgrounds honoured by `NSAttributedString.draw(with:)`. Inline `code`
        // gets a faint panel; a code *row* is drawn its own panel by the view, so
        // don't double it there. `==mark==` always gets its highlight.
        if run.role == "code" && !codeRow {
            attrs[.backgroundColor] = theme.codeBackground
        } else if run.role == "mark" {
            attrs[.backgroundColor] = theme.markBackground
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
