//  RegionEditingTests.swift
//
//  A fence a host's editor edits, as the macOS view answers it: drawn by the
//  editor's view, opened by → or a click into a session whose view stands
//  where the drawing was, and written back through its `RegionHost` — see
//  `RegionEditing.swift`. The editor here is a stand-in for a Mermaid one.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class RegionEditingTests: XCTestCase {
    private let source = "intro\n\n```demo\nA\n```\n\nend\n"

    /// What the stand-in editor was asked, in order.
    private final class Log {
        var entries: [String] = []
    }

    /// A session with a plain view of a fixed height, keeping its host.
    private final class DemoSession: RegionSession {
        let host: RegionHost
        let log: Log
        let box = NSView(frame: NSRect(x: 0, y: 0, width: 100, height: 120))
        var view: LeafView? { box }

        init(host: RegionHost, log: Log) {
            self.host = host
            self.log = log
        }

        func reload(content: String) -> Bool {
            log.entries.append("reload \(content)")
            return true
        }

        func close() { log.entries.append("close") }
    }

    private var sessions: [DemoSession] = []

    private func editor(_ log: Log) throws -> LeafTextView {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.regionEditors = [
            "fence:demo": RegionEditor(
                view: { r in
                    log.entries.append("view \(r.content)")
                    return NSView(frame: NSRect(x: 0, y: 0, width: 100, height: 60))
                },
                open: { [unowned self] _, entry, host in
                    log.entries.append("open \(entry)")
                    let s = DemoSession(host: host, log: log)
                    sessions.append(s)
                    return s
                }),
            // Not a fence: not the host's to edit.
            "math": RegionEditor(open: { _, _, _ in nil }),
        ]
        view.layout()
        return view
    }

    private func region(_ view: LeafTextView) -> RegionView { view.doc.view().regions[0] }

    private func place(_ view: LeafTextView, _ at: UInt32) {
        view.command { $0.selectRange(start: at, end: at) }
    }

    func testAFenceTheHostEditsIsItsDrawing() throws {
        let log = Log()
        let view = try editor(log)
        XCTAssertEqual(view.regionEditors.keys.sorted(), ["fence:demo"])
        XCTAssertEqual(view.doc.view().regions.map(\.kind), ["fence:demo"])
        XCTAssertEqual(region(view).content, "A\n")
        XCTAssertEqual(log.entries, ["view A\n"])
        let rects = view.layoutEngine.regionRects()
        XCTAssertEqual(rects.count, 1)
        XCTAssertEqual(rects.values.first?.height, 60)
    }

    func testAnArrowOpensItsViewWhereTheDrawingWas() throws {
        let log = Log()
        let view = try editor(log)
        let drawn = try XCTUnwrap(view.layoutEngine.regionRects().values.first)
        place(view, region(view).src)
        view.doCommand(by: #selector(NSResponder.moveRight(_:)))
        let s = try XCTUnwrap(sessions.last)
        XCTAssertEqual(log.entries.last, "open home")
        XCTAssertTrue(s.box.superview === view, "the session's view is not in the text view")
        XCTAssertEqual(s.box.frame.minY, drawn.minY, accuracy: 0.5)
        XCTAssertEqual(s.box.frame.height, 120, "the open view sets its own height")
        XCTAssertTrue(view.regionSlot.isOpen)
    }

    func testWhatTheHostWritesIsOneUndoStepAndUndoReloadsIt() throws {
        let log = Log()
        let view = try editor(log)
        place(view, region(view).end)
        view.doCommand(by: #selector(NSResponder.moveLeft(_:)))
        let s = try XCTUnwrap(sessions.last)
        XCTAssertEqual(log.entries.last, "open end")
        s.host.replace("A\nB é\n")
        XCTAssertEqual(view.doc.source(), "intro\n\n```demo\nA\nB é\n```\n\nend\n")
        s.host.edit([RegionSplice(from: 0, to: 1, text: "Z")])
        XCTAssertEqual(view.doc.source(), "intro\n\n```demo\nZ\nB é\n```\n\nend\n")
        XCTAssertTrue(view.regionSlot.isOpen, "an edit closed the host's view")
        s.host.undo()
        XCTAssertEqual(view.doc.source(), "intro\n\n```demo\nA\nB é\n```\n\nend\n")
        XCTAssertEqual(log.entries.last, "reload A\nB é\n")
        XCTAssertTrue(view.regionSlot.isOpen, "undo closed the host's view")
        // Leaving puts the caret behind the fence and the drawing back.
        s.host.leave(atEnd: true)
        XCTAssertFalse(view.regionSlot.isOpen)
        XCTAssertNil(s.box.superview)
        XCTAssertEqual(view.doc.caretOffset(), region(view).end)
        XCTAssertEqual(log.entries.suffix(2), ["close", "view A\nB é\n"])
        // A host that calls after it closed changes nothing.
        s.host.replace("gone\n")
        XCTAssertEqual(view.doc.source(), "intro\n\n```demo\nA\nB é\n```\n\nend\n")
    }

    func testAClickOnTheDrawingOpensItAndAClickElsewhereClosesIt() throws {
        let log = Log()
        let view = try editor(log)
        let drawn = try XCTUnwrap(view.layoutEngine.regionRects().values.first)
        XCTAssertTrue(view.regionSlot.click(at: CGPoint(x: drawn.minX + 5, y: drawn.minY + 3),
                                            in: view.layoutEngine, theme: view.theme))
        XCTAssertEqual(log.entries.last, "open point((5.0, 3.0), size: \(view.theme.fontSize))")
        XCTAssertFalse(view.regionSlot.click(at: CGPoint(x: drawn.minX + 5, y: 1), in: view.layoutEngine,
                                             theme: view.theme))
        XCTAssertFalse(view.regionSlot.isOpen)
        XCTAssertEqual(log.entries.last, "close")
    }

    func testAnUndoThatTakesTheFenceAwayClosesItsView() throws {
        let log = Log()
        let view = try editor(log)
        place(view, region(view).src)
        view.doCommand(by: #selector(NSResponder.moveRight(_:)))
        view.command { $0.editSource(from: 0, to: UInt32(self.source.utf8.count), text: "gone\n") }
        XCTAssertFalse(view.regionSlot.isOpen, "the fence went and its view stayed")
        XCTAssertEqual(log.entries.last, "close")
    }

    func testTheOneSpliceBetweenTwoContents() {
        XCTAssertEqual(RegionSplice.diff("abc", "abc"), [])
        XCTAssertEqual(RegionSplice.diff("abc", "abXc"), [RegionSplice(from: 2, to: 2, text: "X")])
        XCTAssertEqual(RegionSplice.diff("aéc", "aèc"), [RegionSplice(from: 1, to: 3, text: "è")])
        XCTAssertEqual(RegionSplice.diff("", "x\n"), [RegionSplice(from: 0, to: 0, text: "x\n")])
    }
}
#endif
