//  EditorLayout.swift
//
//  The platform-neutral geometry of a rendered frame. Core hands back a
//  `DocView` whose rows are already wrapped to a column budget, so each `Row` is
//  one visual line: this turns that into laid-out `CTLine`s stacked top-down, and
//  answers the two geometry questions both the AppKit and UIKit views need — where
//  the caret sits, and which `(row, ch)` a point hits. All of it is Core Text +
//  Foundation, so it compiles once for both toolkits.

import CoreGraphics
import CoreText
import Foundation
import LeafFFI

/// Toolbar/chrome state pushed to the host after every repaint — the subset of a
/// `DocView` a surrounding UI reflects. Platform-neutral so both views share it.
public struct EditorState: Equatable {
    public var view: String          // "wysiwyg" | "source"
    public var dirty: Bool
    public var heading: UInt32?      // heading level at the caret, or nil
    public var active: [String]      // inline marks active at the caret

    public init(view: String, dirty: Bool, heading: UInt32?, active: [String]) {
        self.view = view; self.dirty = dirty; self.heading = heading; self.active = active
    }

    /// Project a full `DocView` down to the chrome-facing state.
    public init(_ v: DocView) {
        self.init(view: v.view, dirty: v.dirty, heading: v.heading, active: v.active)
    }
}

/// One visual row's laid-out geometry.
struct RowLayout {
    let row: Row
    let attributed: NSAttributedString
    let line: CTLine
    let top: CGFloat
    let height: CGFloat
}

/// The laid-out rows of one `DocView` plus the geometry queries over them.
struct EditorLayout {
    let rows: [RowLayout]
    /// Total content height including top+bottom padding — the view's fitting size.
    let contentHeight: CGFloat

    init(_ docView: DocView, theme: EditorTheme) {
        var layouts: [RowLayout] = []
        layouts.reserveCapacity(docView.rows.count)
        var y = theme.padding.top
        for row in docView.rows {
            let attributed = AttributedRow.make(row, theme: theme)
            let line = CTLineCreateWithAttributedString(attributed)
            let h = theme.rowHeight(heading: row.heading)
            layouts.append(RowLayout(row: row, attributed: attributed, line: line, top: y, height: h))
            y += h
        }
        rows = layouts
        contentHeight = y + theme.padding.bottom
    }

    /// A 1.5pt-wide vertical rect at row `row`, UTF-16 offset `ch` — the geometry a
    /// caret or a selection endpoint occupies. `nil` if the row is out of range.
    func rect(row: Int, ch: Int, theme: EditorTheme) -> CGRect? {
        guard rows.indices.contains(row) else { return nil }
        let rl = rows[row]
        let x = CTLineGetOffsetForStringIndex(rl.line, CFIndex(ch), nil)
        return CGRect(x: theme.padding.left + x, y: rl.top, width: 1.5, height: rl.height)
    }

    /// The caret's frame — column `caret_ch` (UTF-16) mapped through Core Text to a
    /// pixel x, spanning its row's height. `nil` if the caret row is out of range.
    func caretRect(_ docView: DocView, theme: EditorTheme) -> CGRect? {
        rect(row: Int(docView.caretRow), ch: Int(docView.caretCh), theme: theme)
    }

    /// Map a point (view coordinates) to core's `(row, ch)`: the row from the
    /// vertical band it falls in, the UTF-16 offset from Core Text's hit-test of
    /// the horizontal position. `click_ch` then clamps `ch` to a real caret stop.
    func hit(_ point: CGPoint, theme: EditorTheme) -> (row: Int, ch: Int) {
        guard !rows.isEmpty else { return (0, 0) }
        let row = rows.firstIndex { point.y < $0.top + $0.height } ?? rows.count - 1
        let rl = rows[row]
        let localX = point.x - theme.padding.left
        let idx = CTLineGetStringIndexForPosition(rl.line, CGPoint(x: max(0, localX), y: 0))
        return (row, min(max(0, idx), rl.attributed.length))
    }

    /// Fill the selection background behind every run core marked `sel`, at the
    /// given row, into `ctx` (driven directly so the same code paints on AppKit and
    /// UIKit). Core has already carved the selection into run boundaries, so no
    /// offset math is needed beyond walking run lengths.
    func fillSelection(row rl: RowLayout, padLeft: CGFloat, color: LeafColor, in ctx: CGContext) {
        var utf16 = 0
        ctx.setFillColor(color.cgColor)
        for run in rl.row.runs {
            let len = run.text.utf16.count
            if run.sel {
                let x0 = CTLineGetOffsetForStringIndex(rl.line, CFIndex(utf16), nil)
                let x1 = CTLineGetOffsetForStringIndex(rl.line, CFIndex(utf16 + len), nil)
                ctx.fill(CGRect(x: padLeft + x0, y: rl.top, width: x1 - x0, height: rl.height))
            }
            utf16 += len
        }
    }
}
