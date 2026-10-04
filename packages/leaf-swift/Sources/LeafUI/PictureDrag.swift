//  PictureDrag.swift  (AppKit / macOS)
//
//  A picture dragged out of the editor arrives as a picture. Pressing on an
//  image picks its block up (see `LeafTextView.beginDraggingBlock`), and
//  within the document that is a move; dropped on the Finder, the desktop, or
//  another app's window, it is the image file — the way a picture dragged out
//  of Mail, Notes or a web page is — with the block's markup alongside for an
//  app that would rather have text.
//
//  Two ways to hand over a file. A picture that lives in a file (beside the
//  document, or wherever the host resolved a remote one to) goes as that
//  file's URL, and the receiver copies it. A picture with no file of its own —
//  a `data:` URI, its bytes written into the markup — goes as a *promise*,
//  written out under a name the receiver chooses the folder for, which is
//  what `NSFilePromiseProvider` exists to do.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import UniformTypeIdentifiers

/// A file promise that also carries the dragged block's text, so a receiver
/// that reads strings rather than files still gets what was picked up.
final class PictureFilePromiseProvider: NSFilePromiseProvider {
    /// The block's markup, offered as `.string`.
    var text = ""
    /// Held here because the provider holds its delegate weakly.
    var promise: PictureFilePromise?

    override func writableTypes(for pasteboard: NSPasteboard) -> [NSPasteboard.PasteboardType] {
        super.writableTypes(for: pasteboard) + [.string]
    }

    override func writingOptions(forType type: NSPasteboard.PasteboardType,
                                 pasteboard: NSPasteboard) -> NSPasteboard.WritingOptions {
        type == .string ? [] : super.writingOptions(forType: type, pasteboard: pasteboard)
    }

    override func pasteboardPropertyList(forType type: NSPasteboard.PasteboardType) -> Any? {
        type == .string ? text : super.pasteboardPropertyList(forType: type)
    }
}

/// The bytes a promise writes and the name it writes them under.
final class PictureFilePromise: NSObject, NSFilePromiseProviderDelegate {
    let data: Data
    let fileName: String

    init(data: Data, fileName: String) {
        self.data = data
        self.fileName = fileName
    }

    func filePromiseProvider(_ filePromiseProvider: NSFilePromiseProvider, fileNameForType fileType: String) -> String {
        fileName
    }

    func filePromiseProvider(_ filePromiseProvider: NSFilePromiseProvider, writePromiseTo url: URL,
                             completionHandler: @escaping (Error?) -> Void) {
        do {
            try data.write(to: url)
            completionHandler(nil)
        } catch {
            completionHandler(error)
        }
    }

    /// The type and a file name for `data`, read from the bytes themselves —
    /// a `data:` URI's own MIME type is the writer's claim, the header is the
    /// fact. `nil` for bytes that are no image the system can read.
    static func describe(_ data: Data, named base: String) -> (type: UTType, fileName: String)? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let identifier = CGImageSourceGetType(source) as String?,
              let type = UTType(identifier) else { return nil }
        let ext = type.preferredFilenameExtension ?? "png"
        return (type, base + "." + ext)
    }
}
#endif
