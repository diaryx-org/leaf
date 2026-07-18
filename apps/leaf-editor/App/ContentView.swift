import SwiftUI
import LeafUI

/// A minimal cross-platform host for the `LeafUI` editor: a formatting toolbar
/// bound to the document's live state, and the `LeafEditor` surface below it.
/// Everything — caret math, wrapping, selection, WYSIWYG resolution — comes from
/// leaf-core over the FFI; this file is only chrome. The same view builds for
/// macOS and iOS because `LeafEditor`/`LeafTextView` carry both surfaces.
struct ContentView: View {
    @StateObject private var editor = makeEditor()

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
                btn("undo", "arrow.uturn.backward", active: false) { editor.undo() }
                btn("redo", "arrow.uturn.forward", active: false) { editor.redo() }
                Divider().frame(height: 22)
                btn("view", editor.isSource ? "doc.richtext" : "chevron.left.slash.chevron.right",
                    active: editor.isSource) { editor.toggleView() }
                if editor.state.dirty {
                    Circle().fill(.secondary).frame(width: 6, height: 6)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
        }
        .background(.bar)
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

> The document is a live, round-trippable AST the whole time you type.

```rust
fn main() {
    println!("rendered by leaf-core");
}
```

Try the toolbar, or the keyboard's arrows and ⌘B / ⌘I.
"""
