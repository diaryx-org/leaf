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
        let result = NSMutableAttributedString()
        let size = row.heading.map { theme.headingSize(Int($0)) } ?? theme.fontSize
        let isHeadingRow = row.heading != nil

        for run in row.runs {
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

    /// Build the attributed text for one table cell. A header cell draws bold
    /// (via the same path a heading row takes); everything else — role colours,
    /// inline `code`/`mark` backgrounds, emphasis — is the ordinary run styling.
    static func makeCell(_ cell: TableCellView, head: Bool, theme: EditorTheme) -> NSAttributedString {
        let result = NSMutableAttributedString()
        for run in cell.runs {
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
        case "list", "quote": attrs[.foregroundColor] = theme.secondaryColor
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
}
