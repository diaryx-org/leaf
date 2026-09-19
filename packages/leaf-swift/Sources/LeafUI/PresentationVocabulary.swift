//  PresentationVocabulary.swift
//
//  The UI's side of the presentation vocabulary — alignment, line spacing, size
//  step, and generic face — exactly as `HighlightColors.swift` is the UI's side
//  of a highlight's colour, and for the same reason: the *renderer* is handed a
//  name (`Row.align`, `Run.size`, `Run.font` are all `String?`, so a token a
//  newer core knows still draws as something rather than not at all) while a
//  *control* offers a closed set the user can press. `name` is the join between
//  the two, and `presentationVocabularyIsWholeAndNamed` holds them to each other.
//
//  Colour is missing from this file on purpose. The seven names a run's ink is
//  spelled with are a highlight's seven, `MarkColor` is the one enum for both,
//  and `HighlightColors.swift` already offers them — what differs is the theme
//  that paints them (`EditorTheme.textColor(_:)` against
//  `EditorTheme.markBackground(_:)`), not the vocabulary.
//
//  The titles are the words a word processor's menus use rather than the tokens
//  the document carries: an author picks "Double", and `2` is what gets written.

import Foundation
import LeafFFI

public extension Align {
    /// The alignments a control offers, in the order they are shown — the order
    /// every alignment segment on every platform has run in for forty years.
    /// *Left* is not among them: it is absence, and a control spells it by
    /// clearing (`LeafEditorModel.setAlignment(nil)`).
    static var all: [Align] { [.center, .right, .justify] }

    /// The token the document records — `Row.align`, and the class leaf's
    /// stylesheet selects on.
    var name: String {
        switch self {
        case .center: return "center"
        case .right: return "right"
        case .justify: return "justify"
        }
    }

    /// The alignment a row's `align` names, or nil for a row that names none and
    /// for a token outside the vocabulary — which then reads as the theme's
    /// default, exactly as an unknown colour reads as a plain highlight.
    init?(name: String?) {
        switch name {
        case "center": self = .center
        case "right": self = .right
        case "justify": self = .justify
        default: return nil
        }
    }

    /// The SF Symbol for this alignment's button. `text.alignleft` is the one a
    /// *clearing* button shows, which is why it is not on this enum.
    var symbol: String {
        switch self {
        case .center: return "text.aligncenter"
        case .right: return "text.alignright"
        case .justify: return "text.justify"
        }
    }

    var title: String {
        switch self {
        case .center: return loc("menu.align.center", "Center")
        case .right: return loc("menu.align.right", "Right")
        case .justify: return loc("menu.align.justify", "Justify")
        }
    }
}

public extension LineSpacing {
    /// The spacings a menu offers, opening outwards — the word processor's own
    /// list less "single", which is absence.
    static var all: [LineSpacing] { [.oneFifteen, .oneHalf, .double] }

    /// The token the document records — `Row.lineHeight`, the ratio spelled as
    /// itself so the source reads as arithmetic and the stylesheet's line is
    /// `line-height: 1.5`.
    var name: String {
        switch self {
        case .oneFifteen: return "1.15"
        case .oneHalf: return "1.5"
        case .double: return "2"
        }
    }

    init?(name: String?) {
        switch name {
        case "1.15": self = .oneFifteen
        case "1.5": self = .oneHalf
        case "2": self = .double
        default: return nil
        }
    }

    /// The ratio as a word where there is one, and as the number where there
    /// isn't: "Double" is what an author asks for, and nobody says "one point
    /// one five".
    var title: String {
        switch self {
        case .oneFifteen: return loc("menu.spacing.oneFifteen", "1.15")
        case .oneHalf: return loc("menu.spacing.oneHalf", "1.5")
        case .double: return loc("menu.spacing.double", "Double")
        }
    }
}

public extension SizeStep {
    /// The ramp, smallest first — CSS's `<absolute-size>` keywords with `medium`
    /// removed, `medium` being absence. The order is the one a size menu lists
    /// and the one `stepped(from:up:)` walks.
    static var ramp: [SizeStep] {
        [.xxSmall, .xSmall, .small, .large, .xLarge, .xxLarge, .xxxLarge]
    }

    /// The token the document records — `Run.size`, CSS's own spelling, so a
    /// `data-size` leaf writes is a `font-size` a browser already understands.
    var name: String {
        switch self {
        case .xxSmall: return "xx-small"
        case .xSmall: return "x-small"
        case .small: return "small"
        case .large: return "large"
        case .xLarge: return "x-large"
        case .xxLarge: return "xx-large"
        case .xxxLarge: return "xxx-large"
        }
    }

    init?(name: String?) {
        guard let name,
              let step = SizeStep.ramp.first(where: { $0.name == name })
        else { return nil }
        self = step
    }

    var title: String {
        switch self {
        case .xxSmall: return loc("menu.size.xxSmall", "XX-Small")
        case .xSmall: return loc("menu.size.xSmall", "X-Small")
        case .small: return loc("menu.size.small", "Small")
        case .large: return loc("menu.size.large", "Large")
        case .xLarge: return loc("menu.size.xLarge", "X-Large")
        case .xxLarge: return loc("menu.size.xxLarge", "XX-Large")
        case .xxxLarge: return loc("menu.size.xxxLarge", "XXX-Large")
        }
    }

    /// One step up or down the ramp from `current` — what ⌘⇧+ and ⌘⇧- mean, and
    /// the only place the *gap in the middle* of the ramp is written down.
    ///
    /// `nil` is the ramp's own middle rung (CSS's `medium`, the theme's size),
    /// not "off the end": stepping up from it reaches `large` and stepping down
    /// reaches `small`, so an author can walk from small text through ordinary
    /// text to large without the document ever carrying a key that says nothing.
    /// At either end the step stays put rather than wrapping — a size control
    /// that jumped from XXX-Large back to XX-Small on one press would be a
    /// surprise nobody recovers from by pressing again.
    static func stepped(from current: SizeStep?, up: Bool) -> SizeStep? {
        // The ramp with absence in its place: [xx-small … small, nil, large …].
        let rungs: [SizeStep?] = ramp.prefix(3).map { $0 } + [nil] + ramp.suffix(4).map { $0 }
        let here = rungs.firstIndex { $0 == current } ?? rungs.firstIndex(of: nil)!
        let there = min(max(here + (up ? 1 : -1), 0), rungs.count - 1)
        return rungs[there]
    }
}

public extension FontFamily {
    /// The faces a menu offers — CSS's generic families less `fantasy` and
    /// `system-ui`, neither of which an author asks for by name.
    static var all: [FontFamily] { [.serif, .sansSerif, .monospace, .cursive] }

    /// The token the document records — `Run.font`, the CSS generic. Which face
    /// the generic *is* is the theme's answer (`EditorTheme.fontFamilies`), so a
    /// document never names a family the machine may not have.
    var name: String {
        switch self {
        case .serif: return "serif"
        case .sansSerif: return "sans-serif"
        case .monospace: return "monospace"
        case .cursive: return "cursive"
        }
    }

    init?(name: String?) {
        switch name {
        case "serif": self = .serif
        case "sans-serif": self = .sansSerif
        case "monospace": self = .monospace
        case "cursive": self = .cursive
        default: return nil
        }
    }

    var title: String {
        switch self {
        case .serif: return loc("menu.font.serif", "Serif")
        case .sansSerif: return loc("menu.font.sansSerif", "Sans Serif")
        case .monospace: return loc("menu.font.monospace", "Monospace")
        case .cursive: return loc("menu.font.cursive", "Cursive")
        }
    }
}
