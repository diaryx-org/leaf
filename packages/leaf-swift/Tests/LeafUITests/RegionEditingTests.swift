//  RegionEditingTests.swift
//
//  A fence a host's editor edits, as the macOS view answers it: drawn as its
//  renderer's picture, opened by → or a click into a session whose view stands
//  where the picture was, and written back through its `RegionHost` — see
//  `RegionEditing.swift`. The renderer and editor here are stand-ins for
//  Mermaid's.

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
        let box = DemoBox(frame: NSRect(x: 0, y: 0, width: 100, height: 120))
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

    private final class DemoBox: NSView {}

    private var sessions: [DemoSession] = []

    private func editor(_ log: Log) throws -> LeafTextView {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.regionRenderers = [
            "fence:demo": { r, _ in
                log.entries.append("render \(r.content)")
                return RegionPicture(width: 100, height: 60) { _, _ in }
            },
            // Not a kind a host may name.
            "code": { _, _ in nil },
        ]
        view.regionEditors = [
            "fence:demo": RegionEditor(
                open: { [unowned self] _, entry, host in
                    log.entries.append("open \(entry)")
                    let s = DemoSession(host: host, log: log)
                    sessions.append(s)
                    return s
                }),
            // Not kinds a host may edit: leaf's own name, and formulas, which
            // leaf's session edits inside a line.
            "directive:page-break": RegionEditor(open: { _, _, _ in nil }),
            "math": RegionEditor(open: { _, _, _ in nil }),
        ]
        view.layout()
        return view
    }

    private func region(_ view: LeafTextView) -> RegionView { view.doc.view().regions[0] }

    private func place(_ view: LeafTextView, _ at: UInt32) {
        view.command { $0.selectRange(start: at, end: at) }
    }

    /// The picture of the demo fence, where the layout put it.
    private func drawn(_ view: LeafTextView) -> CGRect? {
        view.layoutEngine.pictureRects().first { $0.key.hasPrefix("fence:demo@") }?.value
    }

    func testAFenceTheHostDrawsIsItsPicture() throws {
        let log = Log()
        let view = try editor(log)
        XCTAssertEqual(view.regionEditors.keys.sorted(), ["fence:demo"])
        XCTAssertEqual(view.regionRenderers.keys.sorted(), ["fence:demo"])
        XCTAssertEqual(view.doc.view().regions.map(\.kind), ["fence:demo"])
        XCTAssertEqual(region(view).content, "A\n")
        XCTAssertEqual(log.entries, ["render A\n"], "the picture was not kept")
        XCTAssertEqual(drawn(view)?.height, 60)
        XCTAssertTrue(view.subviews.allSatisfy { !($0 is DemoBox) }, "a closed region is a live view")
        // Asked again, it draws anew.
        view.redrawRegions(kind: "fence:demo")
        XCTAssertEqual(log.entries, ["render A\n", "render A\n"])
        view.redrawRegions(kind: "fence:other")
        XCTAssertEqual(log.entries.count, 2, "redrawing another kind drew this one")
    }

    func testARendererAloneDrawsTheFenceAndLeavesItShut() throws {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.regionRenderers = ["fence:demo": { _, _ in RegionPicture(width: 80, height: 40) { _, _ in } }]
        view.layout()
        XCTAssertEqual(drawn(view)?.height, 40)
        let r = region(view)
        place(view, r.src)
        view.doCommand(by: #selector(NSResponder.moveRight(_:)))
        XCTAssertFalse(view.regionSlot.isOpen)
        XCTAssertEqual(view.doc.caretOffset(), r.end, "→ did not step over the picture")
    }

    func testAPictureComesOutOnPaperAndPrintingKeepsTheViewsRegions() throws {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        var drawnOnPaper = 0
        var dark: [Bool] = []
        view.regionRenderers = ["fence:demo": { _, context in
            dark.append(context.dark)
            return RegionPicture(width: 80, height: 40) { _, _ in drawnOnPaper += 1 }
        }]
        view.regionEditors = ["fence:demo": RegionEditor(open: { _, _, _ in nil })]
        view.appearance = NSAppearance(named: .darkAqua)
        view.redrawRegions()
        view.layout()
        XCTAssertEqual(dark.last, true, "the renderer was not told the view is dark")
        drawnOnPaper = 0
        let pdf = view.pdfData(page: .usLetter)
        XCTAssertFalse(pdf.isEmpty)
        XCTAssertGreaterThan(drawnOnPaper, 0, "the picture is not on the page")
        XCTAssertEqual(dark.last, false, "paper is drawn dark")
        // The sheet shares the document: it must own what the view owns.
        XCTAssertEqual(view.doc.view().regions.map(\.kind), ["fence:demo"])
    }

    func testARendererThatDeclinesLeavesThePlaceholder() throws {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.regionRenderers = ["fence:demo": { _, _ in nil }]
        view.layout()
        XCTAssertNil(drawn(view))
        XCTAssertTrue(view.layoutEngine.rows.contains { $0.row.runs.map(\.text).joined().hasPrefix("▤") })
    }

    func testAnArrowOpensItsViewWhereTheDrawingWas() throws {
        let log = Log()
        let view = try editor(log)
        let drawn = try XCTUnwrap(drawn(view))
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
        XCTAssertEqual(log.entries.suffix(2), ["close", "render A\nB é\n"])
        // A host that calls after it closed changes nothing.
        s.host.replace("gone\n")
        XCTAssertEqual(view.doc.source(), "intro\n\n```demo\nA\nB é\n```\n\nend\n")
    }

    func testAClickOnTheDrawingOpensItAndAClickElsewhereClosesIt() throws {
        let log = Log()
        let view = try editor(log)
        let drawn = try XCTUnwrap(drawn(view))
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

    func testAnEditAboveAnOpenFenceKeepsItOpen() throws {
        let log = Log()
        let view = try editor(log)
        let r = region(view)
        place(view, r.src)
        view.doCommand(by: #selector(NSResponder.moveRight(_:)))
        let s = try XCTUnwrap(sessions.last)
        view.command { $0.editSource(from: 0, to: 0, text: "more ") }
        XCTAssertTrue(view.regionSlot.isOpen, "an edit above closed the fence")
        XCTAssertEqual(view.regionSlot.open?.src, r.src + 5)
        s.host.replace("A\nC\n")
        XCTAssertEqual(view.doc.source(), "more intro\n\n```demo\nA\nC\n```\n\nend\n")
    }

    func testAnEditThatKeepsTheFencesLengthIsStillAnEdit() throws {
        let log = Log()
        let view = try editor(log)
        var edits = 0
        view.onEdit = { edits += 1 }
        place(view, region(view).src)
        view.doCommand(by: #selector(NSResponder.moveRight(_:)))
        let s = try XCTUnwrap(sessions.last)
        s.host.replace("B\n")
        XCTAssertEqual(edits, 1, "an edit inside the fence that kept its length was not reported")
        s.host.leave(atEnd: true)
        XCTAssertEqual(log.entries.last, "render B\n", "the picture was not drawn again")
    }

    func testAnEditThatWouldCloseTheFenceIsRefused() throws {
        let log = Log()
        let view = try editor(log)
        place(view, region(view).src)
        view.doCommand(by: #selector(NSResponder.moveRight(_:)))
        let s = try XCTUnwrap(sessions.last)
        s.host.replace("A\n```\n")
        XCTAssertEqual(view.doc.source(), source, "a closing fence was written into the fence")
        XCTAssertEqual(log.entries.last, "reload A\n")
        XCTAssertTrue(view.regionSlot.isOpen)
    }

    func testTheOneSpliceBetweenTwoContents() {
        XCTAssertEqual(RegionSplice.diff("abc", "abc"), [])
        XCTAssertEqual(RegionSplice.diff("abc", "abXc"), [RegionSplice(from: 2, to: 2, text: "X")])
        XCTAssertEqual(RegionSplice.diff("aéc", "aèc"), [RegionSplice(from: 1, to: 3, text: "è")])
        XCTAssertEqual(RegionSplice.diff("", "x\n"), [RegionSplice(from: 0, to: 0, text: "x\n")])
    }
}
#endif
