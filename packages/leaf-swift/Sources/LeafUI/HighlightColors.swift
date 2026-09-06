//  HighlightColors.swift
//
//  The UI's side of a highlight's colour: the palette a control offers, the
//  swatch it draws each entry with, and the wash a coloured highlight is painted
//  in. `MarkColor` comes across the binding as a closed enum (core's own
//  vocabulary — the seven circles Markdown spells `==🔴 text==` with), and this
//  is what a menu needs to put it on screen.
//
//  Why a Swift-side table at all, when `Palette.markBackground(named:)` already
//  keys the washes by name: the *renderer* is handed a name — `Run.markColor` is
//  a `String?`, deliberately, so a colour a newer core knows and this build
//  doesn't still draws as a plain highlight rather than not at all — while a
//  *control* offers a closed set the user can press. `name` is the join between
//  the two, and the tests hold it to the palette: every colour this offers has a
//  wash to draw it with.

import Foundation
import LeafFFI

public extension MarkColor {
    /// The colours a palette offers, in the order they are shown — the spectrum,
    /// which is the order twig's own table is written in and the one a row of
    /// swatches reads as deliberate rather than arbitrary.
    ///
    /// A hand-written list because the binding's enum is not `CaseIterable`: a
    /// colour added to core reaches this file as a compile error only in
    /// `name`/`swatch` below, so `highlightColorPaletteIsWholeAndDrawable`
    /// checks the count as well as the contents.
    static var palette: [MarkColor] { [.red, .orange, .yellow, .green, .blue, .purple, .brown] }

    /// The name the document records — a `mark` node's `data-color`, and the key
    /// `Palette.markBackground(named:)` and `Run.markColor` both speak.
    var name: String {
        switch self {
        case .red: return "red"
        case .orange: return "orange"
        case .yellow: return "yellow"
        case .green: return "green"
        case .blue: return "blue"
        case .purple: return "purple"
        case .brown: return "brown"
        }
    }

    /// The circle the document spells this colour with, and the swatch a menu
    /// row shows.
    ///
    /// The emoji rather than a tinted `Image`: a menu item's symbol is drawn in
    /// the menu's own tint on both platforms, which is exactly the wrong
    /// behaviour for a swatch, and this is in any case *what gets written* —
    /// the row shows the reader the bytes their document will carry.
    var swatch: String {
        switch self {
        case .red: return "\u{1F534}"
        case .orange: return "\u{1F7E0}"
        case .yellow: return "\u{1F7E1}"
        case .green: return "\u{1F7E2}"
        case .blue: return "\u{1F535}"
        case .purple: return "\u{1F7E3}"
        case .brown: return "\u{1F7E4}"
        }
    }

    /// The colour's name for a menu row, translatable by a host the way every
    /// other string in the package is.
    var title: String {
        switch self {
        case .red: return loc("highlight.red", "Red")
        case .orange: return loc("highlight.orange", "Orange")
        case .yellow: return loc("highlight.yellow", "Yellow")
        case .green: return loc("highlight.green", "Green")
        case .blue: return loc("highlight.blue", "Blue")
        case .purple: return loc("highlight.purple", "Purple")
        case .brown: return loc("highlight.brown", "Brown")
        }
    }

    /// The swatch and the name as one menu label — `🔴 Red`.
    var menuTitle: String { "\(swatch) \(title)" }
}
