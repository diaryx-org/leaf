//  PictureSessionTests.swift
//
//  A host's picture session (`RegionPictureSession`): leaf drives it with keys
//  and draws its picture, here for an inline directive, `:chip[abc]`. Leaf
//  draws the caret and the selection where the session says they stand,
//  composes an input method's text itself and passes only what is committed,
//  coalesces a run of typing into one undo step, gives the host the caret's
//  rect for a popover, and lets a host replace leaf's own math editor.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class PictureSessionTests: XCTestCase {
    private let source = "Before :chip[abc] after\n"
    private let none = NSRange(location: NSNotFound, length: 0)

    /// What the sessions were asked, in order.
    private final class Log { var entries: [String] = [] }

    /// A chip whose label is edited a letter at a time: the caret is an index
    /// into the label, drawn 10 points a letter; ← and → walk it and leave at
    /// the ends; typing inserts at it; ⇧→ selects a letter.
    private final class Chip: RegionPictureSession {
        let log: Log
        var label: String
        var caret: Int
        var anchor: Int?
        /// Where the label starts in the directive's content: `:chip[`.
        let labelAt = 6

        init(_ region: RegionView, entry: RegionEntry, log: Log) {
            self.log = log
            label = region.label
            switch entry {
            case .home: caret = 0
            case .end: caret = region.label.count
            case let .point(p, _): caret = min(region.label.count, max(0, Int((p.x / 10).rounded())))
            }
            log.entries.append("open \(caret)")
        }

        func picture(size: CGFloat, ink: LeafColor) -> RegionSessionPicture? {
            let picture = RegionPicture(width: CGFloat(label.count) * 10, height: 16, baseline: 12) { _, _ in }
            let selection = anchor.map { a in
                [CGRect(x: CGFloat(min(a, caret)) * 10, y: 0, width: CGFloat(abs(a - caret)) * 10, height: 16)]
            } ?? []
            return RegionSessionPicture(picture: picture, caret: CGRect(x: CGFloat(caret) * 10, y: 2, width: 0, height: 12),
                                        selection: selection)
        }

        func input(_ input: RegionInput) -> RegionOutcome {
            log.entries.append("key \(input)")
            switch input {
            case .left where caret == 0: return .leave(atEnd: false)
            case .right where caret == label.count: return .leave(atEnd: true)
            case .left: caret -= 1; anchor = nil
            case .right: caret += 1; anchor = nil
            case .extendRight where caret < label.count:
                if anchor == nil { anchor = caret }
                caret += 1
            default: return .pass
            }
            return .moved
        }

        func type(_ text: String) -> RegionOutcome {
            log.entries.append("type \(text)")
            let at = UInt32(labelAt + label.utf8.prefix(caret).count)
            label.insert(contentsOf: text, at: label.index(label.startIndex, offsetBy: caret))
            caret += text.count
            return .edited([RegionSplice(from: at, to: at, text: text)])
        }

        func reload(content: String) -> Bool {
            label = String(content.dropFirst(labelAt).dropLast())
            caret = min(caret, label.count)
            return true
        }

        var accessibilityValue: String? { "chip \(label)" }

        func close() { log.entries.append("close") }
    }

    private var hosts: [RegionHost] = []

    private func view(_ log: Log, source: String? = nil) throws -> LeafTextView {
        let doc = try LeafDoc(source: source ?? self.source, format: "markdown")
        // What `LeafEditorModel` says for every document it shows.
        _ = doc.setInlinePictures(on: true)
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.regionRenderers = ["directive:chip": RegionRenderer { r, _ in
            RegionPicture(width: CGFloat(r.label.count) * 10, height: 16, baseline: 12) { _, _ in }
        }]
        view.regionEditors = ["directive:chip": RegionEditor { [unowned self] region, entry, host in
            hosts.append(host)
            return Chip(region, entry: entry, log: log)
        }]
        view.layout()
        return view
    }

    private func region(_ view: LeafTextView) throws -> RegionView {
        try XCTUnwrap(view.doc.view().regions.first { $0.kind == "directive:chip" })
    }

    private func place(_ view: LeafTextView, _ at: UInt32) {
        view.command { $0.selectRange(start: at, end: at) }
    }

    private func send(_ selector: Selector, to view: LeafTextView) { view.doCommand(by: selector) }

    private func chip(_ view: LeafTextView) -> Chip? { view.regionSlot.open?.session as? Chip }

    func testAnInlineChipOpensByArrowsAndLeavesAtItsEnds() throws {
        let log = Log()
        let view = try view(log)
        let r = try region(view)
        XCTAssertTrue(r.inline)
        XCTAssertEqual(r.label, "abc")
        place(view, r.src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        XCTAssertEqual(log.entries, ["open 0"])
        XCTAssertTrue(view.regionSlot.isOpen)
        send(#selector(NSResponder.moveLeft(_:)), to: view)
        XCTAssertFalse(view.regionSlot.isOpen)
        XCTAssertEqual(view.doc.caretOffset(), r.src, "← at the first stop did not leave in front")
        place(view, r.end)
        send(#selector(NSResponder.moveLeft(_:)), to: view)
        XCTAssertEqual(log.entries.last, "open 3")
        send(#selector(NSResponder.moveRight(_:)), to: view)
        XCTAssertFalse(view.regionSlot.isOpen)
        XCTAssertEqual(view.doc.caretOffset(), r.end, "→ at the last stop did not leave behind")
    }

    func testAClickOnTheChipOpensItAtThePoint() throws {
        let log = Log()
        let view = try view(log)
        let frame = try XCTUnwrap(view.layoutEngine.pictureFrame(kind: "directive:chip", src: try region(view).src,
                                                                 theme: view.theme))
        XCTAssertEqual(frame.rect.width, 30)
        XCTAssertTrue(view.regionSlot.click(at: CGPoint(x: frame.rect.minX + 21, y: frame.rect.midY),
                                            in: view.layoutEngine, theme: view.theme))
        XCTAssertEqual(log.entries, ["open 2"])
    }

    func testLeafDrawsTheCaretAndTheSelectionWhereTheSessionSays() throws {
        let log = Log()
        let view = try view(log)
        place(view, try region(view).src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        let frame = try XCTUnwrap(view.regionSlot.regionRect())
        let caret = try XCTUnwrap(view.regionSlot.caretRect())
        XCTAssertEqual(caret.midX, frame.minX + 10, accuracy: 0.01)
        XCTAssertEqual(caret.minY, frame.minY + 2, accuracy: 0.01)
        XCTAssertEqual(caret.height, 12, accuracy: 0.01)
        XCTAssertEqual(view.shownCaretRect, caret, "leaf's caret is not the session's")
        XCTAssertEqual(view.regionSlot.selectionRects(), [])
        send(#selector(NSResponder.moveRightAndModifySelection(_:)), to: view)
        let selection = view.regionSlot.selectionRects()
        XCTAssertEqual(selection.count, 1)
        XCTAssertEqual(selection.first?.minX ?? 0, frame.minX + 10, accuracy: 0.01)
        XCTAssertEqual(selection.first?.width ?? 0, 10, accuracy: 0.01)
        // The host's view of the same, for a popover.
        let host = try XCTUnwrap(hosts.last)
        XCTAssertEqual(host.caretRect(), view.regionSlot.caretRect().map(view.regionViewRect))
        XCTAssertEqual(host.regionRect(), view.regionViewRect(frame))
        // Gone once the region closes.
        view.regionSlot.close()
        XCTAssertNil(host.caretRect())
    }

    func testARunOfTypingIsOneUndoStepAndAKeyEndsIt() throws {
        let log = Log()
        let view = try view(log)
        place(view, try region(view).end)
        send(#selector(NSResponder.moveLeft(_:)), to: view)
        for c in "xy" { view.insertText(String(c), replacementRange: none) }
        send(#selector(NSResponder.moveLeft(_:)), to: view)
        view.insertText("z", replacementRange: none)
        XCTAssertEqual(view.doc.source(), "Before :chip[abcxzy] after\n")
        view.undo(nil)
        XCTAssertEqual(view.doc.source(), "Before :chip[abcxy] after\n")
        view.undo(nil)
        XCTAssertEqual(view.doc.source(), source, "the run before the key was not one step")
    }

    func testLeafComposesAndPassesOnlyCommittedText() throws {
        let log = Log()
        let view = try view(log)
        place(view, try region(view).end)
        send(#selector(NSResponder.moveLeft(_:)), to: view)
        view.setMarkedText("か", selectedRange: NSRange(location: 1, length: 0), replacementRange: none)
        XCTAssertTrue(view.hasMarkedText())
        XCTAssertEqual(view.regionSlot.marked?.text, "か")
        XCTAssertEqual(view.doc.source(), source, "a composition reached the document")
        view.setMarkedText("かん", selectedRange: NSRange(location: 2, length: 0), replacementRange: none)
        XCTAssertFalse(log.entries.contains { $0.hasPrefix("type") }, "the session saw a composition")
        // The input method's window goes at the caret.
        let caret = try XCTUnwrap(view.regionSlot.caretRect())
        XCTAssertNotEqual(view.firstRect(forCharacterRange: view.markedRange(), actualRange: nil), .zero)
        view.insertText("漢", replacementRange: none)
        XCTAssertFalse(view.hasMarkedText())
        XCTAssertEqual(log.entries.last, "type 漢")
        XCTAssertEqual(view.doc.source(), "Before :chip[abc漢] after\n")
        // A composition the focus leaves is committed.
        view.setMarkedText("え", selectedRange: NSRange(location: 1, length: 0), replacementRange: none)
        view.unmarkText()
        XCTAssertEqual(log.entries.last, "type え")
        XCTAssertTrue(caret.height > 0)
    }

    func testAKeyTheSessionDoesNotKnowLeavesAndActs() throws {
        let log = Log()
        let view = try view(log)
        place(view, try region(view).src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        send(#selector(NSResponder.moveWordRight(_:)), to: view)
        XCTAssertEqual(log.entries.suffix(2), ["key WordRight", "close"])
        XCTAssertFalse(view.regionSlot.isOpen)
        XCTAssertEqual(RegionInput("Hyper"), RegionInput("Hyper"))
    }

    func testTheOpenChipIsWhatAccessibilityReads() throws {
        let log = Log()
        let view = try view(log)
        place(view, try region(view).src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        XCTAssertEqual(view.accessibilitySelectedText(), "chip abc")
    }

    func testAnInlineRegionIsNeverAView() throws {
        let log = Log()
        let view = try view(log)
        final class Boxed: RegionSession {
            var view: LeafView? { NSView() }
        }
        view.regionEditors = ["directive:chip": RegionEditor { _, _, _ in Boxed() }]
        place(view, try region(view).src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        XCTAssertFalse(view.regionSlot.isOpen, "a view stood in a line of text")
    }

    func testAHostsMathEditorReplacesLeafs() throws {
        let log = Log()
        let doc = try LeafDoc(source: "say $x^2$ here\n", format: "markdown")
        _ = doc.setInlinePictures(on: true)
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        final class Formula: RegionPictureSession {
            let tex: String
            init(_ tex: String) { self.tex = tex }
            func picture(size: CGFloat, ink: LeafColor) -> RegionSessionPicture? {
                RegionSessionPicture(picture: RegionPicture(width: 20, height: 10) { _, _ in }, caret: nil)
            }
        }
        view.regionEditors = ["math": RegionEditor { region, _, _ in
            log.entries.append("math \(region.content)")
            return Formula(region.content)
        }]
        view.layout()
        let r = try XCTUnwrap(view.doc.view().regions.first { $0.kind == "math" })
        place(view, r.src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        XCTAssertEqual(log.entries, ["math x^2"])
        XCTAssertTrue(view.regionSlot.open?.session is Formula)
        XCTAssertNil(view.regionSlot.caretRect(), "a session that hides its caret showed one")
        // Taking the host's away gives formulas back to leaf's editor.
        view.regionEditors = [:]
        place(view, r.src)
        send(#selector(NSResponder.moveRight(_:)), to: view)
        XCTAssertTrue(view.regionSlot.open?.session is LeafRegionSession)
    }
}
#endif
