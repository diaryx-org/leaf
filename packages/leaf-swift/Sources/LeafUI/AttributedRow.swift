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
            result.append(
                NSAttributedString(
                    string: run.text,
                    attributes: attributes(
                        run: run,
                        size: size,
                        headingRow: isHeadingRow,
                        codeRow: row.code,
                        theme: theme
                    )
                )
            )
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

        var attrs: [NSAttributedString.Key: Any] = [:]
        attrs[.font] = isCode
            ? theme.monospaceFont(size: size, bold: bold, italic: run.italic)
            : theme.proportionalFont(size: size, bold: bold, italic: run.italic)

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
            attrs[.foregroundColor] = LeafColor.clear
            if let kern = gutterKern(run.text, font: attrs[.font] as? LeafFont, theme: theme) {
                attrs[.kern] = kern
            }
        // Likewise a thematic break's `───`: the row draws a line, not dashes. Any
        // other rule glyph (a table picture's box drawing, when the grid can't be
        // laid out) still paints as text.
        case "rule": attrs[.foregroundColor] = theme.ruleColor
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

        // A link is underlined; the author's own `{+ins+}` underline adds to it.
        if run.underline || run.role == "link" {
            attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue
        }
        if run.strike {
            attrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
        }
        return attrs
    }

    /// The per-character kerning that sizes a `│ `-per-level gutter run to exactly
    /// `theme.quoteIndent` per level — widening it in a font where `│ ` is narrow,
    /// tightening it where it's wide. Pinning the width (rather than only padding
    /// it out) is what keeps every level's bar at the same x down a quote whatever
    /// each row's font size is: a heading inside a quote shapes its gutter at the
    /// heading's size, and would otherwise sit its bar further right than the body
    /// rows around it.
    ///
    /// Spread over every character rather than the last one, since Core Text drops
    /// the trailing character's kern at a line end — a quoted thematic break's
    /// gutter, which is all the line holds, is exactly that case. `nil` when the
    /// run carries no bar: there's no level count to size against.
    private static func gutterKern(_ text: String, font: LeafFont?, theme: EditorTheme) -> CGFloat? {
        let levels = text.filter { $0 == Self.quoteBar }.count
        guard levels > 0, !text.isEmpty else { return nil }
        var attrs: [NSAttributedString.Key: Any] = [:]
        if let font { attrs[.font] = font }
        let line = CTLineCreateWithAttributedString(
            NSAttributedString(string: text, attributes: attrs) as CFAttributedString)
        let natural = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        // Never tighten past the painted bar itself — a gutter narrower than the
        // bar would run the quoted text over it.
        let target = max(CGFloat(levels) * theme.quoteIndent, theme.quoteBarWidth)
        return (target - natural) / CGFloat(text.count)
    }

    /// The character core spells one blockquote level's gutter with.
    static let quoteBar: Character = "│"
}
