//  ContentView.swift
//
//  One document's window: the editor, and the native toolbar above it —
//  LeafToolbar's formatting controls, the ones the iOS app and Diaryx show,
//  with the display chrome (the source toggle, line breaks, and the column
//  and size choices) after them. Each tool ticks or lights from the editor's
//  state, which it republishes after every repaint.

import LeafToolbar
import SwiftUI

struct ContentView: View {
    let model: EditorModel
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
            .focusedSceneValue(\.editorModel, model)
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
        LeafToolbarItems(editor: model)
        ToolbarItemGroup(placement: .primaryAction) {
            Button { model.send(.cmd("e", "KeyE")) } label: {
                Label(model.state.source_view ? "Show Rich Text" : "Show Source",
                      systemImage: model.state.source_view ? "doc.richtext" : "chevron.left.slash.chevron.right")
            }
            .help(model.state.source_view ? "Show rich text" : "Show source")
            .foregroundStyle(model.state.source_view ? Color.accentColor : Color.primary)
            displayMenu
        }
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
}
