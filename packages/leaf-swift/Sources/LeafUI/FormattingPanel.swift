//  FormattingPanel.swift
//
//  The key grid that takes the keyboard's place when the reader presses `Aa` on
//  the iOS formatting row. The row holds the handful of tools used mid-sentence;
//  this holds the rest, so nothing has to be reached by scrolling a row
//  sideways. The layout is Bear's (see `docs/proposals/keyboard-panel.md`): keys
//  the size and shape of the system's, white for formatting and grey for typing,
//  six across and four down.
//
//  **It is still a keyboard.** Delete, Space and Return stay on it, so a writer
//  can break a line, restructure a list and fix a typo without calling the
//  keyboard back up. They are sent through the text view's own `insertText` and
//  `deleteBackward`, the methods the system keyboard calls, which makes them
//  the same keystrokes: one undo history, Return continuing a list, Return in a
//  table dropping a cell. Each fires on touch-down, as a key does, and Delete
//  repeats while held. What the panel doesn't do is anything the keyboard
//  itself does: no autocorrection, no predictions, no dictation. Those are the
//  keyboard's, and `Aa` brings it back.
//
//  **The swap is an input view.** The panel is the text view's `inputView`
//  while it is up, and the accessory row stays above it. Swapping one input
//  view for another is `reloadInputViews()` on a view that stays first
//  responder, so the caret, the selection and the row all survive — nothing
//  resigns, and nothing has to be restored. The panel is as tall as the
//  keyboard it replaces, measured from the keyboard's own frame notifications
//  the last time the system keyboard was up (`noteKeyboard`), so the document
//  doesn't jump when the two swap. A panel raised before any keyboard has been
//  measured takes a height near a phone keyboard's.
//
//  **Keys with a ▾** carry a small triangle in the corner, and are a SwiftUI
//  `Menu`. With a primary action (List, Code, Highlight), a tap runs it and a
//  press-and-hold opens the variants, the way Highlight on the row always has.
//  Without one (Style, Align, Text, Insert) a tap opens them. A menu key gets
//  no pressed state of its own, because a `Menu`'s label has none to read.

#if canImport(UIKit)
import LeafFFI
import SwiftUI
import UIKit

/// The panel as an input view: the system's keyboard backdrop
/// (`UIInputView.Style.keyboard`), with the SwiftUI grid laid over it.
///
/// `UIInputViewAudioFeedback` is what lets `playInputClick()` sound. The
/// system plays the click only for an input view that says it wants it, and
/// only while that view is on screen, so the typing keys click as the
/// keyboard's do and follow the reader's Keyboard Clicks setting.
final class FormattingPanelHost: UIInputView, UIInputViewAudioFeedback {
    private let hosting: UIHostingController<FormattingPanelView>

    init(editor: LeafEditorModel) {
        hosting = UIHostingController(rootView: FormattingPanelView(editor: editor))
        super.init(frame: CGRect(x: 0, y: 0, width: 320, height: 260), inputViewStyle: .keyboard)
        // The height is ours to set (`fit`), not the content's: a panel that
        // sized itself to its keys would be a different height from the
        // keyboard it replaces, and the document above would jump.
        allowsSelfSizing = false
        autoresizingMask = [.flexibleWidth]
        hosting.view.backgroundColor = .clear
        hosting.view.translatesAutoresizingMaskIntoConstraints = false
        addSubview(hosting.view)
        NSLayoutConstraint.activate([
            hosting.view.leadingAnchor.constraint(equalTo: leadingAnchor),
            hosting.view.trailingAnchor.constraint(equalTo: trailingAnchor),
            hosting.view.topAnchor.constraint(equalTo: topAnchor),
            hosting.view.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    var enableInputClicksWhenVisible: Bool { true }

    /// Whether this device can show the panel at all. Not on a Mac, where an
    /// iPad app under Catalyst or an iPhone app on Apple silicon has a
    /// hardware keyboard and no soft one to stand in for — `Aa` would swap
    /// nothing for nothing — so the row leaves `Aa` off there.
    static var isAvailable: Bool { !ProcessInfo.processInfo.isMacCatalystApp }

    /// The system keyboard's height, without the accessory above it, the last
    /// time one was up in each orientation — a phone's landscape keyboard is
    /// a good deal shorter than its portrait one. Shared across editors,
    /// because a keyboard is the device's rather than the document's.
    /// Internal rather than private so a test can clear it.
    static var keyboardHeights: [Orientation: CGFloat] = [:]

    enum Orientation { case portrait, landscape }

    /// The orientation a keyboard in `window` is laid out for: the scene's
    /// interface orientation, which is the one the keyboard follows. Not the
    /// window's shape — on an iPad in Split View or Stage Manager a window
    /// can be tall and narrow on a landscape screen, and its keyboard is the
    /// landscape one.
    static func orientation(of window: UIWindow) -> Orientation {
        orientation(interface: window.windowScene?.interfaceOrientation, bounds: window.bounds)
    }

    /// `interface` when it says, and the shape of `bounds` when it doesn't —
    /// a window not yet in a scene, or a scene whose orientation is
    /// `.unknown` — which is the best a window with no scene has to go on.
    static func orientation(interface: UIInterfaceOrientation?, bounds: CGRect) -> Orientation {
        switch interface {
        case .landscapeLeft?, .landscapeRight?: return .landscape
        case .portrait?, .portraitUpsideDown?: return .portrait
        default: return bounds.width > bounds.height ? .landscape : .portrait
        }
    }

    /// Remember the keyboard's height from a frame-change notification's end
    /// frame, which counts the accessory as part of the keyboard. Called only
    /// while leaf's own text view is first responder with the system keyboard
    /// up, so the accessory subtracted is the one on top of this keyboard. A
    /// frame below the screen is a keyboard going away, and a sliver no taller
    /// than an accessory is a hardware keyboard's (it shows the accessory
    /// alone), and neither is a height to copy.
    static func noteKeyboard(frame: CGRect, accessory: UIView?, in window: UIWindow) {
        let screen = window.screen.bounds
        guard frame.minY < screen.maxY - 1 else { return }
        let height = frame.height - (accessory?.bounds.height ?? 0)
        guard height > 120 else { return }
        keyboardHeights[orientation(of: window)] = height
    }

    /// Size the panel for the window `textView` is in: the window's width,
    /// and the keyboard's height in this orientation, or a phone keyboard's
    /// before one has been measured. True when the height moved, which is
    /// when the text view has to re-read its input views for it to show.
    @discardableResult
    func fit(to textView: UIView) -> Bool {
        let bounds = textView.window?.bounds ?? textView.bounds
        let orientation = textView.window.map(Self.orientation(of:))
            ?? Self.orientation(interface: nil, bounds: bounds)
        let fallback = orientation == .landscape
            ? min(max(bounds.height * 0.5, 160), 220)
            : min(max(bounds.height * 0.38, 216), 336)
        let height = Self.keyboardHeights[orientation] ?? fallback
        let moved = abs(frame.height - height) > 0.5
        frame.size = CGSize(width: bounds.width, height: height)
        return moved
    }
}

/// The grid, over one editor. Every key is a `ToolCatalogue` tool, apart from
/// the three that type.
struct FormattingPanelView: View {
    @ObservedObject var editor: LeafEditorModel

    /// Dynamic Type, as the row reads it: the glyphs grow with the reader's
    /// text size up to the row's cap. The keys themselves don't, because the
    /// grid is the keyboard's size and every key is already a large target.
    @ScaledMetric(relativeTo: .body) private var typeScale: CGFloat = 1

    var body: some View {
        let tools = ToolCatalogue(editor: editor, beginLink: beginLink)
        GeometryReader { proxy in
            let grid = PanelGrid(size: proxy.size)
            VStack(spacing: grid.rowGap) {
                HStack(spacing: grid.columnGap) {
                    key(tools.style, grid)
                        .accessibilityValue(tools.styleValue)
                    key(tools.bold, grid)
                    key(tools.italic, grid)
                    key(tools.underline, grid)
                    key(tools.strikethrough, grid)
                    TypingKey(symbol: "delete.left", label: loc("panel.delete", "Delete"),
                              repeats: true, size: grid.size(span: 1), glyphSize: glyphSize) {
                        editor.deleteFromPanel()
                    }
                }
                HStack(spacing: grid.columnGap) {
                    key(tools.list, grid)
                    key(tools.checklist, grid)
                    key(tools.quote, grid)
                    key(tools.code, grid)
                    key(tools.highlight, grid)
                    key(tools.link, grid)
                }
                HStack(spacing: grid.columnGap) {
                    key(tools.align, grid)
                    key(tools.text, grid)
                    key(tools.outdent, grid)
                    key(tools.indent, grid)
                    key(tools.moveUp, grid)
                    key(tools.moveDown, grid)
                }
                HStack(spacing: grid.columnGap) {
                    key(tools.insert, grid)
                    key(tools.undo, grid)
                    key(tools.redo, grid)
                    TypingKey(symbol: "space", label: loc("panel.space", "Space"),
                              size: grid.size(span: 2), glyphSize: glyphSize) {
                        editor.typeFromPanel(" ")
                    }
                    TypingKey(symbol: "return", label: loc("panel.return", "Return"),
                              size: grid.size(span: 1), glyphSize: glyphSize) {
                        editor.typeFromPanel("\n")
                    }
                }
            }
            .padding(.horizontal, grid.inset)
            .padding(.vertical, grid.inset)
            .frame(width: proxy.size.width, height: proxy.size.height)
        }
    }

    private var glyphSize: CGFloat { 20 * min(typeScale, PanelGrid.maxTypeScale) }

    /// A formatting key: a button, or a menu when the tool has variants.
    @ViewBuilder
    private func key(_ item: ToolItem, _ grid: PanelGrid) -> some View {
        let size = grid.size(span: 1)
        Group {
            if let variants = item.variants {
                if let action = item.action {
                    Menu { variants } label: {
                        PanelKeyFace(item: item, kind: .format, pressed: false, glyphSize: glyphSize)
                    } primaryAction: {
                        UIDevice.current.playInputClick()
                        action()
                    }
                    .accessibilityHint(loc("panel.holdForMore", "Touch and hold for more options."))
                } else {
                    Menu { variants } label: {
                        PanelKeyFace(item: item, kind: .format, pressed: false, glyphSize: glyphSize)
                    }
                }
            } else {
                Button {
                    UIDevice.current.playInputClick()
                    item.action?()
                } label: {
                    PanelKeyFace(item: item, kind: .format, pressed: false, glyphSize: glyphSize)
                }
                .buttonStyle(PanelKeyStyle(item: item, glyphSize: glyphSize))
            }
        }
        // `.plain` for the reason the row's menus take it; the face is ours.
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .frame(width: size.width, height: size.height)
        .disabled(!item.enabled)
        .accessibilityLabel(item.label)
        .accessibilityAddTraits(item.active ? .isSelected : [])
    }

    /// Link from the panel: the host's `onEditLink` first, and leaf's own
    /// field as a sheet otherwise — see `LeafEditorModel.beginLinkInSheet`
    /// for why not a popover on this key.
    private func beginLink() { editor.beginLinkInSheet() }
}

/// The grid's arithmetic: six keys across and four down, filling the panel,
/// with the system keyboard's gaps between them.
struct PanelGrid {
    static let columns: CGFloat = 6
    static let rows: CGFloat = 4
    /// How far the glyphs follow Dynamic Type. The row's cap, for its reason:
    /// the keys already fill the keyboard's space, and a glyph taller than
    /// that would crowd its key rather than help anyone read it.
    static let maxTypeScale: CGFloat = 1.6

    let size: CGSize
    var inset: CGFloat { 6 }
    var columnGap: CGFloat { 6 }
    var rowGap: CGFloat { 8 }

    var keyWidth: CGFloat {
        max(0, (size.width - 2 * inset - (Self.columns - 1) * columnGap) / Self.columns)
    }

    var keyHeight: CGFloat {
        max(0, (size.height - 2 * inset - (Self.rows - 1) * rowGap) / Self.rows)
    }

    /// A key `span` columns wide — Space takes two, so the last row is as
    /// wide as the others.
    func size(span: Int) -> CGSize {
        let span = CGFloat(span)
        return CGSize(width: keyWidth * span + columnGap * (span - 1), height: keyHeight)
    }
}

/// What a key looks like: the keyboard's rounded key with its one-point
/// shadow, the glyph, a ▾ in the corner for a key with variants, and the
/// accent over a tool that is in force.
struct PanelKeyFace: View {
    enum Kind { case format, typing }

    let item: ToolItem
    let kind: Kind
    let pressed: Bool
    let glyphSize: CGFloat

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(Color(fill))
                .shadow(color: Color(PanelColours.shadow), radius: 0, x: 0, y: 1)
            if item.active {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Color.accentColor.opacity(0.18))
            }
            glyph
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            if item.variants != nil {
                Image(systemName: "arrowtriangle.down.fill")
                    .font(.system(size: max(5, glyphSize * 0.3)))
                    .foregroundStyle(.secondary)
                    .padding(5)
            }
        }
        .foregroundStyle(!item.enabled ? Color(Palette.tertiary)
                         : item.active ? Color.accentColor : Color.primary)
        .contentShape(Rectangle())
    }

    @ViewBuilder
    private var glyph: some View {
        switch item.glyph {
        case .symbol(let name):
            Image(systemName: name).font(.system(size: glyphSize))
        case .text(let text):
            Text(text)
                .font(.system(size: glyphSize * 0.9, weight: .medium))
                .lineLimit(1)
                .minimumScaleFactor(0.6)
        }
    }

    /// A pressed formatting key takes the typing keys' grey and a pressed
    /// typing key the formatting keys' white — which is how the system's own
    /// keys answer a finger.
    private var fill: UIColor {
        switch (kind, pressed) {
        case (.format, false), (.typing, true): return PanelColours.formatKey
        case (.format, true), (.typing, false): return PanelColours.typingKey
        }
    }
}

/// The keyboard's key colours, light and dark. Not system colours by name,
/// because UIKit names none for a keyboard key; these are the values the
/// system keyboard draws with, over the `.keyboard` backdrop they are meant
/// for.
enum PanelColours {
    static let formatKey = UIColor { traits in
        traits.userInterfaceStyle == .dark ? UIColor(white: 0.42, alpha: 1) : .white
    }
    static let typingKey = UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(white: 0.27, alpha: 1)
            : UIColor(red: 0.67, green: 0.69, blue: 0.73, alpha: 1)
    }
    static let shadow = UIColor { traits in
        UIColor(white: 0, alpha: traits.userInterfaceStyle == .dark ? 0.6 : 0.3)
    }
}

/// A button key's press: the face redrawn pressed while the finger is down.
private struct PanelKeyStyle: ButtonStyle {
    let item: ToolItem
    let glyphSize: CGFloat

    func makeBody(configuration: Configuration) -> some View {
        PanelKeyFace(item: item, kind: .format, pressed: configuration.isPressed, glyphSize: glyphSize)
    }
}

/// Delete, Space, Return. These fire on touch-down rather than on release,
/// as the keyboard's keys do, and Delete repeats while held. They are a
/// gesture rather than a `Button` because a button acts on release, and a
/// Delete that waited for the finger to lift would feel broken to anyone
/// used to the keyboard.
///
/// The first press fires from the gesture's own `updating` callback, the
/// first time it sees the touch — not from `onChange(of: held)`. A tap quick
/// enough for SwiftUI to fold the touch's start and end into one update
/// never changes `held` as far as `onChange` can see, and a key that fired
/// only from there would drop it. `held` is what guards the callback, so it
/// fires once per touch.
///
/// "Held" is `@GestureState`, which SwiftUI resets when the touch ends *and*
/// when it is cancelled — a system gesture taking the touch, the panel going
/// away under the finger — where a flag set in `onChanged` and cleared in
/// `onEnded` stays set on a cancel, and a repeating Delete with it. The
/// repeat starts and stops on its changes, stops when the key leaves the
/// screen (the panel put away, the text view resigning), and checks it again
/// on every tick, so no path leaves a Delete repeating with no finger down.
private struct TypingKey: View {
    let symbol: String
    let label: String
    var repeats = false
    let size: CGSize
    let glyphSize: CGFloat
    let fire: () -> Void

    @GestureState private var held = false
    @State private var timer: Timer?

    var body: some View {
        PanelKeyFace(item: ToolItem(id: symbol, glyph: .symbol(symbol), label: label),
                     kind: .typing, pressed: held, glyphSize: glyphSize)
            .frame(width: size.width, height: size.height)
            .gesture(DragGesture(minimumDistance: 0).updating($held) { _, held, _ in
                guard !held else { return }
                held = true
                // A turn later: this runs inside the gesture's update, and the
                // keystroke publishes the editor's state.
                DispatchQueue.main.async { press() }
            })
            .onChange(of: held) { down in
                if down, repeats {
                    startRepeating()
                } else if !down {
                    stopRepeating()
                }
            }
            .onDisappear(perform: stopRepeating)
            .accessibilityElement()
            .accessibilityLabel(label)
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { press() }
    }

    private func press() {
        UIDevice.current.playInputClick()
        fire()
    }

    /// The keyboard's cadence: a pause long enough that a tap never repeats,
    /// then about eleven a second.
    private func startRepeating() {
        stopRepeating()
        let timer = Timer(fire: Date().addingTimeInterval(0.45), interval: 0.09, repeats: true) { _ in
            guard held else { return stopRepeating() }
            press()
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    private func stopRepeating() {
        timer?.invalidate()
        timer = nil
    }
}

/// Leaf's own Link field on iOS, raised by `LeafEditor` as a sheet over the
/// editor while `pendingLinkDestination` is set — from the row's Link and
/// the panel's alike. Seeded with the caret link's
/// destination, so it re-points a link as readily as it makes one.
struct LinkPromptHost: View {
    @ObservedObject var editor: LeafEditorModel

    var body: some View {
        Color.clear.sheet(isPresented: Binding(
            get: { editor.pendingLinkDestination != nil },
            set: { if !$0 { editor.pendingLinkDestination = nil } }
        )) {
            LinkPromptSheet(editor: editor, typed: editor.pendingLinkDestination ?? "")
        }
    }
}

private struct LinkPromptSheet: View {
    @ObservedObject var editor: LeafEditorModel
    @State var typed: String
    @FocusState private var focused: Bool

    var body: some View {
        LinkDestinationField(text: $typed, commit: commit)
            .focused($focused)
            .onAppear { DispatchQueue.main.async { focused = true } }
            .presentationDetents([.height(120)])
    }

    private func commit() {
        editor.pendingLinkDestination = nil
        editor.commitLinkDestination(typed)
    }
}

extension LeafTextView {
    /// Type `text` as the keyboard would, from the panel. The input delegate is
    /// told around it, as `command` tells it: the keyboard is not the one
    /// typing, so nothing else will say the text moved, and the system's text
    /// machinery would otherwise be left holding a stale selection when the
    /// keyboard comes back.
    func typeFromPanel(_ text: String) {
        commitMarkedText()
        inputDelegate?.selectionWillChange(self)
        inputDelegate?.textWillChange(self)
        insertText(text)
        inputDelegate?.textDidChange(self)
        inputDelegate?.selectionDidChange(self)
    }

    /// Delete backwards as the keyboard would, from the panel.
    func deleteFromPanel() {
        commitMarkedText()
        inputDelegate?.selectionWillChange(self)
        inputDelegate?.textWillChange(self)
        deleteBackward()
        inputDelegate?.textDidChange(self)
        inputDelegate?.selectionDidChange(self)
    }

    /// Keep an input method's composition as the text it is, and stop
    /// composing. `insertText` and `deleteBackward` both *replace* a marked
    /// range, which is right from the keyboard that owns the composition —
    /// it is committing or cancelling it — and wrong from the panel: a
    /// Japanese or Chinese phrase half-composed when `Aa` swapped the
    /// keyboard out would be replaced by a space, or erased whole by one
    /// Delete. Called as the panel goes up, and again before each of its
    /// keystrokes in case a composition began in between. The input delegate
    /// is told, so the keyboard drops its own idea of the composition too.
    func commitMarkedText() {
        guard markedTextRange != nil else { return }
        inputDelegate?.selectionWillChange(self)
        inputDelegate?.textWillChange(self)
        unmarkText()
        inputDelegate?.textDidChange(self)
        inputDelegate?.selectionDidChange(self)
    }
}
#endif
