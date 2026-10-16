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
    /// What a Mac window needed; the Mac's app is `apps/leaf-mac` now.
    func sized(fileURL: URL?) -> some View {
        self
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
