//  PrintingTests.swift
//
//  File ▸ Print lays the document onto the printer's paper. Verified by running
//  the same operation the menu runs, but saved to a PDF rather than sent to a
//  printer — the one place in this suite that drives a real `LeafDoc`, since a
//  page count is only meaningful over a real document.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class PrintingTests: XCTestCase {
    private func pdf(of source: String, paper: CGSize = CGSize(width: 612, height: 792)) throws -> CGPDFDocument {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let info = NSPrintInfo()
        info.paperSize = paper
        info.topMargin = 72; info.bottomMargin = 72; info.leftMargin = 72; info.rightMargin = 72
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("leaf-print-\(UUID().uuidString).pdf")
        info.jobDisposition = .save
        info.dictionary()[NSPrintInfo.AttributeKey.jobSavingURL] = url
        let operation = view.printOperation(with: info)
        operation.showsPrintPanel = false
        operation.showsProgressPanel = false
        XCTAssertTrue(operation.run(), "the print operation should complete")
        defer { try? FileManager.default.removeItem(at: url) }
        return try XCTUnwrap(CGPDFDocument(url as CFURL))
    }

    func testAShortDocumentIsOnePageOfThePrintersPaper() throws {
        let pdf = try pdf(of: "# Title\n\nA paragraph.\n")
        XCTAssertEqual(pdf.numberOfPages, 1)
        let page = try XCTUnwrap(pdf.page(at: 1))
        let box = page.getBoxRect(.mediaBox)
        XCTAssertEqual(box.width, 612, accuracy: 1)
        XCTAssertEqual(box.height, 792, accuracy: 1)
    }

    func testALongDocumentPrintsOnAsManyPagesAsItLaysOutTo() throws {
        // Enough paragraphs to overrun several US Letter sheets at 16pt/24pt
        // with inch margins — and the count must match the layout's own.
        let paragraphs = (1...120).map { "Paragraph \($0) of a long document, long enough to wrap onto more than one line at the printer's measure." }
        let source = paragraphs.joined(separator: "\n\n") + "\n"
        let pdf = try pdf(of: source)
        XCTAssertGreaterThan(pdf.numberOfPages, 2)

        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 612, height: 0)
        view.pageSetup = PageSetup(size: CGSize(width: 612, height: 792), margins: PageSetup.inch, gap: 0, backdrop: 0)
        var range = NSRange()
        XCTAssertTrue(view.knowsPageRange(&range))
        XCTAssertEqual(range.length, pdf.numberOfPages, "one printed page per laid-out sheet")
    }

    func testTheContinuousViewDoesNotClaimToKnowItsPages() throws {
        let doc = try LeafDoc(source: "hello\n", format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        var range = NSRange()
        XCTAssertFalse(view.knowsPageRange(&range), "off paper, AppKit's own slicing takes over")
    }
}
#endif
