//  EditorModel.swift
//
//  One document's editor, as the app's chrome sees it: what it shows (the
//  `LeafState` the menus and toolbar tick from), and every command they send
//  it. The editor itself is Rust — leaf-dioxus behind `LeafEditorView`'s
//  handle — so a command is either the key chord it stands for, replayed
//  exactly as the keyboard would send it, or one of the C ABI's calls for
//  what no key does.
//
//  The model outlives its view: the document makes it, with the source it
//  read, before any window shows it, and asks it for the source to save
//  whether or not a view ever appeared.

import AppKit
import Combine

/// A key chord, as the editor's bindings match it: a DOM key value, a DOM
/// code, and the modifiers.
struct Chord {
    let key: String
    let code: String
    let mods: UInt32

    static func cmd(_ key: String, _ code: String, shift: Bool = false, alt: Bool = false) -> Chord {
        var m = UInt32(LEAF_MOD_COMMAND)
        if shift { m |= UInt32(LEAF_MOD_SHIFT) }
        if alt { m |= UInt32(LEAF_MOD_ALT) }
        return Chord(key: key, code: code, mods: m)
    }

    static func ctrl(_ key: String, _ code: String) -> Chord {
        Chord(key: key, code: code, mods: UInt32(LEAF_MOD_CONTROL))
    }

    static func alt(_ key: String, _ code: String) -> Chord {
        Chord(key: key, code: code, mods: UInt32(LEAF_MOD_ALT))
    }
}

/// The document formats, as the ABI numbers them.
enum DocumentFormat: UInt32 {
    case markdown = 0, djot = 1, html = 2
}

final class EditorModel: ObservableObject {
    /// What the menus and toolbar show, refreshed after every repaint.
    @Published private(set) var state = LeafState()
    /// The text zoom over the settings' size — View ▸ Make Text Bigger.
    @Published var zoom: Double = 1

    let format: DocumentFormat
    /// The source the document was read with, until a view takes it over.
    private var initialSource: String
    /// Where the document lives, for its pictures' relative paths.
    var fileURL: URL? {
        didSet { if fileURL != oldValue { pushPath() } }
    }
    /// The edit count the document system was last told about.
    private var reportedRevision: UInt64 = 0
    /// Tell the document system the document changed: the edited dot, Save,
    /// the autosave clock.
    var onEdit: (() -> Void)?

    weak var view: LeafEditorNSView?
    var handle: OpaquePointer? { view?.handle }

    init(source: String, format: DocumentFormat) {
        self.initialSource = source
        self.format = format
    }

    // MARK: The view's half

    /// The source a new view starts from.
    var sourceForNewView: String { initialSource }

    /// Where a new view says the document is: the file, or for an untitled
    /// one the bundle's resources, where the sample's media live.
    var pathForNewView: String {
        if let fileURL { return fileURL.path }
        return (Bundle.main.resourceURL ?? URL(fileURLWithPath: "/"))
            .appendingPathComponent("Untitled.md").path
    }

    private func pushPath() {
        guard let handle else { return }
        leaf_view_set_path(handle, pathForNewView)
    }

    /// The editor repainted: re-read what the chrome shows, and tell the
    /// document system of an edit.
    func refresh() {
        guard let handle else { return }
        var s = LeafState()
        leaf_view_state(handle, &s)
        if s.dirty && s.revision != reportedRevision {
            reportedRevision = s.revision
            onEdit?()
        }
        state = s
    }

    // MARK: The document's half

    /// The source to save, from the editor if it is on screen.
    func source() -> String {
        guard let handle, let raw = leaf_view_source(handle) else { return initialSource }
        defer { leaf_string_free(raw) }
        let s = String(cString: raw)
        initialSource = s
        return s
    }

    /// The document system wrote what `source()` gave it.
    func markSaved() {
        guard let handle else { return }
        leaf_view_mark_saved(handle)
    }

    // MARK: Commands

    func send(_ chord: Chord) {
        guard let handle else { return }
        leaf_view_key(handle, chord.key, chord.code, chord.mods)
    }

    func table(_ cmd: Int, _ arg: Int32 = 0) {
        guard let handle else { return }
        leaf_view_table(handle, UInt32(cmd), arg)
    }

    func action(_ action: Int) {
        guard let handle else { return }
        leaf_view_action(handle, UInt32(action))
    }

    func highlight(_ color: Int32) {
        guard let handle else { return }
        leaf_view_highlight(handle, color)
    }

    func setPreserveBreaks(_ preserve: Bool) {
        guard let handle else { return }
        leaf_view_set_preserve_breaks(handle, preserve)
    }

    /// The link at the caret, or nil.
    func linkAtCaret() -> String? {
        guard let handle, let raw = leaf_view_link_at_caret(handle) else { return nil }
        defer { leaf_string_free(raw) }
        return String(cString: raw)
    }

    func insertLink(_ destination: String) {
        guard let handle else { return }
        leaf_view_insert_link(handle, destination)
    }

    /// Format ▸ Link…: ask for a destination, seeded with the link at the
    /// caret, and make or change the link.
    func promptForLink() {
        let alert = NSAlert()
        alert.messageText = "Link"
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 24))
        field.stringValue = linkAtCaret() ?? ""
        field.placeholderString = "https://"
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        let done: (NSApplication.ModalResponse) -> Void = { [weak self] response in
            if response == .alertFirstButtonReturn, !field.stringValue.isEmpty {
                self?.insertLink(field.stringValue)
            }
            self?.view?.window?.makeFirstResponder(self?.view)
        }
        if let window = view?.window {
            alert.beginSheetModal(for: window, completionHandler: done)
        } else {
            done(alert.runModal())
        }
    }

    func isActive(_ mark: Int) -> Bool { state.marks & UInt32(mark) != 0 }

    /// The selected text, or nil.
    func selectedText() -> String? {
        guard let handle, let raw = leaf_view_selected_text(handle) else { return nil }
        defer { leaf_string_free(raw) }
        return String(cString: raw)
    }

    // MARK: Look

    /// The display choices last applied, kept for a view that appears later
    /// and for a change of zoom.
    private var look: Look?

    /// Draw with these display choices, at the model's zoom.
    func apply(_ look: Look) {
        self.look = look
        guard let handle else { return }
        leaf_view_set_look(handle, look.body, look.mono, Float(look.size * zoom),
                           Float(look.leading), Float(look.measure ?? 0), 0)
    }

    // MARK: Paper

    /// The document as a PDF on `paper` — the editor's own layout, paginated
    /// between rows (`crates/leaf-dioxus-embed/src/pdf.rs`).
    func pdfData(paper: NSSize, margin: CGFloat = 72) -> Data? {
        let look = look ?? DisplayChoice.look(columnWidth: .medium, textSize: .medium, type: Typography())
        let bytes = Array(source().utf8)
        var len = 0
        let ptr = bytes.withUnsafeBufferPointer { buf in
            leaf_pdf(buf.baseAddress, buf.count, format.rawValue, fileURL?.path ?? pathForNewView,
                     look.body, look.mono, Float(look.size), Float(look.leading),
                     Float(paper.width), Float(paper.height), Float(margin), &len)
        }
        guard let ptr else { return nil }
        defer { leaf_bytes_free(ptr, len) }
        return Data(bytes: ptr, count: len)
    }

    /// The text zoom's steps: a browser's, which a reader already knows.
    static let zooms: [Double] = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3]

    /// One step bigger (1), smaller (-1), or back to actual size (0).
    func step(zoom direction: Int) {
        switch direction {
        case 1: zoom = Self.zooms.first { $0 > zoom + 0.001 } ?? Self.zooms.last!
        case -1: zoom = Self.zooms.last { $0 < zoom - 0.001 } ?? Self.zooms.first!
        default: zoom = 1
        }
        if let look { apply(look) }
    }

    /// The view made its editor: give it the look, and read its state.
    func viewDidAppear() {
        if let look { apply(look) }
        refresh()
    }
}

/// What the editor draws with, resolved from the display choices.
struct Look: Equatable {
    var body: String
    var mono: String
    var size: Double
    var leading: Double
    var measure: Double?
}
