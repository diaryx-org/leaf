//  LeafEditorView.swift
//
//  The editor in an AppKit view. Blitz draws into this view's layer through
//  wgpu; everything else about it is AppKit's: it is the window's first
//  responder and the input method's text input client, takes the pointer,
//  answers the Edit menu's Cut, Copy and Paste, and pops up the context menu.
//  Each event goes to the editor across `crates/leaf-dioxus-embed`'s C ABI, in
//  points from the view's top-left. The scroll is an `NSScrollView`'s.

import AppKit
import SwiftUI

/// The editor, for SwiftUI.
struct LeafEditorView: NSViewRepresentable {
    @ObservedObject var model: EditorModel

    func makeNSView(context: Context) -> LeafScrollView {
        LeafScrollView(model: model)
    }

    func updateNSView(_ view: LeafScrollView, context: Context) {}

    static func dismantleNSView(_ view: LeafScrollView, coordinator: ()) {
        view.editor.close()
    }
}

/// The editor in a scroll view of AppKit's own, so that a scroll is the
/// system's: its momentum, its give at either end, its scroller. The scroll
/// view scrolls an empty document as tall as the editor's content, and the
/// editor's view is kept over the part of it in view, drawing the content
/// from wherever the scroll has got to. A view as tall as the content would
/// be a surface wgpu cannot make, and every row drawn.
final class LeafScrollView: NSScrollView {
    let editor: LeafEditorNSView
    /// Set while following a move the editor made itself, which it need not
    /// be told of.
    private var following = false

    init(model: EditorModel) {
        editor = LeafEditorNSView(model: model)
        super.init(frame: NSRect(x: 0, y: 0, width: 700, height: 900))
        hasVerticalScroller = true
        autohidesScrollers = true
        drawsBackground = false
        horizontalScrollElasticity = .none
        verticalScrollElasticity = .allowed
        // The toolbar's inset would start the content under it; the editor
        // keeps clear of it already.
        automaticallyAdjustsContentInsets = false
        contentInsets = NSEdgeInsetsZero
        let document = ScrolledContent(frame: bounds)
        documentView = document
        document.addSubview(editor)
        contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(clipMoved),
                                               name: NSView.boundsDidChangeNotification,
                                               object: contentView)
        editor.scrolled = { [unowned self] in follow($0) }
    }

    required init?(coder: NSCoder) { fatalError("not from a nib") }

    override func tile() {
        super.tile()
        documentView?.setFrameSize(NSSize(width: contentView.bounds.width,
                                          height: documentView?.frame.height ?? 0))
        placeEditor()
    }

    @objc private func clipMoved() {
        placeEditor()
        if !following {
            leaf_view_set_scroll(editor.handle, contentView.bounds.minY)
        }
    }

    /// Over the part of the document in view — past either end of it too,
    /// while the scroll gives there.
    private func placeEditor() {
        let visible = contentView.bounds
        if editor.frame.size != visible.size {
            editor.setFrameSize(visible.size)
        }
        editor.setFrameOrigin(visible.origin)
    }

    /// The editor scrolled itself (to the caret, to a match, to the top of a
    /// document it was given) or its content changed height.
    private func follow(_ s: LeafEditorNSView.Scroll) {
        documentView?.setFrameSize(NSSize(width: contentView.bounds.width, height: s.height))
        scrollerInsets = NSEdgeInsets(top: s.insetTop, left: 0, bottom: s.insetBottom, right: 0)
        guard contentView.bounds.minY != s.top else { return }
        following = true
        contentView.scroll(to: NSPoint(x: 0, y: s.top))
        reflectScrolledClipView(contentView)
        following = false
    }
}

/// What the scroll view scrolls: nothing to draw, only a height. Not
/// scrolled off the main thread, as responsive scrolling would, since the
/// editor's view is moved over it on the main thread.
private final class ScrolledContent: NSView {
    override var isFlipped: Bool { true }
    override class var isCompatibleWithResponsiveScrolling: Bool { false }
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
    /// The last pressure stage, to see a force click begin.
    private var pressureStage = 0
    private(set) lazy var spelling = Spelling(view: self)
    /// The text the system indexes, with the revision and view it was read
    /// at: asked for by the input method around most keystrokes.
    private var textCache: (revision: UInt64, source: Bool, prose: Bool, text: String)?

    /// The `scrolled` callback's report, in points.
    struct Scroll { var top, height, insetTop, insetBottom: Double }
    /// Where the editor scrolled itself, for the scroll view around it.
    var scrolled: ((Scroll) -> Void)?

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
        callbacks.changed = { ctx in
            let view = LeafEditorNSView.from(ctx)
            view.model.refresh()
            view.spelling.editorChanged()
        }
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
        callbacks.scrolled = { ctx, top, height, insetTop, insetBottom in
            LeafEditorNSView.from(ctx).scrolled?(Scroll(top: top, height: height,
                                                        insetTop: insetTop, insetBottom: insetBottom))
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
        // A right-click places the caret, so the word under it is the
        // caret's.
        if !has(LEAF_CONTEXT_SELECTION), let word = spelling.misspellingAtCaret() {
            spelling.addItems(for: word, to: menu)
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
        } else if let caret = selectedUTF16(), let word = word(at: caret.location) {
            // The right-click put the caret in the word.
            let title = (text(prose: false) as NSString).substring(with: word)
            menu.addItem(.separator())
            add("Look Up “\(title)”") { [weak self] in self?.showDefinition(of: word) }
        }
        menu.popUp(positioning: nil, at: menuPoint, in: self)
    }

    private func lookUpSelection() {
        // The Dictionary's panel, for the selected text, over it.
        guard let range = selectedUTF16(), range.length > 0 else { return }
        showDefinition(of: range)
    }

    // MARK: Edit menu

    /// Edit ▸ Spelling and Grammar ▸ Check Document Now.
    @objc func checkSpelling(_ sender: Any?) { spelling.checkDocumentNow() }

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

    /// A force click: the press deepening to its second stage. AppKit would
    /// turn it into `quickLook(with:)` for a view of its own, but under
    /// SwiftUI's hosting view that never arrives here, so the stage is read
    /// directly — and acted on only where the trackpad's Look Up is a force
    /// click, as it is in every other app.
    override func pressureChange(with event: NSEvent) {
        defer { pressureStage = event.stage }
        guard event.stage == 2, pressureStage < 2, Self.forceClickLooksUp else { return }
        lookUp(at: convert(event.locationInWindow, from: nil))
    }

    /// Whether System Settings ▸ Trackpad ▸ Look up & data detectors is
    /// Force Click: force clicking on, and Look Up not given to a three-finger
    /// tap instead.
    static var forceClickLooksUp: Bool {
        let global = UserDefaults.standard
        let trackpad = UserDefaults(suiteName: "com.apple.AppleMultitouchTrackpad")
        let forceClick = global.object(forKey: "com.apple.trackpad.forceClick") as? Bool ?? true
        let threeFingerTap = trackpad?.integer(forKey: "TrackpadThreeFingerTapGesture") ?? 0
        return forceClick && threeFingerTap != 2
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
        let composing = !marked.isEmpty
        let replacing = replacementRange.location != NSNotFound
        if replacing, !composing {
            // Press-and-hold replaces the letter before the caret, and
            // dictation or a correction the range it measured.
            leaf_view_select_range(handle, replacementRange.location, replacementRange.length)
        }
        marked = ""
        leaf_view_insert_text(handle, text)
        if !composing, !replacing { spelling.afterTyping(text) }
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

    // While an input method composes, its marked text is the whole of what
    // it is told about, as before; otherwise it sees the document's text.
    func selectedRange() -> NSRange {
        guard marked.isEmpty, let range = selectedUTF16() else { return NSRange(location: 0, length: 0) }
        return range
    }

    func markedRange() -> NSRange {
        marked.isEmpty ? NSRange(location: NSNotFound, length: 0)
            : NSRange(location: 0, length: (marked as NSString).length)
    }

    func hasMarkedText() -> Bool { !marked.isEmpty }

    func attributedSubstring(forProposedRange range: NSRange,
                             actualRange: NSRangePointer?) -> NSAttributedString? {
        guard marked.isEmpty, range.location != NSNotFound else { return nil }
        let text = self.text(prose: false) as NSString
        let actual = NSIntersectionRange(range, NSRange(location: 0, length: text.length))
        guard actual.location == range.location else { return nil }
        actualRange?.pointee = actual
        return NSAttributedString(string: text.substring(with: actual))
    }

    // MARK: The text, as the system sees it

    /// The editor's state now, not as of the model's last refresh.
    func currentState() -> LeafState {
        var s = LeafState()
        leaf_view_state(handle, &s)
        return s
    }

    /// The text the system indexes (`leaf_view_text`), or with `prose` the
    /// same with all but prose blanked, for the spelling checker.
    func text(prose: Bool) -> String {
        let s = currentState()
        if let c = textCache, c.revision == s.revision, c.source == s.source_view, c.prose == prose {
            return c.text
        }
        guard let raw = leaf_view_text(handle, prose) else { return "" }
        defer { leaf_string_free(raw) }
        let text = String(cString: raw)
        textCache = (s.revision, s.source_view, prose, text)
        return text
    }

    /// The selection, or the caret alone, as a UTF-16 range of `text`.
    func selectedUTF16() -> NSRange? {
        guard handle != nil else { return nil }
        var loc = 0
        var len = 0
        leaf_view_selected_range(handle, &loc, &len)
        return NSRange(location: loc, length: len)
    }

    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

    /// The character at a point on the screen, for the system's own lookups.
    func characterIndex(for point: NSPoint) -> Int {
        guard marked.isEmpty, let window else { return NSNotFound }
        let p = convert(window.convertPoint(fromScreen: point), from: nil)
        let index = leaf_view_index_at_point(handle, p.x, p.y)
        return index < 0 ? NSNotFound : Int(index)
    }

    /// Where a range is drawn on the screen: an input method's candidates go
    /// beside it, and Look Up's highlight over it. The caret, while composing.
    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let window else { return .zero }
        let rect = marked.isEmpty ? self.rect(of: range) ?? caretRect : caretRect
        return window.convertToScreen(convert(rect, to: nil))
    }

    /// Where a UTF-16 range of the text is drawn (its first line), in this
    /// view's points.
    func rect(of range: NSRange) -> NSRect? {
        var r = [Double](repeating: 0, count: 4)
        guard range.location != NSNotFound,
              leaf_view_range_rect(handle, range.location, range.length, &r) else { return nil }
        return NSRect(x: r[0], y: r[1], width: r[2], height: r[3])
    }

    // MARK: Look Up

    /// A force click, or a three-finger tap: the word under the pointer in
    /// the Dictionary's popover, drawn over the word where it stands — what
    /// any text view does.
    override func quickLook(with event: NSEvent) {
        if !lookUp(at: convert(event.locationInWindow, from: nil)) {
            super.quickLook(with: event)
        }
    }

    /// Look up the word at a point of this view; whether there was one.
    @discardableResult
    private func lookUp(at p: NSPoint) -> Bool {
        let index = leaf_view_index_at_point(handle, p.x, p.y)
        guard marked.isEmpty, index >= 0, let word = word(at: Int(index)) else { return false }
        showDefinition(of: word)
        return true
    }

    /// The word around UTF-16 `index` of the text, if the index is in one.
    private func word(at index: Int) -> NSRange? {
        let text = self.text(prose: false) as NSString
        guard index < text.length else { return nil }
        let paragraph = text.paragraphRange(for: NSRange(location: index, length: 0))
        var found: NSRange?
        text.enumerateSubstrings(in: paragraph, options: [.byWords, .substringNotRequired]) { _, range, _, stop in
            if NSLocationInRange(index, range) {
                found = range
                stop.pointee = true
            } else if range.location > index {
                stop.pointee = true
            }
        }
        return found
    }

    /// The face and size the character at UTF-16 `index` is drawn in — a
    /// heading's, a bold run's — for drawing it again over itself.
    private func font(at index: Int) -> NSFont? {
        var size = 0.0
        guard let raw = leaf_view_font_at(handle, index, &size) else { return nil }
        defer { leaf_string_free(raw) }
        return NSFont(name: String(cString: raw), size: size)
    }

    /// The Dictionary's popover for a range of the text, set on its baseline.
    private func showDefinition(of range: NSRange) {
        let text = self.text(prose: false) as NSString
        guard NSMaxRange(range) <= text.length, let rect = rect(of: range) else { return }
        let font = font(at: range.location) ?? model.bodyFont
        // The line box centres the face's ascent and descent.
        let baseline = rect.minY + (rect.height + font.ascender + font.descender) / 2
        let word = NSAttributedString(string: text.substring(with: range), attributes: [.font: font])
        showDefinition(for: word, at: NSPoint(x: rect.minX, y: baseline))
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
