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
//  **The accessory is short.** Seven targets — `Aa`, Style, Bold, Italic,
//  List, Insert, Link — then the host's tools. Seven fit an iPhone 15 to 17
//  at accessory size with room for a host's tool or two, so the row a writer
//  sees is the whole row, not the first third of one that runs on past the
//  screen's edge; the row of every tool it replaced was about 1300 points
//  wide. Underline and Checklist were on it once, and are on the panel, and
//  Checklist under List's ▾ too. `Aa` swaps the
//  keyboard for a panel of every other tool (`FormattingPanel.swift`) and back,
//  and lights while the panel is up. Where the host's tools, or a large
//  Dynamic Type size, still make the row too wide, it scrolls: a finger
//  flicks a row, and a scroll can't clip. (It was once a paged TabView —
//  three pages, swipe or tap a dot — and read badly at accessory height:
//  `.page` reserves a strip of its own frame for the dot indicator, so inside
//  a 44pt bar the dots and the 34pt buttons fought over the same points and
//  both got clipped.)
//
//  **The bar is categories.** On the Mac the strip is six menus — Style,
//  Format, Align, Lists, Text, Insert — each drawn from `ToolCatalogue`, with a
//  ▾ beside its glyph. Style spells the caret's style out ("Heading 1") and
//  Align wears the alignment in force, so the two things a reader glances at
//  are readable without opening anything, and the rows tick what is in force.
//  The row of every tool it replaced ran to two or three pages at a usual
//  window width. Undo and Redo are not here: the Edit menu has them, with ⌘Z.
//
//  The rows carry no shortcuts. SwiftUI shows a key equivalent beside a menu
//  row only by registering it, and a chord registered by a view in the window
//  fires wherever the window's focus is — ⌘B typed into a host's sidebar
//  field would bold the document. The menu bar's Format menu has the chords,
//  enabled only for the focused editor.
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
//  Table sits under Insert, and neither has a primary action: a press opens
//  the rows. Inserting a table is one thing the submenu does and editing the
//  one the caret is in is the other, and a tap that inserted a table while the
//  caret stood in a table would be a gesture nobody asked for. The rows are
//  `TableRows`, shared with the Format menu the way `HighlightColourRows` is,
//  so the two surfaces cannot disagree about what a table can do or when.
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
//  `Tool` each, drawn with the bar's own chrome, as one more group at the end:
//  after the categories on the desktop, after Link on the accessory. They are
//  not on the panel, which is leaf's own keyboard. Values rather than a
//  `@ViewBuilder` because the paging has to know how wide the group is before
//  it lays it out, and a view has no width until it is drawn.

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
///     // iOS: a short row above the keyboard, whose `Aa` swaps the
///     // keyboard for a panel of the rest.
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
        /// Keyboard-accessory metrics: 44pt tall, finger-sized targets, the
        /// most-used tools and an `Aa` for the panel of the rest, and a
        /// row that scrolls when it does not fit. On macOS there is no panel
        /// and no `Aa`.
        case accessory
        /// Static-strip metrics: 32pt tall, pointer-sized targets, six
        /// category menus, and a row that pages by category when it does not
        /// fit.
        case bar
        /// The platform's usual choice.
        case automatic
    }

    /// A tool of the host's own, drawn on the bar with the bar's chrome: the
    /// same target, the same bare glyph, the same accent pill when `active`.
    ///
    /// Two kinds, because the bar's own tools come in two: a button that acts,
    /// and a menu whose rows are the whole tool (the way Insert is). A menu's
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

    #if !canImport(UIKit)
    /// The fallback destination field on the Mac: shown only when no host has
    /// claimed the question (see `beginLink`), seeded with the caret link's
    /// current destination so the button re-points a link as readily as it
    /// makes one. iOS asks through a sheet instead (`beginLinkInSheet`).
    @State private var askingForDestination = false
    @State private var typedDestination = ""
    @FocusState private var destinationFocused: Bool
    #endif

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
        case .accessory, .automatic: accessoryRow
        case .bar: pagedRow
        }
    }

    /// The accessory: `Aa`, Style, Bold, Italic, List, Insert and Link, then
    /// the host's tools after a hairline.
    ///
    /// Seven targets because seven fit an iPhone 15 to 17 (393–402 points) at
    /// accessory size with room to spare, so the row never has to scroll and
    /// nothing on it is ever half off the edge. They are the tools a writer reaches for in the
    /// middle of a sentence; the rest are one press of `Aa` away, on a panel
    /// in the keyboard's place (`FormattingPanel.swift`). Where the host's
    /// tools, or a large Dynamic Type size, make the row wider than the
    /// screen, it scrolls as the whole row used to: a finger can flick it,
    /// and it can't clip.
    ///
    /// `ViewThatFits` is macOS 13, and this style can be asked for on the
    /// Mac too, so macOS 12 takes the scroll unconditionally.
    @ViewBuilder
    private var accessoryRow: some View {
        let tools = ToolCatalogue(editor: editor, beginLink: beginLink)
        Group {
            if #available(macOS 13, iOS 16, *) {
                ViewThatFits(in: .horizontal) {
                    accessoryTools(tools)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    ScrollView(.horizontal, showsIndicators: false) { accessoryTools(tools) }
                }
            } else {
                ScrollView(.horizontal, showsIndicators: false) { accessoryTools(tools) }
            }
        }
        .frame(height: metrics.barHeight)
        .background(.bar)
    }

    private func accessoryTools(_ tools: ToolCatalogue) -> some View {
        HStack(spacing: 0) {
            HStack(spacing: metrics.spacing) {
                #if canImport(UIKit)
                if FormattingPanelHost.isAvailable {
                    target(tools.panel, width: metrics.buttonWidth)
                }
                #endif
                target(tools.style, width: rowStyleWidth, indicator: .corner)
                    .modifier(StyleValue(name: tools.styleValue))
                target(tools.bold, width: metrics.buttonWidth)
                target(tools.italic, width: metrics.buttonWidth)
                target(tools.list, width: metrics.buttonWidth, indicator: .corner)
                target(tools.insert, width: metrics.buttonWidth, indicator: .corner)
                target(tools.link, width: metrics.buttonWidth)
                    #if !canImport(UIKit)
                    .popover(isPresented: $askingForDestination) {
                        LinkDestinationField(text: $typedDestination, commit: commitLink)
                            .focused($destinationFocused)
                    }
                    #endif
            }
            if !hostTools.isEmpty {
                separator
                hostGroup
            }
        }
        .padding(.horizontal, metrics.edgePadding)
    }

    /// The row's Style key: as wide as the widest short name ("Body", "Code"),
    /// so it holds still as the caret moves.
    private var rowStyleWidth: CGFloat {
        max(metrics.buttonWidth, widestStyleName(short: true) + 2 * metrics.textInset)
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
            #if !canImport(UIKit)
            .popover(isPresented: $askingForDestination) {
                LinkDestinationField(text: $typedDestination, commit: commitLink)
                    .focused($destinationFocused)
            }
            #endif
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
        let tools = ToolCatalogue(editor: editor, beginLink: beginLink)
        let style = tools.style
        let name = tools.styleName(short: false)
        var groups = [BarGroup(id: "style", gap: .space, widths: [styleWidth],
                               content: AnyView(target(labelled(style, name), width: styleWidth,
                                                       indicator: .chevron)
                                                    .modifier(StyleValue(name: tools.styleValue))))]
        for item in [tools.format, tools.align, tools.lists, tools.text, tools.insert] {
            let width = categoryWidth(item)
            groups.append(BarGroup(id: item.id, gap: .space, widths: [width],
                                   content: AnyView(target(item, width: width, indicator: .chevron))))
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
        max(metrics.buttonWidth, widestStyleName(short: false) + 2 * metrics.textInset)
            + metrics.indicatorWidth
    }

    /// The widest name Style can show, at this bar's label size.
    private func widestStyleName(short: Bool) -> CGFloat {
        GlyphWidths.widestStyleName(short: short, size: metrics.labelSize)
    }

    /// How wide a glyph draws at the bar's sizes: a label at `labelSize`, a
    /// symbol at `glyphSize`.
    private func glyphWidth(_ glyph: ToolGlyph) -> CGFloat {
        switch glyph {
        case .text: return GlyphWidths.width(of: glyph, size: metrics.labelSize)
        case .symbol: return GlyphWidths.width(of: glyph, size: metrics.glyphSize)
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

    /// How a target says it has a menu behind it. A chevron beside the glyph
    /// on the Mac, where the categories are menus and a reader should see so
    /// before clicking; a small ▾ in the corner on the iOS row, which has no
    /// width to spare beside its glyphs — the mark the panel's keys wear too.
    private enum Indicator { case none, chevron, corner }

    /// A tool drawn in the bar's chrome, whatever its shape: a button for a
    /// tool with an action and nothing else, a menu with `primaryAction` for
    /// one with both (tap to act, press-and-hold or right-click for the
    /// rest), and a plain menu for one that is only its variants.
    ///
    /// `.buttonStyle(.plain)` rather than `.menuStyle(.button)`: the latter is
    /// macOS 13, this package is macOS 12, and a plain button style reaches a
    /// menu's own label the same way — a bare glyph on the bar's material.
    @ViewBuilder
    private func target(_ item: ToolItem, width: CGFloat, indicator: Indicator = .none) -> some View {
        let face = targetFace(item, width: width, indicator: indicator)
        Group {
            if let variants = item.variants {
                if let action = item.action {
                    Menu { variants } label: { face } primaryAction: { action() }
                        #if canImport(UIKit)
                        .accessibilityHint(loc("panel.holdForMore", "Touch and hold for more options."))
                        #endif
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
        .accessibilityAddTraits(item.active ? .isSelected : [])
        #if !canImport(UIKit)
        .help(item.label)
        #endif
    }

    /// The Style target reads the style's full name as its value, so
    /// VoiceOver says "Style, Heading 1" where the row's key says "H1".
    private struct StyleValue: ViewModifier {
        let name: String
        func body(content: Content) -> some View {
            content.accessibilityValue(name)
        }
    }

    /// The glyph, the ▾ when there is one, and the active pill.
    ///
    /// A symbol is centred in what is left of the target after the ▾, which
    /// then stands close beside it. A label is set from the leading edge with
    /// the ▾ straight after it: the Style button is as wide as its widest
    /// name, and centring "Body" in room for "Code Block" left the ▾
    /// stranded at the far end.
    private func targetFace(_ item: ToolItem, width: CGFloat, indicator: Indicator) -> some View {
        let chevron = indicator == .chevron
        return Group {
            switch item.glyph {
            case .symbol(let name):
                HStack(spacing: 0) {
                    Image(systemName: name)
                        .font(.system(size: metrics.glyphSize))
                        .frame(width: chevron ? width - metrics.indicatorWidth : width)
                    if chevron { self.chevron }
                }
            case .text(let text):
                HStack(spacing: 0) {
                    Text(text)
                        .font(.system(size: metrics.labelSize, weight: .medium))
                        .lineLimit(1)
                    if chevron { self.chevron }
                }
                .padding(.horizontal, metrics.textInset)
                .frame(width: width, alignment: chevron ? .leading : .center)
            }
        }
        .frame(width: width, height: metrics.buttonHeight)
        .overlay(alignment: .bottomTrailing) {
            if indicator == .corner {
                Image(systemName: "arrowtriangle.down.fill")
                    .font(.system(size: metrics.glyphSize * 0.3))
                    .foregroundStyle(.secondary)
                    .padding(metrics.cornerRadius * 0.5)
            }
        }
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

    // MARK: the host's tools

    /// The host's tools, each in the bar's own chrome: a button tool as a
    /// catalogue button, a menu tool as a catalogue tool that is only its
    /// variants — the shape Insert is.
    private var hostGroup: some View {
        HStack(spacing: metrics.spacing) {
            ForEach(hostTools) { tool in
                target(item(for: tool), width: metrics.buttonWidth)
            }
        }
    }

    private func item(for tool: Tool) -> ToolItem {
        var item = ToolItem(id: tool.id, glyph: .symbol(tool.systemImage), label: tool.label,
                            active: tool.active, enabled: tool.enabled)
        switch tool.kind {
        case .button(let action): item.action = action
        case .menu(let rows): item.variants = rows
        }
        return item
    }

    // MARK: the link destination

    /// Ask for a destination and link with it, through `LeafEditorModel.beginLink`:
    /// the host's `onEditLink` first, and leaf's own field when no host is
    /// listening. On iOS that field is a sheet over the editor, the one the
    /// panel's Link raises — a popover on the row would hang off the
    /// keyboard's accessory, and go away with it the moment its own field
    /// took focus. On the Mac it is a popover on the button, which stays.
    private func beginLink() {
        #if canImport(UIKit)
        editor.beginLinkInSheet()
        #else
        editor.beginLink { seed in
            typedDestination = seed
            askingForDestination = true
            // Raised on the next runloop: the field doesn't exist to focus
            // until the popover has been presented.
            DispatchQueue.main.async { destinationFocused = true }
        }
        #endif
    }

    #if !canImport(UIKit)
    private func commitLink() {
        askingForDestination = false
        editor.commitLinkDestination(typedDestination)
    }
    #endif

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
        ///
        /// The row's seven tools are 40 wide with a point between them, and
        /// the Style key as wide as a quoted "“Body" (56): 314 points with the
        /// edges, measured in the simulator, which leaves an iPhone 15's 393
        /// room for two of a host's tools before the row has to scroll.
        static let accessory = Metrics(
            barHeight: 44, buttonWidth: 40, buttonHeight: 36,
            glyphSize: 17, labelSize: 15, cornerRadius: 8,
            spacing: 1, edgePadding: 6, separatorHeight: 22, separatorPadding: 6,
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
}

extension LeafEditorModel {
    /// The Link question, asked once for every surface that offers Link —
    /// the Mac's bar, the iOS row, the panel. The host's `onEditLink` gets
    /// first refusal: it is already the answer to this question everywhere
    /// else in the package, and only a host can offer a picker for the
    /// destinations that aren't URLs. With no host listening, `fallback`
    /// raises leaf's own field, seeded with what it is given.
    ///
    /// Seeded from the caret's link either way, so Link pressed inside one
    /// re-points it rather than nesting a second link in its text; empty
    /// elsewhere, which is `insertLink`'s "make one" case.
    func beginLink(fallback: (String) -> Void) {
        let current = state.link ?? ""
        if let ask = onEditLink {
            ask(current)
            return
        }
        fallback(current)
    }

    /// Link with what was typed into leaf's own field. An empty destination
    /// cancels rather than writing `[text]()` — core would take it, and a
    /// link that points nowhere is never what the empty field meant.
    func commitLinkDestination(_ typed: String) {
        let destination = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !destination.isEmpty else { return }
        insertLink(destination)
    }
}

/// The fallback destination field: a URL field and a Link button, for a host
/// that has not claimed the question with `onEditLink`. Shared by every
/// surface's Link, so they all ask the same way.
struct LinkDestinationField: View {
    @Binding var text: String
    let commit: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            TextField("https://\u{2026}", text: $text)
                .textFieldStyle(.roundedBorder)
                .frame(width: 260)
                .onSubmit(commit)
                .autocorrectionDisabled()
                #if canImport(UIKit)
                // A destination is not prose: iOS capitalizing the first letter
                // of a URL is a wrong answer every time.
                .textInputAutocapitalization(.never)
                .keyboardType(.URL)
                #endif
            Button(loc("toolbar.link", "Link"), action: commit)
                .keyboardShortcut(.defaultAction)
        }
        .padding(12)
    }
}

/// Glyph widths, measured once per glyph and size and kept. The bar's body
/// runs on every change to the published state — every keystroke, every
/// caret move — and each run needs the width of every category's glyph and
/// of every name Style can show, which is a string measured or an SF Symbol
/// loaded apiece. None of them changes with the text; they change with the
/// size they are drawn at, which is the style's metrics under the reader's
/// Dynamic Type, so that size is the key. A handful of sizes in a session, a
/// few dozen glyphs at each.
enum GlyphWidths {
    private struct Key: Hashable {
        let glyph: ToolGlyph
        let size: CGFloat
    }

    private struct StyleKey: Hashable {
        let short: Bool
        let size: CGFloat
    }

    private static var widths: [Key: CGFloat] = [:]
    private static var styleNames: [StyleKey: CGFloat] = [:]

    /// How wide `glyph` draws at `size`: a label in the bar's medium weight,
    /// a symbol at that point size.
    static func width(of glyph: ToolGlyph, size: CGFloat) -> CGFloat {
        let key = Key(glyph: glyph, size: size)
        if let known = widths[key] { return known }
        let width = measure(glyph, size: size)
        widths[key] = width
        return width
    }

    /// The widest of the names Style can show at `size` — the width the
    /// Style button holds, so it does not change as the caret moves.
    static func widestStyleName(short: Bool, size: CGFloat) -> CGFloat {
        let key = StyleKey(short: short, size: size)
        if let known = styleNames[key] { return known }
        let widest = ToolCatalogue.styleNames(short: short)
            .map { width(of: .text($0), size: size) }.max() ?? 0
        styleNames[key] = widest
        return widest
    }

    private static func measure(_ glyph: ToolGlyph, size: CGFloat) -> CGFloat {
        switch glyph {
        case .text(let text):
            let font = LeafFont.systemFont(ofSize: size, weight: .medium)
            return ceil((text as NSString).size(withAttributes: [.font: font]).width)
        case .symbol(let name):
            #if canImport(UIKit)
            let image = UIImage(systemName: name,
                                withConfiguration: UIImage.SymbolConfiguration(pointSize: size))
            #else
            let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: size, weight: .regular))
            #endif
            return ceil(image?.size.width ?? size)
        }
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
