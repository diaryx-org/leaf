//  Platform.swift
//
//  The thin AppKit⇄UIKit shim that lets the rest of LeafUI stay platform-neutral.
//  Fonts, colours, and attributed-string drawing are the only places macOS and
//  iOS truly diverge; everything above this file (theme, attribute mapping, the
//  CoreText layout/caret/hit-test engine) is written once against these aliases.

import CoreGraphics
import Foundation

#if canImport(UIKit)
import UIKit
public typealias LeafColor = UIColor
public typealias LeafFont = UIFont
public typealias LeafView = UIView
#elseif canImport(AppKit)
import AppKit
public typealias LeafColor = NSColor
public typealias LeafFont = NSFont
public typealias LeafView = NSView
#endif

/// Text insets, kept as a plain struct so the theme doesn't depend on either
/// toolkit's edge-inset type.
public struct LeafInsets: Equatable {
    public var top: CGFloat
    public var left: CGFloat
    public var bottom: CGFloat
    public var right: CGFloat
    public init(top: CGFloat, left: CGFloat, bottom: CGFloat, right: CGFloat) {
        self.top = top; self.left = left; self.bottom = bottom; self.right = right
    }
}

/// The default semantic colours, resolved to each toolkit's dynamic system
/// colours so light/dark just works on both platforms. Public because the
/// `EditorTheme` initializer names them in its default arguments.
public enum Palette {
    #if canImport(UIKit)
    public static var label: LeafColor { .label }
    public static var secondary: LeafColor { .secondaryLabel }
    public static var tertiary: LeafColor { .tertiaryLabel }
    public static var link: LeafColor { .link }
    public static var separator: LeafColor { .separator }
    public static var selection: LeafColor { UIColor.systemBlue.withAlphaComponent(0.30) }
    public static var inactiveSelection: LeafColor { UIColor.systemGray.withAlphaComponent(0.30) }
    public static var accent: LeafColor { .tintColor }
    /// The insertion point. UIKit draws the iOS caret itself, in the tint.
    public static var caret: LeafColor { .tintColor }
    /// The paper a paginated document's sheets are drawn on, and the surface they
    /// sit on. Semantic on both platforms, so a page tracks light/dark like the
    /// rest of the chrome.
    public static var page: LeafColor { .systemBackground }
    public static var pageBackdrop: LeafColor { .systemGray2 }
    #elseif canImport(AppKit)
    public static var label: LeafColor { .labelColor }
    public static var secondary: LeafColor { .secondaryLabelColor }
    public static var tertiary: LeafColor { .tertiaryLabelColor }
    public static var link: LeafColor { .linkColor }
    public static var separator: LeafColor { .separatorColor }
    public static var selection: LeafColor { .selectedTextBackgroundColor }
    public static var inactiveSelection: LeafColor { .unemphasizedSelectedTextBackgroundColor }
    /// The insertion point, as the system paints it in its own text views —
    /// what a user's accent colour reaches. Named only from macOS 14; before
    /// that the caret is the label's ink, as `NSTextView`'s was.
    public static var caret: LeafColor {
        if #available(macOS 14, *) { return .textInsertionPointColor }
        return .labelColor
    }
    public static var accent: LeafColor { .controlAccentColor }
    /// AppKit names both of these outright — `underPageBackgroundColor` is the
    /// surface Preview and Pages set a sheet on — so the paginated view gets the
    /// system's own answer rather than a hand-picked grey.
    public static var page: LeafColor { .textBackgroundColor }
    public static var pageBackdrop: LeafColor { .underPageBackgroundColor }
    #endif
    public static var codeBackground: LeafColor { secondary.withAlphaComponent(0.08) }
    /// A `:::name{.class}` directive container's (diaryx's `:::vis{.audience}`
    /// visibility block, say) outline — a dashed border round the whole span
    /// rather than a filled panel, so it reads as a distinct aside without
    /// competing with prose for attention the way a solid tint would.
    public static var directiveBorderColor: LeafColor { separator }
    public static var markBackground: LeafColor { LeafColor.systemYellow.withAlphaComponent(0.28) }
    /// The wash behind a highlight that named its own colour (`==🔴 text==`),
    /// keyed by the name twig records — `red`, `orange`, `yellow`, `green`,
    /// `blue`, `purple`, `brown` — and nil for anything outside that vocabulary,
    /// which then falls back to `markBackground`.
    ///
    /// The system colours at `markBackground`'s own alpha, not hand-picked
    /// hexes: a wash over the page has to stay a wash in both appearances, and
    /// the system's reds are the ones already tuned for that. Yellow is
    /// `markBackground` exactly — a document that says yellow and one that says
    /// nothing both mean a yellow highlighter.
    public static func markBackground(named name: String) -> LeafColor? {
        let base: LeafColor
        switch name {
        case "red": base = .systemRed
        case "orange": base = .systemOrange
        case "yellow": base = .systemYellow
        case "green": base = .systemGreen
        case "blue": base = .systemBlue
        case "purple": base = .systemPurple
        case "brown": base = .systemBrown
        default: return nil
        }
        return base.withAlphaComponent(0.28)
    }
    /// The wash behind a host-painted highlight (`LeafEditorModel.setHighlights`)
    /// — an annotation's footprint, a search hit. The same yellow family as
    /// `markBackground` (both mean "someone marked this"), a step stronger so a
    /// host's mark reads over an author's `==mark==` where the two coincide.
    public static var hostHighlight: LeafColor { LeafColor.systemYellow.withAlphaComponent(0.36) }
    /// The light a landing leaves on the block it arrived at, for the moment
    /// before it fades. The accent colour, because this is the app telling the
    /// reader where it took them — not a mark in their document, which is what
    /// `markBackground`'s yellow means and must go on meaning. Faint, because it
    /// sits *behind* prose the reader is about to read.
    public static var landingFlash: LeafColor { accent.withAlphaComponent(0.22) }
    // Table chrome — a grid line, a header fill, and a body stripe, all derived
    // from the label colour so they track light/dark like everything else.
    public static var tableBorder: LeafColor { separator }
    public static var tableHeader: LeafColor { secondary.withAlphaComponent(0.12) }
    public static var tableStripe: LeafColor { secondary.withAlphaComponent(0.05) }
}

/// Parse a `#RRGGBB` hex string into a colour, or nil for anything else — the
/// one spelling a `Highlight.color` hint may use. Deliberately strict: a hint
/// that doesn't parse falls back to the theme's default wash rather than
/// guessing at CSS names.
func leafColor(hex: String) -> LeafColor? {
    let trimmed = hex.trimmingCharacters(in: .whitespaces)
    guard trimmed.hasPrefix("#"), trimmed.count == 7,
          let value = UInt32(trimmed.dropFirst(), radix: 16)
    else { return nil }
    return LeafColor(
        red: CGFloat((value >> 16) & 0xFF) / 255,
        green: CGFloat((value >> 8) & 0xFF) / 255,
        blue: CGFloat(value & 0xFF) / 255,
        alpha: 1
    )
}

/// Build a font by family name + size with optional bold/italic traits — the one
/// call that papers over `NSFontDescriptor` vs `UIFontDescriptor`.
///
/// Two names are not families but requests for the system's own type — see
/// `EditorTheme.systemFontName` — because the system font has no stable
/// PostScript name to ask for: `.AppleSystemUIFont` is private, and a font
/// looked up by it is not the one the OS would pick for this size and weight.
func makeFont(name: String, size: CGFloat, bold: Bool, italic: Bool) -> LeafFont {
    let base: LeafFont
    switch name {
    case EditorTheme.systemFontName: base = LeafFont.systemFont(ofSize: size)
    case EditorTheme.systemMonospacedFontName: base = LeafFont.monospacedSystemFont(ofSize: size, weight: .regular)
    default: base = LeafFont(name: name, size: size) ?? LeafFont.systemFont(ofSize: size)
    }
    #if canImport(UIKit)
    var traits: UIFontDescriptor.SymbolicTraits = []
    if bold { traits.insert(.traitBold) }
    if italic { traits.insert(.traitItalic) }
    guard !traits.isEmpty, let desc = base.fontDescriptor.withSymbolicTraits(traits) else { return base }
    return UIFont(descriptor: desc, size: size)
    #elseif canImport(AppKit)
    var traits: NSFontDescriptor.SymbolicTraits = []
    if bold { traits.insert(.bold) }
    if italic { traits.insert(.italic) }
    guard !traits.isEmpty else { return base }
    let desc = base.fontDescriptor.withSymbolicTraits(traits)
    return NSFont(descriptor: desc, size: size) ?? base
    #endif
}
