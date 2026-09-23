//  FormattingToolbar.swift
//
//  The formatting bar. Every tool it offers is defined once, in
//  `ToolCatalogue`, and every action there already exists on `LeafEditorModel`,
//  so this is arrangement rather than new editing capability; a host that wants
//  a different arrangement can still build its own against the same public
//  commands.
//
//  It ships in two shapes, because the two platforms hang it in different
//  places. On iOS it's a keyboard accessory, floated above the soft keyboard by
//  `LeafEditor(model:accessory:)`; on macOS there's no soft keyboard to float
//  above, so it's a static strip the host stacks over the editor. One catalogue,
//  one set of state bindings — so the two live here as a `Style` rather than as
//  two files that would drift. What differs is the metrics, which tools are on
//  the row, and how the row copes with a width it does not fit in:
//
//  **The accessory scrolls.** A finger flicks a row, and a scroll row shows the
//  first group in full, hints at the next, and can't clip. (It was once a paged
//  TabView — three pages, swipe or tap a dot — and read badly at accessory
//  height: `.page` reserves a strip of its own frame for the dot indicator, so
//  inside a 44pt bar the dots and the 34pt buttons fought over the same points
//  and both got clipped, and the paging hid two thirds of the tools behind a
//  gesture with no affordance once the dots were gone.)
//
//  **The bar is categories.** On the Mac the strip is six menus — Style,
//  Format, Align, Lists, Text, Insert — each drawn from `ToolCatalogue`, with a
//  ▾ beside its glyph. Style spells the caret's style out ("Heading 1") and
//  Align wears the alignment in force, so the two things a reader glances at
//  are readable without opening anything; the rows tick what is in force and
//  carry the Format menu's shortcuts, which is how a reader learns them. The
//  row of every tool it replaced ran to two or three pages at a usual window
//  width. Undo and Redo are not here: the Edit menu has them, with ⌘Z.
//
//  Showing the shortcuts registers them a second time, beside the menu bar's.
//  Both call the same command on the same editor and the first to see the
//  chord takes it, so ⌘B still bolds once — checked in the running app.
//
//  A pointer has no sideways scroll, so where even six categories don't fit
//  the bar pages: as many whole groups as fit, and a pair of chevrons at its
//  trailing end turns to the rest. Every category is a group of its own, so a
//  page can break between any two; the host's tools are one more. The width
//  is measured on the container, not the row: measured on the row it could
//  never be narrower than its own tools, so it always fit. The current page
//  is clamped rather than reset when the width changes, so a page that has
//  just become the last one stays up. `ToolbarPaging` is the arithmetic, kept
//  apart from the view so a test can drive it, and it takes each group's
//  width rather than a count, because Style's name is wider than a glyph.
//
//  The buttons are bare glyphs rather than filled capsules: the bar already sits
//  on its own `.bar` material, and a row of capsules on top of that reads as
//  chrome stacked on chrome. It also gives the active state somewhere to go — an
//  accent-tinted pill behind the glyph, which a bordered button's tint could
//  barely express.
//
//  Table is the other tool with a menu behind it, and unlike Highlight it has
//  no primary action: a press opens the rows. Inserting a table is one thing
//  the button does and editing the one the caret is in is the other, and a
//  tap that inserted a table while the caret stood in a table would be a
//  gesture nobody asked for. The rows are `TableRows`, shared with the Format
//  menu the way `HighlightColourRows` is, so the two surfaces cannot disagree
//  about what a table can do or when.
//
//  Link is the one tool that can't be a bare command: every other button here
//  knows everything it needs from the selection, and a link needs a destination
//  from outside it. `LeafEditorModel.onEditLink` is where a host answers that
//  question for the context menu's "Edit Link…", and this asks through the same
//  hook so an app resolving `id:6tzwsxg` gets its own document picker from the
//  toolbar too — with a plain field of the bar's own as the fallback, because a
//  ready-made toolbar whose Link button does nothing until you wire a callback
//  isn't ready-made.
//
//  A host's own tools — a paperclip, a source toggle — go on the row as a
//  `Tool` each, drawn with the bar's own chrome, as one more group after
//  history: paged with the rest on the desktop, scrolled with the rest on the
//  accessory. Values rather than a `@ViewBuilder` because the paging has to
//  know how wide the group is before it lays it out, and a view has no width
//  until it is drawn.

import LeafFFI
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
///
///     // With a tool of the host's own at the row's end.
///     LeafFormattingToolbar(editor: editor, tools: [
///         .button("attach", systemImage: "paperclip", label: "Attach a file") { attach() }
///     ])
public struct LeafFormattingToolbar: View {
    /// Which shape the bar takes. `.automatic` resolves to `.accessory` on iOS
    /// and `.bar` on macOS, which is what a host wants unless it's deliberately
    /// putting the iOS-sized bar somewhere other than above the keyboard.
    public enum Style {
        /// Keyboard-accessory metrics: 44pt tall, finger-sized targets, and a
        /// row that scrolls when it does not fit.
        case accessory
        /// Static-strip metrics: 32pt tall, pointer-sized targets, and a row
        /// that pages by group when it does not fit.
        case bar
        /// The platform's usual choice.
        case automatic
    }

    /// A tool of the host's own, drawn on the bar with the bar's chrome: the
    /// same target, the same bare glyph, the same accent pill when `active`.
    ///
    /// Two kinds, because the bar's own tools come in two: a button that acts,
    /// and a menu whose rows are the whole tool (the way Table is). A menu's
    /// rows are a view of the host's, and the glyph is the bar's, so a host's
    /// Format-style dropdown stands beside the built-in tools looking like one.
    public struct Tool: Identifiable {
        public let id: String
        /// The SF Symbol on the button.
        public var systemImage: String
        /// The accessibility name and, on the desktop, the tooltip.
        public var label: String
        /// Lit with the accent pill, the way Bold is lit inside bold text.
        public var active: Bool
        /// Dimmed to the tertiary label and inert, the way Undo is with
        /// nothing to undo. Inert rather than absent keeps the row's shape.
        public var enabled: Bool
        var kind: Kind

        enum Kind {
            case button(() -> Void)
            case menu(AnyView)
        }

        /// A button: a press runs `action`.
        public static func button(
            _ id: String, systemImage: String, label: String,
            active: Bool = false, enabled: Bool = true,
            action: @escaping () -> Void
        ) -> Tool {
            Tool(id: id, systemImage: systemImage, label: label,
                 active: active, enabled: enabled, kind: .button(action))
        }

        /// A menu: a press drops `rows`, which are whatever a `Menu` takes —
        /// buttons, toggles, dividers, submenus.
        public static func menu<Rows: View>(
            _ id: String, systemImage: String, label: String,
            active: Bool = false, enabled: Bool = true,
            @ViewBuilder rows: () -> Rows
        ) -> Tool {
            Tool(id: id, systemImage: systemImage, label: label,
                 active: active, enabled: enabled, kind: .menu(AnyView(rows())))
        }
    }

    @ObservedObject private var editor: LeafEditorModel
    private let style: Style
    private let hostTools: [Tool]

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

    /// The `.bar` style's paging: which page of groups is up, and the width
    /// of the container the row has to fit. `page` is what was last turned
    /// to, not what is showing — the view clamps it against the page count
    /// each time it draws, so a width change never resets it (see
    /// `ToolbarPaging.clamp`).
    @State private var page = 0
    @State private var width: CGFloat = 0

    /// - Parameter tools: the host's own tools, as one more group at the end of
    ///   the row. Empty by default, which is the bar as it is.
    public init(editor: LeafEditorModel, style: Style = .automatic, tools: [Tool] = []) {
        self.editor = editor
        self.style = style
        self.hostTools = tools
    }

    public var body: some View {
        switch resolvedStyle {
        case .accessory, .automatic: scrollingRow
        case .bar: pagedRow
        }
    }

    /// The accessory: one horizontal scroll, every group in a row.
    private var scrollingRow: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                ForEach(Array(groups.enumerated()), id: \.element.id) { offset, group in
                    if offset > 0 { separator }
                    group.content
                }
            }
            .padding(.horizontal, metrics.edgePadding)
        }
        .frame(height: metrics.barHeight)
        .background(.bar)
    }

    /// The bar: a row of category menus, and chevrons to the rest when the
    /// window is too narrow for them all.
    ///
    /// The width is the container's, measured on a view with no content to
    /// hold it open, and the row is laid over it at that width. Measured on
    /// the row itself it could never be narrower than the row's own tools, so
    /// the row would run past the container's edge and report that it fit.
    /// A `GeometryReader` in the background rather than `onGeometryChange`,
    /// which is a macOS 13 modifier and this package is macOS 12.
    private var pagedRow: some View {
        Color.clear
            .frame(height: metrics.barHeight)
            .frame(maxWidth: .infinity)
            .background(GeometryReader { proxy in
                Color.clear.preference(key: WidthKey.self, value: proxy.size.width)
            })
            .onPreferenceChange(WidthKey.self) { width = $0 }
            .overlay(alignment: .leading) { pagedContent }
            .background(.bar)
            .clipped()
            .popover(isPresented: $askingForDestination) { destinationField }
    }

    private var pagedContent: some View {
        let groups = barGroups
        let widths = groups.map { ToolbarPaging.span(of: $0.widths, spacing: metrics.spacing) }
        let gaps = groups.dropFirst().map { gapWidth($0.gap) }
        let pages = paging.pages(of: widths, gaps: gaps, in: width)
        let current = ToolbarPaging.clamp(page, to: pages.count)
        let showing = pages.indices.contains(current) ? pages[current] : []
        return HStack(spacing: 0) {
            ForEach(Array(showing.enumerated()), id: \.element) { offset, index in
                if offset > 0 { gap(groups[index].gap) }
                groups[index].content
            }
            Spacer(minLength: 0)
            if pages.count > 1 {
                separator
                pager(current: current, count: pages.count)
            }
        }
        .padding(.horizontal, metrics.edgePadding)
        .frame(width: width > 0 ? width : nil, height: metrics.barHeight)
    }

    // MARK: the bar's categories

    /// The Mac's categories: Style, Format, Align, Lists, Text, Insert, each
    /// its own group so a narrow window can page between any two of them,
    /// with a little space between rather than a hairline. The host's tools
    /// come after a hairline, as one group.
    ///
    /// No Undo or Redo: the Edit menu has them, with ⌘Z and ⇧⌘Z, and a Mac
    /// reader does not look for them on a formatting strip.
    private var barGroups: [BarGroup] {
        let tools = ToolCatalogue(editor: editor, beginLink: beginLink, shortcuts: true)
        let style = tools.style
        var groups = [BarGroup(id: "style", gap: .space, widths: [styleWidth],
                               content: AnyView(target(labelled(style, tools.styleName(short: false)),
                                                       width: styleWidth, indicator: true)))]
        for item in [tools.format, tools.align, tools.lists, tools.text, tools.insert] {
            let width = categoryWidth(item)
            groups.append(BarGroup(id: item.id, gap: .space, widths: [width],
                                   content: AnyView(target(item, width: width, indicator: true))))
        }
        if !hostTools.isEmpty {
            groups.append(BarGroup(id: "host", gap: .separator,
                                   widths: hostTools.map { _ in metrics.buttonWidth },
                                   content: AnyView(hostGroup)))
        }
        return groups
    }

    /// `item` with its glyph replaced by `name` — the Style button, which on
    /// the Mac spells the style out.
    private func labelled(_ item: ToolItem, _ name: String) -> ToolItem {
        var item = item
        item.glyph = .text(name)
        return item
    }

    /// A glyph category and its ▾: the glyph's own width, inset, and never
    /// narrower than a plain tool. Measured rather than assumed, because
    /// Format's `bold.italic.underline` is half again as wide as a single
    /// letter and overran a `buttonWidth` target into its own chevron.
    private func categoryWidth(_ item: ToolItem) -> CGFloat {
        max(metrics.buttonWidth, glyphWidth(item.glyph) + 2 * metrics.textInset) + metrics.indicatorWidth
    }

    /// The Style button is as wide as its widest name, so it does not change
    /// width as the caret walks from a heading into body text — and so the
    /// paging, which has to know the width before the button is drawn, can.
    private var styleWidth: CGFloat {
        let widest = ToolCatalogue.styleNames(short: false).map { glyphWidth(.text($0)) }.max() ?? 0
        return max(metrics.buttonWidth, widest + 2 * metrics.textInset) + metrics.indicatorWidth
    }

    /// How wide a glyph draws at the bar's sizes: a label at `labelSize`, a
    /// symbol at `glyphSize`.
    private func glyphWidth(_ glyph: ToolGlyph) -> CGFloat {
        switch glyph {
        case .text(let text):
            let font = LeafFont.systemFont(ofSize: metrics.labelSize, weight: .medium)
            return ceil((text as NSString).size(withAttributes: [.font: font]).width)
        case .symbol(let name):
            #if canImport(UIKit)
            let image = UIImage(systemName: name,
                                withConfiguration: UIImage.SymbolConfiguration(pointSize: metrics.glyphSize))
            #else
            let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: metrics.glyphSize, weight: .regular))
            #endif
            return ceil(image?.size.width ?? metrics.glyphSize)
        }
    }

    /// One group of the `.bar` row: what sits between it and the group before,
    /// the widths of its targets, and the targets.
    private struct BarGroup {
        enum Gap { case space, separator }
        let id: String
        let gap: Gap
        let widths: [CGFloat]
        let content: AnyView
    }

    private func gapWidth(_ gap: BarGroup.Gap) -> CGFloat {
        gap == .separator ? metrics.separatorWidth : metrics.categoryGap
    }

    @ViewBuilder
    private func gap(_ gap: BarGroup.Gap) -> some View {
        switch gap {
        case .separator: separator
        case .space: Color.clear.frame(width: metrics.categoryGap, height: 1)
        }
    }

    // MARK: a catalogue tool as a target

    /// A tool drawn in the bar's chrome, whatever its shape: a button for a
    /// tool with an action and nothing else, a menu with `primaryAction` for
    /// one with both (tap to act, press-and-hold or right-click for the
    /// rest), and a plain menu for one that is only its variants.
    ///
    /// `.buttonStyle(.plain)` rather than `.menuStyle(.button)`: the latter is
    /// macOS 13, this package is macOS 12, and a plain button style reaches a
    /// menu's own label the same way — a bare glyph on the bar's material.
    @ViewBuilder
    private func target(_ item: ToolItem, width: CGFloat, indicator: Bool = false) -> some View {
        let face = targetFace(item, width: width, indicator: indicator)
        Group {
            if let variants = item.variants {
                if let action = item.action {
                    Menu { variants } label: { face } primaryAction: { action() }
                } else {
                    Menu { variants } label: { face }
                }
            } else {
                Button(action: item.action ?? {}) { face }
            }
        }
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .frame(width: width, height: metrics.buttonHeight)
        .disabled(!item.enabled)
        .accessibilityLabel(item.label)
        .modifier(StyleValue(item: item))
        #if !canImport(UIKit)
        .help(item.label)
        #endif
    }

    /// A Style target reads its name as the value, so VoiceOver says "Style,
    /// Heading 1" rather than "H1".
    private struct StyleValue: ViewModifier {
        let item: ToolItem
        func body(content: Content) -> some View {
            if item.id == "style", case .text(let name) = item.glyph {
                content.accessibilityValue(name)
            } else {
                content
            }
        }
    }

    /// The glyph, the ▾ when there is one, and the active pill.
    ///
    /// A symbol is centred in what is left of the target after the ▾, which
    /// then stands close beside it. A label is set from the leading edge with
    /// the ▾ straight after it: the Style button is as wide as its widest
    /// name, and centring "Body" in room for "Code Block" left the ▾
    /// stranded at the far end.
    private func targetFace(_ item: ToolItem, width: CGFloat, indicator: Bool) -> some View {
        Group {
            switch item.glyph {
            case .symbol(let name):
                HStack(spacing: 0) {
                    Image(systemName: name)
                        .font(.system(size: metrics.glyphSize))
                        .frame(width: indicator ? width - metrics.indicatorWidth : width)
                    if indicator { chevron }
                }
            case .text(let text):
                HStack(spacing: 0) {
                    Text(text)
                        .font(.system(size: metrics.labelSize, weight: .medium))
                        .lineLimit(1)
                    if indicator { chevron }
                }
                .padding(.horizontal, metrics.textInset)
                .frame(width: width, alignment: indicator ? .leading : .center)
            }
        }
        .frame(width: width, height: metrics.buttonHeight)
        // `.plain` leaves a disabled button looking pressed-and-ignored; the
        // tertiary label is what the system's own bars dim to.
        .foregroundStyle(!item.enabled ? Color(Palette.tertiary)
                         : item.active ? Color.accentColor : Color.primary)
        .background(
            RoundedRectangle(cornerRadius: metrics.cornerRadius)
                .fill(item.active ? Color.accentColor.opacity(0.15) : Color.clear)
        )
        .contentShape(Rectangle())
    }

    /// The ▾ that says a target opens a menu.
    private var chevron: some View {
        Image(systemName: "chevron.down")
            .font(.system(size: metrics.glyphSize * 0.55, weight: .semibold))
            .frame(width: metrics.indicatorWidth, alignment: .center)
            .foregroundStyle(.secondary)
    }

    // MARK: paging

    private struct WidthKey: PreferenceKey {
        static let defaultValue: CGFloat = 0
        static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
    }

    /// The paging arithmetic at this bar's (Dynamic-Type-scaled) sizes.
    private var paging: ToolbarPaging {
        ToolbarPaging(
            edgePadding: metrics.edgePadding,
            separatorWidth: metrics.separatorWidth,
            pagerWidth: 2 * metrics.buttonWidth + metrics.spacing
        )
    }

    private func pager(current: Int, count: Int) -> some View {
        HStack(spacing: metrics.spacing) {
            pageButton("chevron.left", loc("toolbar.previousTools", "Previous tools"),
                       enabled: current > 0) { page = current - 1 }
            pageButton("chevron.right", loc("toolbar.moreTools", "More tools"),
                       enabled: current < count - 1) { page = current + 1 }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(loc("toolbar.pages", "Tool pages"))
        .accessibilityValue(String(format: loc("toolbar.pageOf", "Page %d of %d"), current + 1, count))
    }

    private func pageButton(
        _ symbol: String, _ label: String, enabled: Bool, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: metrics.glyphSize, weight: .semibold))
                .frame(width: metrics.buttonWidth, height: metrics.buttonHeight)
                .foregroundStyle(enabled ? Color.primary : Color(Palette.tertiary))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(label)
        #if !canImport(UIKit)
        .help(label)
        #endif
    }

    // MARK: tool groups

    /// One group on the row: its tools as a view, and how many there are, which
    /// is what the paging measures. The count sits beside the builder it counts
    /// so the two are edited together.
    private struct ToolGroup {
        let id: String
        let count: Int
        let content: AnyView
    }

    /// The groups in order — the bar's five, then the host's, if it gave any.
    private var groups: [ToolGroup] {
        var groups = [
            ToolGroup(id: "inline", count: 7, content: AnyView(inlineMarks)),
            ToolGroup(id: "block", count: 11, content: AnyView(blockStyles)),
            ToolGroup(id: "presentation", count: 9, content: AnyView(presentationTools)),
            ToolGroup(id: "indent", count: 2, content: AnyView(indentTools)),
            ToolGroup(id: "history", count: 2, content: AnyView(historyTools)),
        ]
        if !hostTools.isEmpty {
            groups.append(ToolGroup(id: "host", count: hostTools.count, content: AnyView(hostGroup)))
        }
        return groups
    }

    /// Seven tools: the five marks, Highlight, Link.
    private var inlineMarks: some View {
        HStack(spacing: metrics.spacing) {
            tool("bold", "Bold", active: editor.isActive("bold")) { editor.toggleBold() }
            tool("italic", "Italic", active: editor.isActive("italic")) { editor.toggleItalic() }
            // Dark in Markdown, which has no underline to write — djot's
            // `{+text+}` has no Markdown spelling, even under leaf's extensions.
            tool("underline", "Underline", active: editor.isActive("underline"),
                 enabled: editor.capabilities.underline) { editor.toggleUnderline() }
            tool("strikethrough", "Strikethrough", active: editor.isActive("strike")) { editor.toggleStrike() }
            tool("chevron.left.forwardslash.chevron.right", "Code", active: editor.isActive("code")) { editor.toggleCode() }
            highlightTool
            linkTool
        }
    }

    /// Highlight, and the seven colours a highlight can be — the one tool here
    /// with a menu behind it.
    ///
    /// A press marks (or unmarks) the selection, exactly like the buttons beside
    /// it; the colour lives in the menu, because a colour is a property of a
    /// highlight rather than a mark of its own, and seven more buttons in a row
    /// this size would be a palette pretending to be formatting. `primaryAction`
    /// is what keeps both on one target: tap to highlight, press-and-hold — or
    /// right-click, on the desktop — for the colours.
    private var highlightTool: some View {
        Menu {
            HighlightColourRows(editor: editor)
        } label: {
            Image(systemName: "highlighter")
                .font(.system(size: metrics.glyphSize))
        } primaryAction: {
            editor.toggleMark()
        }
        // `.buttonStyle(.plain)` rather than `.menuStyle(.button)`: the latter is
        // macOS 13, this package is macOS 12, and a plain button style reaches
        // the menu's own label the same way — a bare glyph on the bar's
        // material, like every tool beside it.
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .frame(width: metrics.buttonWidth, height: metrics.buttonHeight)
        .foregroundStyle(editor.isActive("mark") ? Color.accentColor : Color.primary)
        .background(
            RoundedRectangle(cornerRadius: metrics.cornerRadius)
                .fill(editor.isActive("mark") ? Color.accentColor.opacity(0.15) : Color.clear)
        )
        .contentShape(Rectangle())
        .accessibilityLabel(loc("menu.highlight", "Highlight"))
        #if !canImport(UIKit)
        .help(loc("menu.highlight", "Highlight"))
        #endif
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

    /// Eleven tools: H1, H2, body, quote, code block, the two lists, Checklist,
    /// the rule, Footnote, Table.
    private var blockStyles: some View {
        HStack(spacing: metrics.spacing) {
            textTool("H1", "Heading 1", active: editor.state.heading == 1) { editor.setHeading(1) }
            textTool("H2", "Heading 2", active: editor.state.heading == 2) { editor.setHeading(2) }
            tool("paragraphsign", "Body text", active: editor.state.heading == nil) { editor.setParagraph() }
            tool("quote.opening", "Quote") { editor.toggleBlockquote() }
            // Beside Quote because it is the same kind of thing — a block the
            // caret's paragraph becomes — and lit while the caret stands in one
            // for the same reason Bold is lit inside bold text. The braces
            // rather than the angle-bracket glyph inline Code wears, so the two
            // read as different tools and not as one button drawn twice.
            tool("curlybraces", "Code Block", active: editor.state.codeBlock,
                 enabled: editor.capabilities.codeBlock) { editor.toggleCodeBlock() }
            tool("list.bullet", "Bulleted list") { editor.toggleList(ordered: false) }
            tool("list.number", "Numbered list") { editor.toggleList(ordered: true) }
            // The third list, lit while the caret's item has a box for the
            // reason Code Block is lit inside a fence. Ticking the box is not a
            // tool here: a click or a tap on the box itself does that, and the
            // Format menu's Checked item is the keyboard's way.
            tool("checklist", "Checklist", active: editor.state.task != nil,
                 enabled: editor.capabilities.task) { editor.toggleTaskItem() }
            tool("rectangle.compress.vertical", "Horizontal Rule") { editor.insertThematicBreak() }
            // Beside the rule rather than among the inline marks: a footnote is
            // not a mark over the selection, it's a thing written into the
            // document — and like the rule it acts once rather than toggling, so
            // it has no active state to show. The glyph is the raised character
            // because that is what the gesture puts on screen; if the eight
            // inline marks ever grow a Superscript button of their own, that one
            // takes this symbol and this takes `asterisk`.
            tool("textformat.superscript", "Footnote") { editor.insertFootnote() }
            tableTool
        }
    }

    /// Table: a fresh one at the caret, and the grid ops over the one the
    /// caret is in, as one menu (see the file's note on why it has no primary
    /// action). Lit while the caret stands in a table, the way Link is lit
    /// inside a link; dark where the format spells no table at all
    /// (`Capabilities.table`), which is the whole gate for inserting one — the
    /// grid rows gate themselves on the caret.
    private var tableTool: some View {
        Menu {
            TableRows(editor: editor)
        } label: {
            Image(systemName: "tablecells")
                .font(.system(size: metrics.glyphSize))
        }
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .frame(width: metrics.buttonWidth, height: metrics.buttonHeight)
        .foregroundStyle(!editor.capabilities.table ? Color(Palette.tertiary)
                         : editor.caretInTable ? Color.accentColor : Color.primary)
        .background(
            RoundedRectangle(cornerRadius: metrics.cornerRadius)
                .fill(editor.caretInTable ? Color.accentColor.opacity(0.15) : Color.clear)
        )
        .contentShape(Rectangle())
        .disabled(!editor.capabilities.table)
        .accessibilityLabel(loc("menu.table", "Table"))
        #if !canImport(UIKit)
        .help(loc("menu.table", "Table"))
        #endif
    }

    /// The presentation vocabulary: how the block is laid (alignment, spacing),
    /// how the letters are set (size, face, colour), and where the paper ends.
    ///
    /// Its own group rather than four more tools in the block group, and in this
    /// order: the two that describe a *line* first, the three that describe the
    /// *letters* after, and the page break last because it is the only one that
    /// writes something into the document rather than restyling what is there.
    /// A group is the unit the `.bar` style pages by, so nine tools arriving as
    /// one group is nine tools that turn onto a page together instead of nine
    /// more the trailing edge cuts in half.
    ///
    /// Alignment is four buttons and not a menu: it is the one property here a
    /// reader glances at to see how the block they are in is set, and a segment
    /// shows that where a menu would have to be opened to find out. The other
    /// four are menus, which is also what lets them ask the document for the
    /// caret's value only when they open (see `LeafEditorModel`'s queries).
    private var presentationTools: some View {
        HStack(spacing: metrics.spacing) {
            alignmentSegment
            menuTool("arrow.up.and.down.text.horizontal", loc("menu.lineSpacing", "Line Spacing"),
                     enabled: editor.capabilities.lineSpacing) {
                LineSpacingRows(editor: editor)
            }
            menuTool("textformat.size", loc("menu.textSize", "Text Size"),
                     enabled: editor.capabilities.fontSize) {
                TextSizeRows(editor: editor)
            }
            menuTool("textformat", loc("menu.font", "Font"),
                     enabled: editor.capabilities.fontFamily) {
                FontFamilyRows(editor: editor)
            }
            menuTool("paintpalette", loc("menu.textColour", "Text Colour"),
                     enabled: editor.capabilities.textColor) {
                TextColourRows(editor: editor)
            }
            tool("arrow.down.to.line", loc("menu.pageBreak", "Page Break"),
                 enabled: editor.capabilities.pageBreak) { editor.insertPageBreak() }
        }
    }

    /// Left · Centre · Right · Justify, lit by the block the caret is in. Left is
    /// *clearing* the key — the vocabulary has no `left` token, because absence
    /// is left — so the button that looks like the default is the one that
    /// restores it.
    ///
    /// The light reads `editor.alignment`, which rides the published frame for
    /// the reason the Link button's does: walking the caret out of a centred
    /// paragraph changes no mark and no heading, so a segment asking core for
    /// itself would never be told (see `EditorState.align`).
    private var alignmentSegment: some View {
        Group {
            tool("text.alignleft", loc("menu.align.left", "Left"),
                 active: editor.alignment == nil,
                 enabled: editor.capabilities.alignment) { editor.setAlignment(nil) }
            ForEach(Align.all, id: \.self) { align in
                tool(align.symbol, align.title,
                     active: editor.alignment == align,
                     enabled: editor.capabilities.alignment) { editor.setAlignment(align) }
            }
        }
    }

    /// Two tools: Indent, Outdent.
    private var indentTools: some View {
        HStack(spacing: metrics.spacing) {
            tool("increase.indent", "Indent") { editor.indent() }
            tool("decrease.indent", "Outdent") { editor.outdent() }
        }
    }

    /// Enabled by the history's depth, the way the Edit menu's items are: a
    /// button that does nothing when pressed says the document is broken, where
    /// a dimmed one says there is nothing to take back.
    private var historyTools: some View {
        HStack(spacing: metrics.spacing) {
            tool("arrow.uturn.backward", "Undo", enabled: editor.state.canUndo) { editor.undo() }
            tool("arrow.uturn.forward", "Redo", enabled: editor.state.canRedo) { editor.redo() }
        }
    }

    /// The host's tools, each in the bar's own chrome. A menu tool is the
    /// Table tool's shape — a plain `Menu` behind a bare glyph, the pill
    /// outside it — and a button tool is every other tool's.
    private var hostGroup: some View {
        HStack(spacing: metrics.spacing) {
            ForEach(hostTools) { tool in
                switch tool.kind {
                case .button(let action):
                    self.tool(tool.systemImage, tool.label, active: tool.active,
                              enabled: tool.enabled, action: action)
                case .menu(let rows):
                    Menu {
                        rows
                    } label: {
                        Image(systemName: tool.systemImage)
                            .font(.system(size: metrics.glyphSize))
                    }
                    .buttonStyle(.plain)
                    .menuIndicator(.hidden)
                    .frame(width: metrics.buttonWidth, height: metrics.buttonHeight)
                    .foregroundStyle(!tool.enabled ? Color(Palette.tertiary)
                                     : tool.active ? Color.accentColor : Color.primary)
                    .background(
                        RoundedRectangle(cornerRadius: metrics.cornerRadius)
                            .fill(tool.active ? Color.accentColor.opacity(0.15) : Color.clear)
                    )
                    .contentShape(Rectangle())
                    .disabled(!tool.enabled)
                    .accessibilityLabel(tool.label)
                    #if !canImport(UIKit)
                    .help(tool.label)
                    #endif
                }
            }
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
        /// The hairline and the six points either side of it (`separator`),
        /// which the paging has to count.
        var separatorWidth: CGFloat { 1 + 2 * separatorPadding }
        var separatorPadding: CGFloat
        /// The ▾ beside a category's glyph.
        var indicatorWidth: CGFloat
        /// A text label's inset from its target's edges.
        var textInset: CGFloat
        /// The space between two of the Mac's categories, where a hairline
        /// would read as a border round each.
        var categoryGap: CGFloat

        /// 44 is the tap-target floor, and the row spends all of it — there's no
        /// indicator strip to leave room for any more.
        static let accessory = Metrics(
            barHeight: 44, buttonWidth: 40, buttonHeight: 36,
            glyphSize: 17, labelSize: 15, cornerRadius: 8,
            spacing: 2, edgePadding: 8, separatorHeight: 22, separatorPadding: 6,
            indicatorWidth: 10, textInset: 6, categoryGap: 4
        )

        /// A pointer hits a much smaller target than a fingertip, and the strip
        /// competes with the document for vertical space in a way a keyboard
        /// accessory never does — so this is roughly a system toolbar's height.
        static let bar = Metrics(
            barHeight: 32, buttonWidth: 26, buttonHeight: 24,
            glyphSize: 13, labelSize: 12, cornerRadius: 5,
            spacing: 1, edgePadding: 8, separatorHeight: 16, separatorPadding: 6,
            indicatorWidth: 10, textInset: 3, categoryGap: 6
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
            m.separatorPadding *= factor
            m.indicatorWidth *= factor
            m.textInset *= factor
            m.categoryGap *= factor
            return m
        }
    }

    private var metrics: Metrics {
        base.scaled(by: min(typeScale, Metrics.maxTypeScale))
    }

    /// The style's own sizes, before Dynamic Type is applied.
    private var base: Metrics {
        resolvedStyle == .bar ? .bar : .accessory
    }

    /// `style` with `.automatic` settled: the accessory on iOS, the bar on the
    /// desktop.
    private var resolvedStyle: Style {
        guard style == .automatic else { return style }
        #if canImport(UIKit)
        return .accessory
        #else
        return .bar
        #endif
    }

    // MARK: shared chrome

    /// A hairline between groups, inset from the bar's edges so it reads as a
    /// separator rather than a border.
    private var separator: some View {
        Divider()
            .frame(height: metrics.separatorHeight)
            .padding(.horizontal, metrics.separatorPadding)
    }

    private func tool(
        _ systemImage: String,
        _ label: String,
        active: Bool = false,
        enabled: Bool = true,
        action: @escaping () -> Void
    ) -> some View {
        button(active: active, enabled: enabled, label: label, action: action) {
            Image(systemName: systemImage)
                .font(.system(size: metrics.glyphSize))
        }
    }

    /// A tool whose whole job is its menu — no primary action, the way Table has
    /// none: there is no "apply the last size" gesture, only a choice. Dimmed by
    /// its capability, so a format that cannot spell the property offers no rows
    /// to open.
    ///
    /// Written once here because the vocabulary added four of them at a stroke
    /// and they differ only in glyph, label, and which rows they drop.
    private func menuTool<Rows: View>(
        _ systemImage: String,
        _ label: String,
        enabled: Bool,
        @ViewBuilder rows: () -> Rows
    ) -> some View {
        Menu {
            rows()
        } label: {
            Image(systemName: systemImage)
                .font(.system(size: metrics.glyphSize))
        }
        // `.plain` for the reason the Highlight menu takes it: `.menuStyle(.button)`
        // is macOS 13 and this package is macOS 12.
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .frame(width: metrics.buttonWidth, height: metrics.buttonHeight)
        .foregroundStyle(enabled ? Color.primary : Color(Palette.tertiary))
        .contentShape(Rectangle())
        .disabled(!enabled)
        .accessibilityLabel(label)
        #if !canImport(UIKit)
        .help(label)
        #endif
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
        enabled: Bool = true,
        label: String,
        action: @escaping () -> Void,
        @ViewBuilder glyph: () -> Glyph
    ) -> some View {
        Button(action: action) {
            glyph()
                .frame(width: metrics.buttonWidth, height: metrics.buttonHeight)
                // `.plain` leaves a disabled button looking pressed-and-ignored;
                // the tertiary label is what the system's own bars dim to.
                .foregroundStyle(!enabled ? Color(Palette.tertiary) : active ? Color.accentColor : Color.primary)
                .background(
                    RoundedRectangle(cornerRadius: metrics.cornerRadius)
                        .fill(active ? Color.accentColor.opacity(0.15) : Color.clear)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(label)
        // A pointer can hover; a fingertip can't. On iOS `.help` lands as an
        // accessibility hint, which duplicates the label above — so the tooltip
        // is desktop-only rather than unconditional.
        #if !canImport(UIKit)
        .help(label)
        #endif
    }
}

/// The `.bar` style's paging arithmetic: which groups go on which page for a
/// given width, and where a remembered page lands when the count changes.
/// Apart from the view so a test can drive it with numbers; the view builds
/// one from its scaled `Metrics`.
struct ToolbarPaging: Equatable {
    /// The row's inset from the bar's edges, both sides.
    var edgePadding: CGFloat
    /// The hairline between two groups, with its padding.
    var separatorWidth: CGFloat
    /// The two chevrons and the spacing between them.
    var pagerWidth: CGFloat

    /// Which groups go on which page, for a bar `available` points wide, as
    /// indices into `widths`, with a separator between every two groups.
    func pages(of widths: [CGFloat], in available: CGFloat) -> [[Int]] {
        pages(of: widths, gaps: Array(repeating: separatorWidth, count: max(0, widths.count - 1)),
              in: available)
    }

    /// Which groups go on which page, for a bar `available` points wide, as
    /// indices into `widths`. `gaps[i]` is what stands between group `i` and
    /// group `i + 1` when both are on one page: a hairline between the bar's
    /// own tools and the host's, a little space between two of the Mac's
    /// categories. Groups need not be the same width — the Style button spells
    /// its name out and is wider than a glyph — and nothing here assumes they
    /// are.
    ///
    /// One page when everything fits, chevrons and all — a single page has no
    /// pager. Otherwise the pager's own width and the separator before it
    /// come off the top, and the groups are packed in order, each page taking
    /// whole groups until the next would not fit. A gap at a page break is
    /// not drawn, so it is not counted. A group wider than a page on its own
    /// still gets one — clipped at the edge, which is the host's column being
    /// narrower than any toolbar could be.
    ///
    /// Unmeasured (`available <= 0`) is one page: the chevrons should not blink
    /// in on the first frame and out on the second.
    func pages(of widths: [CGFloat], gaps: [CGFloat], in available: CGFloat) -> [[Int]] {
        guard !widths.isEmpty else { return [] }
        precondition(gaps.count == widths.count - 1, "one gap between every two groups")
        let all = Array(widths.indices)
        guard available > 0 else { return [all] }
        let room = available - 2 * edgePadding
        let total = widths.reduce(0, +) + gaps.reduce(0, +)
        if total <= room { return [all] }
        let pageRoom = room - separatorWidth - pagerWidth
        var pages: [[Int]] = []
        var current: [Int] = []
        var used: CGFloat = 0
        for (index, width) in widths.enumerated() {
            if !current.isEmpty, used + gaps[index - 1] + width > pageRoom {
                pages.append(current)
                current = []
                used = 0
            }
            used += current.isEmpty ? width : gaps[index - 1] + width
            current.append(index)
        }
        pages.append(current)
        return pages
    }

    /// How wide a group is: its targets side by side, `spacing` between each
    /// two. The targets may differ — a category's glyph and ▾, the Style
    /// button's name — so the group is summed rather than counted.
    static func span(of widths: [CGFloat], spacing: CGFloat) -> CGFloat {
        widths.reduce(0, +) + CGFloat(max(0, widths.count - 1)) * spacing
    }

    /// The page to show when `page` was turned to under one page count and
    /// there are now `count`: the same page while it still exists, the last
    /// one otherwise. Clamped rather than reset, so widening a window enough
    /// to drop a page lands on the tools nearest the ones that were up, and a
    /// page that has just become the last stays up.
    static func clamp(_ page: Int, to count: Int) -> Int {
        min(max(0, page), max(0, count - 1))
    }
}

/// The colours of a highlight, as menu rows: the seven the document can spell,
/// each ticked while it is the caret's, and "No Colour" to take one off.
///
/// Its own `View` rather than a `@ViewBuilder` on the bar because both surfaces
/// show the same rows — this bar's Highlight menu and the app's Format menu —
/// and because a `Toggle` per colour inside a `ForEach` inside a `Menu` is more
/// type inference than the compiler will do in one expression.
///
/// The rows tick from `editor.markColor`, which rides the frame: walking the
/// caret from a red highlight into a blue one moves the tick, and nothing else
/// in the published state would have told the menu to redraw.
struct HighlightColourRows: View {
    @ObservedObject var editor: LeafEditorModel

    var body: some View {
        Group {
            ForEach(MarkColor.palette, id: \.self) { color in
                row(color.menuTitle, on: editor.markColor == color) { editor.highlight(color) }
            }
            Divider()
            // Ticked only inside an *uncoloured* highlight: `markColor` is nil
            // outside a highlight too, and a ticked row there would claim the
            // caret was standing in something it isn't.
            row(loc("menu.highlight.noColour", "No Colour"),
                on: editor.caretInMark && editor.markColor == nil) {
                editor.highlight(nil)
            }
        }
        // A colour needs a highlight to belong to — one the caret is in, or one
        // this press would make out of the selection — and a format that spells
        // one (djot writes the highlight and no colour on it). The Highlight
        // button itself stays live either way: it is the way *to* a highlight.
        .disabled(!editor.canColourHighlight)
    }

    /// One row: ticked while it is the caret's colour, and choosing it applies
    /// that colour. A `Toggle`, the way every mark in the Format menu is.
    private func row(_ title: String, on: Bool, _ apply: @escaping () -> Void) -> some View {
        Toggle(title, isOn: Binding(get: { on }, set: { _ in apply() }))
    }
}

/// The table rows: a fresh table, then the grid ops over the table the caret
/// is in. Its own `View` for the reason `HighlightColourRows` is — the bar's
/// Table menu and the app's Format ▸ Table submenu show the same rows, and one
/// definition is what keeps them agreeing.
///
/// A new table is two columns and two body rows under its header, or wider
/// from the submenu. Only the width is offered, because a row is the cheap
/// dimension — Tab past the last cell and Return on the last row both grow
/// one — while a column takes a menu trip. The grid rows enable on
/// `caretInTable`, which `editor.state` re-reads on every caret move, so a
/// menu opened over prose has them dimmed and one opened in a table has them
/// live.
///
/// Public because a host app's own menus — `apps/leaf-editor`'s toolbar, a
/// consumer's Format menu — put the same rows behind their own button.
public struct TableRows: View {
    @ObservedObject var editor: LeafEditorModel

    public init(editor: LeafEditorModel) {
        self.editor = editor
    }

    /// Body rows under the header of a table this inserts.
    static let defaultRows = 2
    /// Columns of the plain "Insert Table" row; the submenu offers wider.
    static let defaultColumns = 2
    static let widths = 3...5

    public var body: some View {
        Group {
            Button(loc("menu.insertTable", "Insert Table")) {
                editor.insertTable(rows: Self.defaultRows, cols: Self.defaultColumns)
            }
            Menu(loc("menu.insertTableWith", "Insert Table With")) {
                ForEach(Self.widths, id: \.self) { n in
                    Button(String(format: loc("menu.insertTableColumns", "%d Columns"), n)) {
                        editor.insertTable(rows: Self.defaultRows, cols: n)
                    }
                }
            }
            Divider()
            Group {
                Button(loc("menu.table.insertRowAbove", "Insert Row Above")) { editor.tableInsertRow(below: false) }
                Button(loc("menu.table.insertRowBelow", "Insert Row Below")) { editor.tableInsertRow(below: true) }
                Button(loc("menu.table.deleteRow", "Delete Row")) { editor.tableDeleteRow() }
                Divider()
                Button(loc("menu.table.insertColumnLeft", "Insert Column Left")) { editor.tableInsertColumn(right: false) }
                Button(loc("menu.table.insertColumnRight", "Insert Column Right")) { editor.tableInsertColumn(right: true) }
                Button(loc("menu.table.deleteColumn", "Delete Column")) { editor.tableDeleteColumn() }
                Divider()
                Menu(loc("menu.table.alignColumn", "Align Column")) {
                    Button(loc("menu.table.align.left", "Left")) { editor.tableSetAlignment(.left) }
                    Button(loc("menu.table.align.center", "Center")) { editor.tableSetAlignment(.center) }
                    Button(loc("menu.table.align.right", "Right")) { editor.tableSetAlignment(.right) }
                    Button(loc("menu.table.align.default", "Default")) { editor.tableSetAlignment(.default) }
                }
                Divider()
                Button(loc("menu.table.moveRowUp", "Move Row Up")) { editor.tableMoveRow(down: false) }
                Button(loc("menu.table.moveRowDown", "Move Row Down")) { editor.tableMoveRow(down: true) }
                Button(loc("menu.table.moveColumnLeft", "Move Column Left")) { editor.tableMoveColumn(right: false) }
                Button(loc("menu.table.moveColumnRight", "Move Column Right")) { editor.tableMoveColumn(right: true) }
            }
            .disabled(!editor.caretInTable)
        }
        .disabled(!editor.capabilities.table)
    }
}
