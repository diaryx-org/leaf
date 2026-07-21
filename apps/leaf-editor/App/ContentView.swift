import SwiftUI
import LeafUI

/// A minimal cross-platform host for the `LeafUI` editor: a formatting toolbar
/// bound to the document's live state, and the `LeafEditor` surface below it.
/// Everything — caret math, wrapping, selection, WYSIWYG resolution — comes from
/// leaf-core over the FFI; this file is only chrome. The same view builds for
/// macOS and iOS because `LeafEditor`/`LeafTextView` carry both surfaces.
struct ContentView: View {
    @StateObject private var editor = makeEditor()
    /// The soft-break flow shown in the dropdown. Held here (not read back off the
    /// model each paint) because flipping it doesn't change the toolbar's other
    /// state, so this is what drives the menu's checkmark.
    @State private var flowPreserved = false

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            Divider()
            LeafEditor(model: editor)
                .background(editorBackground)
        }
        .ignoresSafeArea(.keyboard, edges: .bottom)
    }

    private var toolbar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 14) {
                btn("bold", "bold", active: editor.isActive("bold")) { editor.toggleBold() }
                btn("italic", "italic", active: editor.isActive("italic")) { editor.toggleItalic() }
                btn("code", "chevron.left.forwardslash.chevron.right", active: editor.isActive("code")) { editor.toggleCode() }
                Divider().frame(height: 22)
                btn("h1", "1.square", active: editor.state.heading == 1) { editor.setHeading(1) }
                btn("h2", "2.square", active: editor.state.heading == 2) { editor.setHeading(2) }
                btn("list", "list.bullet", active: false) { editor.toggleList(ordered: false) }
                btn("quote", "text.quote", active: false) { editor.toggleBlockquote() }
                Divider().frame(height: 22)
                tableMenu
                Divider().frame(height: 22)
                btn("undo", "arrow.uturn.backward", active: false) { editor.undo() }
                btn("redo", "arrow.uturn.forward", active: false) { editor.redo() }
                Divider().frame(height: 22)
                btn("view", editor.isSource ? "doc.richtext" : "chevron.left.slash.chevron.right",
                    active: editor.isSource) { editor.toggleView() }
                Divider().frame(height: 22)
                flowMenu
                if editor.state.dirty {
                    Circle().fill(.secondary).frame(width: 6, height: 6)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
        }
        .background(.bar)
    }

    /// The table controls — rows, columns, alignment, and moves. Enabled only
    /// when the caret is in a table (the ops are no-ops otherwise, but a disabled
    /// control says so up front). `editor.state` drives the re-render on caret
    /// moves, so `caretInTable` is re-read as the caret enters or leaves a table.
    private var tableMenu: some View {
        Menu {
            Button("Insert Row Above") { editor.tableInsertRow(below: false) }
            Button("Insert Row Below") { editor.tableInsertRow(below: true) }
            Button("Delete Row") { editor.tableDeleteRow() }
            Divider()
            Button("Insert Column Left") { editor.tableInsertColumn(right: false) }
            Button("Insert Column Right") { editor.tableInsertColumn(right: true) }
            Button("Delete Column") { editor.tableDeleteColumn() }
            Divider()
            Menu("Align Column") {
                Button("Left") { editor.tableSetAlignment(.left) }
                Button("Center") { editor.tableSetAlignment(.center) }
                Button("Right") { editor.tableSetAlignment(.right) }
                Button("Default") { editor.tableSetAlignment(.default) }
            }
            Divider()
            Button("Move Row Up") { editor.tableMoveRow(down: false) }
            Button("Move Row Down") { editor.tableMoveRow(down: true) }
            Button("Move Column Left") { editor.tableMoveColumn(right: false) }
            Button("Move Column Right") { editor.tableMoveColumn(right: true) }
        } label: {
            Image(systemName: "tablecells")
                .font(.system(size: 17))
                .frame(minWidth: 24, minHeight: 24)
                .foregroundStyle(editor.caretInTable ? Color.accentColor : Color.primary)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .disabled(!editor.caretInTable)
        .accessibilityLabel("table")
    }

    /// The soft-break flow dropdown (a "View"-style menu): Fold reflows soft
    /// breaks into the paragraph, Preserve renders each where it was written. The
    /// change takes effect immediately — the editor relays out under the new flow.
    private var flowMenu: some View {
        Menu {
            Button { setFlow(false) } label: {
                Label("Reflow soft breaks", systemImage: flowPreserved ? "" : "checkmark")
            }
            Button { setFlow(true) } label: {
                Label("Preserve line breaks", systemImage: flowPreserved ? "checkmark" : "")
            }
        } label: {
            Image(systemName: "arrow.turn.down.left")
                .font(.system(size: 17))
                .frame(minWidth: 24, minHeight: 24)
                .foregroundStyle(flowPreserved ? Color.accentColor : Color.primary)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .accessibilityLabel("line flow")
    }

    private func setFlow(_ preserve: Bool) {
        flowPreserved = preserve
        editor.setLineFlow(preserve ? .preserve : .fold)
    }

    private func btn(_ id: String, _ symbol: String, active: Bool, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 17))
                .frame(minWidth: 24, minHeight: 24)
                .foregroundStyle(active ? Color.accentColor : Color.primary)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(id)
    }
}

/// The window/content background, resolved to each toolkit's dynamic system
/// colour so light/dark just works on both platforms.
private var editorBackground: Color {
    #if canImport(UIKit)
    Color(.systemBackground)
    #else
    Color(nsColor: .textBackgroundColor)
    #endif
}

private func makeEditor() -> LeafEditorModel {
    // The sample is valid Markdown, so parsing cannot fail here.
    try! LeafEditorModel(source: sampleMarkdown, format: "markdown")
}

private let sampleMarkdown = """
# leaf, natively

A native **SwiftUI** front end driving *leaf-core* over the FFI — the same \
caret model and AST→glyph map the terminal and desktop apps use, on macOS and iOS.

## What's live

- WYSIWYG rendering with `inline code`
- **Bold**, *italic*, and ==highlight==
- Click (or tap) to place the caret, drag to select

| Feature | Status |
| --- | :---: |
| Tables | editable |
| Lists | nesting |

> The document is a live, round-trippable AST the whole time you type.

This paragraph is written in semantic line breaks:
one clause per source line,
a soft break after each.
Toggle the ⏎ menu to fold them into flowing prose or preserve them as written.

```rust
fn main() {
    println!("rendered by leaf-core");
}
```

Try the toolbar, or the keyboard's arrows and ⌘B / ⌘I.
"""
