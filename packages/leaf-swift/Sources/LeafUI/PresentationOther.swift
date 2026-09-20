//  PresentationOther.swift
//
//  The four *Other…* surfaces — a number for a size, a number for a spacing,
//  the platform's font picker for a face, the system colour picker for an ink —
//  and the one place that raises them.
//
//  Why they are raised here and not from the row that asks for them: a `Menu`'s
//  rows exist only while the menu is open, and choosing one closes it. A
//  `.popover` or `.sheet` hung on a menu row is therefore dismissed by the very
//  press that would present it, and on the Mac the rows are also the *menu
//  bar's* — a window's chrome is not in that view hierarchy at all. So the rows
//  set `LeafEditorModel.pendingOther`, which is state on the document, and
//  `LeafEditor` — which is on screen under both surfaces — watches it. One
//  field, opened the same way from the formatting bar's menu and from
//  Format ▸ Text Size ▸ Other….
//
//  A popover on the Mac and a sheet on iOS, which is each platform's own answer
//  to "a handful of controls, over this document, now".
//
//  What the field does with what was typed is `PresentationEntry`, kept apart
//  from the views so it can be tested without standing one up — and because it
//  is the answer to a question the binding asks pointedly: from the other side
//  of the FFI a refusal and a clearing look alike (a value outside the
//  vocabulary writes nothing at all, and `nil` clears), so the field is what
//  has to tell the author which of the two they asked for.

import SwiftUI
import LeafFFI

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// Which of the four *Other…* rows has been pressed — what
/// `LeafEditorModel.pendingOther` holds.
public enum PresentationOther: String, Identifiable, Hashable, CaseIterable {
    case size, spacing, face, colour

    public var id: String { rawValue }

    /// The key a host translates this row's label under, and the English it
    /// falls back to. All four read *Other…* — Apple's own word for the row on
    /// a font-size popup that opens a field — and differ only in their key, so
    /// a host can word "a size" and "a colour" differently if its language
    /// wants to.
    var key: String { "menu.\(rawValue).other" }
    var label: String { loc(key, "Other\u{2026}") }

    /// The title over the field or picker this opens, which is the one place it
    /// says *which* property is being asked about — the row that opened it has
    /// gone by then, along with the menu it named.
    var fieldTitle: String {
        switch self {
        case .size: return loc("other.size.title", "Text Size")
        case .spacing: return loc("other.spacing.title", "Line Spacing")
        case .face: return loc("other.face.title", "Font")
        case .colour: return loc("other.colour.title", "Text Colour")
        }
    }
}

// MARK: - what the field makes of what was typed

/// The three answers a typed value can have, which are the three the binding
/// itself states: a value to write, the *absence* a value can legitimately
/// mean, and a refusal.
///
/// The same shape as leaf-ffi's own `Meant`, on this side of the binding and
/// for the reason that one exists: `setFontSize(.points(0))` and
/// `setFontSize(nil)` are a refusal and a clearing, they are indistinguishable
/// from the answer that comes back, and only the field that took the number can
/// tell the author which one they typed.
enum Meant<Value: Equatable>: Equatable {
    /// A value the vocabulary carries. Set writes it.
    case value(Value)
    /// A value that *means* the theme's own. Set clears the key — only a line
    /// spacing has one, and it is `1`, which is single.
    case absence
    /// A value outside the vocabulary. Set is dimmed, and nothing is written.
    case refused
}

/// What Set does with what the author typed in an *Other…* field.
///
/// The grammar is `PresentationValue`'s, which is core's: a plain decimal from
/// 0.01 to 655.35, no sign and no exponent, and a trailing bare point is a typo
/// rather than a whole number. Anything outside it is refused *here*, where the
/// author can see the button dim and fix the number, rather than in core, where
/// it would be a gesture that quietly wrote nothing.
enum PresentationEntry {
    /// A point size: `14`, `13.5`. Never an absence — the row that clears a
    /// size is *Default*, and an empty field is not a request for it.
    static func size(_ typed: String) -> Meant<FontSize> {
        guard let points = PresentationValue.number(typed.trimmingCharacters(in: .whitespaces))
        else { return .refused }
        return .value(.points(Double(points)))
    }

    /// A line-height ratio: `1.3`. **`1` is single spacing**, which is the
    /// theme's own and has no token of its own, so it clears the key — the one
    /// number in any of these fields that means absence rather than a value,
    /// and the rule core states for it.
    static func spacing(_ typed: String) -> Meant<LineHeight> {
        guard let ratio = PresentationValue.number(typed.trimmingCharacters(in: .whitespaces))
        else { return .refused }
        if ratio == 1 { return .absence }
        return .value(.ratio(Double(ratio)))
    }

    /// A colour the system picker is holding: `picked` is the triple behind it,
    /// `touched` is whether the picker has moved since the field opened, and
    /// `inForce` is the colour at the caret it was seeded from.
    ///
    /// **A picker nobody touched writes nothing.** The field opens seeded — with
    /// the exact colour at the caret where there is one, and with the ordinary
    /// ink where there is not — so Set on an untouched picker would write the
    /// seed, which for a named colour or for no colour at all is the primary
    /// ink: pressing the default action to dismiss a picker would turn the
    /// prose black. Nor does it rewrite a colour that is already in force: a
    /// picker walked around the wheel and back is a gesture that asks for
    /// nothing, and an undo step for it is one the author did not earn.
    static func colour(_ picked: TextColor?, touched: Bool,
                       inForce: TextColor?) -> Meant<TextColor> {
        guard touched, let picked, picked != inForce else { return .refused }
        return .value(picked)
    }
}

// MARK: - raising the field

extension View {
    /// Raise whichever *Other…* field `editor`'s menus have asked for — a
    /// popover on the Mac, a sheet on iOS. Applied once, by `LeafEditor`, over
    /// the editing surface itself.
    func presentingOtherValues(_ editor: LeafEditorModel) -> some View {
        PresentationOtherHost(editor: editor, content: self)
    }
}

/// The observer under `presentingOtherValues(_:)`. A view of its own because
/// the presentation is driven by a `@Published` on the model, and a binding to
/// one needs a view that is actually observing it.
private struct PresentationOtherHost<Content: View>: View {
    @ObservedObject var editor: LeafEditorModel
    let content: Content

    var body: some View {
        #if canImport(UIKit)
        content.sheet(item: $editor.pendingOther) { field in
            PresentationOtherView(editor: editor, field: field)
        }
        #else
        // From the surface's top edge, which is where the bar that opened it
        // is: the anchor a menu row would have given is gone with the menu.
        content.popover(item: $editor.pendingOther,
                        attachmentAnchor: .rect(.bounds),
                        arrowEdge: .top) { field in
            PresentationOtherView(editor: editor, field: field)
        }
        #endif
    }
}

/// The field or picker itself, whichever of the four was asked for.
struct PresentationOtherView: View {
    @ObservedObject var editor: LeafEditorModel
    let field: PresentationOther

    /// What the author has typed, for the two numeric fields — seeded with the
    /// value at the caret when there is an exact one, so re-opening the field
    /// from the ticked row above *Other…* shows what it is about to change.
    @State private var typed: String = ""
    /// What the colour picker is holding, likewise seeded.
    @State private var picked: Color = .primary
    /// Whether the picker has been moved since `seed()` put a colour in it —
    /// what tells Set apart from a Set the author never asked for. Written by
    /// the picker's own binding rather than watched with `onChange`, which is
    /// the same thing one platform version later.
    @State private var pickedSomething = false
    @FocusState private var focused: Bool
    #if !canImport(UIKit)
    /// Every family this machine has, read once when the picker opens.
    @State private var installedFamilies: [String] = []
    #endif

    var body: some View {
        Group {
            switch field {
            case .size, .spacing: numberField
            case .face: facePicker
            case .colour: colourPicker
            }
        }
        .onAppear(perform: seed)
        #if canImport(UIKit)
        // A sheet is the whole screen until it is told otherwise; these are a
        // few controls, and a form-sheet-sized one reads as a dialogue.
        .presentationDetents(field == .face ? [.large] : [.medium])
        #endif
    }

    // ── a number ─────────────────────────────────────────────────────────────

    /// Points, or a ratio, in a field the author types into — the pattern the
    /// bar's link destination already uses, down to the focus raised on the
    /// next runloop (the field does not exist to focus until the popover has
    /// been presented) and the default-action Set beside it.
    private var numberField: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(field.fieldTitle).font(.headline)
            HStack(spacing: 8) {
                TextField(placeholder, text: $typed)
                    .textFieldStyle(.roundedBorder)
                    .focused($focused)
                    .frame(width: 120)
                    .onSubmit(commitNumber)
                    .autocorrectionDisabled()
                    #if canImport(UIKit)
                    .keyboardType(.decimalPad)
                    .textInputAutocapitalization(.never)
                    #endif
                if field == .size { Text(loc("other.size.unit", "pt")).foregroundStyle(.secondary) }
            }
            Text(hint).font(.caption).foregroundStyle(.secondary)
            HStack {
                Button(loc("other.cancel", "Cancel")) { editor.pendingOther = nil }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button(loc("other.set", "Set"), action: commitNumber)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!wouldWrite)
            }
        }
        .padding(16)
        .frame(minWidth: 260)
        .onAppear { DispatchQueue.main.async { focused = true } }
    }

    private var placeholder: String { field == .size ? "14" : "1.3" }

    /// Whether Set would write anything. The button dims on a number the
    /// vocabulary cannot carry — 0, 700, `abc` — which is where an author can
    /// still see the typo: past this point a refusal is a gesture that quietly
    /// writes nothing, and looks exactly like a clearing.
    private var wouldWrite: Bool {
        switch field {
        case .size: return PresentationEntry.size(typed) != .refused
        case .spacing: return PresentationEntry.spacing(typed) != .refused
        default: return true
        }
    }

    private var hint: String {
        switch field {
        case .size:
            return loc("other.size.hint",
                       "Points of the page \u{2014} 0.01 to 655.35. An exact size "
                           + "replaces the size a heading or a step would give.")
        default:
            return loc("other.spacing.hint",
                       "A multiple of the theme\u{2019}s line height \u{2014} 0.5 to "
                           + "655.35. 1 is single spacing, which is the theme\u{2019}s own. "
                           + "Anything tighter than half is drawn at half, which is as "
                           + "close as lines lay.")
        }
    }

    private func commitNumber() {
        switch field {
        case .size:
            guard case let .value(size) = PresentationEntry.size(typed) else { return }
            editor.setFontSize(size)
        case .spacing:
            switch PresentationEntry.spacing(typed) {
            case let .value(height): editor.setLineSpacing(height)
            case .absence: editor.setLineSpacing(nil)
            case .refused: return
            }
        default: return
        }
        editor.pendingOther = nil
    }

    // ── a face ───────────────────────────────────────────────────────────────

    #if canImport(UIKit)
    /// iOS: the system's own picker, which is the list every other app on the
    /// phone shows and knows about the families installed since this build.
    /// Families rather than faces — `data-font` names a family, and a weight
    /// the document could not carry would be a picker offering to write
    /// something that gets dropped.
    private var facePicker: some View {
        FontFamilyPicker { family in
            if let family { editor.setFontFamily(.named(family)) }
            editor.pendingOther = nil
        }
        .ignoresSafeArea()
    }
    #else
    /// macOS: a searchable list of the installed families, rather than
    /// `NSFontPanel`.
    ///
    /// The panel is four columns — family, typeface, size, effects — and
    /// `data-font` is one of them. A panel that offered a weight and a size the
    /// document cannot carry would be offering to write things leaf will drop,
    /// and it is a floating window that outlives the press that opened it, with
    /// a `changeFont(_:)` action that has to be answered from a responder chain
    /// this package does not own. A list of families is exactly the vocabulary,
    /// in the popover the press asked for.
    private var facePicker: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(field.fieldTitle).font(.headline)
            TextField(loc("other.face.search", "Search"), text: $typed)
                .textFieldStyle(.roundedBorder)
                .focused($focused)
            List(families, id: \.self) { family in
                Button {
                    editor.setFontFamily(.named(family))
                    editor.pendingOther = nil
                } label: {
                    Text(family)
                        .font(.custom(family, size: NSFont.systemFontSize))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .frame(width: 280, height: 320)
            HStack {
                Button(loc("other.cancel", "Cancel")) { editor.pendingOther = nil }
                    .keyboardShortcut(.cancelAction)
                Spacer()
            }
        }
        .padding(16)
        .onAppear {
            installedFamilies = NSFontManager.shared.availableFontFamilies
            DispatchQueue.main.async { focused = true }
        }
    }

    /// The families in `installedFamilies`, narrowed by what has been typed.
    ///
    /// Read from the font manager once, when the picker appears, rather than on
    /// every keystroke: `body` is evaluated on each character typed, and asking
    /// the manager to enumerate every family installed on the machine each time
    /// is work a search field does between one letter and the next. A font
    /// installed while this popover is open is found by closing and re-opening
    /// it, which is what every other font list on the machine asks for too.
    private var families: [String] {
        let query = typed.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return installedFamilies }
        return installedFamilies.filter { $0.range(of: query, options: .caseInsensitive) != nil }
    }
    #endif

    // ── a colour ─────────────────────────────────────────────────────────────

    /// The system colour picker — `NSColorPanel` behind it on the Mac and the
    /// system picker on iOS, which is what `ColorPicker` is on each.
    ///
    /// Converted to an 8-bit sRGB triple on Set, because that is the whole of
    /// what `data-color` carries: `#c03030`, six lowercase digits, painted as
    /// written in both appearances. Opacity is off — a document's ink has none.
    private var colourPicker: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(field.fieldTitle).font(.headline)
            // Through a binding of its own rather than `$picked`, so that the
            // picker *writing* a colour is what marks one as chosen: seeding is
            // not choosing, and Set has to be able to tell the two apart.
            ColorPicker(loc("other.colour.pick", "Colour"),
                        selection: Binding(get: { picked },
                                           set: { picked = $0; pickedSomething = true }),
                        supportsOpacity: false)
            Text(loc("other.colour.hint",
                     "An exact colour is painted as written in both the light and the "
                         + "dark appearance. A named colour has one of each."))
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button(loc("other.cancel", "Cancel")) { editor.pendingOther = nil }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button(loc("other.set", "Set"), action: commitColour)
                    .keyboardShortcut(.defaultAction)
                    .disabled(chosenColour == .refused)
            }
        }
        .padding(16)
        .frame(minWidth: 280)
    }

    /// What Set would write — a colour the author picked, or a refusal for a
    /// picker they never touched and for one holding the colour already in
    /// force. Set dims on the refusal, the way the number field's does.
    private var chosenColour: Meant<TextColor> {
        PresentationEntry.colour(srgb(picked).map { .rgb(r: $0.0, g: $0.1, b: $0.2) },
                                 touched: pickedSomething, inForce: editor.textColor)
    }

    private func commitColour() {
        guard case let .value(colour) = chosenColour else { return }
        editor.setTextColor(colour)
        editor.pendingOther = nil
    }

    // ── seeding ──────────────────────────────────────────────────────────────

    /// Open showing what is in force, where there is anything exact to show —
    /// so the ticked row above *Other…* and the field it re-opens agree, and so
    /// "make it a point bigger" is an edit rather than a retype.
    private func seed() {
        switch field {
        case .size:
            typed = editor.fontSize?.pointsOnly.map(PresentationValue.spell) ?? ""
        case .spacing:
            typed = editor.lineSpacing?.ratioOnly.map(PresentationValue.spell) ?? ""
        case .face:
            typed = ""
        case .colour:
            // Seeded from an exact triple where there is one. A *named* colour
            // is deliberately not converted into a triple to open with: the
            // theme owns both of its inks, and handing the picker one of them
            // would turn "red under this theme" into a hex the moment the
            // author pressed Set on a colour they had not changed.
            picked = (editor.textColor?.hexOnly)
                .flatMap { PresentationValue.ink(color: $0) }
                .map(Color.init) ?? .primary
        }
    }
}

/// The 8-bit sRGB triple behind a SwiftUI colour, or nil for one that has no
/// place in that space (a pattern, a catalogue colour the system cannot
/// convert) — which is a colour a `#rrggbb` cannot spell either.
func srgb(_ color: Color) -> (UInt8, UInt8, UInt8)? {
    var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
    #if canImport(UIKit)
    guard UIColor(color).getRed(&r, green: &g, blue: &b, alpha: &a) else { return nil }
    #elseif canImport(AppKit)
    guard let converted = NSColor(color).usingColorSpace(.sRGB) else { return nil }
    converted.getRed(&r, green: &g, blue: &b, alpha: &a)
    #endif
    let byte = { (value: CGFloat) in UInt8(min(max(value, 0), 1) * 255 + 0.5) }
    return (byte(r), byte(g), byte(b))
}

#if canImport(UIKit)
/// `UIFontPickerViewController`, as a SwiftUI view. Configured for *families*:
/// `includeFaces` off, so what comes back is the name a `data-font` carries.
struct FontFamilyPicker: UIViewControllerRepresentable {
    let picked: (String?) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(picked: picked) }

    func makeUIViewController(context: Context) -> UIFontPickerViewController {
        let config = UIFontPickerViewController.Configuration()
        config.includeFaces = false
        let picker = UIFontPickerViewController(configuration: config)
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ picker: UIFontPickerViewController, context: Context) {}

    final class Coordinator: NSObject, UIFontPickerViewControllerDelegate {
        let picked: (String?) -> Void
        init(picked: @escaping (String?) -> Void) { self.picked = picked }

        func fontPickerViewControllerDidPickFont(_ picker: UIFontPickerViewController) {
            let family = picker.selectedFontDescriptor?
                .object(forKey: .family) as? String
            picked(family)
        }

        func fontPickerViewControllerDidCancel(_ picker: UIFontPickerViewController) {
            picked(nil)
        }
    }
}
#endif
