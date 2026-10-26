//  ToolbarEditor.swift
//
//  `LeafEditorModel` as LeafToolbar's editor, so a host draws the same
//  window toolbar over this editor that the Mac app draws over leaf-dioxus:
//
//      .toolbar { LeafToolbarItems(editor: model) }
//
//  LeafToolbar is re-exported, so `import LeafUI` is enough.
//
//  Each command is the public method the formatting bar's own tool calls, so
//  the window toolbar and the bar cannot do different things for one name.

@_exported import LeafToolbar
import LeafFFI
import SwiftUI

extension LeafEditorModel: LeafToolbarEditor {
    public var toolbarState: LeafToolbarState {
        let caps = capabilities
        let names: [(LeafMark, String)] = [(.bold, "bold"), (.italic, "italic"), (.underline, "underline"),
                                           (.strikethrough, "strike"), (.code, "code"), (.highlight, "mark")]
        return LeafToolbarState(
            marks: Set(names.filter { isActive($0.1) }.map(\.0)),
            heading: state.heading.map(Int.init),
            codeBlock: state.codeBlock,
            verse: state.verse,
            blockquote: state.blockquote,
            task: state.task,
            link: state.link != nil,
            highlightColour: markColor.map(LeafHighlightColour.init),
            inHighlight: caretInMark,
            canColourHighlight: canColourHighlight,
            alignment: alignment.map(LeafAlignment.init),
            inTable: caretInTable,
            canUndo: state.canUndo,
            canRedo: state.canRedo,
            capabilities: LeafToolbarCapabilities(
                underline: caps.underline, highlightColour: caps.markColor, blockquote: caps.blockquote,
                codeBlock: caps.codeBlock, verse: caps.verse, task: caps.task, moveBlock: caps.moveBlock,
                table: caps.table, footnote: caps.footnote, pageBreak: caps.pageBreak,
                directives: caps.directives, alignment: caps.alignment,
                text: caps.fontSize || caps.fontFamily || caps.textColor || caps.lineSpacing),
            directives: directiveCatalogue.map {
                LeafToolbarDirective(id: $0.id, title: $0.title, systemImage: $0.icon)
            })
    }

    public func perform(_ command: LeafToolbarCommand) {
        switch command {
        case .toggle(.bold): toggleBold()
        case .toggle(.italic): toggleItalic()
        case .toggle(.underline): toggleUnderline()
        case .toggle(.strikethrough): toggleStrike()
        case .toggle(.code): toggleCode()
        case .toggle(.highlight): toggleMark()
        case .highlightColour(let colour): highlight(colour.map(MarkColor.init))
        case .link: beginLinkInSheet()
        case .paragraph: setParagraph()
        case .heading(let level): setHeading(UInt32(level))
        case .blockquote: toggleBlockquote()
        case .codeBlock: toggleCodeBlock()
        case .verse: toggleVerse()
        case .bulletList: toggleList(ordered: false)
        case .numberedList: toggleList(ordered: true)
        case .checklist: toggleTaskItem()
        case .checked: toggleTaskChecked()
        case .indent: indent()
        case .outdent: outdent()
        case .moveBlockUp: moveBlockUp()
        case .moveBlockDown: moveBlockDown()
        case .align(let align): setAlignment(align.map(Align.init))
        case .horizontalRule: insertThematicBreak()
        case .footnote: insertFootnote()
        case .pageBreak: insertPageBreak()
        case .table(let op): table(op)
        case .directive(let id):
            if let item = directiveCatalogue.first(where: { $0.id == id }) { insert(item) }
        case .undo: undo()
        case .redo: redo()
        }
    }

    /// Size, face, colour and line spacing: the Format menu's own rows, each
    /// dimmed where the format cannot spell it.
    public var toolbarTextRows: some View {
        let caps = capabilities
        return Group {
            Menu(loc("menu.textSize", "Text Size")) { TextSizeRows(editor: self) }
                .disabled(!caps.fontSize)
            Menu(loc("menu.font", "Font")) { FontFamilyRows(editor: self) }
                .disabled(!caps.fontFamily)
            Menu(loc("menu.textColour", "Text Colour")) { TextColourRows(editor: self) }
                .disabled(!caps.textColor)
            Menu(loc("menu.lineSpacing", "Line Spacing")) { LineSpacingRows(editor: self) }
                .disabled(!caps.lineSpacing)
        }
    }

    private func table(_ op: LeafTableCommand) {
        switch op {
        case .insert(let columns): insertTable(rows: TableRows.defaultRows, cols: columns)
        case .insertRow(let below): tableInsertRow(below: below)
        case .deleteRow: tableDeleteRow()
        case .insertColumn(let right): tableInsertColumn(right: right)
        case .deleteColumn: tableDeleteColumn()
        case .align(.default): tableSetAlignment(.default)
        case .align(.left): tableSetAlignment(.left)
        case .align(.center): tableSetAlignment(.center)
        case .align(.right): tableSetAlignment(.right)
        case .moveRow(let down): tableMoveRow(down: down)
        case .moveColumn(let right): tableMoveColumn(right: right)
        }
    }
}

extension LeafHighlightColour {
    init(_ colour: MarkColor) {
        switch colour {
        case .red: self = .red
        case .orange: self = .orange
        case .yellow: self = .yellow
        case .green: self = .green
        case .blue: self = .blue
        case .purple: self = .purple
        case .brown: self = .brown
        }
    }
}

extension MarkColor {
    init(_ colour: LeafHighlightColour) {
        switch colour {
        case .red: self = .red
        case .orange: self = .orange
        case .yellow: self = .yellow
        case .green: self = .green
        case .blue: self = .blue
        case .purple: self = .purple
        case .brown: self = .brown
        }
    }
}

extension LeafAlignment {
    init(_ align: Align) {
        switch align {
        case .center: self = .center
        case .right: self = .right
        case .justify: self = .justify
        }
    }
}

extension Align {
    init(_ align: LeafAlignment) {
        switch align {
        case .center: self = .center
        case .right: self = .right
        case .justify: self = .justify
        }
    }
}
