//  ContentView.swift
//
//  One document's window: the editor, and the native toolbar above it — the
//  Style, Format, Lists and Insert menus the iOS app's formatting bar has,
//  with the display chrome (the source toggle, line breaks, and the column
//  and size choices) after them. Each tool ticks or lights from the editor's
//  state, which it republishes after every repaint.

import SwiftUI

struct ContentView: View {
    @ObservedObject var model: EditorModel
    let fileURL: URL?

    /// The scene's undo manager is the document's: telling it of a change is
    /// how a `ReferenceFileDocument` says it is edited — the dot in the close
    /// button, Save enabled, the autosave clock.
    @Environment(\.undoManager) private var undoManager

    @AppStorage(DisplayChoice.columnWidthKey) private var columnWidth: ColumnWidth = .medium
    @AppStorage(DisplayChoice.textSizeKey) private var textSize: TextSize = .medium
    @AppStorage(DisplayChoice.flowKey) private var flowSetting = false
    private var typography = StoredTypography()

    private var look: Look {
        DisplayChoice.look(columnWidth: columnWidth, textSize: textSize, type: typography.value)
    }

    var body: some View {
        LeafEditorView(model: model)
            .background(Color(nsColor: .textBackgroundColor))
            .focusedSceneObject(model)
            .toolbar { toolbar }
            .onAppear {
                model.fileURL = fileURL
                model.apply(look)
                if flowSetting { model.setPreserveBreaks(true) }
                wire()
            }
            .onChange(of: fileURL) { _, url in model.fileURL = url }
            .onChange(of: undoManager) { _, _ in wire() }
            .onChange(of: look) { _, look in model.apply(look) }
    }

    private func wire() {
        model.onEdit = { [weak model, weak undoManager] in
            // A no-op registration: the document system reads it as "changed"
            // and does the rest. The edit itself is the editor's to undo.
            guard let model else { return }
            undoManager?.registerUndo(withTarget: model) { _ in }
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .principal) {
            styleMenu
            Button { model.send(.cmd("b", "KeyB")) } label: { Label("Bold", systemImage: "bold") }
                .help("Bold")
                .toolbarActive(model.isActive(LEAF_MARK_STRONG))
            Button { model.send(.cmd("i", "KeyI")) } label: { Label("Italic", systemImage: "italic") }
                .help("Italic")
                .toolbarActive(model.isActive(LEAF_MARK_EMPH))
            Button { model.send(.cmd("u", "KeyU")) } label: { Label("Underline", systemImage: "underline") }
                .help("Underline")
                .toolbarActive(model.isActive(LEAF_MARK_UNDERLINE))
            formatMenu
            listsMenu
            insertMenu
        }
        ToolbarItemGroup(placement: .primaryAction) {
            Button { model.send(.cmd("e", "KeyE")) } label: {
                Label(model.state.source_view ? "Show Rich Text" : "Show Source",
                      systemImage: model.state.source_view ? "doc.richtext" : "chevron.left.slash.chevron.right")
            }
            .help(model.state.source_view ? "Show rich text" : "Show source")
            .toolbarActive(model.state.source_view)
            displayMenu
        }
    }

    private var styleMenu: some View {
        Menu {
            Toggle("Paragraph", isOn: binding(model.state.heading == 0 && !model.state.code_block) {
                model.send(.ctrl("0", "Digit0"))
            })
            ForEach(1...6, id: \.self) { n in
                Toggle("Heading \(n)", isOn: binding(model.state.heading == UInt32(n)) {
                    model.send(.ctrl("\(n)", "Digit\(n)"))
                })
            }
            Divider()
            Toggle("Block Quote", isOn: binding(model.state.blockquote) { model.send(.cmd("(", "Digit9", shift: true)) })
            Toggle("Code Block", isOn: binding(model.state.code_block) { model.send(.cmd("ç", "KeyC", alt: true)) })
            Toggle("Verse", isOn: binding(model.state.verse) { model.action(LEAF_ACTION_TOGGLE_VERSE) })
        } label: {
            Label(styleName, systemImage: "textformat")
        }
        .help("Paragraph style")
    }

    private var styleName: String {
        if model.state.code_block { return "Code" }
        if model.state.heading > 0 { return "Heading \(model.state.heading)" }
        return "Paragraph"
    }

    private var formatMenu: some View {
        Menu {
            Toggle("Strikethrough", isOn: binding(model.isActive(LEAF_MARK_STRIKE)) {
                model.send(.cmd("x", "KeyX", shift: true))
            })
            Toggle("Code", isOn: binding(model.isActive(LEAF_MARK_CODE)) {
                model.send(.cmd("c", "KeyC", shift: true))
            })
            Toggle("Highlight", isOn: binding(model.isActive(LEAF_MARK_HIGHLIGHT)) {
                model.send(.cmd("m", "KeyM", shift: true))
            })
            HighlightColourMenu(model: model)
            Divider()
            Button("Link…") { model.promptForLink() }
        } label: {
            Label("Format", systemImage: "character.textbox")
        }
        .help("Format")
    }

    private var listsMenu: some View {
        Menu {
            Button("Bulleted List") { model.send(.cmd("*", "Digit8", shift: true)) }
            Button("Numbered List") { model.send(.cmd("&", "Digit7", shift: true)) }
            Toggle("Checklist", isOn: binding(model.state.task >= 0) { model.send(.cmd("l", "KeyL", shift: true)) })
            Toggle("Checked", isOn: binding(model.state.task == 1) { model.send(.cmd("u", "KeyU", shift: true)) })
                .disabled(model.state.task < 0)
            Divider()
            Button("Indent") { model.send(.cmd("]", "BracketRight")) }
            Button("Outdent") { model.send(.cmd("[", "BracketLeft")) }
            Divider()
            Button("Move Block Up") { model.send(.alt("ArrowUp", "ArrowUp")) }
            Button("Move Block Down") { model.send(.alt("ArrowDown", "ArrowDown")) }
        } label: {
            Label("Lists", systemImage: "list.bullet")
        }
        .help("Lists")
    }

    private var insertMenu: some View {
        Menu {
            TableMenu(model: model)
            Button("Horizontal Rule") { model.action(LEAF_ACTION_INSERT_RULE) }
            Button("Footnote") { model.action(LEAF_ACTION_INSERT_FOOTNOTE) }
            Button("Page Break") { model.action(LEAF_ACTION_INSERT_PAGE_BREAK) }
        } label: {
            Label("Insert", systemImage: "plus")
        }
        .help("Insert")
    }

    private var displayMenu: some View {
        Menu {
            Picker("Line Breaks", selection: Binding(get: { model.state.preserve_breaks },
                                                     set: { model.setPreserveBreaks($0) })) {
                Text("Reflow soft breaks").tag(false)
                Text("Preserve line breaks").tag(true)
            }
            .pickerStyle(.inline)
            Picker("Column Width", selection: $columnWidth) {
                ForEach(ColumnWidth.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.inline)
            Picker("Text Size", selection: $textSize) {
                ForEach(TextSize.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.inline)
        } label: {
            Label("Appearance", systemImage: "textformat.size")
        }
        .help("Line breaks, column width and text size")
    }

    /// A toggle row that ticks from the editor's state and sends a command.
    private func binding(_ on: Bool, _ run: @escaping () -> Void) -> Binding<Bool> {
        Binding(get: { on }, set: { _ in run() })
    }
}

private extension View {
    /// A toolbar button lit while its choice is in force.
    func toolbarActive(_ on: Bool) -> some View {
        foregroundStyle(on ? Color.accentColor : Color.primary)
    }
}
