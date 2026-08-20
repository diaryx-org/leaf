//  FormattingToolbar.swift
//
//  The formatting bar — one horizontally scrolling row of tools, grouped by kind
//  (inline marks · block structure · indent · history) with a hairline between
//  groups. Every action here already exists on `LeafEditorModel`, so this is
//  wiring rather than new editing capability; a host that wants a different
//  arrangement can still build its own against the same public commands.
//
//  It ships in two shapes, because the two platforms hang it in different
//  places. On iOS it's a keyboard accessory, floated above the soft keyboard by
//  `LeafEditor(model:accessory:)`; on macOS there's no soft keyboard to float
//  above, so it's a static strip the host stacks over the editor. Only the
//  metrics differ — same tools, same order, same state bindings — so the two
//  live here as a `Style` rather than as two files that would drift.
//
//  The iOS bar was once a paged TabView (three pages, swipe or tap a dot). It
//  read badly at accessory height: `.page` reserves a strip of its own frame for
//  the dot indicator, so inside a 44pt bar the dots and the 34pt buttons fought
//  over the same points and both got clipped. Paging also hid two thirds of the
//  tools behind a gesture with no affordance once the dots were gone. A scroll
//  row shows the first group in full, hints at the next, and can't clip — and it
//  earns its keep on macOS too, where a narrow window would otherwise squeeze
//  the groups.
//
//  The buttons are bare glyphs rather than filled capsules: the bar already sits
//  on its own `.bar` material, and a row of capsules on top of that reads as
//  chrome stacked on chrome. It also gives the active state somewhere to go — an
//  accent-tinted pill behind the glyph, which a bordered button's tint could
//  barely express.
//
//  Link is the one tool that can't be a bare command: every other button here
//  knows everything it needs from the selection, and a link needs a destination
//  from outside it. `LeafEditorModel.onEditLink` is where a host answers that
//  question for the context menu's "Edit Link…", and this asks through the same
//  hook so an app resolving `id:6tzwsxg` gets its own document picker from the
//  toolbar too — with a plain field of the bar's own as the fallback, because a
//  ready-made toolbar whose Link button does nothing until you wire a callback
//  isn't ready-made.

import SwiftUI

/// A ready-made formatting bar over a `LeafEditorModel`.
///
///     // macOS: a strip above the editor.
///     VStack(spacing: 0) {
///         LeafFormattingToolbar(editor: editor)
///         Divider()
///         LeafEditor(model: editor)
///     }
///
///     // iOS: the same tools, above the keyboard.
///     LeafEditor(model: editor) { LeafFormattingToolbar(editor: editor) }
public struct LeafFormattingToolbar: View {
    /// Which shape the bar takes. `.automatic` resolves to `.accessory` on iOS
    /// and `.bar` on macOS, which is what a host wants unless it's deliberately
    /// putting the iOS-sized bar somewhere other than above the keyboard.
    public enum Style {
        /// Keyboard-accessory metrics: 44pt tall, finger-sized targets.
        case accessory
        /// Static-strip metrics: 32pt tall, pointer-sized targets.
        case bar
        /// The platform's usual choice.
        case automatic
    }

    @ObservedObject private var editor: LeafEditorModel
    private let style: Style

    /// The reader's Dynamic Type setting, read as a bare multiplier: SwiftUI
    /// resizes this 1 the way it would resize a body-styled length, so dividing
    /// out the seed leaves the factor the whole bar is measured by. Inert on
    /// macOS, which has no Dynamic Type and reports `.large` forever.
    ///
    /// A probe rather than `.font(.body)` on the glyphs because the bar is a row
    /// of *targets*, not text: the tap area, the pill behind it, and the row's
    /// own height all have to move together with the glyph, and only a number
    /// can be handed to `frame(width:height:)`.
    @ScaledMetric(relativeTo: .body) private var typeScale: CGFloat = 1

    /// The fallback destination field: shown only when no host has claimed the
    /// question (see `askForDestination`), seeded with the caret link's current
    /// destination so the button re-points a link as readily as it makes one.
    @State private var askingForDestination = false
    @State private var typedDestination = ""
    @FocusState private var destinationFocused: Bool

    public init(editor: LeafEditorModel, style: Style = .automatic) {
        self.editor = editor
        self.style = style
    }

    public var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: metrics.spacing) {
                inlineMarks
                separator
                blockStyles
                separator
                indentTools
                separator
                historyTools
            }
            .padding(.horizontal, metrics.edgePadding)
        }
        .frame(height: metrics.barHeight)
        .background(.bar)
    }

    // MARK: tool groups

    private var inlineMarks: some View {
        Group {
            tool("bold", "Bold", active: editor.isActive("bold")) { editor.toggleBold() }
            tool("italic", "Italic", active: editor.isActive("italic")) { editor.toggleItalic() }
            tool("underline", "Underline", active: editor.isActive("underline")) { editor.toggleUnderline() }
            tool("strikethrough", "Strikethrough", active: editor.isActive("strike")) { editor.toggleStrike() }
            tool("chevron.left.forwardslash.chevron.right", "Code", active: editor.isActive("code")) { editor.toggleCode() }
            linkTool
        }
    }

    /// Link, among the inline marks rather than beside the rule and the footnote:
    /// it is applied *over the selection* the way bold is, and it is the only
    /// other tool here with something to light up — the caret standing in a link.
    ///
    /// The pill reads `state.link`, which rides the frame precisely so this can:
    /// walking the caret out of a link changes no mark and no heading, so a
    /// button that asked core directly would keep a stale light (see
    /// `EditorState.link`).
    private var linkTool: some View {
        tool("link", "Link", active: editor.state.link != nil) { beginLink() }
            .popover(isPresented: $askingForDestination) { destinationField }
    }

    private var blockStyles: some View {
        Group {
            textTool("H1", "Heading 1", active: editor.state.heading == 1) { editor.setHeading(1) }
            textTool("H2", "Heading 2", active: editor.state.heading == 2) { editor.setHeading(2) }
            tool("paragraphsign", "Body text", active: editor.state.heading == nil) { editor.setParagraph() }
            tool("quote.opening", "Quote") { editor.toggleBlockquote() }
            tool("list.bullet", "Bulleted list") { editor.toggleList(ordered: false) }
            tool("list.number", "Numbered list") { editor.toggleList(ordered: true) }
            tool("rectangle.compress.vertical", "Horizontal Rule") { editor.insertThematicBreak() }
            // Beside the rule rather than among the inline marks: a footnote is
            // not a mark over the selection, it's a thing written into the
            // document — and like the rule it acts once rather than toggling, so
            // it has no active state to show. The glyph is the raised character
            // because that is what the gesture puts on screen; if the eight
            // inline marks ever grow a Superscript button of their own, that one
            // takes this symbol and this takes `asterisk`.
            tool("textformat.superscript", "Footnote") { editor.insertFootnote() }
        }
    }

    private var indentTools: some View {
        Group {
            tool("increase.indent", "Indent") { editor.indent() }
            tool("decrease.indent", "Outdent") { editor.outdent() }
        }
    }

    private var historyTools: some View {
        Group {
            tool("arrow.uturn.backward", "Undo") { editor.undo() }
            tool("arrow.uturn.forward", "Redo") { editor.redo() }
        }
    }

    // MARK: the link destination

    /// Ask for a destination and link with it. The host's `onEditLink` gets first
    /// refusal — it is already the answer to this question everywhere else in the
    /// package, and only a host can offer a picker for the destinations that
    /// aren't URLs — and the bar's own field stands in when there is no host
    /// listening.
    ///
    /// Seeded from the caret's link either way, so pressing this inside one
    /// re-points it rather than nesting a second link in its text; empty
    /// elsewhere, which is `insertLink`'s "make one" case.
    private func beginLink() {
        let current = editor.state.link ?? ""
        if let ask = editor.onEditLink {
            ask(current)
            return
        }
        typedDestination = current
        askingForDestination = true
        // Raised on the next runloop: the field doesn't exist to focus until the
        // popover has been presented.
        DispatchQueue.main.async { destinationFocused = true }
    }

    /// Commit what was typed. An empty destination cancels rather than writing
    /// `[text]()` — core would take it, and a link that points nowhere is never
    /// what the empty field meant.
    private func commitLink() {
        askingForDestination = false
        let destination = typedDestination.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !destination.isEmpty else { return }
        editor.insertLink(destination)
    }

    private var destinationField: some View {
        HStack(spacing: 8) {
            TextField("https://\u{2026}", text: $typedDestination)
                .textFieldStyle(.roundedBorder)
                .focused($destinationFocused)
                .frame(width: 260)
                .onSubmit(commitLink)
                .autocorrectionDisabled()
                #if canImport(UIKit)
                // A destination is not prose: iOS capitalizing the first letter
                // of a URL is a wrong answer every time.
                .textInputAutocapitalization(.never)
                .keyboardType(.URL)
                #endif
            Button("Link", action: commitLink)
                .keyboardShortcut(.defaultAction)
        }
        .padding(12)
    }

    // MARK: metrics

    /// The per-style sizes. Everything that differs between the keyboard
    /// accessory and the desktop strip is here, so the tool definitions above
    /// stay written once.
    private struct Metrics {
        var barHeight: CGFloat
        var buttonWidth: CGFloat
        var buttonHeight: CGFloat
        var glyphSize: CGFloat
        var labelSize: CGFloat
        var cornerRadius: CGFloat
        var spacing: CGFloat
        var edgePadding: CGFloat
        var separatorHeight: CGFloat

        /// 44 is the tap-target floor, and the row spends all of it — there's no
        /// indicator strip to leave room for any more.
        static let accessory = Metrics(
            barHeight: 44, buttonWidth: 40, buttonHeight: 36,
            glyphSize: 17, labelSize: 15, cornerRadius: 8,
            spacing: 2, edgePadding: 8, separatorHeight: 22
        )

        /// A pointer hits a much smaller target than a fingertip, and the strip
        /// competes with the document for vertical space in a way a keyboard
        /// accessory never does — so this is roughly a system toolbar's height.
        static let bar = Metrics(
            barHeight: 32, buttonWidth: 26, buttonHeight: 24,
            glyphSize: 13, labelSize: 12, cornerRadius: 5,
            spacing: 1, edgePadding: 8, separatorHeight: 16
        )

        /// How far Dynamic Type is allowed to take the bar. The tools scale like
        /// everything else up to here and then stop, because this row is chrome
        /// that has to *share* the screen with the keyboard below it and the
        /// document above: taken to AX5's ~3.1× a 44pt accessory becomes a 137pt
        /// slab, which buys a reader nothing they couldn't already get by
        /// scrolling the row sideways, and costs them the four lines of their own
        /// text that used to be visible while they typed.
        ///
        /// 1.6 lands the accessory near 70pt — a comfortably oversized target,
        /// still a bar. Note the *document* is deliberately not capped this way:
        /// prose is the content, and content scales as far as the reader asks.
        static let maxTypeScale: CGFloat = 1.6

        /// Every length here multiplied by `factor` — one scale for the whole bar,
        /// so the glyph, the target it sits in, and the row's height stay in the
        /// proportion they were drawn in.
        func scaled(by factor: CGFloat) -> Metrics {
            guard factor != 1 else { return self }
            var m = self
            m.barHeight *= factor
            m.buttonWidth *= factor
            m.buttonHeight *= factor
            m.glyphSize *= factor
            m.labelSize *= factor
            m.cornerRadius *= factor
            m.spacing *= factor
            m.edgePadding *= factor
            m.separatorHeight *= factor
            return m
        }
    }

    private var metrics: Metrics {
        base.scaled(by: min(typeScale, Metrics.maxTypeScale))
    }

    /// The style's own sizes, before Dynamic Type is applied.
    private var base: Metrics {
        switch style {
        case .accessory: return .accessory
        case .bar: return .bar
        case .automatic:
            #if canImport(UIKit)
            return .accessory
            #else
            return .bar
            #endif
        }
    }

    // MARK: shared chrome

    /// A hairline between groups, inset from the bar's edges so it reads as a
    /// separator rather than a border.
    private var separator: some View {
        Divider()
            .frame(height: metrics.separatorHeight)
            .padding(.horizontal, 6)
    }

    private func tool(
        _ systemImage: String,
        _ label: String,
        active: Bool = false,
        action: @escaping () -> Void
    ) -> some View {
        button(active: active, label: label, action: action) {
            Image(systemName: systemImage)
                .font(.system(size: metrics.glyphSize))
        }
    }

    private func textTool(
        _ text: String,
        _ label: String,
        active: Bool = false,
        action: @escaping () -> Void
    ) -> some View {
        button(active: active, label: label, action: action) {
            Text(text)
                .font(.system(size: metrics.labelSize, weight: .semibold))
        }
    }

    /// The shared button body: a style-sized tap target, accent glyph plus a
    /// tinted pill when the mark is active under the caret.
    private func button<Glyph: View>(
        active: Bool,
        label: String,
        action: @escaping () -> Void,
        @ViewBuilder glyph: () -> Glyph
    ) -> some View {
        Button(action: action) {
            glyph()
                .frame(width: metrics.buttonWidth, height: metrics.buttonHeight)
                .foregroundStyle(active ? Color.accentColor : Color.primary)
                .background(
                    RoundedRectangle(cornerRadius: metrics.cornerRadius)
                        .fill(active ? Color.accentColor.opacity(0.15) : Color.clear)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        // A pointer can hover; a fingertip can't. On iOS `.help` lands as an
        // accessibility hint, which duplicates the label above — so the tooltip
        // is desktop-only rather than unconditional.
        #if !canImport(UIKit)
        .help(label)
        #endif
    }
}
