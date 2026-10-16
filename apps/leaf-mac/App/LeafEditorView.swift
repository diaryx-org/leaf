//  LeafEditorView.swift
//
//  The editor in an AppKit view. Blitz draws into this view's layer through
//  wgpu; everything else about it is AppKit's: it is the window's first
//  responder and the input method's text input client, takes the pointer and
//  the wheel, answers the Edit menu's Cut, Copy and Paste, and pops up the
//  context menu. Each event goes to the editor across
//  `crates/leaf-dioxus-embed`'s C ABI, in points from the view's top-left.

import AppKit
import SwiftUI

/// The editor, for SwiftUI.
struct LeafEditorView: NSViewRepresentable {
    @ObservedObject var model: EditorModel

    func makeNSView(context: Context) -> LeafEditorNSView {
        LeafEditorNSView(model: model)
    }

    func updateNSView(_ view: LeafEditorNSView, context: Context) {}

    static func dismantleNSView(_ view: LeafEditorNSView, coordinator: ()) {
        view.close()
    }
}

final class LeafEditorNSView: NSView, NSTextInputClient {
    private(set) var handle: OpaquePointer?
    private let model: EditorModel
    /// Where the caret is, from the editor, for the input method's windows.
    private var caretRect = NSRect(x: 0, y: 0, width: 2, height: 20)
    /// An input method's composition in progress.
    private var marked = ""
    private var cursorKind: UInt32 = 1
    private var tracking: NSTrackingArea?
    /// Where the last right-click was, for the context menu the editor asks
    /// for after it.
    private var menuPoint = NSPoint.zero

    init(model: EditorModel) {
        self.model = model
        super.init(frame: NSRect(x: 0, y: 0, width: 700, height: 900))
        model.view = self
    }

    required init?(coder: NSCoder) { fatalError("not from a nib") }

    /// Destroy the editor before the view goes.
    func close() {
        leaf_view_free(handle)
        handle = nil
    }

    deinit { close() }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    private var scale: Double { Double(window?.backingScaleFactor ?? 2) }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard window != nil, handle == nil else { return }
        var callbacks = LeafHostCallbacks()
        callbacks.ctx = Unmanaged.passUnretained(self).toOpaque()
        callbacks.changed = { ctx in LeafEditorNSView.from(ctx).model.refresh() }
        callbacks.request = { ctx, kind, arg in LeafEditorNSView.from(ctx).request(kind, arg) }
        callbacks.open = { _, what in
            guard let what, let s = String(validatingUTF8: what) else { return }
            let url = s.contains("://") ? URL(string: s) : URL(fileURLWithPath: s)
            if let url { NSWorkspace.shared.open(url) }
        }
        callbacks.context_menu = { ctx, flags in LeafEditorNSView.from(ctx).contextMenu(flags) }
        callbacks.ime_area = { ctx, x, y, w, h in
            LeafEditorNSView.from(ctx).caretRect = NSRect(x: x, y: y, width: w, height: h)
        }
        callbacks.set_cursor = { ctx, kind in
            let view = LeafEditorNSView.from(ctx)
            guard view.cursorKind != kind else { return }
            view.cursorKind = kind
            view.window?.invalidateCursorRects(for: view)
        }
        let bytes = Array(model.sourceForNewView.utf8)
        handle = bytes.withUnsafeBufferPointer { buf in
            leaf_view_new(Unmanaged.passUnretained(self).toOpaque(),
                          bounds.width, bounds.height, scale,
                          effectiveAppearance.isDark,
                          buf.baseAddress, buf.count, model.format.rawValue,
                          model.pathForNewView, callbacks)
        }
        model.viewDidAppear()
        window?.makeFirstResponder(self)
    }

    private static func from(_ ctx: UnsafeMutableRawPointer?) -> LeafEditorNSView {
        Unmanaged<LeafEditorNSView>.fromOpaque(ctx!).takeUnretainedValue()
    }

    // MARK: What the editor asks

    private func request(_ kind: UInt32, _ arg: Int32) {
        switch Int(kind) {
        case LEAF_REQUEST_SAVE:
            NSApp.sendAction(#selector(NSDocument.save(_:)), to: nil, from: self)
        case LEAF_REQUEST_SAVE_AS:
            NSApp.sendAction(#selector(NSDocument.saveAs(_:)), to: nil, from: self)
        case LEAF_REQUEST_ZOOM:
            model.step(zoom: Int(arg))
        case LEAF_REQUEST_SETTINGS:
            NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
        case LEAF_REQUEST_CHARACTER_PALETTE:
            NSApp.orderFrontCharacterPalette(nil)
        default:
            break
        }
    }

    /// The context menu, at the right-click that asked for it.
    private func contextMenu(_ flags: UInt32) {
        let menu = NSMenu()
        let has = { (bit: Int) in flags & UInt32(bit) != 0 }
        func add(_ title: String, _ action: @escaping () -> Void) {
            menu.addItem(ClosureMenuItem(title: title, action: action))
        }
        if has(LEAF_CONTEXT_GO_TO_NOTE) {
            add("Go to Note") { [model] in model.action(LEAF_ACTION_FOLLOW_FOOTNOTE) }
        }
        if has(LEAF_CONTEXT_BACK_TO_REFERENCE) {
            add("Back to Reference") { [model] in model.action(LEAF_ACTION_FOLLOW_FOOTNOTE) }
        }
        if has(LEAF_CONTEXT_LINK) {
            add("Open Link") { [model] in model.action(LEAF_ACTION_OPEN_LINK) }
            add("Edit Link…") { [model] in model.promptForLink() }
            add("Copy Link") { [model] in model.action(LEAF_ACTION_COPY_LINK) }
        }
        if !menu.items.isEmpty { menu.addItem(.separator()) }
        if has(LEAF_CONTEXT_SELECTION) {
            menu.addItem(withTitle: "Cut", action: #selector(cut(_:)), keyEquivalent: "")
            menu.addItem(withTitle: "Copy", action: #selector(copy(_:)), keyEquivalent: "")
        }
        menu.addItem(withTitle: "Paste", action: #selector(paste(_:)), keyEquivalent: "")
        menu.addItem(withTitle: "Paste and Match Style", action: #selector(pasteAsPlainText(_:)),
                     keyEquivalent: "")
        menu.addItem(withTitle: "Select All", action: #selector(selectAll(_:)), keyEquivalent: "")
        if has(LEAF_CONTEXT_SELECTION) {
            menu.addItem(.separator())
            add("Look Up") { [weak self] in self?.lookUpSelection() }
        }
        menu.popUp(positioning: nil, at: menuPoint, in: self)
    }

    private func lookUpSelection() {
        // The Dictionary's panel, for the selected text.
        guard let text = model.selectedText(), !text.isEmpty else { return }
        showDefinition(for: NSAttributedString(string: text), at: NSPoint(x: caretRect.minX, y: caretRect.maxY))
    }

    // MARK: Edit menu

    @objc func cut(_ sender: Any?) { model.send(.cmd("x", "KeyX")) }
    @objc func copy(_ sender: Any?) { model.send(.cmd("c", "KeyC")) }
    @objc func paste(_ sender: Any?) { model.send(.cmd("v", "KeyV")) }
    @objc func pasteAsPlainText(_ sender: Any?) { model.send(.cmd("v", "KeyV", shift: true, alt: true)) }
    override func selectAll(_ sender: Any?) { model.send(.cmd("a", "KeyA")) }

    // MARK: Geometry and appearance

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        leaf_view_resize(handle, newSize.width, newSize.height, scale)
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        leaf_view_resize(handle, bounds.width, bounds.height, scale)
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        leaf_view_set_dark(handle, effectiveAppearance.isDark)
    }

    override func becomeFirstResponder() -> Bool {
        leaf_view_set_focused(handle, true)
        return true
    }

    override func resignFirstResponder() -> Bool {
        leaf_view_set_focused(handle, false)
        return true
    }

    override func resetCursorRects() {
        let cursor: NSCursor = switch cursorKind {
        case 1: .iBeam
        case 2: .pointingHand
        default: .arrow
        }
        addCursorRect(bounds, cursor: cursor)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds,
                                  options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                  owner: self)
        addTrackingArea(area)
        tracking = area
    }

    // MARK: Pointer

    private func send(_ kind: UInt32, _ event: NSEvent, button: UInt32 = 0) {
        let p = convert(event.locationInWindow, from: nil)
        leaf_view_pointer(handle, kind, p.x, p.y, button, event.modifierFlags.leaf)
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        // ⌃-click is a secondary click on a Mac.
        let secondary = event.modifierFlags.contains(.control)
        if secondary { menuPoint = convert(event.locationInWindow, from: nil) }
        send(1, event, button: secondary ? 1 : 0)
    }
    override func mouseUp(with event: NSEvent) {
        send(2, event, button: event.modifierFlags.contains(.control) ? 1 : 0)
    }
    override func mouseDragged(with event: NSEvent) { send(0, event) }
    override func mouseMoved(with event: NSEvent) { send(0, event) }
    override func mouseExited(with event: NSEvent) { send(3, event) }
    override func rightMouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        menuPoint = convert(event.locationInWindow, from: nil)
        send(1, event, button: 1)
    }
    override func rightMouseUp(with event: NSEvent) { send(2, event, button: 1) }

    override func scrollWheel(with event: NSEvent) {
        leaf_view_scroll(handle, event.scrollingDeltaX, event.scrollingDeltaY,
                         event.hasPreciseScrollingDeltas)
    }

    override func flagsChanged(with event: NSEvent) {
        leaf_view_modifiers(handle, event.modifierFlags.leaf)
    }

    // MARK: Keys

    override func keyDown(with event: NSEvent) {
        let mods = event.modifierFlags.leaf
        let chord = mods & (UInt32(LEAF_MOD_COMMAND) | UInt32(LEAF_MOD_CONTROL)) != 0
        if marked.isEmpty, let named = KeyNames.named[event.keyCode] {
            leaf_view_key(handle, named, KeyNames.code(event.keyCode), mods)
            return
        }
        if chord {
            leaf_view_key(handle, event.charactersIgnoringModifiers ?? "",
                          KeyNames.code(event.keyCode), mods)
            return
        }
        // Text, dead keys, input methods, press-and-hold: AppKit's.
        interpretKeyEvents([event])
    }

    override func doCommand(by selector: Selector) {
        // A key an input method turned into a command rather than text; the
        // keys the editor binds never get here (see `keyDown`).
    }

    // MARK: NSTextInputClient

    func insertText(_ string: Any, replacementRange: NSRange) {
        let text = (string as? NSAttributedString)?.string ?? (string as? String) ?? ""
        if replacementRange.location != NSNotFound, replacementRange.length > 0 {
            // Press-and-hold replaces the letter before the caret.
            leaf_view_delete_surrounding(handle, replacementRange.length, 0)
        }
        marked = ""
        leaf_view_insert_text(handle, text)
    }

    func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let text = (string as? NSAttributedString)?.string ?? (string as? String) ?? ""
        marked = text
        let ns = text as NSString
        let start = ns.substring(to: min(selectedRange.location, ns.length)).utf8.count
        let end = ns.substring(to: min(NSMaxRange(selectedRange), ns.length)).utf8.count
        leaf_view_set_marked_text(handle, text, start, end)
    }

    func unmarkText() {
        guard !marked.isEmpty else { return }
        let text = marked
        marked = ""
        leaf_view_insert_text(handle, text)
    }

    func selectedRange() -> NSRange { NSRange(location: 0, length: 0) }

    func markedRange() -> NSRange {
        marked.isEmpty ? NSRange(location: NSNotFound, length: 0)
            : NSRange(location: 0, length: (marked as NSString).length)
    }

    func hasMarkedText() -> Bool { !marked.isEmpty }

    func attributedSubstring(forProposedRange range: NSRange,
                             actualRange: NSRangePointer?) -> NSAttributedString? { nil }

    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

    func characterIndex(for point: NSPoint) -> Int { NSNotFound }

    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let window else { return .zero }
        return window.convertToScreen(convert(caretRect, to: nil))
    }
}

/// A menu item that runs a closure.
final class ClosureMenuItem: NSMenuItem {
    private let run: () -> Void

    init(title: String, action: @escaping () -> Void) {
        run = action
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
    }

    required init(coder: NSCoder) { fatalError("not from a nib") }

    @objc private func fire() { run() }
}

extension NSAppearance {
    var isDark: Bool { bestMatch(from: [.aqua, .darkAqua]) == .darkAqua }
}

extension NSEvent.ModifierFlags {
    /// As the ABI's `LEAF_MOD_*` bits.
    var leaf: UInt32 {
        var m: UInt32 = 0
        if contains(.shift) { m |= UInt32(LEAF_MOD_SHIFT) }
        if contains(.control) { m |= UInt32(LEAF_MOD_CONTROL) }
        if contains(.option) { m |= UInt32(LEAF_MOD_ALT) }
        if contains(.command) { m |= UInt32(LEAF_MOD_COMMAND) }
        return m
    }
}

/// macOS virtual key codes as DOM key values and codes, which is how the
/// editor's bindings name them.
enum KeyNames {
    /// The keys that are commands rather than text.
    static let named: [UInt16: String] = [
        36: "Enter", 76: "Enter", 48: "Tab", 51: "Backspace", 117: "Delete", 53: "Escape",
        123: "ArrowLeft", 124: "ArrowRight", 125: "ArrowDown", 126: "ArrowUp",
        115: "Home", 119: "End", 116: "PageUp", 121: "PageDown",
    ]

    static let codes: [UInt16: String] = [
        0: "KeyA", 11: "KeyB", 8: "KeyC", 2: "KeyD", 14: "KeyE", 3: "KeyF", 5: "KeyG", 4: "KeyH",
        34: "KeyI", 38: "KeyJ", 40: "KeyK", 37: "KeyL", 46: "KeyM", 45: "KeyN", 31: "KeyO",
        35: "KeyP", 12: "KeyQ", 15: "KeyR", 1: "KeyS", 17: "KeyT", 32: "KeyU", 9: "KeyV",
        13: "KeyW", 7: "KeyX", 16: "KeyY", 6: "KeyZ",
        29: "Digit0", 18: "Digit1", 19: "Digit2", 20: "Digit3", 21: "Digit4", 23: "Digit5",
        22: "Digit6", 26: "Digit7", 28: "Digit8", 25: "Digit9",
        24: "Equal", 27: "Minus", 33: "BracketLeft", 30: "BracketRight", 41: "Semicolon",
        39: "Quote", 43: "Comma", 47: "Period", 44: "Slash", 42: "Backslash", 50: "Backquote",
        49: "Space", 36: "Enter", 76: "NumpadEnter", 48: "Tab", 51: "Backspace", 117: "Delete",
        53: "Escape", 123: "ArrowLeft", 124: "ArrowRight", 125: "ArrowDown", 126: "ArrowUp",
        115: "Home", 119: "End", 116: "PageUp", 121: "PageDown",
    ]

    static func code(_ keyCode: UInt16) -> String { codes[keyCode] ?? "Unidentified" }
}
