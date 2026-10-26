//  DisplayChoice.swift
//
//  The reader's display choices — how wide the column runs, how big the type
//  is set and in which faces, how far apart its lines and paragraphs stand,
//  which paper a page is — and the theme they resolve to. `LeafUI` takes a
//  whole `EditorTheme` and remembers none of this, so the app keeps the
//  choices in `UserDefaults` under the keys below: the toolbar's menus, the
//  Settings window and Export as PDF all read the same values.

import LeafUI
import SwiftUI

enum DisplayChoice {
    static let columnWidthKey = "display.columnWidth"
    static let textSizeKey = "display.textSize"
    static let paperKey = "display.paper"
    static let flowKey = "display.preserveLineBreaks"
    static let bodyFontKey = "display.bodyFont"
    static let codeFontKey = "display.codeFont"
    static let lineSpacingKey = "display.lineSpacing"
    static let paragraphSpacingKey = "display.paragraphSpacing"

    /// The choices resolved into a theme. Everything else stays at the default
    /// — the point of the measure being counted in *characters* is that width
    /// and text size compose without a table of point widths: pick a size, and
    /// the column that holds ~65 characters of it follows.
    ///
    /// On paper the column is the sheet's, so the equation runs the other way:
    /// `fitted(to:)` sets the type so the chosen measure fills the column — the
    /// default 16 points sets a Letter column only 58 characters wide, shorter
    /// than the flow's 68 — and the text-size choice then scales from there, so
    /// both menus still mean something on a page. How big that reads on screen
    /// is the zoom's business, which opens at fit-width.
    ///
    /// The faces go in first, since fitting measures the body font: a wide
    /// face fills the column in fewer points than a narrow one. The leading
    /// goes in last, as a multiple of whatever size the type came out at.
    static func theme(
        columnWidth: ColumnWidth, textSize: TextSize, type: Typography, page: PageSetup?
    ) -> EditorTheme {
        var t = EditorTheme.default
        t.bodyFontName = type.bodyFont
        t.monoFontName = type.codeFont
        if let page {
            t = t.fitted(to: page, measure: columnWidth.measure ?? 88)
            t.fontSize *= textSize.points / TextSize.medium.points
        } else {
            t.fontSize = textSize.points
        }
        t.lineHeight = t.fontSize * type.lineSpacing.ratio
        t.blockGapScale = type.paragraphSpacing.gap
        t.measure = columnWidth.measure
        return t
    }
}

/// The type choices, together: the two faces and the spacing. What
/// `DisplayChoice.theme` reads, apart from the measure and the size, which
/// the toolbar moves on their own.
struct Typography {
    var bodyFont = EditorTheme.serifFontName
    var codeFont = EditorTheme.systemMonospacedFontName
    var lineSpacing = LineSpacing.normal
    var paragraphSpacing = ParagraphSpacing.normal
}

/// The type choices as stored, for a view to hold: reading `value` in a
/// view's body is what redraws it when Settings changes one.
struct StoredTypography: DynamicProperty {
    @AppStorage(DisplayChoice.bodyFontKey) var bodyFont = Typography().bodyFont
    @AppStorage(DisplayChoice.codeFontKey) var codeFont = Typography().codeFont
    @AppStorage(DisplayChoice.lineSpacingKey) var lineSpacing = Typography().lineSpacing
    @AppStorage(DisplayChoice.paragraphSpacingKey) var paragraphSpacing = Typography().paragraphSpacing

    var value: Typography {
        Typography(bodyFont: bodyFont, codeFont: codeFont,
                   lineSpacing: lineSpacing, paragraphSpacing: paragraphSpacing)
    }
}

/// How far apart a paragraph's lines are set, as the line box over the type
/// size. Normal is the theme's own 1.5; a document's `data-line-height` still
/// multiplies whichever this is.
enum LineSpacing: String, CaseIterable, Identifiable {
    case compact, normal, relaxed, double
    var id: String { rawValue }

    var ratio: CGFloat {
        switch self {
        case .compact: return 1.3
        case .normal:  return 1.5
        case .relaxed: return 1.75
        case .double:  return 2
        }
    }

    var label: String {
        switch self {
        case .compact: return "Compact"
        case .normal:  return "Normal"
        case .relaxed: return "Relaxed"
        case .double:  return "Double"
        }
    }
}

/// The space between two blocks, as a fraction of a line — the theme's
/// `blockGapScale`. A heading's wider space above, and a verse's, scale with it.
enum ParagraphSpacing: String, CaseIterable, Identifiable {
    case tight, normal, open, blankLine
    var id: String { rawValue }

    var gap: CGFloat {
        switch self {
        case .tight:     return 0.25
        case .normal:    return 0.5
        case .open:      return 0.75
        case .blankLine: return 1
        }
    }

    var label: String {
        switch self {
        case .tight:     return "Tight"
        case .normal:    return "Normal"
        case .open:      return "Open"
        case .blankLine: return "A blank line"
        }
    }
}

/// How wide the text column may run, in characters of the body font — the
/// typographic "measure". The named tiers are what a reader actually chooses
/// between; 45–75 characters is the comfortable range for continuous prose, and
/// `.full` is the escape hatch for anyone who'd rather fill the window.
enum ColumnWidth: String, CaseIterable, Identifiable {
    case narrow, medium, wide, full
    var id: String { rawValue }

    var measure: CGFloat? {
        switch self {
        case .narrow: return 52
        case .medium: return 68
        case .wide:   return 88
        case .full:   return nil
        }
    }

    var label: String {
        switch self {
        case .narrow: return "Narrow"
        case .medium: return "Medium"
        case .wide:   return "Wide"
        case .full:   return "Full width"
        }
    }
}

/// The body point size. Everything else in the theme is derived from it — the
/// line height here, and the column width through the character-counted measure.
enum TextSize: String, CaseIterable, Identifiable {
    case small, medium, large
    var id: String { rawValue }

    var points: CGFloat {
        switch self {
        case .small:  return 14
        case .medium: return 16
        case .large:  return 19
        }
    }

    var label: String {
        switch self {
        case .small:  return "Small"
        case .medium: return "Medium"
        case .large:  return "Large"
        }
    }
}

/// The sheet a paginated view and an exported PDF are laid onto.
enum Paper: String, CaseIterable, Identifiable {
    case usLetter, a4
    var id: String { rawValue }

    var setup: PageSetup {
        switch self {
        case .usLetter: return .usLetter
        case .a4:       return .a4
        }
    }

    var label: String {
        switch self {
        case .usLetter: return "US Letter"
        case .a4:       return "A4"
        }
    }
}
