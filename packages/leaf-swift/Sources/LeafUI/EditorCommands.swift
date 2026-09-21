//  EditorCommands.swift
//
//  The menu bar's half of the editor. A native Mac app puts Bold in a Format
//  menu, with its ⌘B beside it, and enables the item only when there is a
//  document to make bold; an iPad under a hardware keyboard shows the same menu
//  when ⌘ is held. `LeafEditor` publishes its model as the scene's focused
//  editor, and `LeafEditorCommands` reads it back to build those menus — so a
//  host adds one line, `.commands { LeafEditorCommands() }`, and the standard
//  menus reach whichever document its window is showing.
//
//  The items are the same public commands the toolbar's buttons call, with the
//  same shortcuts every other leaf frontend binds (see leaf-gpui's key map).
//  Checkmarks follow `EditorState`: Bold is ticked while the caret stands in
//  bold text, Source View while the source is showing. View ▸ Zoom follows the
//  model's `zoom`, which the surface's own pinch moves too.

import LeafFFI
import SwiftUI

/// The editor a scene's menus act on. Published by `LeafEditor`; read by
/// `LeafEditorCommands`, or by a host's own `Commands` through
/// `@FocusedValue(\.leafEditor)`.
public struct LeafEditorFocusKey: FocusedValueKey {
    public typealias Value = LeafEditorModel
}

extension FocusedValues {
    public var leafEditor: LeafEditorModel? {
        get { self[LeafEditorFocusKey.self] }
        set { self[LeafEditorFocusKey.self] = newValue }
    }
}

/// Format and View menu items for the focused `LeafEditor`. Add to a scene with
/// `.commands { LeafEditorCommands() }`.
///
/// Format replaces SwiftUI's standard text-formatting group (Font, Text — panels
/// that mean nothing to a document whose format spells its own emphasis) with
/// leaf's inline marks, block kinds, and list structure. View gains the
/// source/rendered toggle. Every item is disabled without a focused editor, or
/// on a reader.
public struct LeafEditorCommands: Commands {
    @FocusedValue(\.leafEditor) private var editor: LeafEditorModel?

    public init() {}

    public var body: some Commands {
        CommandGroup(replacing: .textFormatting) {
            if let editor {
                FormatMenuItems(editor: editor)
            } else {
                FormatMenuItems.placeholders
            }
        }
        #if os(macOS)
        // SwiftUI's default Edit menu stops at Select All: no Find, no Paste and
        // Match Style. Both are answered by the view, so both get their items.
        CommandGroup(after: .pasteboard) {
            Button(loc("menu.pasteAndMatchStyle", "Paste and Match Style")) { editor?.pasteAsPlainText() }
                .keyboardShortcut("v", modifiers: [.command, .option, .shift])
                .disabled(editor == nil || editor?.isReadOnly == true)
            Divider()
            Menu(loc("menu.find", "Find")) {
                Button(loc("menu.findEllipsis", "Find…")) { editor?.find(.showFindInterface) }
                    .keyboardShortcut("f", modifiers: .command)
                Button(loc("menu.findAndReplace", "Find and Replace…")) { editor?.find(.showReplaceInterface) }
                    .keyboardShortcut("f", modifiers: [.command, .option])
                    .disabled(editor?.isReadOnly == true)
                Button(loc("menu.findNext", "Find Next")) { editor?.find(.nextMatch) }
                    .keyboardShortcut("g", modifiers: .command)
                Button(loc("menu.findPrevious", "Find Previous")) { editor?.find(.previousMatch) }
                    .keyboardShortcut("g", modifiers: [.command, .shift])
                // ⌘E is leaf's Source View, on every frontend; this one goes by menu.
                Button(loc("menu.useSelectionForFind", "Use Selection for Find")) { editor?.find(.setSearchString) }
                Button(loc("menu.hideFindBar", "Hide Find Bar")) { editor?.find(.hideFindInterface) }
            }
            .disabled(editor == nil)
            Menu(loc("menu.spellingAndGrammar", "Spelling and Grammar")) {
                Button(loc("menu.checkDocumentNow", "Check Document Now")) {
                    editor?.checkSpelling()
                }
                Toggle(
                    loc("menu.checkSpellingWhileTyping", "Check Spelling While Typing"),
                    isOn: Binding(
                        get: { editor?.isContinuousSpellCheckingEnabled ?? true },
                        set: { _ in editor?.toggleContinuousSpellChecking() }))
            }
            .disabled(editor == nil)
        }
        #endif
        CommandGroup(after: .toolbar) {
            if let editor {
                ViewMenuItems(editor: editor)
            } else {
                ViewMenuItems.placeholders
            }
        }
    }
}

/// The Format menu's items, observing the editor so the checkmarks and the
/// enabled state follow the caret.
private struct FormatMenuItems: View {
    @ObservedObject var editor: LeafEditorModel

    private var editable: Bool { !editor.isReadOnly }

    var body: some View {
        mark("bold", loc("menu.bold", "Bold"), "b", .command) { editor.toggleBold() }
        mark("italic", loc("menu.italic", "Italic"), "i", .command) { editor.toggleItalic() }
        mark("underline", loc("menu.underline", "Underline"), "u", .command) { editor.toggleUnderline() }
        mark("strike", loc("menu.strikethrough", "Strikethrough"), nil, []) { editor.toggleStrike() }
        mark("code", loc("menu.code", "Code"), "c", [.command, .shift]) { editor.toggleCode() }
        mark("mark", loc("menu.highlight", "Highlight"), "m", [.command, .shift]) { editor.toggleMark() }
        // The colours a highlight can be, under the Highlight item that makes
        // one. A submenu rather than six more rows in a menu this long, and the
        // same rows the formatting bar's Highlight button drops down — one
        // definition, so the two cannot come to disagree about what the palette
        // is or when it is live.
        Menu(loc("menu.highlightColour", "Highlight Colour")) {
            HighlightColourRows(editor: editor)
        }
        .disabled(!editable)
        // The letters' own colour, directly under the highlight's: the two share
        // the seven names on purpose, and a menu that offered one without the
        // other beside it would read as two unrelated palettes.
        Menu(loc("menu.textColour", "Text Colour")) {
            TextColourRows(editor: editor)
        }
        .disabled(!editable)
        Divider()
        Toggle(loc("menu.paragraph", "Paragraph"), isOn: block(editor.state.heading == nil) { editor.setParagraph() })
            .keyboardShortcut("0", modifiers: .control)
            .disabled(!editable)
        Menu(loc("menu.heading", "Heading")) {
            ForEach(1...6, id: \.self) { level in
                Toggle(String(format: loc("menu.headingN", "Heading %d"), level),
                       isOn: block(editor.state.heading == UInt32(level)) { editor.setHeading(UInt32(level)) })
                    .keyboardShortcut(KeyEquivalent(Character(String(level))), modifiers: .control)
            }
        }
        .disabled(!editable)
        // The presentation vocabulary, under the block kinds it decorates and
        // above the list structure: how the block is laid, and how its letters
        // are set. The rows are the ones the formatting bar's own menus drop —
        // one definition, as with the highlight colours above — and the
        // shortcuts are the Mac's own, which is why the bar's copies don't
        // carry them.
        Menu(loc("menu.alignment", "Alignment")) {
            AlignmentRows(editor: editor, shortcuts: true)
        }
        .disabled(!editable)
        Menu(loc("menu.lineSpacing", "Line Spacing")) {
            LineSpacingRows(editor: editor)
        }
        .disabled(!editable)
        Menu(loc("menu.textSize", "Text Size")) {
            TextSizeRows(editor: editor, shortcuts: true)
        }
        .disabled(!editable)
        Menu(loc("menu.font", "Font")) {
            FontFamilyRows(editor: editor)
        }
        .disabled(!editable)
        Divider()
        Button(loc("menu.bulletList", "Bullet List")) { editor.toggleList(ordered: false) }
            .keyboardShortcut("8", modifiers: [.command, .shift])
            .disabled(!editable)
        Button(loc("menu.numberedList", "Numbered List")) { editor.toggleList(ordered: true) }
            .keyboardShortcut("7", modifiers: [.command, .shift])
            .disabled(!editable)
        Button(loc("menu.blockQuote", "Block Quote")) { editor.toggleBlockquote() }
            .keyboardShortcut("9", modifiers: [.command, .shift])
            .disabled(!editable)
        // Ticked while the caret stands in one, the way the marks above are:
        // this is the rich view's one door into a code block (a typed backtick
        // is escaped there), and a menu item that only ever said "make one"
        // would leave no sign that the caret was already in one.
        Toggle(loc("menu.codeBlock", "Code Block"),
               isOn: block(editor.state.codeBlock) { editor.toggleCodeBlock() })
            .keyboardShortcut("c", modifiers: [.command, .option])
            .disabled(!editable || !editor.capabilities.codeBlock)
        Button(loc("menu.indent", "Indent")) { editor.indent() }
            .keyboardShortcut("]", modifiers: .command)
            .disabled(!editable)
        Button(loc("menu.outdent", "Outdent")) { editor.outdent() }
            .keyboardShortcut("[", modifiers: .command)
            .disabled(!editable)
        Divider()
        Button(loc("menu.footnote", "Insert Footnote")) { editor.insertFootnote() }
            .disabled(!editable)
        Button(loc("menu.rule", "Insert Horizontal Rule")) { editor.insertThematicBreak() }
            .disabled(!editable)
        // Beside the rule: both write a block into the document rather than
        // restyling one, and this is the only one of the six new gestures that
        // does. Dimmed where the format spells no directive to carry it (HTML,
        // AsciiDoc) rather than failing on the press.
        Button(loc("menu.pageBreak", "Insert Page Break")) { editor.insertPageBreak() }
            .disabled(!editable || !editor.capabilities.pageBreak)
        // The same rows the formatting bar's Table button drops — one
        // definition, as with the highlight colours above.
        Menu(loc("menu.table", "Table")) {
            TableRows(editor: editor)
        }
        .disabled(!editable)
    }

    /// An inline mark: ticked while active at the caret.
    @ViewBuilder
    private func mark(_ id: String, _ title: String, _ key: Character?, _ modifiers: EventModifiers,
                      _ toggle: @escaping () -> Void) -> some View {
        let item = Toggle(title, isOn: Binding(get: { editor.isActive(id) }, set: { _ in toggle() }))
            .disabled(!editable)
        if let key {
            item.keyboardShortcut(KeyEquivalent(key), modifiers: modifiers)
        } else {
            item
        }
    }

    /// A block kind: ticked while the caret's block is it; choosing it sets it.
    private func block(_ on: Bool, _ set: @escaping () -> Void) -> Binding<Bool> {
        Binding(get: { on }, set: { _ in set() })
    }

    /// The same items, disabled, for a scene with no editor in focus — so the
    /// menu keeps its shape rather than emptying.
    @ViewBuilder
    static var placeholders: some View {
        Group {
            Toggle(loc("menu.bold", "Bold"), isOn: .constant(false)).keyboardShortcut("b", modifiers: .command)
            Toggle(loc("menu.italic", "Italic"), isOn: .constant(false)).keyboardShortcut("i", modifiers: .command)
            Toggle(loc("menu.underline", "Underline"), isOn: .constant(false)).keyboardShortcut("u", modifiers: .command)
            Toggle(loc("menu.strikethrough", "Strikethrough"), isOn: .constant(false))
            Toggle(loc("menu.code", "Code"), isOn: .constant(false)).keyboardShortcut("c", modifiers: [.command, .shift])
            Toggle(loc("menu.highlight", "Highlight"), isOn: .constant(false)).keyboardShortcut("m", modifiers: [.command, .shift])
            Menu(loc("menu.highlightColour", "Highlight Colour")) {
                ForEach(MarkColor.palette, id: \.self) { color in
                    Toggle(color.menuTitle, isOn: .constant(false))
                }
                Divider()
                Toggle(loc("menu.highlight.noColour", "No Colour"), isOn: .constant(false))
            }
            Menu(loc("menu.textColour", "Text Colour")) {
                ForEach(MarkColor.palette, id: \.self) { color in
                    Toggle(color.menuTitle, isOn: .constant(false))
                }
            }
            Divider()
            Toggle(loc("menu.paragraph", "Paragraph"), isOn: .constant(false)).keyboardShortcut("0", modifiers: .control)
            Menu(loc("menu.heading", "Heading")) {
                ForEach(1...6, id: \.self) { level in
                    Toggle(String(format: loc("menu.headingN", "Heading %d"), level), isOn: .constant(false))
                        .keyboardShortcut(KeyEquivalent(Character(String(level))), modifiers: .control)
                }
            }
            Menu(loc("menu.alignment", "Alignment")) {
                Toggle(loc("menu.align.left", "Left"), isOn: .constant(false))
                    .keyboardShortcut("{", modifiers: .command)
                Toggle(loc("menu.align.center", "Center"), isOn: .constant(false))
                    .keyboardShortcut("|", modifiers: .command)
                Toggle(loc("menu.align.right", "Right"), isOn: .constant(false))
                    .keyboardShortcut("}", modifiers: .command)
                Toggle(loc("menu.align.justify", "Justify"), isOn: .constant(false))
            }
            Menu(loc("menu.lineSpacing", "Line Spacing")) {
                Toggle(loc("menu.spacing.single", "Single"), isOn: .constant(false))
            }
            Menu(loc("menu.textSize", "Text Size")) {
                Button(loc("menu.size.bigger", "Bigger")) {}.keyboardShortcut("+", modifiers: .command)
                Button(loc("menu.size.smaller", "Smaller")) {}.keyboardShortcut("-", modifiers: .command)
            }
            Menu(loc("menu.font", "Font")) {
                ForEach(FontFamily.all, id: \.self) { face in
                    Toggle(face.title, isOn: .constant(false))
                }
            }
            Divider()
            Button(loc("menu.bulletList", "Bullet List")) {}.keyboardShortcut("8", modifiers: [.command, .shift])
            Button(loc("menu.numberedList", "Numbered List")) {}.keyboardShortcut("7", modifiers: [.command, .shift])
            Button(loc("menu.blockQuote", "Block Quote")) {}.keyboardShortcut("9", modifiers: [.command, .shift])
            Toggle(loc("menu.codeBlock", "Code Block"), isOn: .constant(false)).keyboardShortcut("c", modifiers: [.command, .option])
            Button(loc("menu.indent", "Indent")) {}.keyboardShortcut("]", modifiers: .command)
            Button(loc("menu.outdent", "Outdent")) {}.keyboardShortcut("[", modifiers: .command)
            Divider()
            Button(loc("menu.footnote", "Insert Footnote")) {}
            Button(loc("menu.rule", "Insert Horizontal Rule")) {}
            Button(loc("menu.pageBreak", "Insert Page Break")) {}
            Menu(loc("menu.table", "Table")) {
                Button(loc("menu.insertTable", "Insert Table")) {}
            }
        }
        .disabled(true)
    }
}

/// The View menu's items: the rendered/source toggle, ticked while the source
/// is showing, and the zoom — Pages' bindings, since ⌘+ and ⌘− are leaf's
/// Text Size on every frontend: ⌘> and ⌘< step the stops, ⌘0 is actual size,
/// and the two fits are ticked while they are the rule in force.
private struct ViewMenuItems: View {
    @ObservedObject var editor: LeafEditorModel

    var body: some View {
        Toggle(loc("menu.sourceView", "Source View"),
               isOn: Binding(get: { editor.isSource }, set: { _ in editor.toggleView() }))
            .keyboardShortcut("e", modifiers: .command)
        Divider()
        Menu(loc("menu.zoom", "Zoom")) {
            Button(loc("menu.zoomIn", "Zoom In")) { editor.zoomIn() }
                .keyboardShortcut(">", modifiers: .command)
                .disabled(editor.zoomScale >= Zoom.range.upperBound)
            Button(loc("menu.zoomOut", "Zoom Out")) { editor.zoomOut() }
                .keyboardShortcut("<", modifiers: .command)
                .disabled(editor.zoomScale <= Zoom.range.lowerBound)
            Divider()
            Toggle(loc("menu.actualSize", "Actual Size"),
                   isOn: Binding(get: { editor.zoom == .actualSize }, set: { _ in editor.actualSize() }))
                .keyboardShortcut("0", modifiers: .command)
            Toggle(loc("menu.fitWidth", "Fit Width"),
                   isOn: Binding(get: { editor.zoom == .fitWidth }, set: { _ in editor.zoom = .fitWidth }))
            Toggle(loc("menu.fitPage", "Fit Page"),
                   isOn: Binding(get: { editor.zoom == .fitPage }, set: { _ in editor.zoom = .fitPage }))
        }
    }

    /// The same items with no editor to act on, so the menu keeps its shape and
    /// its shortcuts stay listed.
    @ViewBuilder static var placeholders: some View {
        Toggle(loc("menu.sourceView", "Source View"), isOn: .constant(false))
            .keyboardShortcut("e", modifiers: .command)
            .disabled(true)
        Divider()
        Menu(loc("menu.zoom", "Zoom")) {
            Button(loc("menu.zoomIn", "Zoom In")) {}.keyboardShortcut(">", modifiers: .command)
            Button(loc("menu.zoomOut", "Zoom Out")) {}.keyboardShortcut("<", modifiers: .command)
            Divider()
            Toggle(loc("menu.actualSize", "Actual Size"), isOn: .constant(false))
                .keyboardShortcut("0", modifiers: .command)
            Toggle(loc("menu.fitWidth", "Fit Width"), isOn: .constant(false))
            Toggle(loc("menu.fitPage", "Fit Page"), isOn: .constant(false))
        }
        .disabled(true)
    }
}
