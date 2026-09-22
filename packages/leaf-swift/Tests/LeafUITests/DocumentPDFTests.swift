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

/// A line of prose — where the caret stays, so no formula is on its line and
/// revealed to its TeX — and then two formulas. Whatever ink the page has
/// beyond `prose`'s own is theirs.
private let prose = "Words.\n"
private let proseThenMath = prose + "\n$$\n\\int_0^1 x\\,dx\n$$\n\n$E = mc^2$\n"

/// How many pixels of `page` are darker than paper, drawn at 1 pt = 1 px on
/// white. A page that came out blank counts zero.
private func inkOn(_ page: CGPDFPage) throws -> Int {
    let w = 612, h = 792
    let ctx = try XCTUnwrap(CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
    ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
    ctx.drawPDFPage(page)
    let cg = try XCTUnwrap(ctx.makeImage())
    let data = try XCTUnwrap(cg.dataProvider?.data) as Data
    var dark = 0
    for y in 0..<h {
        for x in 0..<w where data[y * cg.bytesPerRow + x * 4] < 200 { dark += 1 }
    }
    return dark
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

    func testAPageBreakPutsWhatFollowsOnTheNextSheetOfPaper() throws {
        // Two short paragraphs are one page of A4; the break between them is the
        // only reason for a second, and it is the same `EditorLayout` that
        // decides it on paper as on screen.
        let doc = try LeafDoc(source: "One.\n\n::page-break\n\nTwo.\n", format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        XCTAssertEqual(try document(view.pdfData(page: .a4)).numberOfPages, 2)

        let plain = try LeafDoc(source: "One.\n\nTwo.\n", format: "markdown")
        let plainView = LeafTextView(doc: plain, theme: .default)
        XCTAssertEqual(try document(plainView.pdfData(page: .a4)).numberOfPages, 1)
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
    func testAFormulaIsInkOnThePaperWhateverTheScreensAppearance() throws {
        // A formula's ink is bytes handed to the typesetter, resolved from the
        // theme's dynamic colour when the row is laid out — not, like the text's,
        // when it is drawn. Laid out under a dark screen that is white, and the
        // paper is white: the export from a dark-mode window was a gap the
        // shape of each formula. The sheet's own appearance is the one to
        // resolve under, and this asks for the page from under a dark one.
        func ink(_ source: String) throws -> Int {
            var ink = 0
            NSAppearance(named: .darkAqua)!.performAsCurrentDrawingAppearance {
                guard let doc = try? LeafDoc(source: source, format: "markdown"),
                      case _ = doc.setInlinePictures(on: true),
                      let pdf = try? document(LeafTextView.pdf(of: doc, theme: .default, documentDirectory: nil,
                                                               page: .usLetter, title: nil)),
                      let page = pdf.page(at: 1)
                else { return }
                ink = (try? inkOn(page)) ?? 0
            }
            return ink
        }
        XCTAssertGreaterThan(try ink(proseThenMath), try ink(prose) + 100,
                             "the integral and E = mc² are drawn in ink, not in paper")
    }

    func testTheCaretsLineIsNotRevealedOnPaper() throws {
        // The screen shows the line the caret stands on as source — here the
        // formula's TeX, and under the full mode the `*`s too. Paper has no
        // caret, so the page is the same wherever the caret was left, and the
        // screen's frames go on revealing after it.
        let source = "*Words* and $E = mc^2$.\n\nMore words.\n"
        func ink(caret: UInt32) throws -> Int {
            let doc = try LeafDoc(source: source, format: "markdown")
            _ = doc.setInlinePictures(on: true)
            _ = doc.setMarkupMode(mode: .full)
            let screen = doc.setSelectionOffsets(anchor: caret, focus: caret)
            let pdf = try document(LeafTextView.pdf(of: doc, theme: .default, documentDirectory: nil,
                                                    page: .usLetter, title: nil))
            XCTAssertEqual(doc.view().rows.count, screen.rows.count)
            return try inkOn(try XCTUnwrap(pdf.page(at: 1)))
        }
        let away = try ink(caret: UInt32(source.utf8.count - 3))
        XCTAssertGreaterThan(away, 0)
        XCTAssertEqual(try ink(caret: 0), away, "the caret's line prints as every other line does")
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

    func testAPageBreakPutsWhatFollowsOnTheNextSheetOfPaper() throws {
        // Two short paragraphs are one page of A4; the break between them is the
        // only reason for a second, and it is the same `EditorLayout` that
        // decides it on paper as on screen.
        let doc = try LeafDoc(source: "One.\n\n::page-break\n\nTwo.\n", format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        XCTAssertEqual(try document(view.pdfData(page: .a4)).numberOfPages, 2)

        let plain = try LeafDoc(source: "One.\n\nTwo.\n", format: "markdown")
        let plainView = LeafTextView(doc: plain, theme: .default)
        XCTAssertEqual(try document(plainView.pdfData(page: .a4)).numberOfPages, 1)
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
    func testAFormulaIsInkOnThePaperWhateverTheScreensAppearance() throws {
        // A formula's ink is resolved from the theme's dynamic colour when the
        // row is laid out, not when it is drawn — and the layout ran under the
        // screen's traits, where a dark one made white ink for white paper. The
        // sheet is set light, and its layout has to resolve under that.
        func ink(_ source: String) throws -> Int {
            var ink = 0
            UITraitCollection(userInterfaceStyle: .dark).performAsCurrent {
                guard let doc = try? LeafDoc(source: source, format: "markdown"),
                      case _ = doc.setInlinePictures(on: true),
                      let pdf = try? document(LeafTextView(doc: doc, theme: .default).pdfData(page: .usLetter)),
                      let page = pdf.page(at: 1)
                else { return }
                ink = (try? inkOn(page)) ?? 0
            }
            return ink
        }
        XCTAssertGreaterThan(try ink(proseThenMath), try ink(prose) + 100,
                             "the integral and E = mc² are drawn in ink, not in paper")
    }

    func testTheCaretsLineIsNotRevealedOnPaper() throws {
        // The AppKit test's peer: the page is the same wherever the caret was.
        let source = "*Words* and $E = mc^2$.\n\nMore words.\n"
        func ink(caret: UInt32) throws -> Int {
            let doc = try LeafDoc(source: source, format: "markdown")
            _ = doc.setInlinePictures(on: true)
            _ = doc.setMarkupMode(mode: .full)
            _ = doc.setSelectionOffsets(anchor: caret, focus: caret)
            let pdf = try document(LeafTextView(doc: doc, theme: .default).pdfData(page: .usLetter))
            return try inkOn(try XCTUnwrap(pdf.page(at: 1)))
        }
        let away = try ink(caret: UInt32(source.utf8.count - 3))
        XCTAssertGreaterThan(away, 0)
        XCTAssertEqual(try ink(caret: 0), away, "the caret's line prints as every other line does")
    }
}
#endif
