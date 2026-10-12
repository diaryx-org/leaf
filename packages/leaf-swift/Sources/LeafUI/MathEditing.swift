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
//  the formula's picture (`RegionDrawing.picture`), with its caret and empty
//  slots in it, in place of the math renderer's, and lays its row out again
//  whenever the session's revision moves.

import CoreGraphics
import Foundation
import LeafFFI
import ResvgCoreGraphics

/// A formula open for structural editing.
final class MathRegionSession: RegionPictureSession {
    let session: MathSession
    private let accent: LeafColor

    /// Open formula `region` with the caret at `entry`. `nil` when the TeX is
    /// beyond what the structural editor reads, and the formula is left as it
    /// is.
    init?(_ region: RegionView, entry: RegionEntry, accent: LeafColor) {
        guard let session = try? MathSession(tex: region.content, display: region.display) else { return nil }
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
        let selected = session.hasSelection()
        switch input {
        case .left where !selected && session.atHome(): return .leave(atEnd: false)
        case .right where !selected && session.atEnd(): return .leave(atEnd: true)
        case .cancel, .newline, .tab: return .leave(atEnd: true)
        case .left: key = .left
        case .right: key = .right
        case .up: key = .up
        case .down: key = .down
        case .extendLeft: key = .shiftLeft
        case .extendRight: key = .shiftRight
        case .backspace: key = .backspace
        case .deleteForward: key = .delete
        case .selectAll: key = .selectAll
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

    func copy() -> String? { session.selectedTex() }

    func paste(_ text: String) -> RegionOutcome { outcome(session.paste(tex: text)) }

    func picture(size: CGFloat, ink: LeafColor) -> RegionPicture? {
        let rgba = ink.rgbaBytes
        let accent = accent.rgbaBytes
        guard let p = try? session.picture(size: Double(size),
                                           ink: Data([rgba.0, rgba.1, rgba.2, rgba.3]),
                                           accent: Data([accent.0, accent.1, accent.2, accent.3]))
        else { return nil }
        return MathStore.picture(from: p, size: size)
    }

    private func outcome(_ splices: [MathSplice]) -> RegionOutcome {
        splices.isEmpty ? .moved : .edited(splices.map { RegionSplice(from: $0.from, to: $0.to, text: $0.text) })
    }
}
