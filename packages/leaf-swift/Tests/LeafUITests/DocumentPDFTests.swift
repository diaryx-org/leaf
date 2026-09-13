//  DocumentPDFTests.swift
//
//  `pdfData(page:)` is the document on paper without a printer: one PDF page per
//  laid-out sheet, at the sheet's size, in the file's own metadata. Drives a real
//  `LeafDoc` for the same reason `PrintingTests` does — a page count is only
//  meaningful over a real document.
//
//  The two toolkits reach a PDF context differently, so the same expectations
//  run under both: the AppKit half through `swift test` (`scripts/test-swift.sh`),
//  the UIKit half on a simulator (`scripts/test-swift-ios.sh`).

import XCTest
import LeafFFI
@testable import LeafUI

private let long: String = (1...120).map {
    "Paragraph \($0) of a long document, long enough to wrap onto more than one line at the sheet's measure."
}.joined(separator: "\n\n") + "\n"

private func document(_ data: Data) throws -> CGPDFDocument {
    try XCTUnwrap(CGPDFDocument(CGDataProvider(data: data as CFData)!))
}

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit

final class DocumentPDFTests: XCTestCase {

    func testAShortDocumentIsOnePageOfTheSheet() throws {
        let doc = try LeafDoc(source: "# Title\n\nA paragraph.\n", format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let pdf = try document(view.pdfData(page: .a4))
        XCTAssertEqual(pdf.numberOfPages, 1)
        let box = try XCTUnwrap(pdf.page(at: 1)).getBoxRect(.mediaBox)
        XCTAssertEqual(box.width, 595, accuracy: 1)
        XCTAssertEqual(box.height, 842, accuracy: 1)
    }

    func testALongDocumentHasOnePagePerLaidOutSheet() throws {
        let doc = try LeafDoc(source: long, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let pdf = try document(view.pdfData(page: .usLetter))
        XCTAssertGreaterThan(pdf.numberOfPages, 2)

        let sheet = LeafTextView.paperSheet(of: doc, theme: .default, documentDirectory: nil, page: .usLetter)
        var range = NSRange()
        XCTAssertTrue(sheet.knowsPageRange(&range))
        XCTAssertEqual(range.length, pdf.numberOfPages)
    }

    func testTheColumnsAreThePageSetups() throws {
        // Two columns set narrower and read down-then-across; the sheet count is
        // whatever the layout says it is, and the PDF has exactly that many.
        let doc = try LeafDoc(source: long, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let two = PageSetup.usLetter.columned(2)
        let pdf = try document(view.pdfData(page: two))
        let sheet = LeafTextView.paperSheet(of: doc, theme: .default, documentDirectory: nil, page: two)
        XCTAssertEqual(sheet.pages.count, pdf.numberOfPages)
        XCTAssertNotEqual(pdf.numberOfPages, try document(view.pdfData(page: .usLetter)).numberOfPages)
    }

    func testTheTitleIsInTheFilesMetadata() throws {
        let model = try LeafEditorModel(source: "hello\n")
        let pdf = try document(model.pdfData(title: "Tuesday"))
        var title: CGPDFStringRef?
        XCTAssertTrue(CGPDFDictionaryGetString(try XCTUnwrap(pdf.info), "Title", &title))
        XCTAssertEqual(CGPDFStringCopyTextString(try XCTUnwrap(title)) as String?, "Tuesday")
    }

    func testTheModelPaintsWithTheThemeItIsGiven() throws {
        // A larger type wraps to more lines, so the same words take more sheets.
        let model = try LeafEditorModel(source: long)
        var large = EditorTheme.default
        large.fontSize = 28
        large.lineHeight = 42
        let small = try document(model.pdfData(theme: .default)).numberOfPages
        let big = try document(model.pdfData(theme: large)).numberOfPages
        XCTAssertGreaterThan(big, small)
    }

    func testPrintingKeepsTheScreensColumns() throws {
        let doc = try LeafDoc(source: long, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 800, height: 0)
        view.pageSetup = PageSetup.a4.columned(2)
        let info = NSPrintInfo()
        info.paperSize = CGSize(width: 612, height: 792)
        let printed = try XCTUnwrap(view.printOperation(with: info).view as? LeafTextView)
        XCTAssertEqual(printed.pageSetup?.columns, 2)
        XCTAssertEqual(try XCTUnwrap(printed.pageSetup?.size.width), 612, accuracy: 0.01,
                       "the paper is the printer's")
    }

    func testAnSVGReachesThePageAsPathsAndAPNGAsAnImage() throws {
        // The point of drawing SVG through usvg rather than rasterizing it: the
        // PDF gets vector paths, which scale, select, and print as such. A
        // raster picture is still an image XObject — Quartz writes that
        // dictionary uncompressed, so its `/Subtype /Image` is greppable in the
        // bytes, and a page whose only picture is an SVG must not have one.
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("leaf-pdf-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        try Data("""
            <svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
              <rect width="100" height="100" fill="#ff0000"/>
              <circle cx="150" cy="50" r="40" fill="#0000ff"/>
            </svg>
            """.utf8).write(to: dir.appendingPathComponent("shapes.svg"))
        try XCTUnwrap(Data(base64Encoded:
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="))
            .write(to: dir.appendingPathComponent("dot.png"))

        func pdf(_ source: String) throws -> Data {
            let doc = try LeafDoc(source: source, format: "markdown")
            return LeafTextView.pdf(of: doc, theme: .default, documentDirectory: dir, page: .a4, title: nil)
        }
        let image = Data("/Image".utf8)
        XCTAssertNil(try pdf("![shapes](shapes.svg)\n").range(of: image), "an SVG is paths on the page")
        XCTAssertNotNil(try pdf("![dot](dot.png)\n").range(of: image), "a PNG is an image on the page")
        XCTAssertEqual(try document(try pdf("![shapes](shapes.svg)\n")).numberOfPages, 1)
    }
}
#elseif canImport(UIKit)
import UIKit

final class DocumentPDFTests: XCTestCase {

    func testAShortDocumentIsOnePageOfTheSheet() throws {
        let doc = try LeafDoc(source: "# Title\n\nA paragraph.\n", format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let pdf = try document(view.pdfData(page: .a4))
        XCTAssertEqual(pdf.numberOfPages, 1)
        let box = try XCTUnwrap(pdf.page(at: 1)).getBoxRect(.mediaBox)
        XCTAssertEqual(box.width, 595, accuracy: 1)
        XCTAssertEqual(box.height, 842, accuracy: 1)
    }

    func testALongDocumentHasOnePagePerLaidOutSheet() throws {
        let doc = try LeafDoc(source: long, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let pdf = try document(view.pdfData(page: .usLetter))
        XCTAssertGreaterThan(pdf.numberOfPages, 2)

        let sheet = LeafTextView(doc: doc, theme: .default)
        sheet.frame = CGRect(x: 0, y: 0, width: 612, height: 0)
        sheet.pageSetup = PageSetup.usLetter.paper
        XCTAssertEqual(sheet.pages.count, pdf.numberOfPages)
    }

    func testTheColumnsAreThePageSetups() throws {
        let doc = try LeafDoc(source: long, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let two = try document(view.pdfData(page: PageSetup.usLetter.columned(2)))
        let one = try document(view.pdfData(page: .usLetter))
        XCTAssertNotEqual(two.numberOfPages, one.numberOfPages)
    }

    func testTheTitleIsInTheFilesMetadata() throws {
        let model = try LeafEditorModel(source: "hello\n")
        let pdf = try document(model.pdfData(title: "Tuesday"))
        var title: CGPDFStringRef?
        XCTAssertTrue(CGPDFDictionaryGetString(try XCTUnwrap(pdf.info), "Title", &title))
        XCTAssertEqual(CGPDFStringCopyTextString(try XCTUnwrap(title)) as String?, "Tuesday")
    }

    func testThePageHasInkOnIt() throws {
        // A page drawn through the wrong context comes out blank and still
        // counts as a page, so look at the pixels: the text column's first line
        // is not all paper.
        let doc = try LeafDoc(source: "# A heading\n\nSome words.\n", format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        let pdf = try document(view.pdfData(page: .usLetter))
        let page = try XCTUnwrap(pdf.page(at: 1))
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: 612, height: 792))
        let image = renderer.image { ctx in
            UIColor.white.setFill(); ctx.fill(CGRect(x: 0, y: 0, width: 612, height: 792))
            ctx.cgContext.translateBy(x: 0, y: 792); ctx.cgContext.scaleBy(x: 1, y: -1)
            ctx.cgContext.drawPDFPage(page)
        }
        let cg = try XCTUnwrap(image.cgImage)
        let data = try XCTUnwrap(cg.dataProvider?.data) as Data
        // The band the first line sits in: inside the inch margin, a few lines
        // down. Any byte below white is ink.
        let bytesPerRow = cg.bytesPerRow, scale = Int(image.scale)
        var dark = 0
        for y in (80 * scale)..<(110 * scale) {
            for x in (72 * scale)..<(300 * scale) {
                let i = y * bytesPerRow + x * 4
                if data[i] < 200 { dark += 1 }
            }
        }
        XCTAssertGreaterThan(dark, 50)
    }
}
#endif
