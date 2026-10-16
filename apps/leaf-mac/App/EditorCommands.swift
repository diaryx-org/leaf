//  EditorCommands.swift
//
//  The editor's menus — Edit's Undo, Redo and Find, Format, and View — laid
//  out as LeafUI's `EditorCommands.swift` lays them out for the iOS app's
//  sibling, aimed at whichever document window is in front (`@FocusedObject`).
//
//  Every item that has a key binding is that binding: its shortcut is shown,
//  and choosing it — by mouse, or by pressing the shortcut, which AppKit hands
//  to the menu before the view sees it — sends the same chord the keyboard
//  would, so a command is defined once, in the editor's `input.rs`.

import SwiftUI

struct LeafEditorCommands: Commands {
    @FocusedObject private var editor: EditorModel?
    @ObservedObject private var spelling = SpellingSettings.shared

    var body: some Commands {
        CommandGroup(replacing: .undoRedo) {
            Button("Undo") { editor?.send(.cmd("z", "KeyZ")) }
                .keyboardShortcut("z", modifiers: .command)
                .disabled(!(editor?.state.can_undo ?? false))
            Button("Redo") { editor?.send(.cmd("z", "KeyZ", shift: true)) }
                .keyboardShortcut("z", modifiers: [.command, .shift])
                .disabled(!(editor?.state.can_redo ?? false))
        }
        CommandGroup(after: .pasteboard) {
            Button("Paste and Match Style") { editor?.send(.cmd("v", "KeyV", shift: true, alt: true)) }
                .keyboardShortcut("v", modifiers: [.command, .shift, .option])
                .disabled(editor == nil)
        }
        CommandGroup(replacing: .textEditing) {
            Menu("Find") {
                item("Find…", .cmd("f", "KeyF"), "f", .command)
                item("Find and Replace…", .cmd("ƒ", "KeyF", alt: true), "f", [.command, .option])
                item("Find Next", .cmd("g", "KeyG"), "g", .command)
                item("Find Previous", .cmd("g", "KeyG", shift: true), "g", [.command, .shift])
            }
            // What `.textEditing` holds in every other app, besides Find.
            Menu("Spelling and Grammar") {
                Button("Check Document Now") { editor?.view?.spelling.checkDocumentNow() }
                    .keyboardShortcut(";", modifiers: .command)
                    .disabled(editor == nil || editor?.state.source_view == true)
                Divider()
                Toggle("Check Spelling While Typing", isOn: $spelling.checkWhileTyping)
                Toggle("Correct Spelling Automatically", isOn: $spelling.correctAutomatically)
            }
            Menu("Substitutions") {
                Button("Show Substitutions") { NSSpellChecker.shared.substitutionsPanel.orderFront(nil) }
                Divider()
                Toggle("Smart Quotes", isOn: $spelling.smartQuotes)
                Toggle("Smart Dashes", isOn: $spelling.smartDashes)
                Toggle("Text Replacement", isOn: $spelling.textReplacement)
            }
        }
        CommandGroup(replacing: .textFormatting) {
            marks
            Divider()
            blocks
            Divider()
            structure
            Divider()
            inserts
        }
        CommandGroup(before: .toolbar) {
            Toggle("Source View", isOn: Binding(get: { editor?.state.source_view ?? false },
                                                set: { _ in editor?.send(.cmd("e", "KeyE")) }))
                .keyboardShortcut("e", modifiers: .command)
                .disabled(editor == nil)
            Toggle("Preserve Line Breaks", isOn: Binding(get: { editor?.state.preserve_breaks ?? false },
                                                         set: { editor?.setPreserveBreaks($0) }))
                .disabled(editor == nil)
            Divider()
            Button("Make Text Bigger") { editor?.step(zoom: 1) }
                .keyboardShortcut("+", modifiers: .command)
                .disabled(editor == nil)
            Button("Make Text Smaller") { editor?.step(zoom: -1) }
                .keyboardShortcut("-", modifiers: .command)
                .disabled(editor == nil)
            Button("Actual Size") { editor?.step(zoom: 0) }
                .keyboardShortcut("0", modifiers: .command)
                .disabled(editor == nil)
            Divider()
        }
    }

    @ViewBuilder
    private var marks: some View {
        toggle("Bold", LEAF_MARK_STRONG, .cmd("b", "KeyB"), "b", .command)
        toggle("Italic", LEAF_MARK_EMPH, .cmd("i", "KeyI"), "i", .command)
        toggle("Underline", LEAF_MARK_UNDERLINE, .cmd("u", "KeyU"), "u", .command)
        toggle("Strikethrough", LEAF_MARK_STRIKE, .cmd("x", "KeyX", shift: true), "x", [.command, .shift])
        toggle("Code", LEAF_MARK_CODE, .cmd("c", "KeyC", shift: true), "c", [.command, .shift])
        toggle("Highlight", LEAF_MARK_HIGHLIGHT, .cmd("m", "KeyM", shift: true), "m", [.command, .shift])
        if let editor { HighlightColourMenu(model: editor) }
        Button("Link…") { editor?.promptForLink() }
            .keyboardShortcut("k", modifiers: .command)
            .disabled(editor == nil)
    }

    @ViewBuilder
    private var blocks: some View {
        item("Paragraph", .ctrl("0", "Digit0"), "0", .control)
        Menu("Heading") {
            ForEach(1...6, id: \.self) { n in
                Toggle("Heading \(n)", isOn: Binding(get: { editor?.state.heading == UInt32(n) },
                                                     set: { _ in editor?.send(.ctrl("\(n)", "Digit\(n)")) }))
                    .keyboardShortcut(KeyEquivalent(Character("\(n)")), modifiers: .control)
            }
        }
        .disabled(editor == nil)
    }

    @ViewBuilder
    private var structure: some View {
        item("Bulleted List", .cmd("*", "Digit8", shift: true), "8", [.command, .shift])
        item("Numbered List", .cmd("&", "Digit7", shift: true), "7", [.command, .shift])
        item("Block Quote", .cmd("(", "Digit9", shift: true), "9", [.command, .shift])
        item("Code Block", .cmd("ç", "KeyC", alt: true), "c", [.command, .option])
        Button("Verse") { editor?.action(LEAF_ACTION_TOGGLE_VERSE) }.disabled(editor == nil)
        item("Checklist", .cmd("l", "KeyL", shift: true), "l", [.command, .shift])
        item("Checked", .cmd("u", "KeyU", shift: true), "u", [.command, .shift])
        Divider()
        item("Indent", .cmd("]", "BracketRight"), "]", .command)
        item("Outdent", .cmd("[", "BracketLeft"), "[", .command)
        Divider()
        item("Move Block Up", .alt("ArrowUp", "ArrowUp"), .upArrow, .option)
        item("Move Block Down", .alt("ArrowDown", "ArrowDown"), .downArrow, .option)
    }

    @ViewBuilder
    private var inserts: some View {
        Button("Insert Footnote") { editor?.action(LEAF_ACTION_INSERT_FOOTNOTE) }.disabled(editor == nil)
        Button("Insert Horizontal Rule") { editor?.action(LEAF_ACTION_INSERT_RULE) }.disabled(editor == nil)
        Button("Insert Page Break") { editor?.action(LEAF_ACTION_INSERT_PAGE_BREAK) }.disabled(editor == nil)
        if let editor { TableMenu(model: editor) } else { Menu("Table") {}.disabled(true) }
    }

    /// An item that sends `chord`, shown with its shortcut.
    private func item(_ title: String, _ chord: Chord, _ key: KeyEquivalent,
                      _ mods: EventModifiers) -> some View {
        Button(title) { editor?.send(chord) }
            .keyboardShortcut(key, modifiers: mods)
            .disabled(editor == nil)
    }

    /// A mark's item, ticked while the mark is in force at the caret.
    private func toggle(_ title: String, _ mark: Int, _ chord: Chord, _ key: KeyEquivalent,
                        _ mods: EventModifiers) -> some View {
        Toggle(title, isOn: Binding(get: { editor?.isActive(mark) ?? false },
                                    set: { _ in editor?.send(chord) }))
            .keyboardShortcut(key, modifiers: mods)
            .disabled(editor == nil)
    }
}

/// Format ▸ Highlight Colour: the seven colours, and the plain highlighter.
struct HighlightColourMenu: View {
    @ObservedObject var model: EditorModel

    static let colours = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Brown"]

    var body: some View {
        Menu("Highlight Colour") {
            ForEach(Array(Self.colours.enumerated()), id: \.offset) { i, name in
                Toggle(name, isOn: Binding(get: { model.state.mark_color == Int32(i) },
                                           set: { _ in model.highlight(Int32(i)) }))
            }
            Divider()
            Toggle("No Colour", isOn: Binding(get: { model.state.mark_color < 0 && model.isActive(LEAF_MARK_HIGHLIGHT) },
                                              set: { _ in model.highlight(-1) }))
        }
    }
}

/// Format ▸ Table, as LeafUI's `TableRows`: none of it with a shortcut, and
/// everything but the inserts disabled outside a table.
struct TableMenu: View {
    @ObservedObject var model: EditorModel

    var body: some View {
        Menu("Table") {
            Button("Insert Table") { model.table(LEAF_TABLE_INSERT, 2) }
            Menu("Insert Table With") {
                ForEach(3...5, id: \.self) { n in
                    Button("\(n) Columns") { model.table(LEAF_TABLE_INSERT, Int32(n)) }
                }
            }
            Divider()
            Group {
                Button("Insert Row Above") { model.table(LEAF_TABLE_INSERT_ROW, 0) }
                Button("Insert Row Below") { model.table(LEAF_TABLE_INSERT_ROW, 1) }
                Button("Delete Row") { model.table(LEAF_TABLE_DELETE_ROW) }
                Divider()
                Button("Insert Column Left") { model.table(LEAF_TABLE_INSERT_COLUMN, 0) }
                Button("Insert Column Right") { model.table(LEAF_TABLE_INSERT_COLUMN, 1) }
                Button("Delete Column") { model.table(LEAF_TABLE_DELETE_COLUMN) }
                Divider()
                Menu("Align Column") {
                    Button("Left") { model.table(LEAF_TABLE_ALIGN, 1) }
                    Button("Center") { model.table(LEAF_TABLE_ALIGN, 2) }
                    Button("Right") { model.table(LEAF_TABLE_ALIGN, 3) }
                    Button("Default") { model.table(LEAF_TABLE_ALIGN, 0) }
                }
                Divider()
                Button("Move Row Up") { model.table(LEAF_TABLE_MOVE_ROW, 0) }
                Button("Move Row Down") { model.table(LEAF_TABLE_MOVE_ROW, 1) }
                Button("Move Column Left") { model.table(LEAF_TABLE_MOVE_COLUMN, 0) }
                Button("Move Column Right") { model.table(LEAF_TABLE_MOVE_COLUMN, 1) }
            }
            .disabled(!model.state.in_table)
        }
    }
}
