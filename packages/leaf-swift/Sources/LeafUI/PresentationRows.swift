//  PresentationRows.swift
//
//  The menu rows for the presentation vocabulary — alignment, size, face,
//  spacing, and the text colour — each its own `View` for the reason
//  `HighlightColourRows` and `TableRows` are: two surfaces show the same rows
//  (the formatting bar's menus and the app's Format menu), and one definition is
//  what keeps them from coming to disagree about what the vocabulary is or when
//  it is live.
//
//  Every group ticks the value at the caret and offers a "Default" row that
//  clears the key — the vocabulary has no `left`, no `single`, no `medium` and
//  no ink of its own, because absence is what those mean and a document should
//  not carry a key that says nothing.
//
//  Four of the five offer the *names first* and, under a divider, an "Other…"
//  row that opens a field or a platform picker for the exact form — 14pt,
//  Garamond, #c03030, 1.3. A name is portable under every theme and a value is
//  exact, and the row is labelled so the trade is visible (Apple's own word for
//  the row on a font-size popup that opens a field). Above it, when the caret
//  stands in an exact value, a ticked row of that value's own, so the menu
//  always shows what is in force. Alignment has no such row: left, centre,
//  right and justify are the whole of what a line can do.
//
//  Pressing Other… sets `LeafEditorModel.pendingOther` rather than presenting
//  anything here — a `Menu`'s rows are torn down by the press that chooses one,
//  so a popover hung on a row would be dismissed by the gesture that asked for
//  it. `LeafEditor` watches that and raises the field (`PresentationOther.swift`).
//
//  Each group dims on its own capability flag: a format that cannot spell the
//  property at all (XML spells none of them; AsciiDoc refuses the three
//  run-level ones inline) gets a dimmed control rather than a press that turns
//  into a refusal. That is the same bargain the Table tool and the highlight
//  palette already make.

import LeafFFI
import SwiftUI

/// Centre, Right, Justify, and Left — where Left is *clearing* the key, since
/// the vocabulary has no `left` token and absence is the theme's default.
///
/// `shortcuts` adds ⌘{ ⌘| ⌘}, the Mac's own alignment chords: true in the menu
/// bar, which is where a shortcut belongs, and false in the bar's own menu, so
/// the same chord isn't registered twice by two views of one list.
struct AlignmentRows: View {
    @ObservedObject var editor: LeafEditorModel
    var shortcuts = false

    var body: some View {
        Group {
            row(loc("menu.align.left", "Left"), on: editor.alignment == nil, key: "{") {
                editor.setAlignment(nil)
            }
            ForEach(Align.all, id: \.self) { align in
                row(align.title, on: editor.alignment == align,
                    key: align == .center ? "|" : align == .right ? "}" : nil) {
                    editor.setAlignment(align)
                }
            }
        }
        .disabled(!editor.capabilities.alignment)
    }

    @ViewBuilder
    private func row(_ title: String, on: Bool, key: Character?,
                     _ apply: @escaping () -> Void) -> some View {
        let item = Toggle(title, isOn: Binding(get: { on }, set: { _ in apply() }))
        if shortcuts, let key {
            item.keyboardShortcut(KeyEquivalent(key), modifiers: .command)
        } else {
            item
        }
    }
}

/// The seven size steps, plus the theme's own size, plus Bigger and Smaller —
/// which walk the same ramp one rung at a time and are the only rows here with
/// a shortcut worth binding (⌘+ / ⌘-, what a Mac text editor has always used for
/// this; the chord is ⌘⇧+ and ⌘- on a US keyboard, which is how the system
/// draws them).
struct TextSizeRows: View {
    @ObservedObject var editor: LeafEditorModel
    var shortcuts = false

    var body: some View {
        Group {
            if shortcuts {
                Button(loc("menu.size.bigger", "Bigger")) { editor.stepFontSize(up: true) }
                    .keyboardShortcut("+", modifiers: .command)
                Button(loc("menu.size.smaller", "Smaller")) { editor.stepFontSize(up: false) }
                    .keyboardShortcut("-", modifiers: .command)
                Divider()
            }
            // Largest first: a size menu reads as a ramp, and a ramp that runs
            // downwards is the one every font panel draws.
            ForEach(SizeStep.ramp.reversed(), id: \.self) { step in
                toggle(step.title, on: editor.fontSize == .step(step)) {
                    editor.setFontSize(.step(step))
                }
            }
            Divider()
            toggle(loc("menu.size.default", "Default"), on: editor.fontSize == nil) {
                editor.setFontSize(nil)
            }
            OtherRow(editor: editor, field: .size, exact: editor.fontSize?.exactTitle)
        }
        .disabled(!editor.capabilities.fontSize)
    }

    private func toggle(_ title: String, on: Bool, _ apply: @escaping () -> Void) -> some View {
        Toggle(title, isOn: Binding(get: { on }, set: { _ in apply() }))
    }
}

/// The four generic faces, plus the theme's body face. Which type each generic
/// actually is, is `EditorTheme.fontFamilies`' answer — the document names a
/// kind of face, never a family, so it opens the same way on a machine that has
/// none of the same fonts.
struct FontFamilyRows: View {
    @ObservedObject var editor: LeafEditorModel

    var body: some View {
        Group {
            ForEach(FontFamily.all, id: \.self) { face in
                toggle(face.title, on: editor.fontFamily == .generic(face)) {
                    editor.setFontFamily(.generic(face))
                }
            }
            Divider()
            toggle(loc("menu.font.default", "Default"), on: editor.fontFamily == nil) {
                editor.setFontFamily(nil)
            }
            OtherRow(editor: editor, field: .face, exact: editor.fontFamily?.exactTitle)
        }
        .disabled(!editor.capabilities.fontFamily)
    }

    private func toggle(_ title: String, on: Bool, _ apply: @escaping () -> Void) -> some View {
        Toggle(title, isOn: Binding(get: { on }, set: { _ in apply() }))
    }
}

/// The word processor's spacing menu: single (which is clearing the key), 1.15,
/// 1.5, and double.
struct LineSpacingRows: View {
    @ObservedObject var editor: LeafEditorModel

    var body: some View {
        Group {
            toggle(loc("menu.spacing.single", "Single"), on: editor.lineSpacing == nil) {
                editor.setLineSpacing(nil)
            }
            ForEach(LineSpacing.all, id: \.self) { spacing in
                toggle(spacing.title, on: editor.lineSpacing == .step(spacing)) {
                    editor.setLineSpacing(.step(spacing))
                }
            }
            OtherRow(editor: editor, field: .spacing, exact: editor.lineSpacing?.exactTitle)
        }
        .disabled(!editor.capabilities.lineSpacing)
    }

    private func toggle(_ title: String, on: Bool, _ apply: @escaping () -> Void) -> some View {
        Toggle(title, isOn: Binding(get: { on }, set: { _ in apply() }))
    }
}

/// The colour of the *letters*, in the same seven names — and with the same
/// swatches — a highlight's wash is spelled with. Beside `HighlightColourRows`
/// in every menu that offers both, because "red" should mean one red.
struct TextColourRows: View {
    @ObservedObject var editor: LeafEditorModel

    var body: some View {
        Group {
            ForEach(MarkColor.palette, id: \.self) { colour in
                toggle(colour.menuTitle, on: editor.textColor == .named(colour)) {
                    editor.setTextColor(.named(colour))
                }
            }
            Divider()
            toggle(loc("menu.textColour.none", "Default"), on: editor.textColor == nil) {
                editor.setTextColor(nil)
            }
            OtherRow(editor: editor, field: .colour, exact: editor.textColor?.exactTitle)
        }
        .disabled(!editor.capabilities.textColor)
    }

    private func toggle(_ title: String, on: Bool, _ apply: @escaping () -> Void) -> some View {
        Toggle(title, isOn: Binding(get: { on }, set: { _ in apply() }))
    }
}

/// The exact half of one group: a ticked row showing the value at the caret
/// when it is an exact one, and *Other…* under it.
///
/// One view for all four because they differ only in which field the press
/// opens and how the value spells itself, and because the ticked row has a rule
/// worth writing once: it is shown **only** when there is an exact value, since
/// a permanent row reading "14 pt" over a document set in none of it would be
/// offering a size nobody chose. Pressing the ticked row re-opens the field
/// seeded with that value, which is the only sensible thing a ticked row can do
/// — it is already what is in force, so there is nothing to apply.
struct OtherRow: View {
    @ObservedObject var editor: LeafEditorModel
    let field: PresentationOther
    /// The value at the caret, spelled, or nil when it is a name or nothing.
    let exact: String?

    var body: some View {
        Divider()
        if let exact {
            Toggle(exact, isOn: Binding(get: { true }, set: { _ in editor.pendingOther = field }))
        }
        Button(field.label) { editor.pendingOther = field }
    }
}
