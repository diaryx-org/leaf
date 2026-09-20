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

import CoreGraphics
import Foundation
import LeafFFI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

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

// MARK: - the two halves of each open type, for the menu that shows both
//
// The binding's four presentation types carry a name *or* a value —
// `.step(.large)` or `.points(14)`, `.generic(.serif)` or `.named("Garamond")`
// — which is `docs/proposals/exact-presentation-values.md`. The menus offer the
// names first and the value under a divider, so each type is asked twice: which
// of the named rows ticks, and whether there is an exact value to show a row of
// its own for. These eight are those two questions, written once.

public extension FontSize {
    /// The step, or nil for an exact point size no row of the seven can tick.
    var stepOnly: SizeStep? {
        guard case let .step(step) = self else { return nil }
        return step
    }

    /// The exact size in points, or nil for a step — the row a menu draws of
    /// its own, above *Other…*, when the caret stands in one.
    var pointsOnly: CGFloat? {
        guard case let .points(points) = self else { return nil }
        return CGFloat(points)
    }

    /// How this size reads in a menu: the step's own title, or the number with
    /// its unit spelled the way a font panel spells it — `14 pt`, with the
    /// space, because this is a label and not a token.
    var title: String {
        if let step = stepOnly { return step.title }
        return String(format: loc("menu.size.points", "%@ pt"),
                      PresentationValue.spell(pointsOnly ?? 0))
    }

    /// The title, but only when this is an exact value — what the row a menu
    /// draws above *Other…* shows, and nil where there is no such row because
    /// one of the named rows already ticks.
    var exactTitle: String? { stepOnly == nil ? title : nil }

    /// One rung up or down from `current` — ⌘⇧+ and ⌘⇧-, and the whole of what
    /// *Bigger* and *Smaller* decide.
    ///
    /// Which rung depends on which half the size is in. A step, or no size at
    /// all, walks [`SizeStep.stepped(from:up:)`]'s ramp, which has the theme's
    /// own size in the middle of it. **An exact size moves by a point**: the
    /// ramp has no rung to walk to from fourteen, and an author working in
    /// numbers is offered the next number — which is what the stepper beside a
    /// font panel's own field does. Held at a point when it would go below one,
    /// because type smaller than that is not a size anyone means, and at the
    /// top of what the vocabulary carries.
    static func stepped(from current: FontSize?, up: Bool) -> FontSize? {
        if let points = current?.pointsOnly {
            let moved = min(max(points + (up ? 1 : -1), 1), PresentationValue.largest)
            return .points(Double(moved))
        }
        return SizeStep.stepped(from: current?.stepOnly, up: up).map(FontSize.step)
    }
}

public extension LineHeight {
    /// The step, or nil for an exact ratio no row of the three can tick.
    var stepOnly: LineSpacing? {
        guard case let .step(step) = self else { return nil }
        return step
    }

    /// The exact ratio, or nil for a step.
    var ratioOnly: CGFloat? {
        guard case let .ratio(ratio) = self else { return nil }
        return CGFloat(ratio)
    }

    /// How this spacing reads in a menu — the step's title, or the ratio as
    /// itself, which is how the three named ones are already titled.
    var title: String {
        if let step = stepOnly { return step.title }
        return PresentationValue.spell(ratioOnly ?? 0)
    }

    /// The title, but only when this is an exact ratio — [`FontSize`]'s peer.
    var exactTitle: String? { stepOnly == nil ? title : nil }
}

public extension FontFace {
    /// The generic, or nil for a family the author named.
    var genericOnly: FontFamily? {
        guard case let .generic(family) = self else { return nil }
        return family
    }

    /// The family the author named, or nil for a generic.
    var familyOnly: String? {
        guard case let .named(name) = self else { return nil }
        return name
    }

    /// How this face reads in a menu — the generic's title, or the family as
    /// the author named it, which is also what the document carries.
    var title: String { genericOnly?.title ?? familyOnly ?? "" }

    /// The title, but only when the author named a family — [`FontSize`]'s peer.
    var exactTitle: String? { familyOnly }
}

public extension TextColor {
    /// The name, or nil for an exact triple no swatch of the seven can tick.
    var namedOnly: MarkColor? {
        guard case let .named(color) = self else { return nil }
        return color
    }

    /// The exact triple as the document spells it — six lowercase hex digits
    /// behind a `#` — or nil for a name.
    var hexOnly: String? {
        guard case let .rgb(r, g, b) = self else { return nil }
        return String(format: "#%02x%02x%02x", Int(r), Int(g), Int(b))
    }

    /// How this colour reads in a menu: the swatch and name for one of the
    /// seven, and the hex itself for a triple.
    ///
    /// The hex *is* the swatch, deliberately — `HighlightColors.swift` argues
    /// that a menu row's symbol is drawn in the menu's own tint, which is
    /// exactly the wrong behaviour for a colour, and that what the row should
    /// show is the bytes the document will carry. For a named colour those
    /// bytes are a word with an emoji circle beside it; for a triple they are
    /// the triple.
    var title: String { namedOnly?.menuTitle ?? hexOnly ?? "" }

    /// The title, but only when this is an exact triple — [`FontSize`]'s peer.
    var exactTitle: String? { hexOnly }
}

// MARK: - the exact half's grammar

/// The open half of the presentation vocabulary, read off a token — a point
/// size, a ratio, an ink, a family — and spelled back.
///
/// The one place this package parses a `data-` value. The theme's tables are
/// keyed by *name* and a name is all they can ever hold, so every resolver on
/// `EditorTheme` falls through to here when its table has no entry: a
/// `Run.size` of `"14pt"` is fourteen points, a `Row.lineHeight` of `"1.3"` is
/// a ratio, a `Run.textColor` of `"#c03030"` is that ink in both appearances,
/// and a `Run.font` of `"Garamond"` is that family where the machine has it.
///
/// The grammar is `leaf_core::style`'s, cut to what this side needs: CSS's
/// `<number>` (digits, at most one point, digits on whichever side of it there
/// is, no sign and no exponent), a `pt` suffix read without regard to case, a
/// `#rrggbb` or `#rgb`, and 0.01 to 655.35 as the range — core carries the
/// number in hundredths of a `UInt16`, so a value outside that is one no
/// document can hold and is not one this reads.
public enum PresentationValue {
    /// The smallest and largest number the vocabulary carries — core's
    /// hundredths, as a Swift reader sees them.
    static let smallest: CGFloat = 0.01
    static let largest: CGFloat = 655.35

    /// The point size `token` names — `"14pt"`, `"13.5PT"` — or nil for a step,
    /// for a unit that is not points, and for a number the vocabulary cannot
    /// carry. Only `pt`: a screen's units are not a document's, and the
    /// relative ones are what the steps already are.
    public static func points(size token: String) -> CGFloat? {
        let trimmed = token.trimmingCharacters(in: .whitespaces)
        guard trimmed.count > 2 else { return nil }
        let unit = trimmed.suffix(2)
        guard unit.lowercased() == "pt" else { return nil }
        return number(String(trimmed.dropLast(2)))
    }

    /// The ratio `token` names — `"1.3"` — or nil for one of the three named
    /// spacings and for a number outside the vocabulary. A `"1"` never reaches
    /// here: single spacing is absence, and core clears the key rather than
    /// writing a token that says nothing.
    public static func ratio(spacing token: String) -> CGFloat? {
        number(token.trimmingCharacters(in: .whitespaces))
    }

    /// The ink `token` names — `"#c03030"`, or the `"#f00"` shorthand a
    /// stylesheet author writes — or nil for anything else.
    ///
    /// The same colour in both appearances, which is what *exact* means: a name
    /// is two inks and the theme owns both, and a triple is the one the author
    /// wrote. `Palette.dynamic` is deliberately not reached for here.
    public static func ink(color token: String) -> LeafColor? {
        let trimmed = token.trimmingCharacters(in: .whitespaces)
        guard trimmed.hasPrefix("#") else { return nil }
        let digits = trimmed.dropFirst()
        // `#f00` is `#ff0000`, each digit doubled, exactly as CSS expands it.
        let six = digits.count == 3 ? digits.flatMap { [$0, $0] } : Array(digits)
        return leafColor(hex: "#" + String(six))
    }

    /// The family `token` names, if this machine has it installed — asked of
    /// the platform's own registry, which is the only thing that knows.
    ///
    /// Nil for a family it hasn't got, so the run draws in the theme's body
    /// face like the prose around it rather than in whatever the system
    /// substitutes: a document set in a face this machine lacks should read as
    /// *this* document, not as one badly reset. That is the portability cost
    /// the *Other…* row buys, stated where it is paid.
    public static func family(face token: String) -> String? {
        let name = token.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, LeafFont(name: name, size: LeafFont.systemFontSize) != nil
        else { return nil }
        return name
    }

    /// CSS's `<number>`, cut down to what a word processor's field writes and
    /// to what core can carry: digits, at most one point, digits on whichever
    /// side of it there is (`.5` is a number, `14.` is a typo), no sign and no
    /// exponent, and 0.01 to 655.35.
    ///
    /// Hand-read rather than handed to `Double(_:)`, which takes `"14."`,
    /// `"1e3"`, `"-2"`, `"0x1p3"` and `"nan"` — every one of them a value core
    /// would refuse, and three of them a value it would refuse *silently*.
    static func number(_ text: String) -> CGFloat? {
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count <= 2 else { return nil }
        let whole = parts.first.map(String.init) ?? ""
        let fraction = parts.count == 2 ? String(parts[1]) : ""
        guard !(whole.isEmpty && fraction.isEmpty) else { return nil }
        // A trailing bare point is not a `<number>`: `14.` is a typo, and
        // reading it as 14 would write a document the author did not mean.
        guard parts.count < 2 || !fraction.isEmpty else { return nil }
        guard (whole + fraction).allSatisfy(\.isASCII),
              (whole + fraction).allSatisfy({ $0.isNumber })
        else { return nil }
        guard let value = Double(whole.isEmpty ? "0" : whole),
              let frac = Double(fraction.isEmpty ? "0" : fraction)
        else { return nil }
        // Rounded to hundredths the way core rounds, so a field's 1.333 and the
        // token core writes back agree about which row ticks.
        let number = ((value + frac / pow(10, Double(fraction.count))) * 100).rounded() / 100
        guard number >= smallest, number <= largest else { return nil }
        return CGFloat(number)
    }

    /// A number as core spells it back: the shortest decimal, so `14` and not
    /// `14.0`, and `1.3` and not `1.30`.
    static func spell(_ value: CGFloat) -> String {
        let hundredths = (Double(value) * 100).rounded()
        if hundredths.truncatingRemainder(dividingBy: 100) == 0 {
            return String(Int(hundredths / 100))
        }
        var text = String(format: "%.2f", hundredths / 100)
        if text.hasSuffix("0") { text.removeLast() }
        return text
    }
}
