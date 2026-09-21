//  LeafDocument.swift
//
//  A file on disk, as the document system sees it. `DocumentGroup` owns the
//  rest — Open, Save, Save As, Duplicate, Rename, Revert, autosave, Versions,
//  the recents list and the title bar's proxy icon on the Mac; the document
//  browser and the Files app on iOS — and asks this class only three things:
//  read these bytes, write these bytes, and tell me when something changed.
//
//  One class per format rather than one class for all three, because a
//  document's format is a fact about its bytes that never changes: leaf parses
//  Markdown into Markdown and writes Markdown back, and core offers no
//  conversion. A single class listing every type it can read would make Save As
//  offer a Format pop-up whose other choices would write Markdown under a `.dj`
//  extension. With one type per class the pop-up never appears, and File ▸ New
//  becomes a submenu naming each.

import LeafUI
import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    /// `net.daringfireball.markdown` is declared by the system on both
    /// platforms; `.md` and `.markdown` already resolve to it.
    static let markdownDocument = UTType("net.daringfireball.markdown")!
    /// Djot has no system type, so `project.yml` imports one under this
    /// identifier — `.dj` and `.djot` — and this is its Swift name.
    static let djotDocument = UTType(importedAs: "org.djot.djot")
}

/// What one format's document class says about itself.
protocol LeafFormat {
    /// The name core parses by — see `LeafEditorModel(source:format:)`.
    static var name: String { get }
    static var contentType: UTType { get }
}

enum MarkdownFormat: LeafFormat {
    static let name = "markdown"
    static let contentType = UTType.markdownDocument
}

enum DjotFormat: LeafFormat {
    static let name = "djot"
    static let contentType = UTType.djotDocument
}

enum HTMLFormat: LeafFormat {
    static let name = "html"
    static let contentType = UTType.html
}

// Concrete classes rather than type aliases: `DocumentGroup` tells its groups
// apart by class, and three specialisations of one generic read as one.
final class MarkdownDocument: LeafDocument<MarkdownFormat> {}
final class DjotDocument: LeafDocument<DjotFormat> {}
final class HTMLDocument: LeafDocument<HTMLFormat> {}

/// A document in one format, holding the editor model the window edits.
///
/// A reference document, not a value one: the model is a class that owns a
/// live FFI handle, and the surface edits it in place. That is also why this
/// carries no undo of its own — twig keeps the history, and the responder chain
/// reaches it through the text view (see LeafUI's `UndoBridge.swift`).
class LeafDocument<Format: LeafFormat>: ReferenceFileDocument {
    typealias Snapshot = String

    static var readableContentTypes: [UTType] { [Format.contentType] }
    static var writableContentTypes: [UTType] { [Format.contentType] }

    let model: LeafEditorModel

    /// An empty document, or the bundled sample when one has been asked for
    /// (`--sample` on the command line, Help ▸ Open Sample Document). The
    /// sample is written in Markdown, so only a Markdown New answers for it.
    required init() {
        let isMarkdown = Format.name == MarkdownFormat.name
        let source = isMarkdown && SampleDocument.takeRequest() ? SampleDocument.source : ""
        // An empty string and the shipped sample both parse, in every format.
        model = try! LeafEditorModel(source: source, format: Format.name)
    }

    required init(configuration: ReadConfiguration) throws {
        guard let data = configuration.file.regularFileContents,
              let source = String(data: data, encoding: .utf8)
        else { throw CocoaError(.fileReadCorruptFile) }
        model = try LeafEditorModel(source: source, format: Format.name)
    }

    /// Serialised on the main thread while the model is quiet; the document
    /// system then writes it from wherever it likes.
    func snapshot(contentType: UTType) throws -> String {
        model.source()
    }

    func fileWrapper(snapshot: String, configuration: WriteConfiguration) throws -> FileWrapper {
        FileWrapper(regularFileWithContents: Data(snapshot.utf8))
    }
}

/// The bundled sample, and the request for it. Its own type because a generic
/// class can hold no static state, and the request has to be one flag the
/// whole app shares: it is set before the document system is asked for a new
/// document and read by the next `init()` — a flag rather than an initialiser
/// argument because `DocumentGroup(newDocument:)` calls the closure it was
/// given, with nothing to say which New this is.
enum SampleDocument {
    nonisolated(unsafe) private static var requested = false
    /// `--sample` answers once, for the document the launch opens; a ⌘N after
    /// it is an empty document like any other.
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

    /// `App/Sample.md`, copied into the bundle beside the media it refers to.
    static var source: String {
        guard let url = Bundle.main.url(forResource: "Sample", withExtension: "md"),
              let text = try? String(contentsOf: url, encoding: .utf8)
        else { return "# leaf\n\nThe sample document is missing from the bundle.\n" }
        return text
    }
}
