//  DisplayChoice.swift
//
//  The reader's display choices — the faces, the size, the line spacing and
//  how wide the column runs — kept in `UserDefaults` under the keys the iOS
//  app's `DisplayChoice` uses, and the `Look` they resolve to. The Settings
//  window and every document window read the same values, so a change in one
//  redraws them all.

import AppKit
import SwiftUI

enum DisplayChoice {
    static let columnWidthKey = "display.columnWidth"
    static let textSizeKey = "display.textSize"
    static let flowKey = "display.preserveLineBreaks"
    static let bodyFontKey = "display.bodyFont"
    static let codeFontKey = "display.codeFont"
    static let lineSpacingKey = "display.lineSpacing"
    static let paperKey = "display.paper"

    /// The theme's own names for its default faces, as the iOS app stores them.
    static let serifFontName = "IowanOldStyle-Roman"
    static let systemFontName = ".AppleSystemUIFont"
    static let monoFontName = "Menlo"

    /// The choices as what the editor draws with.
    static func look(columnWidth: ColumnWidth, textSize: TextSize, type: Typography) -> Look {
        Look(body: css(body: type.bodyFont), mono: css(mono: type.codeFont),
             size: textSize.points, leading: type.lineSpacing.ratio,
             measure: columnWidth.measure)
    }

    /// A body face as a CSS family list, falling back to the default serif.
    static func css(body name: String) -> String {
        switch name {
        case serifFontName, "Iowan Old Style": return #""Iowan Old Style", Georgia, serif"#
        case systemFontName: return #"system-ui, "Helvetica Neue", sans-serif"#
        default: return #""\#(quoted(name))", "Iowan Old Style", Georgia, serif"#
        }
    }

    /// A code face as a CSS family list, falling back to Menlo.
    static func css(mono name: String) -> String {
        name == monoFontName ? "Menlo, monospace" : #""\#(quoted(name))", Menlo, monospace"#
    }

    private static func quoted(_ s: String) -> String {
        s.replacingOccurrences(of: "\\", with: "").replacingOccurrences(of: "\"", with: "")
    }
}

/// The type choices, together.
struct Typography {
    var bodyFont = DisplayChoice.serifFontName
    var codeFont = DisplayChoice.monoFontName
    var lineSpacing = LineSpacing.normal
}

/// The type choices as stored, for a view to hold: reading `value` in a
/// view's body is what redraws it when Settings changes one.
struct StoredTypography: DynamicProperty {
    @AppStorage(DisplayChoice.bodyFontKey) var bodyFont = Typography().bodyFont
    @AppStorage(DisplayChoice.codeFontKey) var codeFont = Typography().codeFont
    @AppStorage(DisplayChoice.lineSpacingKey) var lineSpacing = Typography().lineSpacing

    var value: Typography {
        Typography(bodyFont: bodyFont, codeFont: codeFont, lineSpacing: lineSpacing)
    }
}

/// How far apart a paragraph's lines are set, as the line box over the size.
enum LineSpacing: String, CaseIterable, Identifiable {
    case compact, normal, relaxed, double
    var id: String { rawValue }

    var ratio: Double {
        switch self {
        case .compact: return 1.3
        case .normal: return 1.5
        case .relaxed: return 1.75
        case .double: return 2
        }
    }

    var label: String {
        switch self {
        case .compact: return "Compact"
        case .normal: return "Normal"
        case .relaxed: return "Relaxed"
        case .double: return "Double"
        }
    }
}

/// How wide the text column may run, in characters of the body face.
enum ColumnWidth: String, CaseIterable, Identifiable {
    case narrow, medium, wide, full
    var id: String { rawValue }

    var measure: Double? {
        switch self {
        case .narrow: return 52
        case .medium: return 68
        case .wide: return 88
        case .full: return nil
        }
    }

    var label: String {
        switch self {
        case .narrow: return "Narrow"
        case .medium: return "Medium"
        case .wide: return "Wide"
        case .full: return "Full width"
        }
    }
}

/// The body size, in points.
enum TextSize: String, CaseIterable, Identifiable {
    case small, medium, large
    var id: String { rawValue }

    var points: Double {
        switch self {
        case .small: return 14
        case .medium: return 16
        case .large: return 19
        }
    }

    var label: String {
        switch self {
        case .small: return "Small"
        case .medium: return "Medium"
        case .large: return "Large"
        }
    }
}

/// The sheet Export as PDF lays the document onto, in points.
enum Paper: String, CaseIterable, Identifiable {
    case usLetter, a4
    var id: String { rawValue }

    var size: NSSize {
        switch self {
        case .usLetter: return NSSize(width: 612, height: 792)
        case .a4: return NSSize(width: 595.28, height: 841.89)
        }
    }

    var label: String {
        switch self {
        case .usLetter: return "US Letter"
        case .a4: return "A4"
        }
    }
}
