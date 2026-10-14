import SwiftUI
import LeafUI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// One document's window: the package's formatting bar with the app's display
/// menus added to it, and the `LeafEditor` surface below. Everything — caret
/// math, wrapping, selection, WYSIWYG resolution — comes from leaf-core over the
/// FFI; this file is only chrome. The same view builds for macOS and iOS
/// because `LeafEditor`/`LeafTextView` carry both surfaces.
struct ContentView: View {
    /// The document's model, owned by its `LeafDocument`; this view wires the
    /// host hooks onto it and shows it.
    let model: LeafEditorModel
    /// Where the file is, or nil until it is first saved. Relative images and
    /// media resolve against its folder.
    let fileURL: URL?

    /// The scene's undo manager is the document's: telling it of a change is
    /// how a `ReferenceFileDocument` says it is edited, which is what enables
    /// Save, shows the dot in the close button, and starts the autosave clock.
    @Environment(\.undoManager) private var undoManager

    /// The soft-break flow shown in the dropdown. Held here (not read back off the
    /// model each paint) because flipping it doesn't change the toolbar's other
    /// state, so this is what drives the menu's checkmark. Its opening value is
    /// the setting's; the toolbar then moves it for this window only.
    @State private var flowPreserved = false
    /// The reader's display choices, shared by every window and remembered
    /// across launches — the Settings window edits the same keys.
    @AppStorage(DisplayChoice.columnWidthKey) private var columnWidth: ColumnWidth = .medium
    @AppStorage(DisplayChoice.textSizeKey) private var textSize: TextSize = .medium
    @AppStorage(DisplayChoice.paperKey) private var paper: Paper = .usLetter
    @AppStorage(DisplayChoice.flowKey) private var flowSetting = false
    private var typography = StoredTypography()
    /// The paginated view: nil is the continuous flow, a `PageSetup` puts the
    /// document on paper. The zoom is not here — it is the model's, because the
    /// surface moves it too (a pinch, View ▸ Zoom), and this view only reads it
    /// back for the label.
    @State private var page: PageSetup?
    #if os(iOS)
    /// The Settings sheet, which the bar's menu opens. The Mac has
    /// a Settings window for these; a phone has no app menu to open one from.
    @State private var showingSettings = false
    #endif

    var body: some View {
        VStack(spacing: 0) {
            #if os(macOS)
            toolbar
            Divider()
            LeafEditor(model: model, theme: theme, page: page)
                .background(page == nil ? editorBackground : Color.clear)
            if page != nil { zoomBar }
            #else
            // Above the keyboard, where the package's row is meant to hang:
            // its `Aa` swaps the keyboard for the formatting panel and the row
            // stays above whichever is up. The demo's display chrome goes in
            // the navigation bar's one menu instead, where it can be reached
            // without a keyboard up — and the row keeps to the nine it was
            // sized for.
            LeafEditor(model: model, theme: theme, page: page) {
                LeafFormattingToolbar(editor: model)
            }
            .background(page == nil ? editorBackground : Color.clear)
            .toolbar {
                ToolbarItem(placement: .primaryAction) { moreMenu }
            }
            .sheet(isPresented: $showingSettings) {
                NavigationStack {
                    SettingsView()
                        .navigationTitle("Settings")
                        .navigationBarTitleDisplayMode(.inline)
                        .toolbar {
                            ToolbarItem(placement: .confirmationAction) {
                                Button("Done") { showingSettings = false }
                            }
                        }
                }
            }
            #endif
        }
        .ignoresSafeArea(.keyboard, edges: .bottom)
        .onAppear {
            flowPreserved = flowSetting
            if flowPreserved { model.setLineFlow(.preserve) }
            wire()
        }
        // Save As moves the file, and its images with it.
        .onChange(of: fileURL) { _ in wire() }
        .onChange(of: undoManager) { _ in wire() }
    }

    /// The host's half of the model: where its files are, and what to do with
    /// what the editor cannot do itself. Set whenever the answers change.
    private func wire() {
        // Core resolves no path itself — it does no I/O and knows no paths. A
        // saved document's attachments live beside it; an unsaved one has no
        // beside, and the bundle is where the sample's media lives.
        model.documentDirectory = fileURL?.deletingLastPathComponent() ?? Bundle.main.resourceURL
        model.onEdit = { [weak model, weak undoManager] in
            // A no-op registration: the document system reads it as "changed"
            // and does the rest. The edit itself is twig's to undo, through the
            // text view's own manager — see LeafUI's `UndoBridge.swift`.
            guard let model else { return }
            undoManager?.registerUndo(withTarget: model) { _ in }
        }
        // With the default `.inline` playback the editor plays media itself, so
        // this is only reached for a source its local-file loader can't resolve
        // — a remote URL. Hand it to the system.
        model.onOpenMedia = { src in
            guard let url = URL(string: src) else { return }
            #if canImport(AppKit)
            NSWorkspace.shared.open(url)
            #else
            UIApplication.shared.open(url)
            #endif
        }
        // The editor never touches the network. It hands us a source it can't
        // read and we answer with a local file: fetched into the caches folder,
        // from whatever thread the download finishes on.
        model.onResolveMedia = { src, done in
            guard let url = URL(string: src), let scheme = url.scheme,
                  scheme == "http" || scheme == "https"
            else { return done(nil) }
            URLSession.shared.downloadTask(with: url) { temp, _, _ in
                guard let temp else { return done(nil) }
                let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
                let kept = caches.appendingPathComponent(url.lastPathComponent)
                try? FileManager.default.removeItem(at: kept)
                done((try? FileManager.default.moveItem(at: temp, to: kept)) == nil ? nil : kept)
            }.resume()
        }
        // Both the toolbar's Link button and the context menu's "Edit Link…" ask
        // the *host* for the destination — the editor ships no prompt of its own.
        // A plain text field is the app's answer. `current` is empty when the
        // caret is in no link, which is the "make one" case.
        model.onEditLink = { [weak model] current in
            promptForLink(seed: current) { destination in
                model?.insertLink(destination)
            }
        }
        // The app's own directive: Insert ▸ Embed… writes an `::embed{src=…}`,
        // and the editor draws each one as the app's card. Every other directive
        // stays leaf's placeholder. And the app's own fence renderer and
        // editor: a ```mermaid fence is drawn as its diagram and opens as its
        // source beside a live preview.
        if model.directiveCatalogue.isEmpty { model.directiveCatalogue = [EmbedDirective.item] }
        // And a date chip, `:date[2026-10-12]`, edited in place.
        if model.regionRenderers.isEmpty {
            model.regionRenderers = [EmbedDirective.kind: EmbedDirective.renderer,
                                     MermaidFence.kind: MermaidFence.renderer,
                                     DateChip.kind: DateChip.renderer]
        }
        if model.regionEditors.isEmpty {
            model.regionEditors = [MermaidFence.kind: MermaidFence.editor, DateChip.kind: DateChip.editor]
        }
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
            Slider(value: Binding(get: { model.zoomScale }, set: { model.zoom = .scale($0) }),
                   in: Zoom.range)
                .frame(width: 180)
                .accessibilityLabel("zoom")
            Image(systemName: "plus.magnifyingglass").foregroundStyle(.secondary)
            Button("\(Int((model.zoomScale * 100).rounded()))%") { model.actualSize() }
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

    private var theme: EditorTheme {
        DisplayChoice.theme(columnWidth: columnWidth, textSize: textSize,
                            type: typography.value, page: page)
    }

    #if os(macOS)
    /// The package's own bar on macOS — six category menus — with the demo's
    /// display chrome as a group of host tools at its end. On iOS the same
    /// chrome is in the navigation bar (see `body`).
    private var toolbar: some View {
        LeafFormattingToolbar(editor: model, tools: displayTools.map(\.barTool))
    }
    #endif

    /// The demo's display chrome as host tools at the end of the Mac's bar:
    /// the source toggle, the flow and appearance menus, and the page menu.
    /// Each lights while its choice is off the default.
    private var displayTools: [DisplayTool] {
        [
            DisplayTool(id: "view",
                        systemImage: model.isSource ? "doc.richtext" : "chevron.left.slash.chevron.right",
                        label: "view", active: model.isSource, kind: .button { model.toggleView() }),
            DisplayTool(id: "flow", systemImage: "arrow.turn.down.left", label: "line flow",
                        active: flowPreserved, kind: .menu(AnyView(flowRows))),
            DisplayTool(id: "appearance", systemImage: "textformat.size", label: "appearance",
                        kind: .menu(AnyView(appearanceRows))),
            DisplayTool(id: "page", systemImage: page == nil ? "doc.plaintext" : "doc.on.doc",
                        label: "page", active: page != nil, kind: .menu(AnyView(pageRows))),
        ]
    }

    #if os(iOS)
    /// The display chrome on iOS, as one menu at the end of the navigation
    /// bar. A phone's bar holds two of the Mac's four tools and folds the
    /// rest into a More menu of its own, so the four go into one menu here,
    /// with Settings — which also holds the Mac's appearance menu — last.
    private var moreMenu: some View {
        Menu {
            Button { model.toggleView() } label: {
                Label(model.isSource ? "Show Rich Text" : "Show Source",
                      systemImage: model.isSource ? "doc.richtext" : "chevron.left.slash.chevron.right")
            }
            Menu { flowRows } label: { Label("Line Breaks", systemImage: "arrow.turn.down.left") }
            Menu { pageRows } label: {
                Label("Page", systemImage: page == nil ? "doc.plaintext" : "doc.on.doc")
            }
            Divider()
            Button { showingSettings = true } label: { Label("Settings…", systemImage: "gearshape") }
        } label: {
            Image(systemName: "ellipsis")
        }
        .accessibilityLabel("Document Options")
    }
    #endif

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
        ForEach(Paper.allCases) { choice in
            Button { setPaper(choice) } label: {
                Label(choice.label, systemImage: paperIs(choice) ? "checkmark" : "")
            }
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
        Text("Zoom — \(Int((model.zoomScale * 100).rounded()))%")
        Button { model.zoomIn() } label: { Label("Zoom In", systemImage: "plus.magnifyingglass") }
        Button { model.zoomOut() } label: { Label("Zoom Out", systemImage: "minus.magnifyingglass") }
        Button { model.actualSize() } label: {
            Label("Actual Size", systemImage: model.zoom == .actualSize ? "checkmark" : "")
        }
        Button { model.zoom = .fitWidth } label: {
            Label("Fit Width", systemImage: model.zoom == .fitWidth ? "checkmark" : "")
        }
        Button { model.zoom = .fitPage } label: {
            Label("Fit Page", systemImage: model.zoom == .fitPage ? "checkmark" : "")
        }
    }
    /// Paper and column count are separate choices on one `PageSetup`, so
    /// switching the sheet keeps the columns and vice versa. The paper chosen
    /// is remembered, and is what Export as PDF prints on.
    private func paperIs(_ choice: Paper) -> Bool { page?.size == choice.setup.size }

    private func setPaper(_ choice: Paper) {
        paper = choice
        page = choice.setup.columned(page?.columns ?? 1)
    }

    private func setColumns(_ n: Int) {
        guard let page else { return }
        self.page = page.columned(n)
    }

    private func setFlow(_ preserve: Bool) {
        flowPreserved = preserve
        model.setLineFlow(preserve ? .preserve : .fold)
    }
}

/// One piece of the demo's display chrome on the Mac: what it is, what it
/// does, and whether it is lit, drawn as one of `LeafFormattingToolbar`'s host
/// tools (`barTool`). iOS gathers the same rows into one menu (`moreMenu`).
private struct DisplayTool: Identifiable {
    enum Kind {
        case button(() -> Void)
        case menu(AnyView)
    }

    let id: String
    let systemImage: String
    let label: String
    var active = false
    let kind: Kind

    /// As a tool on the package's bar, in the bar's own chrome.
    var barTool: LeafFormattingToolbar.Tool {
        switch kind {
        case .button(let action):
            return .button(id, systemImage: systemImage, label: label, active: active, action: action)
        case .menu(let rows):
            return .menu(id, systemImage: systemImage, label: label, active: active) { rows }
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
