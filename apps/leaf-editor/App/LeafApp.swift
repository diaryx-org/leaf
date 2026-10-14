//  LeafApp.swift
//
//  Leaf, the document app: a window per file, one `LeafDocument` behind each.
//  `DocumentGroup` is the whole of the file story — the Open panel at launch,
//  File ▸ New / Open / Save / Save As / Duplicate / Rename / Revert, autosave
//  and Versions, the recents list, the proxy icon in the title bar, Finder's
//  "Open With" once `project.yml` has declared the types; on iOS the document
//  browser and the Files app. Everything this file adds is the chrome around
//  that: the menus the package does not ship, the settings window, the sample.

import LeafUI
import SwiftUI
import UniformTypeIdentifiers

@main
struct LeafApp: App {
    var body: some Scene {
        // One group per format, so a document's type is fixed at birth (see
        // `LeafDocument`). The first is what File ▸ New and the iOS launch
        // screen's Create make; the others hang under New as a submenu.
        DocumentGroup(newDocument: { MarkdownDocument() }) { file in
            ContentView(model: file.document.model, fileURL: file.fileURL)
                .sized(fileURL: file.fileURL)
        }
        .documentWindowSize()
        DocumentGroup(newDocument: { DjotDocument() }) { file in
            ContentView(model: file.document.model, fileURL: file.fileURL)
                .sized(fileURL: file.fileURL)
        }
        .documentWindowSize()
        DocumentGroup(newDocument: { HTMLDocument() }) { file in
            ContentView(model: file.document.model, fileURL: file.fileURL)
                .sized(fileURL: file.fileURL)
        }
        .documentWindowSize()
        // Format and View menus aimed at whichever editor the window shows —
        // `LeafEditor` publishes itself as the scene's focused editor.
        .commands {
            LeafEditorCommands()
            LeafAppCommands()
        }
        #if os(macOS)
        Settings { SettingsView() }
        #endif
    }
}

private extension Scene {
    /// A new document's window is page-shaped, US Letter's 8.5 × 11 at a
    /// width that holds the Medium column with its margins; macOS shrinks it
    /// to a screen too short for it. A document that has been open before
    /// reopens at its own size instead (`remembersWindowFrame`).
    func documentWindowSize() -> some Scene {
        #if os(macOS)
        defaultSize(width: 700, height: 906)
        #else
        self
        #endif
    }
}

private extension View {
    func sized(fileURL: URL?) -> some View {
        #if os(macOS)
        frame(minWidth: 480, minHeight: 320)
            .remembersWindowFrame(for: fileURL)
        #else
        self
        #endif
    }
}

/// The app's own menu items, beside the package's Format and View menus.
struct LeafAppCommands: Commands {
    @FocusedValue(\.leafEditor) private var editor: LeafEditorModel?
    @AppStorage(DisplayChoice.columnWidthKey) private var columnWidth: ColumnWidth = .medium
    @AppStorage(DisplayChoice.textSizeKey) private var textSize: TextSize = .medium
    @AppStorage(DisplayChoice.paperKey) private var paper: Paper = .usLetter
    private var typography = StoredTypography()

    var body: some Commands {
        #if os(macOS)
        // SwiftUI folds three document groups into one "New Document"; this is
        // the submenu TextEdit-style apps show, one item per format.
        CommandGroup(replacing: .newItem) {
            Menu("New") {
                Button("Markdown Document") { newDocument(.markdownDocument) }
                    .keyboardShortcut("n", modifiers: .command)
                Button("Djot Document") { newDocument(.djotDocument) }
                Button("HTML Document") { newDocument(.html) }
            }
        }
        CommandGroup(after: .saveItem) {
            Divider()
            Button("Export as PDF…") { exportPDF() }
                .keyboardShortcut("e", modifiers: [.command, .shift])
                .disabled(editor == nil)
        }
        CommandGroup(after: .help) {
            Divider()
            Button("Open Sample Document") {
                SampleDocument.request()
                NSDocumentController.shared.newDocument(nil)
            }
        }
        #endif
    }

    #if os(macOS)
    /// An untitled document of one type, through the document controller so
    /// that it is the document system's — its window, its Save As, its
    /// autosave — exactly as File ▸ New's own item would make one.
    private func newDocument(_ type: UTType) {
        let controller = NSDocumentController.shared
        do {
            let document = try controller.makeUntitledDocument(ofType: type.identifier)
            controller.addDocument(document)
            document.makeWindowControllers()
            document.showWindows()
        } catch {
            NSAlert(error: error).runModal()
        }
    }

    /// The document as a PDF, on the paper Settings names, in the theme the
    /// window shows — `pdfData` paints with the same layout the screen uses.
    /// The panel is a sheet on the document's window, as Save As is.
    private func exportPDF() {
        guard let editor, let window = NSApp.keyWindow else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.pdf]
        panel.canCreateDirectories = true
        let title = window.title
        panel.nameFieldStringValue = (title as NSString).deletingPathExtension + ".pdf"
        let page = paper.setup
        let theme = DisplayChoice.theme(columnWidth: columnWidth, textSize: textSize,
                                        type: typography.value, page: page)
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            do {
                try editor.pdfData(theme: theme, page: page, title: title).write(to: url)
            } catch {
                NSAlert(error: error).beginSheetModal(for: window)
            }
        }
    }
    #endif
}

#if os(macOS)
/// Leaf ▸ Settings…: the reader's display choices, some of which the window's
/// toolbar menus also move. They live in `UserDefaults` under `DisplayChoice`'s
/// keys, so every window and every launch reads the same ones, and every open
/// window redraws as one changes.
struct SettingsView: View {
    @AppStorage(DisplayChoice.columnWidthKey) private var columnWidth: ColumnWidth = .medium
    @AppStorage(DisplayChoice.textSizeKey) private var textSize: TextSize = .medium
    @AppStorage(DisplayChoice.paperKey) private var paper: Paper = .usLetter
    @AppStorage(DisplayChoice.flowKey) private var flowPreserved = false
    private var typography = StoredTypography()

    var body: some View {
        Form {
            Section("Text") {
                FontPicker(title: "Font:", selection: typography.$bodyFont,
                           defaults: [("Iowan Old Style (default)", EditorTheme.serifFontName),
                                      ("System", EditorTheme.systemFontName)],
                           families: FontPicker.allFamilies)
                FontPicker(title: "Code font:", selection: typography.$codeFont,
                           defaults: [("System monospaced (default)", EditorTheme.systemMonospacedFontName)],
                           families: FontPicker.monospacedFamilies)
                Picker("Size:", selection: $textSize) {
                    ForEach(TextSize.allCases) { Text($0.label).tag($0) }
                }
            }
            Section("Layout") {
                Picker("Line spacing:", selection: typography.$lineSpacing) {
                    ForEach(LineSpacing.allCases) { Text($0.label).tag($0) }
                }
                Picker("Paragraph spacing:", selection: typography.$paragraphSpacing) {
                    ForEach(ParagraphSpacing.allCases) { Text($0.label).tag($0) }
                }
                Picker("Column width:", selection: $columnWidth) {
                    ForEach(ColumnWidth.allCases) { Text($0.label).tag($0) }
                }
                Toggle("Preserve line breaks in new windows", isOn: $flowPreserved)
            }
            Section("Page") {
                Picker("Paper:", selection: $paper) {
                    ForEach(Paper.allCases) { Text($0.label).tag($0) }
                }
            }
            HStack {
                Spacer()
                Button("Restore Defaults", action: restoreDefaults)
            }
        }
        .formStyle(.grouped)
        // A grouped form is a scroll view, and its ideal height came out a
        // point short of its content, which put up a scroller beside a pane
        // with nothing to scroll to.
        .scrollDisabled(true)
        .frame(width: 440)
        .fixedSize(horizontal: false, vertical: true)
    }

    /// Every choice back to what a first launch has. Removing the keys rather
    /// than writing the defaults, so a later release that changes a default
    /// reaches a reader who never chose otherwise.
    private func restoreDefaults() {
        let keys = [
            DisplayChoice.columnWidthKey, DisplayChoice.textSizeKey, DisplayChoice.paperKey,
            DisplayChoice.flowKey, DisplayChoice.bodyFontKey, DisplayChoice.codeFontKey,
            DisplayChoice.lineSpacingKey, DisplayChoice.paragraphSpacingKey,
        ]
        keys.forEach(UserDefaults.standard.removeObject(forKey:))
    }
}

/// A font family menu: the theme's own names first, then every family
/// installed that fits. A family stored by a font since uninstalled keeps
/// its row, so the menu still says what is chosen; the editor sets such a
/// font in the system face.
private struct FontPicker: View {
    let title: String
    @Binding var selection: String
    let defaults: [(label: String, name: String)]
    let families: [String]

    var body: some View {
        Picker(title, selection: $selection) {
            ForEach(defaults, id: \.name) { Text($0.label).tag($0.name) }
            Divider()
            if !families.contains(selection) && !defaults.contains(where: { $0.name == selection }) {
                Text(selection).tag(selection)
            }
            ForEach(families, id: \.self) { Text($0).tag($0) }
        }
    }

    /// Every family on the Mac, hidden ones (a leading `.`) aside.
    static let allFamilies: [String] = NSFontManager.shared.availableFontFamilies
        .filter { !$0.hasPrefix(".") }

    /// The families with a fixed-pitch face, for code.
    static let monospacedFamilies: [String] = {
        let names = NSFontManager.shared.availableFontNames(with: .fixedPitchFontMask) ?? []
        let families = Set(names.compactMap { NSFont(name: $0, size: 12)?.familyName })
        return families.filter { !$0.hasPrefix(".") }.sorted()
    }()
}
#endif
