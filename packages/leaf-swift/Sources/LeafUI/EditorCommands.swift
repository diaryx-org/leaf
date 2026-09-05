//  EditorCommands.swift
//
//  The menu bar's half of the editor. A native Mac app puts Bold in a Format
//  menu, with its ⌘B beside it, and enables the item only when there is a
//  document to make bold; an iPad under a hardware keyboard shows the same menu
//  when ⌘ is held. `LeafEditor` publishes its model as the scene's focused
//  editor, and `LeafEditorCommands` reads it back to build those menus — so a
//  host adds one line, `.commands { LeafEditorCommands() }`, and the standard
//  menus reach whichever document its window is showing.
//
//  The items are the same public commands the toolbar's buttons call, with the
//  same shortcuts every other leaf frontend binds (see leaf-gpui's key map).
//  Checkmarks follow `EditorState`: Bold is ticked while the caret stands in
//  bold text, Source View while the source is showing.

import SwiftUI

/// The editor a scene's menus act on. Published by `LeafEditor`; read by
/// `LeafEditorCommands`, or by a host's own `Commands` through
/// `@FocusedValue(\.leafEditor)`.
public struct LeafEditorFocusKey: FocusedValueKey {
    public typealias Value = LeafEditorModel
}

extension FocusedValues {
    public var leafEditor: LeafEditorModel? {
        get { self[LeafEditorFocusKey.self] }
        set { self[LeafEditorFocusKey.self] = newValue }
    }
}

/// Format and View menu items for the focused `LeafEditor`. Add to a scene with
/// `.commands { LeafEditorCommands() }`.
///
/// Format replaces SwiftUI's standard text-formatting group (Font, Text — panels
/// that mean nothing to a document whose format spells its own emphasis) with
/// leaf's inline marks, block kinds, and list structure. View gains the
/// source/rendered toggle. Every item is disabled without a focused editor, or
/// on a reader.
public struct LeafEditorCommands: Commands {
    @FocusedValue(\.leafEditor) private var editor: LeafEditorModel?

    public init() {}

    public var body: some Commands {
        CommandGroup(replacing: .textFormatting) {
            if let editor {
                FormatMenuItems(editor: editor)
            } else {
                FormatMenuItems.placeholders
            }
        }
        CommandGroup(after: .toolbar) {
            if let editor {
                ViewMenuItems(editor: editor)
            } else {
                Toggle(loc("menu.sourceView", "Source View"), isOn: .constant(false))
                    .keyboardShortcut("e", modifiers: .command)
                    .disabled(true)
            }
        }
    }
}

/// The Format menu's items, observing the editor so the checkmarks and the
/// enabled state follow the caret.
private struct FormatMenuItems: View {
    @ObservedObject var editor: LeafEditorModel

    private var editable: Bool { !editor.isReadOnly }

    var body: some View {
        mark("bold", loc("menu.bold", "Bold"), "b", .command) { editor.toggleBold() }
        mark("italic", loc("menu.italic", "Italic"), "i", .command) { editor.toggleItalic() }
        mark("underline", loc("menu.underline", "Underline"), "u", .command) { editor.toggleUnderline() }
        mark("strike", loc("menu.strikethrough", "Strikethrough"), nil, []) { editor.toggleStrike() }
        mark("code", loc("menu.code", "Code"), "c", [.command, .shift]) { editor.toggleCode() }
        mark("mark", loc("menu.highlight", "Highlight"), "m", [.command, .shift]) { editor.toggleMark() }
        Divider()
        Toggle(loc("menu.paragraph", "Paragraph"), isOn: block(editor.state.heading == nil) { editor.setParagraph() })
            .keyboardShortcut("0", modifiers: .control)
            .disabled(!editable)
        Menu(loc("menu.heading", "Heading")) {
            ForEach(1...6, id: \.self) { level in
                Toggle(String(format: loc("menu.headingN", "Heading %d"), level),
                       isOn: block(editor.state.heading == UInt32(level)) { editor.setHeading(UInt32(level)) })
                    .keyboardShortcut(KeyEquivalent(Character(String(level))), modifiers: .control)
            }
        }
        .disabled(!editable)
        Divider()
        Button(loc("menu.bulletList", "Bullet List")) { editor.toggleList(ordered: false) }
            .keyboardShortcut("8", modifiers: [.command, .shift])
            .disabled(!editable)
        Button(loc("menu.numberedList", "Numbered List")) { editor.toggleList(ordered: true) }
            .keyboardShortcut("7", modifiers: [.command, .shift])
            .disabled(!editable)
        Button(loc("menu.blockQuote", "Block Quote")) { editor.toggleBlockquote() }
            .keyboardShortcut("9", modifiers: [.command, .shift])
            .disabled(!editable)
        Button(loc("menu.indent", "Indent")) { editor.indent() }
            .keyboardShortcut("]", modifiers: .command)
            .disabled(!editable)
        Button(loc("menu.outdent", "Outdent")) { editor.outdent() }
            .keyboardShortcut("[", modifiers: .command)
            .disabled(!editable)
        Divider()
        Button(loc("menu.footnote", "Insert Footnote")) { editor.insertFootnote() }
            .disabled(!editable)
        Button(loc("menu.rule", "Insert Horizontal Rule")) { editor.insertThematicBreak() }
            .disabled(!editable)
    }

    /// An inline mark: ticked while active at the caret.
    @ViewBuilder
    private func mark(_ id: String, _ title: String, _ key: Character?, _ modifiers: EventModifiers,
                      _ toggle: @escaping () -> Void) -> some View {
        let item = Toggle(title, isOn: Binding(get: { editor.isActive(id) }, set: { _ in toggle() }))
            .disabled(!editable)
        if let key {
            item.keyboardShortcut(KeyEquivalent(key), modifiers: modifiers)
        } else {
            item
        }
    }

    /// A block kind: ticked while the caret's block is it; choosing it sets it.
    private func block(_ on: Bool, _ set: @escaping () -> Void) -> Binding<Bool> {
        Binding(get: { on }, set: { _ in set() })
    }

    /// The same items, disabled, for a scene with no editor in focus — so the
    /// menu keeps its shape rather than emptying.
    @ViewBuilder
    static var placeholders: some View {
        Group {
            Toggle(loc("menu.bold", "Bold"), isOn: .constant(false)).keyboardShortcut("b", modifiers: .command)
            Toggle(loc("menu.italic", "Italic"), isOn: .constant(false)).keyboardShortcut("i", modifiers: .command)
            Toggle(loc("menu.underline", "Underline"), isOn: .constant(false)).keyboardShortcut("u", modifiers: .command)
            Toggle(loc("menu.strikethrough", "Strikethrough"), isOn: .constant(false))
            Toggle(loc("menu.code", "Code"), isOn: .constant(false)).keyboardShortcut("c", modifiers: [.command, .shift])
            Toggle(loc("menu.highlight", "Highlight"), isOn: .constant(false)).keyboardShortcut("m", modifiers: [.command, .shift])
            Divider()
            Toggle(loc("menu.paragraph", "Paragraph"), isOn: .constant(false)).keyboardShortcut("0", modifiers: .control)
            Menu(loc("menu.heading", "Heading")) {
                ForEach(1...6, id: \.self) { level in
                    Toggle(String(format: loc("menu.headingN", "Heading %d"), level), isOn: .constant(false))
                        .keyboardShortcut(KeyEquivalent(Character(String(level))), modifiers: .control)
                }
            }
            Divider()
            Button(loc("menu.bulletList", "Bullet List")) {}.keyboardShortcut("8", modifiers: [.command, .shift])
            Button(loc("menu.numberedList", "Numbered List")) {}.keyboardShortcut("7", modifiers: [.command, .shift])
            Button(loc("menu.blockQuote", "Block Quote")) {}.keyboardShortcut("9", modifiers: [.command, .shift])
            Button(loc("menu.indent", "Indent")) {}.keyboardShortcut("]", modifiers: .command)
            Button(loc("menu.outdent", "Outdent")) {}.keyboardShortcut("[", modifiers: .command)
            Divider()
            Button(loc("menu.footnote", "Insert Footnote")) {}
            Button(loc("menu.rule", "Insert Horizontal Rule")) {}
        }
        .disabled(true)
    }
}

/// The View menu's item: the rendered/source toggle, ticked while the source
/// is showing.
private struct ViewMenuItems: View {
    @ObservedObject var editor: LeafEditorModel

    var body: some View {
        Toggle(loc("menu.sourceView", "Source View"),
               isOn: Binding(get: { editor.isSource }, set: { _ in editor.toggleView() }))
            .keyboardShortcut("e", modifiers: .command)
    }
}
