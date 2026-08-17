import SwiftUI
import LeafUI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

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
    /// The reader's display choices. These are the host's to own — `LeafUI` takes
    /// a whole `EditorTheme` and doesn't remember one — so a real app would
    /// persist them (`@AppStorage`) rather than reset them each launch.
    @State private var columnWidth: ColumnWidth = .medium
    @State private var textSize: TextSize = .medium
    #if os(macOS)
    /// The paginated view: nil is the continuous flow, a `PageSetup` puts the
    /// document on paper. macOS only for now — a stack of fixed-width sheets is a
    /// desktop idiom, and the iOS surface stays on the flow it has.
    @State private var page: PageSetup?
    @State private var zoom: CGFloat = 1
    #endif

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            Divider()
            #if os(macOS)
            LeafEditor(model: editor, theme: theme, page: page, zoom: zoom)
                .background(page == nil ? editorBackground : Color.clear)
            if page != nil { zoomBar }
            #else
            LeafEditor(model: editor, theme: theme)
                .background(editorBackground)
            #endif
        }
        .ignoresSafeArea(.keyboard, edges: .bottom)
    }

    #if os(macOS)
    /// The zoom control, shown only on paper — the continuous flow reflows to the
    /// window, so scaling it says nothing a text-size choice doesn't say better.
    /// A page is a fixed width, which is exactly when a zoom is the right knob.
    private var zoomBar: some View {
        HStack(spacing: 10) {
            Spacer()
            Image(systemName: "minus.magnifyingglass").foregroundStyle(.secondary)
            Slider(value: $zoom, in: 0.25...4)
                .frame(width: 180)
                .accessibilityLabel("zoom")
            Image(systemName: "plus.magnifyingglass").foregroundStyle(.secondary)
            Button("\(Int((zoom * 100).rounded()))%") { zoom = 1 }
                .buttonStyle(.plain)
                .monospacedDigit()
                .frame(width: 44, alignment: .trailing)
                .foregroundStyle(.secondary)
                .help("Reset to 100%")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .background(.bar)
    }
    #endif

    /// The two display choices resolved into a theme. Everything else stays at
    /// the default — the point of the measure being counted in *characters* is
    /// that width and text size compose without a table of point widths: pick a
    /// size, and the column that holds ~65 characters of it follows.
    private var theme: EditorTheme {
        var t = EditorTheme.default
        t.fontSize = textSize.points
        t.lineHeight = textSize.points * 1.5
        t.measure = columnWidth.measure
        return t
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
                btn("hr", "rectangle.compress.vertical", active: false) { editor.insertThematicBreak() }
                btn("footnote", "textformat.superscript", active: false) { editor.insertFootnote() }
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
                appearanceMenu
                #if os(macOS)
                pageMenu
                #endif
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

    /// The display menu — how wide the text column runs and how big it's set.
    /// Both take effect on the next paint: the editor re-wraps when the theme's
    /// *geometry* changes and only repaints when it doesn't, so holding the menu
    /// open and stepping through the widths reflows the document live.
    private var appearanceMenu: some View {
        Menu {
            Text("Column width")
            ForEach(ColumnWidth.allCases) { width in
                Button { columnWidth = width } label: {
                    Label(width.label, systemImage: columnWidth == width ? "checkmark" : "")
                }
            }
            Divider()
            Text("Text size")
            ForEach(TextSize.allCases) { size in
                Button { textSize = size } label: {
                    Label(size.label, systemImage: textSize == size ? "checkmark" : "")
                }
            }
        } label: {
            Image(systemName: "textformat.size")
                .font(.system(size: 17))
                .frame(minWidth: 24, minHeight: 24)
                .foregroundStyle(Color.primary)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .accessibilityLabel("appearance")
    }

    #if os(macOS)
    /// The page menu — continuous scrolling, or a document laid onto sheets of a
    /// chosen size. Switching between them re-wraps: a page's margins decide the
    /// text column while one is set, and the theme's `measure` decides it when
    /// none is.
    private var pageMenu: some View {
        Menu {
            Button { page = nil } label: {
                Label("Continuous", systemImage: page == nil ? "checkmark" : "")
            }
            Divider()
            Text("Paper")
            Button { setPaper(.usLetter) } label: {
                Label("US Letter", systemImage: paperIs(.usLetter) ? "checkmark" : "")
            }
            Button { setPaper(.a4) } label: {
                Label("A4", systemImage: paperIs(.a4) ? "checkmark" : "")
            }
            Divider()
            Text("Columns")
            Button { setColumns(1) } label: {
                Label("One", systemImage: (page?.columns ?? 1) == 1 ? "checkmark" : "")
            }
            Button { setColumns(2) } label: {
                Label("Two", systemImage: page?.columns == 2 ? "checkmark" : "")
            }
            .disabled(page == nil)
        } label: {
            Image(systemName: page == nil ? "doc.plaintext" : "doc.on.doc")
                .font(.system(size: 17))
                .frame(minWidth: 24, minHeight: 24)
                .foregroundStyle(page == nil ? Color.primary : Color.accentColor)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .accessibilityLabel("page")
    }
    /// Paper and column count are separate choices on one `PageSetup`, so
    /// switching the sheet keeps the columns and vice versa.
    private func paperIs(_ paper: PageSetup) -> Bool { page?.size == paper.size }

    private func setPaper(_ paper: PageSetup) {
        page = paper.columned(page?.columns ?? 1)
    }

    private func setColumns(_ n: Int) {
        guard let page else { return }
        self.page = page.columned(n)
    }
    #endif

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

/// How wide the text column may run, in characters of the body font — the
/// typographic "measure". The named tiers are what a reader actually chooses
/// between; 45–75 characters is the comfortable range for continuous prose, and
/// `.full` is the escape hatch for anyone who'd rather fill the window.
private enum ColumnWidth: String, CaseIterable, Identifiable {
    case narrow, medium, wide, full
    var id: String { rawValue }

    var measure: CGFloat? {
        switch self {
        case .narrow: return 52
        case .medium: return 68
        case .wide:   return 88
        case .full:   return nil
        }
    }

    var label: String {
        switch self {
        case .narrow: return "Narrow"
        case .medium: return "Medium"
        case .wide:   return "Wide"
        case .full:   return "Full width"
        }
    }
}

/// The body point size. Everything else in the theme is derived from it — the
/// line height here, and the column width through the character-counted measure.
private enum TextSize: String, CaseIterable, Identifiable {
    case small, medium, large
    var id: String { rawValue }

    var points: CGFloat {
        switch self {
        case .small:  return 14
        case .medium: return 16
        case .large:  return 19
        }
    }

    var label: String {
        switch self {
        case .small:  return "Small"
        case .medium: return "Medium"
        case .large:  return "Large"
        }
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
    let model = try! LeafEditorModel(source: sampleMarkdown, format: "markdown")
    // The sample's attachments are relative paths, and core resolves none of them
    // — it does no I/O and knows no paths. For this demo the "document directory"
    // is the app bundle, which is where the sample's media actually lives; a real
    // host would point this at the file's own directory.
    model.documentDirectory = Bundle.main.resourceURL
    // With the default `.inline` playback the editor plays media itself, so this
    // is only reached for a source its local-file loader can't resolve — a remote
    // URL, say. A real app would fetch and present one; the demo just reports it.
    model.onOpenMedia = { src in
        NSLog("leaf-editor: play %@", src)
    }
    // The editor never touches the network. It hands us a source it can't read
    // and we answer with a local file — here by pretending to fetch and handing
    // back a bundled one, which is exactly the shape a real download-and-cache
    // takes: answer whenever you have it, from whatever thread you are on.
    model.onResolveMedia = { src, done in
        NSLog("leaf-editor: resolve %@", src)
        DispatchQueue.global().asyncAfter(deadline: .now() + 0.4) {
            done(Bundle.main.url(forResource: "clip", withExtension: "mp4"))
        }
    }
    // "Edit Link…" asks the *host* for the new destination — the editor ships no
    // prompt of its own, so a host that leaves this nil simply gets no such menu
    // item. A plain text field is the demo's answer; a note app would offer its
    // own document picker here instead.
    model.onEditLink = { [weak model] current in
        promptForLink(seed: current) { destination in
            model?.insertLink(destination)
        }
    }
    return model
}

/// Ask for a link destination, seeded with the current one, and call back with
/// it. Demo chrome — a native alert per platform, which is exactly the kind of
/// thing `LeafUI` leaves to its host.
private func promptForLink(seed: String, done: @escaping (String) -> Void) {
    #if canImport(AppKit)
    let alert = NSAlert()
    alert.messageText = "Edit Link"
    alert.addButton(withTitle: "OK")
    alert.addButton(withTitle: "Cancel")
    let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 24))
    field.stringValue = seed
    alert.accessoryView = field
    if alert.runModal() == .alertFirstButtonReturn, !field.stringValue.isEmpty {
        done(field.stringValue)
    }
    #elseif canImport(UIKit)
    guard let root = UIApplication.shared.connectedScenes
        .compactMap({ ($0 as? UIWindowScene)?.keyWindow?.rootViewController })
        .first
    else { return }
    let alert = UIAlertController(title: "Edit Link", message: nil, preferredStyle: .alert)
    alert.addTextField { $0.text = seed }
    alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
    alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
        let text = alert.textFields?.first?.text ?? ""
        if !text.isEmpty { done(text) }
    })
    root.present(alert, animated: true)
    #endif
}

private let sampleMarkdown = """
# leaf, natively

A native **SwiftUI** front end driving *leaf-core* over the FFI — the same \
caret model and AST→glyph map the terminal and desktop apps use, on macOS and iOS.

## What's live

- WYSIWYG rendering with `inline code`
- **Bold**, *italic*, and ==highlight==
- Click (or tap) to place the caret, drag to select
- A [link](https://example.invalid/docs) you edit by clicking into it — \
⌘-click or right-click to follow it

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

## Attachments

Images draw inline. Video and audio show a still and a play badge until you
tap one, and then play right where they sit.

![the leaf banner](banner.png)

<video src="clip.mp4" poster="banner.png" controls></video>

<audio src="take.mp3" controls></audio>

A `data:` picture carries its own bytes, so it needs neither a document
directory nor the app — the editor decodes it:

![a dot](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAGAAAABgCAIAAABt+uBvAAAACXBIWXMAAAABAAAAAQBPJcTWAAAA2ElEQVR4nO3QQQ3AIADAQEgwO5+IQM4ULH2yx52CpvPsZ/Bt3Q74O4OCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUXjPZA4Om5tBAAAAAAElFTkSuQmCC)

And a source the editor can't read itself is handed to the app, which
fetches it and answers with a file:

<video src="https://example.invalid/remote.mp4" controls></video>

Try the toolbar, or the keyboard's arrows and ⌘B / ⌘I.
"""
