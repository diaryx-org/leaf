//  ToolCatalogue.swift
//
//  Every formatting tool leaf-swift offers, defined once. `LeafFormattingToolbar`
//  draws three arrangements of this list: the short row over the iOS keyboard,
//  the key grid that can take the keyboard's place (`FormattingPanel.swift`),
//  and the macOS row of category menus. Each of them decides which tools go
//  where. What a tool does, what it is called, when it lights and when it is
//  dark is decided here, so the three cannot disagree about Bold.
//
//  A tool has a stable string `id` (`"bold"`, `"move-up"`, `"insert"`). Nothing
//  persists the IDs yet. They are stable because a reader who reorders their
//  row is the obvious next step (Obsidian's toolbar is exactly that), and a
//  saved arrangement is a list of IDs, so renaming one would lose someone's
//  row. See `docs/proposals/keyboard-panel.md`.
//
//  A tool can have *variants*: the menu behind a ▾. It may also have a primary
//  action, and if it does, a tap runs the action and a press-and-hold (or a
//  right-click on the desktop) opens the variants. That is Highlight's shape,
//  and now Code's and List's too. A tool with variants and no action (Style,
//  Align, Text, Insert) opens the menu on a tap, because none of its rows is
//  the obvious one. Table is still that shape, for the reason the toolbar's
//  header gives.

import LeafFFI
import SwiftUI

/// What a tool shows on its target: an SF Symbol, or a short label (Style's
/// "H1", "Body").
enum ToolGlyph: Equatable {
    case symbol(String)
    case text(String)
}

/// One tool, resolved against the editor as it is now. A value, rebuilt on
/// every draw, and cheap to rebuild because each field is a read of the
/// published state or a capability flag.
struct ToolItem: Identifiable {
    /// Stable across releases. See the file's header.
    let id: String
    var glyph: ToolGlyph
    /// The accessibility name, the tooltip, and a menu row's title unless
    /// `menuTitle` says otherwise.
    var label: String
    /// A menu row's title where it differs from `label` (`"Link…"`, which
    /// asks a question, where the key it stands for is just `"Link"`).
    var menuTitle: String?
    /// Lit with the accent, and ticked as a menu row.
    var active = false
    /// Dimmed and inert where false. Kept in its place rather than removed, so
    /// the row keeps its shape.
    var enabled = true
    /// Whether a menu row for this tool is a toggle, ticked while `active`,
    /// rather than a plain command. Bold is a toggle; Horizontal Rule is not.
    var toggles = false
    /// What a tap does. Nil for a tool that is only its variants.
    var action: (() -> Void)?
    /// The rows behind the ▾. Nil for a plain button.
    var variants: AnyView?
    /// The Format menu's key equivalent for this tool, shown beside its menu
    /// row where the rendering asks for shortcuts (the macOS bar).
    var shortcut: ToolShortcut?

    var title: String { menuTitle ?? label }
}

/// A key equivalent, held as a value so a catalogue built on either platform
/// can name one; only a menu row on the Mac applies it.
struct ToolShortcut {
    var key: KeyEquivalent
    var modifiers: EventModifiers
}

/// The catalogue, over one editor. Built in a view's `body`, with whatever
/// that view does to ask for a link destination.
struct ToolCatalogue {
    let editor: LeafEditorModel
    /// Starts the link question: the host's `onEditLink`, or the rendering's
    /// own field. The rendering owns the field because a popover has to hang
    /// on a view it draws.
    let beginLink: () -> Void
    /// Whether menu rows carry the Format menu's key equivalents. True on the
    /// macOS bar, where the reader has a keyboard and a hint beside a row is
    /// how they learn it. False on iOS, whose menus are for touch.
    var shortcuts = false

    // MARK: inline marks

    var bold: ToolItem {
        mark("bold", .symbol("bold"), loc("menu.bold", "Bold"), "bold",
             shortcut: ToolShortcut(key: "b", modifiers: .command)) { editor.toggleBold() }
    }

    var italic: ToolItem {
        mark("italic", .symbol("italic"), loc("menu.italic", "Italic"), "italic",
             shortcut: ToolShortcut(key: "i", modifiers: .command)) { editor.toggleItalic() }
    }

    /// Dark in Markdown, which has no underline to write. djot's `{+text+}`
    /// has no Markdown spelling, even under leaf's extensions.
    var underline: ToolItem {
        var item = mark("underline", .symbol("underline"), loc("menu.underline", "Underline"), "underline",
                        shortcut: ToolShortcut(key: "u", modifiers: .command)) { editor.toggleUnderline() }
        item.enabled = editor.capabilities.underline
        return item
    }

    var strikethrough: ToolItem {
        mark("strikethrough", .symbol("strikethrough"), loc("menu.strikethrough", "Strikethrough"),
             "strike", shortcut: nil) { editor.toggleStrike() }
    }

    /// Inline code, with Code Block as its variant. The two are one key on
    /// the panel because they are one idea at two sizes, and the block form is
    /// the rarer one. On the Mac they sit apart: Code in Format, Code Block in
    /// Style.
    var code: ToolItem {
        var item = mark("code", .symbol("chevron.left.forwardslash.chevron.right"),
                        loc("menu.code", "Code"), "code",
                        shortcut: ToolShortcut(key: "c", modifiers: [.command, .shift])) { editor.toggleCode() }
        item.variants = AnyView(ToolMenuRow(item: codeBlock, shortcuts: shortcuts))
        return item
    }

    /// Highlight, and the seven colours a highlight can be. A press marks (or
    /// unmarks) the selection, like the buttons beside it; the colour lives in
    /// the variants, because a colour is a property of a highlight rather than
    /// a mark of its own, and seven more buttons would be a palette pretending
    /// to be formatting.
    var highlight: ToolItem {
        var item = mark("highlight", .symbol("highlighter"), loc("menu.highlight", "Highlight"), "mark",
                        shortcut: ToolShortcut(key: "m", modifiers: [.command, .shift])) { editor.toggleMark() }
        item.variants = AnyView(HighlightColourRows(editor: editor))
        return item
    }

    /// Link is applied over the selection the way bold is, and it lights while
    /// the caret stands in a link. The light reads `state.link`, which rides
    /// the frame precisely so it can: walking the caret out of a link changes
    /// no mark and no heading, so a button that asked core directly would keep
    /// a stale light (see `EditorState.link`).
    var link: ToolItem {
        ToolItem(id: "link", glyph: .symbol("link"), label: loc("toolbar.link", "Link"),
                 menuTitle: loc("toolbar.linkEllipsis", "Link\u{2026}"),
                 active: editor.state.link != nil, action: beginLink)
    }

    // MARK: block styles

    /// The caret's block kind as a name, and the menu that changes it. No
    /// primary action: none of the six is the obvious one.
    ///
    /// The label names what is in force: a heading's level, a code block, or
    /// Body. A quote is not among them, because nothing in the published
    /// state says the caret stands in one; a paragraph inside a quote reads
    /// Body, which it also is.
    var style: ToolItem {
        let rows = Group {
            ForEach(1...3, id: \.self) { level in
                ToolMenuRow(item: heading(level), shortcuts: shortcuts)
            }
            ToolMenuRow(item: paragraph, shortcuts: shortcuts)
            Divider()
            ToolMenuRow(item: quote, shortcuts: shortcuts)
            ToolMenuRow(item: codeBlock, shortcuts: shortcuts)
        }
        return ToolItem(id: "style", glyph: .text(styleName(short: true)),
                        label: loc("toolbar.style", "Style"), variants: AnyView(rows))
    }

    /// The name `style` shows. Short on a key (`H1`, `Body`), long on the
    /// Mac's button (`Heading 1`, `Body`), where there is room to say it.
    func styleName(short: Bool) -> String {
        if let level = editor.state.heading {
            return short ? "H\(level)"
                : String(format: loc("menu.headingN", "Heading %d"), Int(level))
        }
        if editor.state.codeBlock {
            return short ? loc("toolbar.style.code", "Code") : loc("menu.codeBlock", "Code Block")
        }
        return loc("toolbar.style.body", "Body")
    }

    /// Every name `styleName` can return, for a rendering that sizes the
    /// button to the widest so it doesn't jump as the caret moves.
    static func styleNames(short: Bool) -> [String] {
        let headings = (1...6).map {
            short ? "H\($0)" : String(format: loc("menu.headingN", "Heading %d"), $0)
        }
        return headings + [
            short ? loc("toolbar.style.code", "Code") : loc("menu.codeBlock", "Code Block"),
            loc("toolbar.style.body", "Body"),
        ]
    }

    func heading(_ level: Int) -> ToolItem {
        ToolItem(id: "heading-\(level)", glyph: .text("H\(level)"),
                 label: String(format: loc("menu.headingN", "Heading %d"), level),
                 active: editor.state.heading == UInt32(level), toggles: true,
                 action: { editor.setHeading(UInt32(level)) },
                 shortcut: ToolShortcut(key: KeyEquivalent(Character(String(level))), modifiers: .control))
    }

    var paragraph: ToolItem {
        ToolItem(id: "body", glyph: .symbol("paragraphsign"), label: loc("toolbar.style.body", "Body"),
                 active: editor.state.heading == nil && !editor.state.codeBlock, toggles: true,
                 action: { editor.setParagraph() },
                 shortcut: ToolShortcut(key: "0", modifiers: .control))
    }

    /// Not a toggle row: nothing in the published state says the caret is in
    /// a quote, so there is nothing to tick.
    var quote: ToolItem {
        ToolItem(id: "quote", glyph: .symbol("quote.opening"), label: loc("menu.blockQuote", "Block Quote"),
                 action: { editor.toggleBlockquote() },
                 shortcut: ToolShortcut(key: "9", modifiers: [.command, .shift]))
    }

    /// Lit while the caret stands in one, for the reason Bold is lit inside
    /// bold text. The braces rather than the angle brackets inline Code wears,
    /// so the two read as different tools.
    var codeBlock: ToolItem {
        ToolItem(id: "code-block", glyph: .symbol("curlybraces"), label: loc("menu.codeBlock", "Code Block"),
                 active: editor.state.codeBlock, enabled: editor.capabilities.codeBlock, toggles: true,
                 action: { editor.toggleCodeBlock() },
                 shortcut: ToolShortcut(key: "c", modifiers: [.command, .option]))
    }

    // MARK: lists and structure

    /// Bulleted list as the tap, and the three kinds as variants. Not lit:
    /// the published state says whether the caret's item has a box (which is
    /// Checklist's light) but not whether it is in a list at all.
    var list: ToolItem {
        let rows = Group {
            ToolMenuRow(item: bulletList, shortcuts: shortcuts)
            ToolMenuRow(item: numberedList, shortcuts: shortcuts)
            ToolMenuRow(item: checklist, shortcuts: shortcuts)
        }
        return ToolItem(id: "list", glyph: .symbol("list.bullet"), label: loc("toolbar.list", "List"),
                        action: { editor.toggleList(ordered: false) }, variants: AnyView(rows))
    }

    var bulletList: ToolItem {
        ToolItem(id: "bullet-list", glyph: .symbol("list.bullet"), label: loc("menu.bulletList", "Bullet List"),
                 action: { editor.toggleList(ordered: false) },
                 shortcut: ToolShortcut(key: "8", modifiers: [.command, .shift]))
    }

    var numberedList: ToolItem {
        ToolItem(id: "numbered-list", glyph: .symbol("list.number"),
                 label: loc("menu.numberedList", "Numbered List"),
                 action: { editor.toggleList(ordered: true) },
                 shortcut: ToolShortcut(key: "7", modifiers: [.command, .shift]))
    }

    /// Lit while the caret's item has a box, whichever way it faces. Ticking
    /// the box is not a tool: a tap on the box does that, and the Format
    /// menu's Checked item is the keyboard's way.
    var checklist: ToolItem {
        ToolItem(id: "checklist", glyph: .symbol("checklist"), label: loc("menu.checklist", "Checklist"),
                 active: editor.state.task != nil, enabled: editor.capabilities.task, toggles: true,
                 action: { editor.toggleTaskItem() },
                 shortcut: ToolShortcut(key: "l", modifiers: [.command, .shift]))
    }

    var indent: ToolItem {
        ToolItem(id: "indent", glyph: .symbol("increase.indent"), label: loc("menu.indent", "Indent"),
                 action: { editor.indent() }, shortcut: ToolShortcut(key: "]", modifiers: .command))
    }

    var outdent: ToolItem {
        ToolItem(id: "outdent", glyph: .symbol("decrease.indent"), label: loc("menu.outdent", "Outdent"),
                 action: { editor.outdent() }, shortcut: ToolShortcut(key: "[", modifiers: .command))
    }

    /// The caret's block one place up, with its children if it is a list
    /// item: ⌥↑ and Format ▸ Move Block Up. Dark where the format has no
    /// blocks a caret could name.
    var moveUp: ToolItem {
        ToolItem(id: "move-up", glyph: .symbol("arrow.up.to.line"), label: loc("menu.moveBlockUp", "Move Block Up"),
                 enabled: editor.capabilities.moveBlock, action: { editor.moveBlockUp() },
                 shortcut: ToolShortcut(key: .upArrow, modifiers: .option))
    }

    var moveDown: ToolItem {
        ToolItem(id: "move-down", glyph: .symbol("arrow.down.to.line"),
                 label: loc("menu.moveBlockDown", "Move Block Down"),
                 enabled: editor.capabilities.moveBlock, action: { editor.moveBlockDown() },
                 shortcut: ToolShortcut(key: .downArrow, modifiers: .option))
    }

    // MARK: presentation

    /// Left, Centre, Right, Justify, as a menu whose glyph is the alignment in
    /// force. Left is *clearing* the key: the vocabulary has no `left` token,
    /// because absence is left.
    ///
    /// The glyph reads `editor.alignment`, which rides the published frame for
    /// Link's reason: walking out of a centred paragraph changes no mark and no
    /// heading, so a control that asked core for itself would never be told
    /// (see `EditorState.align`).
    var align: ToolItem {
        ToolItem(id: "align", glyph: .symbol(editor.alignment?.symbol ?? "text.alignleft"),
                 label: loc("menu.alignment", "Alignment"), enabled: editor.capabilities.alignment,
                 variants: AnyView(AlignmentRows(editor: editor, shortcuts: shortcuts)))
    }

    /// How the letters are set: size, face, colour, and the line spacing
    /// between them. One menu of four submenus, each the Format menu's own
    /// rows, which ask the document for the caret's value when they open.
    /// Dark only when the format can spell none of the four.
    var text: ToolItem {
        let caps = editor.capabilities
        let editor = self.editor
        let shortcuts = self.shortcuts
        let rows = Group {
            Menu(loc("menu.textSize", "Text Size")) { TextSizeRows(editor: editor, shortcuts: shortcuts) }
                .disabled(!caps.fontSize)
            Menu(loc("menu.font", "Font")) { FontFamilyRows(editor: editor) }
                .disabled(!caps.fontFamily)
            Menu(loc("menu.textColour", "Text Colour")) { TextColourRows(editor: editor) }
                .disabled(!caps.textColor)
            Menu(loc("menu.lineSpacing", "Line Spacing")) { LineSpacingRows(editor: editor) }
                .disabled(!caps.lineSpacing)
        }
        return ToolItem(id: "text", glyph: .symbol("textformat.size"), label: loc("toolbar.text", "Text"),
                        enabled: caps.fontSize || caps.fontFamily || caps.textColor || caps.lineSpacing,
                        variants: AnyView(rows))
    }

    // MARK: insert

    /// The things written *into* the document rather than restyling what is
    /// there: a table, a rule, a footnote, a page break.
    var insert: ToolItem {
        let editor = self.editor
        let rows = Group {
            Menu(loc("menu.table", "Table")) { TableRows(editor: editor) }
                .disabled(!editor.capabilities.table)
            ToolMenuRow(item: rule, shortcuts: shortcuts)
            ToolMenuRow(item: footnote, shortcuts: shortcuts)
            ToolMenuRow(item: pageBreak, shortcuts: shortcuts)
        }
        return ToolItem(id: "insert", glyph: .symbol("plus"), label: loc("toolbar.insert", "Insert"),
                        variants: AnyView(rows))
    }

    var rule: ToolItem {
        ToolItem(id: "rule", glyph: .symbol("rectangle.compress.vertical"),
                 label: loc("toolbar.rule", "Horizontal Rule"), action: { editor.insertThematicBreak() })
    }

    /// A reference at the caret and its note at the end, as one edit. The
    /// raised character because that is what the gesture puts on screen.
    var footnote: ToolItem {
        ToolItem(id: "footnote", glyph: .symbol("textformat.superscript"),
                 label: loc("toolbar.footnote", "Footnote"), action: { editor.insertFootnote() })
    }

    var pageBreak: ToolItem {
        ToolItem(id: "page-break", glyph: .symbol("arrow.down.to.line"),
                 label: loc("toolbar.pageBreak", "Page Break"),
                 enabled: editor.capabilities.pageBreak, action: { editor.insertPageBreak() })
    }

    // MARK: history

    /// Enabled by the history's depth, the way the Edit menu's items are: a
    /// dimmed button says there is nothing to take back, where a live one
    /// that does nothing says the document is broken.
    var undo: ToolItem {
        ToolItem(id: "undo", glyph: .symbol("arrow.uturn.backward"), label: loc("toolbar.undo", "Undo"),
                 enabled: editor.state.canUndo, action: { editor.undo() })
    }

    var redo: ToolItem {
        ToolItem(id: "redo", glyph: .symbol("arrow.uturn.forward"), label: loc("toolbar.redo", "Redo"),
                 enabled: editor.state.canRedo, action: { editor.redo() })
    }

    // MARK: the Mac's categories

    /// The inline marks and Link, as one menu. Highlight's colours are a
    /// submenu under it, as they are in the Format menu.
    var format: ToolItem {
        let editor = self.editor
        let rows = Group {
            ToolMenuRow(item: bold, shortcuts: shortcuts)
            ToolMenuRow(item: italic, shortcuts: shortcuts)
            ToolMenuRow(item: underline, shortcuts: shortcuts)
            ToolMenuRow(item: strikethrough, shortcuts: shortcuts)
            ToolMenuRow(item: code, shortcuts: shortcuts)
            ToolMenuRow(item: highlight, shortcuts: shortcuts)
            Menu(loc("menu.highlightColour", "Highlight Colour")) { HighlightColourRows(editor: editor) }
            Divider()
            ToolMenuRow(item: link, shortcuts: shortcuts)
        }
        return ToolItem(id: "format", glyph: .symbol("bold.italic.underline"),
                        label: loc("toolbar.format", "Format"), variants: AnyView(rows))
    }

    /// The list kinds and the gestures that restructure rather than restyle:
    /// indent, outdent, and moving the block.
    var lists: ToolItem {
        let rows = Group {
            ToolMenuRow(item: bulletList, shortcuts: shortcuts)
            ToolMenuRow(item: numberedList, shortcuts: shortcuts)
            ToolMenuRow(item: checklist, shortcuts: shortcuts)
            Divider()
            ToolMenuRow(item: indent, shortcuts: shortcuts)
            ToolMenuRow(item: outdent, shortcuts: shortcuts)
            Divider()
            ToolMenuRow(item: moveUp, shortcuts: shortcuts)
            ToolMenuRow(item: moveDown, shortcuts: shortcuts)
        }
        return ToolItem(id: "lists", glyph: .symbol("list.bullet"), label: loc("toolbar.lists", "Lists"),
                        variants: AnyView(rows))
    }

    // MARK: shared construction

    private func mark(_ id: String, _ glyph: ToolGlyph, _ label: String, _ mark: String,
                      shortcut: ToolShortcut?, _ action: @escaping () -> Void) -> ToolItem {
        ToolItem(id: id, glyph: glyph, label: label, active: editor.isActive(mark), toggles: true,
                 action: action, shortcut: shortcut)
    }
}

/// A tool as a menu row: a submenu for a tool that is only its variants, a
/// toggle ticked while `active` for a tool that has a state, and a plain
/// command otherwise.
struct ToolMenuRow: View {
    let item: ToolItem
    var shortcuts = false

    var body: some View {
        if let variants = item.variants, item.action == nil {
            Menu(item.title) { variants }
                .disabled(!item.enabled)
        } else if item.toggles {
            Toggle(item.title, isOn: Binding(get: { item.active }, set: { _ in item.action?() }))
                .toolShortcut(shortcuts ? item.shortcut : nil)
                .disabled(!item.enabled)
        } else {
            Button(item.title) { item.action?() }
                .toolShortcut(shortcuts ? item.shortcut : nil)
                .disabled(!item.enabled)
        }
    }
}

extension View {
    /// The shortcut, when there is one.
    @ViewBuilder
    func toolShortcut(_ shortcut: ToolShortcut?) -> some View {
        if let shortcut {
            keyboardShortcut(shortcut.key, modifiers: shortcut.modifiers)
        } else {
            self
        }
    }
}
