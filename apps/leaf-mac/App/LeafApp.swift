//  LeafApp.swift
//
//  Leaf, the document app: a window per file, one `LeafDocument` behind each.
//  `DocumentGroup` is the whole of the file story — the Open panel at launch,
//  File ▸ New / Open / Save / Save As / Duplicate / Rename / Revert, autosave
//  and Versions, the recents list, the proxy icon, Finder's Open With once
//  project.yml has declared the types. This file adds the chrome around it:
//  the menus, the Settings window and the sample.

import SwiftUI
import UniformTypeIdentifiers

@main
struct LeafApp: App {
    var body: some Scene {
        DocumentGroup(newDocument: { MarkdownDocument() }) { file in
            ContentView(model: file.document.model, fileURL: file.fileURL)
                .sized(fileURL: file.fileURL)
        }
        .defaultSize(width: 700, height: 906)
        DocumentGroup(newDocument: { DjotDocument() }) { file in
            ContentView(model: file.document.model, fileURL: file.fileURL)
                .sized(fileURL: file.fileURL)
        }
        .defaultSize(width: 700, height: 906)
        DocumentGroup(newDocument: { HTMLDocument() }) { file in
            ContentView(model: file.document.model, fileURL: file.fileURL)
                .sized(fileURL: file.fileURL)
        }
        .defaultSize(width: 700, height: 906)
        .commands {
            LeafEditorCommands()
            LeafAppCommands()
        }
        Settings { SettingsView() }
    }
}

private extension View {
    /// A new window is page-shaped, US Letter's 8.5 × 11 at a width that holds
    /// the Medium column with its margins; a document opened before reopens
    /// where it was left.
    func sized(fileURL: URL?) -> some View {
        frame(minWidth: 480, minHeight: 320)
            .remembersWindowFrame(for: fileURL)
    }
}

/// The app's own menu items, beside the editor's.
struct LeafAppCommands: Commands {
    var body: some Commands {
        // SwiftUI folds three document groups into one "New Document"; this is
        // the submenu TextEdit-style apps show, one item per format.
        CommandGroup(replacing: .newItem) {
            Menu("New") {
                Button("Markdown Document") { newDocument(.markdownDocument) }
                    .keyboardShortcut("n", modifiers: .command)
                Button("Djot Document") { newDocument(.djotDocument) }
                Button("HTML Document") { newDocument(.html) }
            }
            Button("Open…") { NSDocumentController.shared.openDocument(nil) }
                .keyboardShortcut("o", modifiers: .command)
        }
        CommandGroup(after: .help) {
            Divider()
            Button("Open Sample Document") {
                SampleDocument.request()
                newDocument(.markdownDocument)
            }
        }
    }

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
}
