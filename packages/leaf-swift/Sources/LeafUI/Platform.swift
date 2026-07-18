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
public struct LeafInsets {
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
    public static var accent: LeafColor { .tintColor }
    #elseif canImport(AppKit)
    public static var label: LeafColor { .labelColor }
    public static var secondary: LeafColor { .secondaryLabelColor }
    public static var tertiary: LeafColor { .tertiaryLabelColor }
    public static var link: LeafColor { .linkColor }
    public static var separator: LeafColor { .separatorColor }
    public static var selection: LeafColor { .selectedTextBackgroundColor }
    public static var accent: LeafColor { .controlAccentColor }
    #endif
    public static var codeBackground: LeafColor { secondary.withAlphaComponent(0.08) }
    public static var markBackground: LeafColor { LeafColor.systemYellow.withAlphaComponent(0.28) }
}

/// Build a font by family name + size with optional bold/italic traits — the one
/// call that papers over `NSFontDescriptor` vs `UIFontDescriptor`.
func makeFont(name: String, size: CGFloat, bold: Bool, italic: Bool) -> LeafFont {
    let base = LeafFont(name: name, size: size) ?? LeafFont.systemFont(ofSize: size)
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
