//  DisplayChoice.swift
//
//  The reader's display choices — how wide the column runs, how big the type
//  is set, which paper a page is — and the theme they resolve to. `LeafUI`
//  takes a whole `EditorTheme` and remembers none of this, so the app keeps
//  the choices in `UserDefaults` under the keys below: the toolbar's menus,
//  the Settings window and Export as PDF all read the same three values.

import LeafUI
import SwiftUI

enum DisplayChoice {
    static let columnWidthKey = "display.columnWidth"
    static let textSizeKey = "display.textSize"
    static let paperKey = "display.paper"
    static let flowKey = "display.preserveLineBreaks"

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
    static func theme(columnWidth: ColumnWidth, textSize: TextSize, page: PageSetup?) -> EditorTheme {
        var t = EditorTheme.default
        if let page {
            t = t.fitted(to: page, measure: columnWidth.measure ?? 88)
            let factor = textSize.points / TextSize.medium.points
            t.fontSize *= factor
            t.lineHeight *= factor
        } else {
            t.fontSize = textSize.points
            t.lineHeight = textSize.points * 1.5
        }
        t.measure = columnWidth.measure
        return t
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
