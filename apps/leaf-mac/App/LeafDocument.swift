//  LeafDocument.swift
//
//  A file on disk, as the document system sees it. `DocumentGroup` owns the
//  rest — Open, Save, Save As, Duplicate, Rename, Revert, autosave, Versions,
//  the recents list and the title bar's proxy icon — and asks this class only
//  three things: read these bytes, write these bytes, and tell me when
//  something changed.
//
//  One class per format, because a document's format is a fact about its
//  bytes that never changes: leaf parses Markdown into Markdown and writes
//  Markdown back. With one type per class Save As offers no Format pop-up,
//  and File ▸ New becomes a submenu naming each.

import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    static let markdownDocument = UTType("net.daringfireball.markdown")!
    /// Imported by project.yml, for `.dj` and `.djot`.
    static let djotDocument = UTType(importedAs: "org.djot.djot")
}

protocol LeafFormat {
    static var format: DocumentFormat { get }
    static var contentType: UTType { get }
}

enum MarkdownFormat: LeafFormat {
    static let format = DocumentFormat.markdown
    static let contentType = UTType.markdownDocument
}

enum DjotFormat: LeafFormat {
    static let format = DocumentFormat.djot
    static let contentType = UTType.djotDocument
}

enum HTMLFormat: LeafFormat {
    static let format = DocumentFormat.html
    static let contentType = UTType.html
}

// Concrete classes: `DocumentGroup` tells its groups apart by class.
final class MarkdownDocument: LeafDocument<MarkdownFormat> {}
final class DjotDocument: LeafDocument<DjotFormat> {}
final class HTMLDocument: LeafDocument<HTMLFormat> {}

/// A document in one format, holding the editor model its window edits.
///
/// A reference document: the model is a class the editor edits in place, and
/// the editor keeps its own history, so this carries no undo of its own.
class LeafDocument<Format: LeafFormat>: ReferenceFileDocument {
    typealias Snapshot = String

    static var readableContentTypes: [UTType] { [Format.contentType] }
    static var writableContentTypes: [UTType] { [Format.contentType] }

    let model: EditorModel

    /// An empty document, or the bundled sample when one was asked for (Help ▸
    /// Open Sample Document, or `--sample`). The sample is Markdown, so only a
    /// Markdown New answers for it.
    required init() {
        let sample = Format.format == .markdown && SampleDocument.takeRequest()
        model = EditorModel(source: sample ? SampleDocument.source : "", format: Format.format)
    }

    required init(configuration: ReadConfiguration) throws {
        guard let data = configuration.file.regularFileContents,
              let source = String(data: data, encoding: .utf8)
        else { throw CocoaError(.fileReadCorruptFile) }
        model = EditorModel(source: source, format: Format.format)
    }

    /// Taken on the main thread while the editor is quiet; the document
    /// system then writes it from wherever it likes.
    func snapshot(contentType: UTType) throws -> String {
        let source = model.source()
        model.markSaved()
        return source
    }

    func fileWrapper(snapshot: String, configuration: WriteConfiguration) throws -> FileWrapper {
        FileWrapper(regularFileWithContents: Data(snapshot.utf8))
    }
}

/// The bundled sample, and the request for it: one flag the whole app shares,
/// set before the document system is asked for a new document and read by
/// the next `init()`.
enum SampleDocument {
    nonisolated(unsafe) private static var requested = false
    nonisolated(unsafe) private static var launchArgumentTaken = false

    static func request() { requested = true }

    static func takeRequest() -> Bool {
        if requested {
            requested = false
            return true
        }
        if !launchArgumentTaken, ProcessInfo.processInfo.arguments.contains("--sample") {
            launchArgumentTaken = true
            return true
        }
        return false
    }

    /// `Sample.md`, copied into the bundle beside the media it refers to.
    static var source: String {
        guard let url = Bundle.main.url(forResource: "Sample", withExtension: "md"),
              let text = try? String(contentsOf: url, encoding: .utf8)
        else { return "# leaf\n\nThe sample document is missing from the bundle.\n" }
        return text
    }
}
