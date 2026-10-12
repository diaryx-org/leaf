//  LeafSessions.swift
//
//  The regions leaf edits itself — its own clients of the region protocol
//  (`RegionEditing.swift`). Each is an `EditSession` from the binding: one
//  Rust editor per kind (`editSessionKinds()`, leaf-session), so a second
//  editor written in Rust needs nothing new here. Today that is a formula,
//  edited in place as structure: once a view owns the `math` kind, a formula
//  on the caret's line stays its picture; → in front of one, ← behind it, or
//  a click on its picture opens it. Then `/` makes a fraction of the term
//  before the caret, `^` and `_` open scripts, `sqrt` a root, and the arrows
//  walk the slots. What the session writes is the region's source — TeX: each
//  keystroke answers with splices on its bytes, which the view lands through
//  `editSource` as one undo step — the web editor's behaviour, from the same
//  Rust.
//
//  Leaf runs these sessions and draws them: the layout asks the open region
//  for its picture (`RegionDrawing.picture`), with its caret and empty slots
//  in it, in place of the kind's renderer's, and lays its row out again
//  whenever the session's revision moves.

import CoreGraphics
import Foundation
import LeafFFI

/// A region open in one of leaf's own editors.
final class LeafRegionSession: RegionPictureSession {
    /// The kinds leaf's own editors take.
    static let kinds = Set(editSessionKinds())

    let session: EditSession
    private let accent: LeafColor

    /// Open `region` with the caret at `entry`. `nil` when its content is
    /// beyond what the editor reads, and the region is left as it is.
    init?(_ region: RegionView, entry: RegionEntry, accent: LeafColor) {
        guard let session = try? EditSession(kind: region.kind, content: region.content, display: region.display)
        else { return nil }
        self.session = session
        self.accent = accent
        switch entry {
        case .home: session.home()
        case .end: session.end()
        case let .point(p, size): session.click(x: Double(p.x), y: Double(p.y), size: Double(size))
        }
    }

    /// The arrows and Backspace are the session's; ← at the first stop and →
    /// at the last leave the region, as do Escape, Return and Tab, behind it.
    func input(_ input: RegionInput) -> RegionOutcome {
        let key: EditKey
        let selected = session.hasSelection()
        switch input {
        case .left where !selected && session.atHome(): return .leave(atEnd: false)
        case .right where !selected && session.atEnd(): return .leave(atEnd: true)
        case .cancel, .newline, .tab: return .leave(atEnd: true)
        case .left: key = .left
        case .right: key = .right
        case .up: key = .up
        case .down: key = .down
        case .extendLeft: key = .extendLeft
        case .extendRight: key = .extendRight
        case .backspace: key = .backspace
        case .deleteForward: key = .delete
        case .selectAll: key = .selectAll
        }
        return outcome(session.key(key: key))
    }

    /// Typed text, one character at a time: `sqrt` is four keys, the last of
    /// which turns the letters into a root.
    func type(_ text: String) -> RegionOutcome {
        var splices: [EditSplice] = []
        for c in text { splices += session.key(key: .char(ch: String(c))) }
        return outcome(splices)
    }

    func click(at point: CGPoint, size: CGFloat) -> RegionOutcome {
        session.click(x: Double(point.x), y: Double(point.y), size: Double(size))
        return .moved
    }

    func reload(content: String) -> Bool {
        (try? session.reload(content: content)) != nil
    }

    func copy() -> String? { session.selectedText() }

    func paste(_ text: String) -> RegionOutcome { outcome(session.paste(text: text)) }

    func picture(size: CGFloat, ink: LeafColor) -> RegionPicture? {
        let rgba = ink.rgbaBytes
        let accent = accent.rgbaBytes
        guard let p = try? session.picture(size: Double(size),
                                           ink: Data([rgba.0, rgba.1, rgba.2, rgba.3]),
                                           accent: Data([accent.0, accent.1, accent.2, accent.3]))
        else { return nil }
        let ascent = CGFloat(p.height) * size
        guard let picture = RegionPicture(svg: p.svg, width: CGFloat(p.width) * size,
                                          height: ascent + CGFloat(p.depth) * size,
                                          baseline: ascent, systemFonts: false),
              picture.isDrawable
        else { return nil }
        return picture
    }

    private func outcome(_ splices: [EditSplice]) -> RegionOutcome {
        splices.isEmpty ? .moved : .edited(splices.map { RegionSplice(from: $0.from, to: $0.to, text: $0.text) })
    }
}
