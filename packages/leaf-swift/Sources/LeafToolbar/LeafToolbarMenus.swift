//  LeafToolbarMenus.swift
//
//  The toolbar's controls, each its own view, so a host can put any one of
//  them somewhere else: the Style menu in a sidebar, the table rows in its
//  menu bar's Format menu. `LeafToolbarControls` is all of them in the
//  toolbar's order.
//
//  The order is Style, Bold, Italic, Underline, then the Format, Lists,
//  Align, Text and Insert menus. Style names what is in force ("Heading 1")
//  so it can be read without opening it. Bold, Italic and Underline are
//  buttons because they are the marks pressed most. Every other control is a
//  menu, so the bar fits a window of ordinary width.
//
//  The rows carry no shortcuts. A chord on a row of a menu that lives in a
//  view is registered by that view, and fires wherever the window's focus
//  is: ⌘B in a sidebar's search field would bold the document. The menu
//  bar's Format menu has the chords.

import SwiftUI

/// Every control, in the toolbar's order. Put it in a `ToolbarItemGroup`, or
/// use `LeafToolbarItems`, which does.
public struct LeafToolbarControls<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    public var body: some View {
        let caps = editor.toolbarState.capabilities
        LeafStyleMenu(editor: editor)
        LeafMarkButton(editor: editor, mark: .bold)
        LeafMarkButton(editor: editor, mark: .italic)
        LeafMarkButton(editor: editor, mark: .underline)
        LeafFormatMenu(editor: editor)
        LeafListsMenu(editor: editor)
        if caps.alignment { LeafAlignMenu(editor: editor) }
        if caps.text { LeafTextMenu(editor: editor) }
        LeafInsertMenu(editor: editor)
    }
}

/// The caret's style by name, and the menu that changes it. Body and the six
/// headings exclude one another; Block Quote, Code Block and Verse each tick
/// on their own, under a divider.
public struct LeafStyleMenu<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    public var body: some View {
        let state = editor.toolbarState
        let caps = state.capabilities
        Menu {
            LeafToggleRow(loc("toolbar.style.body", "Body"), on: state.isBody) { editor.perform(.paragraph) }
            ForEach(1...6, id: \.self) { level in
                LeafToggleRow(String(format: loc("menu.headingN", "Heading %d"), level),
                              on: state.heading == level) { editor.perform(.heading(level)) }
            }
            Divider()
            LeafToggleRow(loc("menu.blockQuote", "Block Quote"), on: state.blockquote) {
                editor.perform(.blockquote)
            }
            .disabled(!caps.blockquote)
            LeafToggleRow(loc("menu.codeBlock", "Code Block"), on: state.codeBlock) {
                editor.perform(.codeBlock)
            }
            .disabled(!caps.codeBlock)
            LeafToggleRow(loc("menu.verse", "Verse"), on: state.verse) { editor.perform(.verse) }
                .disabled(!caps.verse)
        } label: {
            Text(state.styleName)
        }
        .help(loc("toolbar.style", "Style"))
        .accessibilityLabel(loc("toolbar.style", "Style"))
        .accessibilityValue(state.styleName)
    }
}

/// A mark as a button, lit with the accent while it is in force at the caret.
public struct LeafMarkButton<Editor: LeafToolbarEditor>: View {
    let editor: Editor
    let mark: LeafMark

    public init(editor: Editor, mark: LeafMark) {
        self.editor = editor
        self.mark = mark
    }

    public var body: some View {
        let state = editor.toolbarState
        let on = state.marks.contains(mark)
        Button { editor.perform(.toggle(mark)) } label: {
            Label(mark.title, systemImage: mark.systemImage)
        }
        .help(mark.title)
        .foregroundStyle(on ? Color.accentColor : Color.primary)
        .accessibilityAddTraits(on ? .isSelected : [])
        .disabled(mark == .underline && !state.capabilities.underline)
    }
}

/// The marks without a button of their own, the highlight's colours, and Link.
public struct LeafFormatMenu<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    public var body: some View {
        let state = editor.toolbarState
        Menu {
            ForEach([LeafMark.strikethrough, .code, .highlight], id: \.self) { mark in
                LeafToggleRow(mark.title, on: state.marks.contains(mark)) { editor.perform(.toggle(mark)) }
            }
            Menu(loc("menu.highlightColour", "Highlight Colour")) {
                LeafHighlightColourRows(editor: editor)
            }
            Divider()
            LeafToggleRow(loc("toolbar.linkEllipsis", "Link\u{2026}"), on: state.link) {
                editor.perform(.link)
            }
        } label: {
            Label(loc("toolbar.format", "Format"), systemImage: "bold.italic.underline")
        }
        .help(loc("toolbar.format", "Format"))
    }
}

/// The list kinds, and the gestures that restructure rather than restyle.
public struct LeafListsMenu<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    public var body: some View {
        let state = editor.toolbarState
        let caps = state.capabilities
        Menu {
            Button(loc("menu.bulletList", "Bullet List")) { editor.perform(.bulletList) }
            Button(loc("menu.numberedList", "Numbered List")) { editor.perform(.numberedList) }
            LeafToggleRow(loc("menu.checklist", "Checklist"), on: state.task != nil) {
                editor.perform(.checklist)
            }
            .disabled(!caps.task)
            LeafToggleRow(loc("menu.checked", "Checked"), on: state.task == true) { editor.perform(.checked) }
                .disabled(!caps.task || state.task == nil)
            Divider()
            Button(loc("menu.indent", "Indent")) { editor.perform(.indent) }
            Button(loc("menu.outdent", "Outdent")) { editor.perform(.outdent) }
            Divider()
            Group {
                Button(loc("menu.moveBlockUp", "Move Block Up")) { editor.perform(.moveBlockUp) }
                Button(loc("menu.moveBlockDown", "Move Block Down")) { editor.perform(.moveBlockDown) }
            }
            .disabled(!caps.moveBlock)
        } label: {
            Label(loc("toolbar.lists", "Lists"), systemImage: "list.bullet")
        }
        .help(loc("toolbar.lists", "Lists"))
    }
}

/// Left, Center, Right and Justify, wearing the alignment in force.
public struct LeafAlignMenu<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    public var body: some View {
        let state = editor.toolbarState
        Menu {
            LeafAlignmentRows(editor: editor)
        } label: {
            Label(loc("menu.alignment", "Alignment"),
                  systemImage: state.alignment?.systemImage ?? "text.alignleft")
        }
        .help(loc("menu.alignment", "Alignment"))
    }
}

/// The alignment rows, ticked at the caret's alignment.
public struct LeafAlignmentRows<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    public var body: some View {
        let state = editor.toolbarState
        LeafToggleRow(loc("menu.align.left", "Left"), on: state.alignment == nil) { editor.perform(.align(nil)) }
        ForEach(LeafAlignment.allCases, id: \.self) { align in
            LeafToggleRow(align.title, on: state.alignment == align) { editor.perform(.align(align)) }
        }
    }
}

/// Size, face, colour and line spacing, as the editor's own rows.
public struct LeafTextMenu<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    public var body: some View {
        Menu {
            editor.toolbarTextRows
        } label: {
            Label(loc("toolbar.text", "Text"), systemImage: "textformat.size")
        }
        .help(loc("toolbar.text", "Text"))
    }
}

/// What is written into the document rather than restyling what is there: a
/// table, a rule, a footnote, a page break, and the host's own directives
/// under a divider.
public struct LeafInsertMenu<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    public var body: some View {
        let state = editor.toolbarState
        let caps = state.capabilities
        Menu {
            Menu(loc("menu.table", "Table")) { LeafTableRows(editor: editor) }
            Button(loc("toolbar.rule", "Horizontal Rule")) { editor.perform(.horizontalRule) }
            Button(loc("toolbar.footnote", "Footnote")) { editor.perform(.footnote) }
                .disabled(!caps.footnote)
            Button(loc("toolbar.pageBreak", "Page Break")) { editor.perform(.pageBreak) }
                .disabled(!caps.pageBreak)
            if !state.directives.isEmpty {
                Divider()
                ForEach(state.directives) { item in
                    Button { editor.perform(.directive(item.id)) } label: {
                        Label(item.title, systemImage: item.systemImage)
                    }
                    .disabled(!caps.directives)
                }
            }
        } label: {
            Label(loc("toolbar.insert", "Insert"), systemImage: "plus")
        }
        .help(loc("toolbar.insert", "Insert"))
    }
}

/// The seven colours, each ticked while it is the caret's, and No Colour.
/// Dimmed where a colour would not land.
public struct LeafHighlightColourRows<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    public var body: some View {
        let state = editor.toolbarState
        Group {
            ForEach(LeafHighlightColour.allCases, id: \.self) { colour in
                LeafToggleRow(colour.menuTitle, on: state.highlightColour == colour) {
                    editor.perform(.highlightColour(colour))
                }
            }
            Divider()
            // Ticked only inside an uncoloured highlight: the colour is nil
            // outside a highlight too.
            LeafToggleRow(loc("menu.highlight.noColour", "No Colour"),
                          on: state.inHighlight && state.highlightColour == nil) {
                editor.perform(.highlightColour(nil))
            }
        }
        .disabled(!state.capabilities.highlightColour || !state.canColourHighlight)
    }
}

/// A new table, then the gestures over the table the caret is in, dimmed
/// outside one.
public struct LeafTableRows<Editor: LeafToolbarEditor>: View {
    let editor: Editor

    public init(editor: Editor) {
        self.editor = editor
    }

    /// Columns of a plain Insert Table; the submenu offers wider.
    static var defaultColumns: Int { 2 }

    public var body: some View {
        let state = editor.toolbarState
        Group {
            Button(loc("menu.insertTable", "Insert Table")) { table(.insert(columns: Self.defaultColumns)) }
            Menu(loc("menu.insertTableWith", "Insert Table With")) {
                ForEach(3...5, id: \.self) { n in
                    Button(String(format: loc("menu.insertTableColumns", "%d Columns"), n)) {
                        table(.insert(columns: n))
                    }
                }
            }
            Divider()
            Group {
                Button(loc("menu.table.insertRowAbove", "Insert Row Above")) { table(.insertRow(below: false)) }
                Button(loc("menu.table.insertRowBelow", "Insert Row Below")) { table(.insertRow(below: true)) }
                Button(loc("menu.table.deleteRow", "Delete Row")) { table(.deleteRow) }
                Divider()
                Button(loc("menu.table.insertColumnLeft", "Insert Column Left")) { table(.insertColumn(right: false)) }
                Button(loc("menu.table.insertColumnRight", "Insert Column Right")) { table(.insertColumn(right: true)) }
                Button(loc("menu.table.deleteColumn", "Delete Column")) { table(.deleteColumn) }
                Divider()
                Menu(loc("menu.table.alignColumn", "Align Column")) {
                    Button(loc("menu.table.align.left", "Left")) { table(.align(.left)) }
                    Button(loc("menu.table.align.center", "Center")) { table(.align(.center)) }
                    Button(loc("menu.table.align.right", "Right")) { table(.align(.right)) }
                    Button(loc("menu.table.align.default", "Default")) { table(.align(.default)) }
                }
                Divider()
                Button(loc("menu.table.moveRowUp", "Move Row Up")) { table(.moveRow(down: false)) }
                Button(loc("menu.table.moveRowDown", "Move Row Down")) { table(.moveRow(down: true)) }
                Button(loc("menu.table.moveColumnLeft", "Move Column Left")) { table(.moveColumn(right: false)) }
                Button(loc("menu.table.moveColumnRight", "Move Column Right")) { table(.moveColumn(right: true)) }
            }
            .disabled(!state.inTable)
        }
        .disabled(!state.capabilities.table)
    }

    private func table(_ command: LeafTableCommand) {
        editor.perform(.table(command))
    }
}

/// A menu row ticked while `on`, which runs `action` when chosen.
struct LeafToggleRow: View {
    let title: String
    let on: Bool
    let action: () -> Void

    init(_ title: String, on: Bool, action: @escaping () -> Void) {
        self.title = title
        self.on = on
        self.action = action
    }

    var body: some View {
        Toggle(title, isOn: Binding(get: { on }, set: { _ in action() }))
    }
}
