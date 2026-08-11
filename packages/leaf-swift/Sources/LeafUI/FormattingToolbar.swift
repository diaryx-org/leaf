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
        }
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
    }

    private var metrics: Metrics {
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
