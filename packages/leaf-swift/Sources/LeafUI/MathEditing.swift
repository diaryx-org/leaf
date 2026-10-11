//  MathEditing.swift
//
//  A formula edited in place, as structure — leaf's own client of the region
//  protocol (`RegionEditing.swift`). Once a view owns the `math` kind, a
//  formula on the caret's line stays its picture; → in front of one, ← behind
//  it, or a click on its picture opens it in a `MathSession` (leaf-math-edit,
//  through the binding). Then `/` makes a fraction of the term before the
//  caret, `^` and `_` open scripts, `sqrt` a root, and the arrows walk the
//  slots. What the session writes is TeX: each keystroke answers with splices
//  on the formula's bytes, which the view lands through `editSource` as one
//  undo step — the web editor's behaviour, from the same Rust.
//
//  Leaf runs this session and draws it: the layout asks the open region for
//  the formula's picture (`OpenRegionLayout.glyph`), with its caret and empty
//  slots in it, and lays its row out again whenever the session's revision
//  moves.

import CoreGraphics
import Foundation
import LeafFFI
import ResvgCoreGraphics

/// A formula open for structural editing.
final class MathRegionSession: RegionPictureSession {
    /// The region kind every formula is.
    static let kind = "math"

    let session: MathSession
    private let accent: LeafColor

    /// Open `region`, the formula `math`, with the caret at `entry`. `nil` when
    /// the TeX is beyond what the structural editor reads, and the formula is
    /// left as it is.
    init?(_ region: RegionView, display: Bool, entry: RegionEntry, accent: LeafColor) {
        guard let session = try? MathSession(tex: region.content, display: display) else { return nil }
        self.session = session
        self.accent = accent
        switch entry {
        case .home: session.home()
        case .end: session.end()
        case let .point(p, size): session.click(x: Double(p.x), y: Double(p.y), size: Double(size))
        }
    }

    /// The arrows and Backspace are the session's; ← at the first stop and →
    /// at the last leave the formula, as do Escape, Return and Tab, behind it.
    func input(_ input: RegionInput) -> RegionOutcome {
        let key: MathKey
        switch input {
        case .left where session.atHome(): return .leave(atEnd: false)
        case .right where session.atEnd(): return .leave(atEnd: true)
        case .cancel, .newline, .tab: return .leave(atEnd: true)
        case .left: key = .left
        case .right: key = .right
        case .up: key = .up
        case .down: key = .down
        case .backspace: key = .backspace
        }
        return outcome(session.key(key: key))
    }

    /// Typed text, one character at a time: `sqrt` is four keys, the last of
    /// which turns the letters into a root.
    func type(_ text: String) -> RegionOutcome {
        var splices: [MathSplice] = []
        for c in text { splices += session.key(key: .char(ch: String(c))) }
        return outcome(splices)
    }

    func click(at point: CGPoint, size: CGFloat) -> RegionOutcome {
        session.click(x: Double(point.x), y: Double(point.y), size: Double(size))
        return .moved
    }

    func reload(content: String) -> Bool {
        (try? session.reload(tex: content)) != nil
    }

    func picture(size: CGFloat, ink: LeafColor) -> MathGlyph? {
        let rgba = ink.rgbaBytes
        let accent = accent.rgbaBytes
        guard let p = try? session.picture(size: Double(size),
                                           ink: Data([rgba.0, rgba.1, rgba.2, rgba.3]),
                                           accent: Data([accent.0, accent.1, accent.2, accent.3]))
        else { return nil }
        return MathStore.glyph(from: p, size: size)
    }

    private func outcome(_ splices: [MathSplice]) -> RegionOutcome {
        splices.isEmpty ? .moved : .edited(splices.map { RegionSplice(from: $0.from, to: $0.to, text: $0.text) })
    }
}

extension EditorLayout {
    /// The formula whose picture is at `point` — a display block's box, or an
    /// inline formula's attachment — with that picture's rect and the size it
    /// is set at, in points per em, so a click can be handed to a session.
    func mathPicture(at point: CGPoint, theme: EditorTheme) -> (math: MathView, rect: CGRect, size: CGFloat)? {
        for (i, rl) in rows.enumerated() {
            if rl.mathFirst, let box = rl.math {
                let r = box.rect(top: rl.mathTop, left: rl.originX + rl.shaped.prefixWidth,
                                 width: rl.columnWidth - rl.shaped.prefixWidth)
                if r.contains(point) {
                    // A block fitted to a narrow column is drawn smaller.
                    let scale = box.glyph.width > 0 ? box.size.width / box.glyph.width : 1
                    return (box.math, r, theme.fontSize * scale)
                }
                continue
            }
            guard rl.drawsItsGlyphs, rl.row.runs.contains(where: { $0.role == "math" }) else { continue }
            let size = rl.row.heading.map { theme.headingSize(Int($0)) } ?? theme.fontSize
            var ch = 0
            for run in rl.row.runs {
                defer { ch += run.text.utf16.count }
                guard run.role == "math",
                      let m = math.first(where: { $0.inline && $0.src == run.src }),
                      let glyph = OpenRegionLayout.glyph(m, size: size, ink: theme.textColor, open: open),
                      let base = baselineOrigin(row: i, ch: ch)
                else { continue }
                let r = CGRect(x: base.x, y: base.y - glyph.ascent, width: glyph.width, height: glyph.ascent + glyph.descent)
                if r.contains(point) { return (m, r, size) }
            }
        }
        return nil
    }
}
