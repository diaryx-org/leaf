//  MathEditing.swift
//
//  A formula edited in place, as structure. Once a view has told core it edits
//  formulas itself (`setEditsMath`), a formula on the caret's line stays its
//  picture; → in front of one, ← behind it, or a click on its picture opens it
//  in a `MathSession` (leaf-math-edit, through the binding). Then `/` makes a
//  fraction of the term before the caret, `^` and `_` open scripts, `sqrt` a
//  root, and the arrows walk the slots. What the session writes is TeX: each
//  keystroke answers with splices on the formula's bytes, which land in the
//  document through `editSource` as one undo step — the web editor's
//  behaviour, from the same Rust.
//
//  The view draws the open formula from the session, its caret and empty
//  slots in it. It does that without the layout knowing: the frame it lays
//  out names the open formula by a stand-in TeX unique to each revision, and
//  `MathStore` answers that name with the session's picture. A new revision
//  is a formula that differs, which is exactly what the layout already
//  re-sets a row for.

import CoreGraphics
import Foundation
import LeafFFI
import ResvgCoreGraphics

/// The formula one view has open for structural editing.
final class OpenFormula {
    let session: MathSession
    /// The formula's source start: core's caret waits here, in front of the
    /// picture, while it is open.
    let src: UInt32
    /// Its source end and the start of its TeX, followed across every frame
    /// (`follow`), since the formula's own edits move the one.
    private(set) var end: UInt32
    private(set) var texStart: UInt32
    let display: Bool
    /// Bumped by every change to what the picture shows — a keystroke, a
    /// caret move inside it — so the frame names it afresh.
    private(set) var revision = 1
    private let token = UUID().uuidString

    /// Where the caret goes when a formula is opened.
    enum Entry {
        case home, end
        /// Nearest a point, in points from the picture's top left, at
        /// `size` points per em.
        case point(CGPoint, size: CGFloat)
    }

    /// Open `math` with the caret at `entry`. `nil` when the TeX is beyond
    /// what the structural editor reads, and the formula is left as it is.
    init?(_ math: MathView, entry: Entry, accent: LeafColor) {
        guard let session = try? MathSession(tex: math.tex, display: math.display) else { return nil }
        self.session = session
        src = math.src
        end = math.end
        texStart = math.texStart
        display = math.display
        switch entry {
        case .home: session.home()
        case .end: session.end()
        case let .point(p, size): session.click(x: Double(p.x), y: Double(p.y), size: Double(size))
        }
        let accent = accent.rgbaBytes
        MathStore.open[token] = { [weak session] size, ink in
            guard let session else { return nil }
            let rgba = ink.rgbaBytes
            guard let p = try? session.picture(size: Double(size),
                                               ink: Data([rgba.0, rgba.1, rgba.2, rgba.3]),
                                               accent: Data([accent.0, accent.1, accent.2, accent.3]))
            else { return nil }
            return MathStore.glyph(from: p, size: size)
        }
    }

    deinit { MathStore.open[token] = nil }

    /// The TeX the laid-out frame names this formula by: unique to the view
    /// and the revision, and recognised by `MathStore`.
    var standIn: String { MathStore.openPrefix + token + " " + String(revision) }

    /// `frame` as the layout should see it: the open formula named by its
    /// stand-in, every other formula as it is.
    func laidOut(_ frame: DocView) -> DocView {
        var f = frame
        f.math = frame.math.map { m in
            guard m.src == src else { return m }
            var m = m
            m.tex = standIn
            return m
        }
        return f
    }

    /// Follow the formula into a new frame: its end and TeX start, and its
    /// TeX, which an undo can change under the session. `false` when the
    /// frame has no formula here any more, or one the session cannot read,
    /// and the view closes it.
    func follow(_ frame: DocView) -> Bool {
        guard let m = frame.math.first(where: { $0.src == src }) else { return false }
        end = m.end
        texStart = m.texStart
        if m.tex != session.tex() {
            guard (try? session.reload(tex: m.tex)) != nil else { return false }
            revision += 1
        }
        return true
    }

    /// What a key did, for the view to act on.
    enum Outcome {
        /// The document changed: render this frame.
        case edited(DocView)
        /// Only the caret inside the formula moved: redraw it.
        case moved
        /// Leave the formula, the caret in front of it or behind it.
        case leave(atEnd: Bool)
    }

    /// Apply `key` — through the session, and its splices into `doc` as one
    /// undo step. ← at the first stop and → at the last leave the formula.
    func press(_ key: MathKey, in doc: LeafDoc) -> Outcome {
        switch key {
        case .left where session.atHome(): return .leave(atEnd: false)
        case .right where session.atEnd(): return .leave(atEnd: true)
        default: break
        }
        return apply(session.key(key: key), in: doc)
    }

    /// Typed text, one character at a time: `sqrt` is four keys, the last of
    /// which turns the letters into a root.
    func type(_ text: String, in doc: LeafDoc) -> Outcome {
        var splices: [MathSplice] = []
        for c in text { splices += session.key(key: .char(ch: String(c))) }
        return apply(splices, in: doc)
    }

    private func apply(_ splices: [MathSplice], in doc: LeafDoc) -> Outcome {
        revision += 1
        guard !splices.isEmpty else { return .moved }
        // Every splice is against the TeX as the one before it left it, and
        // none moves the TeX's start.
        doc.beginUndoGroup()
        for s in splices {
            _ = doc.editSource(from: texStart + s.from, to: texStart + s.to, text: s.text)
        }
        doc.endUndoGroup()
        return .edited(doc.selectRange(start: src, end: src))
    }

    /// A click inside the open formula's picture.
    func click(_ p: CGPoint, size: CGFloat) {
        session.click(x: Double(p.x), y: Double(p.y), size: Double(size))
        revision += 1
    }
}

extension MathStore {
    /// The stand-in TeX's prefix: a byte no real formula's TeX can start with.
    static let openPrefix = "\u{0}leaf-open "

    /// The open formulas' pictures, by their stand-in's token: a picture at a
    /// size in an ink, drawn by the session with its caret.
    static var open: [String: (CGFloat, LeafColor) -> MathGlyph?] = [:]

    /// The open formula a stand-in names, drawn — or `nil` when `tex` is not
    /// a stand-in.
    static func openGlyph(tex: String, size: CGFloat, ink: LeafColor) -> MathGlyph?? {
        guard tex.hasPrefix(openPrefix) else { return nil }
        let token = tex.dropFirst(openPrefix.count).split(separator: " ").first.map(String.init) ?? ""
        return .some(open[token]?(size, ink) ?? nil)
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
                      let glyph = MathStore.glyph(tex: m.tex, display: m.display, size: size, ink: theme.textColor),
                      let base = baselineOrigin(row: i, ch: ch)
                else { continue }
                let r = CGRect(x: base.x, y: base.y - glyph.ascent, width: glyph.width, height: glyph.ascent + glyph.descent)
                if r.contains(point) { return (m, r, size) }
            }
        }
        return nil
    }
}
