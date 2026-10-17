//  LeafToolbarEditor.swift
//
//  What a formatting toolbar needs from an editor, and nothing else: what is
//  in force at the caret, what the document's format can spell, and one
//  command per control. Two editors answer it. LeafUI's `LeafEditorModel` is
//  one; the Mac app's model over leaf-dioxus (`apps/leaf-mac`), which turns
//  each command into the key chord it stands for, is the other.
//
//  This target links no Rust, so a host that cannot link LeafUI can still
//  draw this toolbar. Two Rust staticlibs in one binary carry two Rust
//  runtimes, which is why the Mac app cannot link LeafUI's: it links its own.
//
//  The state is a value, read whole by every control on every draw. An
//  editor computes it from what it publishes. Both editors are `@Observable`,
//  so a control that reads `toolbarState` in its `body` is redrawn when
//  anything the read touched changes. An `ObservableObject` editor would not
//  be: SwiftUI compares a view's stored reference, finds it unchanged, and
//  never asks the body again.

import SwiftUI

/// An editor the toolbar can drive.
///
/// Conform with a `toolbarState` built from what the editor publishes, and a
/// `perform` that runs each command. An editor that offers the Text menu
/// (size, face, colour and line spacing) supplies its rows as
/// `toolbarTextRows`, because those rows ask the document for the caret's
/// value as they open, and only the editor knows how.
public protocol LeafToolbarEditor: AnyObject {
    associatedtype TextRows: View = EmptyView

    /// What is in force at the caret, and what the format can spell.
    var toolbarState: LeafToolbarState { get }

    /// Run one control's command.
    func perform(_ command: LeafToolbarCommand)

    /// The Text menu's rows. Shown only where
    /// `toolbarState.capabilities.text` is true.
    @ViewBuilder var toolbarTextRows: TextRows { get }
}

public extension LeafToolbarEditor where TextRows == EmptyView {
    var toolbarTextRows: EmptyView { EmptyView() }
}

/// The inline marks with a control of their own.
public enum LeafMark: String, CaseIterable, Hashable, Sendable {
    case bold, italic, underline, strikethrough, code, highlight

    public var title: String {
        switch self {
        case .bold: return loc("menu.bold", "Bold")
        case .italic: return loc("menu.italic", "Italic")
        case .underline: return loc("menu.underline", "Underline")
        case .strikethrough: return loc("menu.strikethrough", "Strikethrough")
        case .code: return loc("menu.code", "Code")
        case .highlight: return loc("menu.highlight", "Highlight")
        }
    }

    public var systemImage: String {
        switch self {
        case .bold: return "bold"
        case .italic: return "italic"
        case .underline: return "underline"
        case .strikethrough: return "strikethrough"
        case .code: return "chevron.left.forwardslash.chevron.right"
        case .highlight: return "highlighter"
        }
    }
}

/// The seven colours a highlight can be, in the order every palette shows them.
public enum LeafHighlightColour: Int, CaseIterable, Hashable, Sendable {
    case red, orange, yellow, green, blue, purple, brown

    /// The swatch and the name as one menu row: `🔴 Red`.
    public var menuTitle: String { "\(swatch) \(title)" }

    public var title: String {
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
}

/// How a block is aligned. Left is no value at all, because absence is left.
public enum LeafAlignment: CaseIterable, Hashable, Sendable {
    case center, right, justify

    public var title: String {
        switch self {
        case .center: return loc("menu.align.center", "Center")
        case .right: return loc("menu.align.right", "Right")
        case .justify: return loc("menu.align.justify", "Justify")
        }
    }

    public var systemImage: String {
        switch self {
        case .center: return "text.aligncenter"
        case .right: return "text.alignright"
        case .justify: return "text.justify"
        }
    }
}

/// How a table column is aligned.
public enum LeafColumnAlignment: Hashable, Sendable {
    case `default`, left, center, right
}

/// One gesture over the table the caret is in, or a new table.
public enum LeafTableCommand: Hashable, Sendable {
    case insert(columns: Int)
    case insertRow(below: Bool)
    case deleteRow
    case insertColumn(right: Bool)
    case deleteColumn
    case align(LeafColumnAlignment)
    case moveRow(down: Bool)
    case moveColumn(right: Bool)
}

/// What a control asks the editor to do.
public enum LeafToolbarCommand: Hashable, Sendable {
    case toggle(LeafMark)
    /// Colour the highlight at the caret, or make one of the selection; nil
    /// takes the colour off.
    case highlightColour(LeafHighlightColour?)
    /// Ask for a destination and link with it. The editor asks: the host's
    /// picker if it has one, its own field otherwise.
    case link

    case paragraph
    case heading(Int)
    case blockquote
    case codeBlock
    case verse

    case bulletList
    case numberedList
    case checklist
    /// Tick or untick the caret's task box.
    case checked
    case indent
    case outdent
    case moveBlockUp
    case moveBlockDown

    /// Nil is left.
    case align(LeafAlignment?)

    case horizontalRule
    case footnote
    case pageBreak
    case table(LeafTableCommand)
    /// One of `toolbarState.directives`, by its id.
    case directive(String)

    case undo
    case redo
}

/// Which controls the document's format can spell. A control the format
/// cannot spell is dimmed rather than removed, so the menus keep their shape;
/// a whole menu the editor does not offer (Align, Text) is left off the bar.
public struct LeafToolbarCapabilities: Hashable, Sendable {
    public var underline: Bool
    public var highlightColour: Bool
    public var blockquote: Bool
    public var codeBlock: Bool
    public var verse: Bool
    public var task: Bool
    public var moveBlock: Bool
    public var table: Bool
    public var footnote: Bool
    public var pageBreak: Bool
    public var directives: Bool
    /// The Align menu is on the bar.
    public var alignment: Bool
    /// The Text menu is on the bar.
    public var text: Bool

    public init(underline: Bool = true, highlightColour: Bool = true, blockquote: Bool = true,
                codeBlock: Bool = true, verse: Bool = true, task: Bool = true, moveBlock: Bool = true,
                table: Bool = true, footnote: Bool = true, pageBreak: Bool = true,
                directives: Bool = true, alignment: Bool = true, text: Bool = true) {
        self.underline = underline
        self.highlightColour = highlightColour
        self.blockquote = blockquote
        self.codeBlock = codeBlock
        self.verse = verse
        self.task = task
        self.moveBlock = moveBlock
        self.table = table
        self.footnote = footnote
        self.pageBreak = pageBreak
        self.directives = directives
        self.alignment = alignment
        self.text = text
    }
}

/// A host's own thing to insert, under Insert's divider.
public struct LeafToolbarDirective: Identifiable, Hashable, Sendable {
    public var id: String
    public var title: String
    public var systemImage: String

    public init(id: String, title: String, systemImage: String) {
        self.id = id
        self.title = title
        self.systemImage = systemImage
    }
}

/// What the toolbar shows: what is in force at the caret, and what the format
/// can spell.
public struct LeafToolbarState: Hashable, Sendable {
    /// The marks in force at the caret.
    public var marks: Set<LeafMark>
    /// The heading level at the caret, 1 to 6, or nil.
    public var heading: Int?
    public var codeBlock: Bool
    public var verse: Bool
    /// In a quote at any depth. A layer over the block's style, not one of them.
    public var blockquote: Bool
    /// The caret's task box: nil for none, false open, true ticked.
    public var task: Bool?
    /// In a link.
    public var link: Bool
    /// The colour of the highlight at the caret, or nil.
    public var highlightColour: LeafHighlightColour?
    /// In a highlight, coloured or not.
    public var inHighlight: Bool
    /// A colour would land: the format spells one, and there is a highlight
    /// to colour or a selection to make one of.
    public var canColourHighlight: Bool
    /// Nil is left.
    public var alignment: LeafAlignment?
    public var inTable: Bool
    public var canUndo: Bool
    public var canRedo: Bool
    public var capabilities: LeafToolbarCapabilities
    public var directives: [LeafToolbarDirective]

    public init(marks: Set<LeafMark> = [], heading: Int? = nil, codeBlock: Bool = false,
                verse: Bool = false, blockquote: Bool = false, task: Bool? = nil, link: Bool = false,
                highlightColour: LeafHighlightColour? = nil, inHighlight: Bool = false,
                canColourHighlight: Bool = true, alignment: LeafAlignment? = nil, inTable: Bool = false,
                canUndo: Bool = false, canRedo: Bool = false,
                capabilities: LeafToolbarCapabilities = LeafToolbarCapabilities(),
                directives: [LeafToolbarDirective] = []) {
        self.marks = marks
        self.heading = heading
        self.codeBlock = codeBlock
        self.verse = verse
        self.blockquote = blockquote
        self.task = task
        self.link = link
        self.highlightColour = highlightColour
        self.inHighlight = inHighlight
        self.canColourHighlight = canColourHighlight
        self.alignment = alignment
        self.inTable = inTable
        self.canUndo = canUndo
        self.canRedo = canRedo
        self.capabilities = capabilities
        self.directives = directives
    }

    /// Neither a heading, a code block nor a verse.
    public var isBody: Bool { heading == nil && !codeBlock && !verse }

    /// The caret's style by name: `Heading 1`, `Code Block`, `Verse`, `Body`,
    /// with an opening quote mark in front inside a quote.
    public var styleName: String {
        let name: String
        if let heading {
            name = String(format: loc("menu.headingN", "Heading %d"), heading)
        } else if codeBlock {
            name = loc("menu.codeBlock", "Code Block")
        } else if verse {
            name = loc("menu.verse", "Verse")
        } else {
            name = loc("toolbar.style.body", "Body")
        }
        return blockquote ? Self.quoteMark + name : name
    }

    /// Every name `styleName` can return, for sizing the Style menu to the
    /// widest so it does not jump as the caret moves.
    public static var styleNames: [String] {
        let names = (1...6).map { String(format: loc("menu.headingN", "Heading %d"), $0) } + [
            loc("menu.codeBlock", "Code Block"), loc("menu.verse", "Verse"), loc("toolbar.style.body", "Body"),
        ]
        return names + names.map { quoteMark + $0 }
    }

    static let quoteMark = "\u{201C}"
}

/// A localized string with an English fallback, looked up in the app's
/// bundle under the keys LeafUI uses, so one `Localizable.strings` translates
/// both.
func loc(_ key: String, _ value: String) -> String {
    NSLocalizedString(key, tableName: nil, bundle: .main, value: value, comment: "")
}
