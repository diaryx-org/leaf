import SwiftUI
import LeafUI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// A minimal cross-platform host for the `LeafUI` editor: the package's
/// formatting bar with the demo's display menus added to it, and the
/// `LeafEditor` surface below. Everything — caret math, wrapping, selection,
/// WYSIWYG resolution — comes from leaf-core over the FFI; this file is only
/// chrome. The same view builds for macOS and iOS because
/// `LeafEditor`/`LeafTextView` carry both surfaces.
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
    /// The paginated view: nil is the continuous flow, a `PageSetup` puts the
    /// document on paper. The zoom is not here — it is the model's, because the
    /// surface moves it too (a pinch, View ▸ Zoom), and this view only reads it
    /// back for the label.
    @State private var page: PageSetup?

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            Divider()
            LeafEditor(model: editor, theme: theme, page: page)
                .background(page == nil ? editorBackground : Color.clear)
            #if os(macOS)
            if page != nil { zoomBar }
            #endif
        }
        .ignoresSafeArea(.keyboard, edges: .bottom)
    }

    #if os(macOS)
    /// The zoom control, shown only on paper — the continuous flow reflows to the
    /// window, so scaling it says nothing a text-size choice doesn't say better.
    /// A page is a fixed width, which is exactly when a zoom is the right knob.
    /// The slider writes a plain scale; a fit chosen from the menu (or the
    /// default, which fits the width) shows here as the number it resolved to.
    private var zoomBar: some View {
        HStack(spacing: 10) {
            Spacer()
            Image(systemName: "minus.magnifyingglass").foregroundStyle(.secondary)
            Slider(value: Binding(get: { editor.zoomScale }, set: { editor.zoom = .scale($0) }),
                   in: Zoom.range)
                .frame(width: 180)
                .accessibilityLabel("zoom")
            Image(systemName: "plus.magnifyingglass").foregroundStyle(.secondary)
            Button("\(Int((editor.zoomScale * 100).rounded()))%") { editor.actualSize() }
                .buttonStyle(.plain)
                .monospacedDigit()
                .frame(width: 44, alignment: .trailing)
                .foregroundStyle(.secondary)
                .help("Actual size")
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
    ///
    /// On paper the column is the sheet's, so the equation runs the other way:
    /// `fitted(to:)` sets the type so the chosen measure fills the column — the
    /// default 16 points sets a Letter column only 58 characters wide, shorter
    /// than the flow's 68 — and the text-size choice then scales from there, so
    /// both menus still mean something on a page. How big that reads on screen
    /// is the zoom's business, which opens at fit-width.
    private var theme: EditorTheme {
        var t = EditorTheme.default
        if let page {
            t = t.fitted(to: page, measure: columnWidth.measure ?? 88)
            let factor = textSize.points / TextSize.medium.points
            t.fontSize *= factor
            t.lineHeight *= factor
        } else {
            t.fontSize = textSize.points
            t.lineHeight = textSize.points * 1.5
        }
        t.measure = columnWidth.measure
        return t
    }

    /// The package's own bar — scrolling on iOS, paged by group on macOS —
    /// with the demo's display chrome as a group of host tools at its end:
    /// the source toggle, the flow and appearance menus, and the page menu
    /// where there is a paginated view to choose.
    private var toolbar: some View {
        LeafFormattingToolbar(editor: editor, tools: hostTools)
    }

    private var hostTools: [LeafFormattingToolbar.Tool] {
        var tools: [LeafFormattingToolbar.Tool] = [
            .button("view", systemImage: editor.isSource ? "doc.richtext" : "chevron.left.slash.chevron.right",
                    label: "view", active: editor.isSource) { editor.toggleView() },
            .menu("flow", systemImage: "arrow.turn.down.left", label: "line flow",
                  active: flowPreserved) { flowRows },
            .menu("appearance", systemImage: "textformat.size", label: "appearance") { appearanceRows },
        ]
        tools.append(.menu("page", systemImage: page == nil ? "doc.plaintext" : "doc.on.doc",
                           label: "page", active: page != nil) { pageRows })
        return tools
    }

    /// The soft-break flow rows (a "View"-style menu): Fold reflows soft breaks
    /// into the paragraph, Preserve renders each where it was written. The
    /// change takes effect immediately — the editor relays out under the new flow.
    @ViewBuilder
    private var flowRows: some View {
        Button { setFlow(false) } label: {
            Label("Reflow soft breaks", systemImage: flowPreserved ? "" : "checkmark")
        }
        Button { setFlow(true) } label: {
            Label("Preserve line breaks", systemImage: flowPreserved ? "checkmark" : "")
        }
    }

    /// The display rows — how wide the text column runs and how big it's set.
    /// Both take effect on the next paint: the editor re-wraps when the theme's
    /// *geometry* changes and only repaints when it doesn't, so holding the menu
    /// open and stepping through the widths reflows the document live.
    @ViewBuilder
    private var appearanceRows: some View {
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
    }

    /// The page rows — continuous scrolling, or a document laid onto sheets of a
    /// chosen size. Switching between them re-wraps: a page's margins decide the
    /// text column while one is set, and the theme's `measure` decides it when
    /// none is. The zoom rows are the same commands View ▸ Zoom binds on the
    /// Mac, here for the phone, where the menu bar is a pinch.
    @ViewBuilder
    private var pageRows: some View {
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
        Divider()
        Text("Zoom — \(Int((editor.zoomScale * 100).rounded()))%")
        Button { editor.zoomIn() } label: { Label("Zoom In", systemImage: "plus.magnifyingglass") }
        Button { editor.zoomOut() } label: { Label("Zoom Out", systemImage: "minus.magnifyingglass") }
        Button { editor.actualSize() } label: {
            Label("Actual Size", systemImage: editor.zoom == .actualSize ? "checkmark" : "")
        }
        Button { editor.zoom = .fitWidth } label: {
            Label("Fit Width", systemImage: editor.zoom == .fitWidth ? "checkmark" : "")
        }
        Button { editor.zoom = .fitPage } label: {
            Label("Fit Page", systemImage: editor.zoom == .fitPage ? "checkmark" : "")
        }
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

    private func setFlow(_ preserve: Bool) {
        flowPreserved = preserve
        editor.setLineFlow(preserve ? .preserve : .fold)
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
    // Both the toolbar's Link button and the context menu's "Edit Link…" ask the
    // *host* for the destination — the editor ships no prompt of its own, so a
    // host that leaves this nil gets no such menu item (the toolbar button falls
    // back to a field of its own). A plain text field is the demo's answer; a note
    // app would offer its own document picker here instead. `current` is empty
    // when the caret is in no link, which is the "make one" case.
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
///
/// Titled "Link" rather than "Edit Link": the same prompt makes one over the
/// selection when the seed is empty.
private func promptForLink(seed: String, done: @escaping (String) -> Void) {
    #if canImport(AppKit)
    let alert = NSAlert()
    alert.messageText = "Link"
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
    let alert = UIAlertController(title: "Link", message: nil, preferredStyle: .alert)
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

## Math

A formula in a line, $E = mc^2$, is typeset where it sits, and one on lines \
of its own is set in display style:

$$
\\int_0^1 x\\,dx = \\frac{1}{2}
$$

Put the caret on a formula's line and it shows its TeX, ready to edit.

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
