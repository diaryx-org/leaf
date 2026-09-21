//  DocumentPDF.swift
//
//  The document as a PDF: one page per sheet of a `PageSetup`, laid out by the
//  same `EditorLayout` and painted by the same `draw` the editor uses on screen,
//  so what comes out is what the editor shows — minus everything that only
//  exists on screen. No caret, no selection, no landing flash, no placeholder
//  cue, no marker in the margin, and no picture of a sheet on a backdrop: the
//  page is the sheet.
//
//  Done by a second view over the same document, paginated to the paper, so
//  nothing on screen moves — the same shape File ▸ Print takes on the Mac, and
//  the sheet it builds is the one printing uses. Each toolkit reaches a PDF
//  context its own way (AppKit through a print operation, which is what already
//  knows how to page a view; UIKit through `UIGraphicsPDFRenderer`), and that
//  is the only thing written twice below.
//
//  A sheet is drawn at the theme's stated size, on paper as light as the theme's
//  ink expects: a point is a point on paper, and a dark screen would print white.

import CoreGraphics
import Foundation
import LeafFFI

extension LeafEditorModel {
    /// The document as a PDF, one page per sheet of `page`, painted as
    /// `LeafEditor(model:theme:)` would paint it on screen — pass the same
    /// `theme`. `title` goes into the file's metadata, where a viewer's title
    /// bar reads it. A relative image resolves against `documentDirectory`, as
    /// it does on screen; one only the host can fetch (`onResolveMedia`) is
    /// drawn as its labelled chip, since the page is made now rather than when
    /// the host answers.
    public func pdfData(theme: EditorTheme = .default, page: PageSetup = .usLetter,
                        title: String? = nil) -> Data {
        LeafTextView.pdf(of: doc, theme: theme, documentDirectory: documentDirectory,
                         page: page, title: title)
    }
}

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit

extension LeafTextView {
    /// The document as a PDF, one page per sheet of `page`, in this view's theme
    /// and with its images. See `LeafEditorModel.pdfData(theme:page:title:)`.
    public func pdfData(page: PageSetup = .usLetter, title: String? = nil) -> Data {
        Self.pdf(of: doc, theme: theme, documentDirectory: documentDirectory, page: page, title: title)
    }

    static func pdf(of doc: LeafDoc, theme: EditorTheme, documentDirectory: URL?,
                    page: PageSetup, title: String?) -> Data {
        let sheet = paperSheet(of: doc, theme: theme, documentDirectory: documentDirectory, page: page)
        // The print info carries the paper and no margins: the sheet's own margins
        // are inside the page setup, and `rectForPage` hands each sheet over whole.
        let info = NSPrintInfo()
        info.paperSize = page.size
        info.topMargin = 0; info.bottomMargin = 0; info.leftMargin = 0; info.rightMargin = 0
        info.isHorizontallyCentered = false
        info.isVerticallyCentered = false
        // A print job saved to a file, not `pdfOperation(with:inside:to:)`: that
        // one pages nothing — it draws the rect it is given as a single page and
        // never asks `knowsPageRange` — where a job saved runs the same pagination
        // as a job printed.
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("leaf-\(UUID().uuidString).pdf")
        info.jobDisposition = .save
        info.dictionary()[NSPrintInfo.AttributeKey.jobSavingURL] = url
        let operation = NSPrintOperation(view: sheet, printInfo: info)
        operation.showsPrintPanel = false
        operation.showsProgressPanel = false
        if let title { operation.jobTitle = title }
        defer { try? FileManager.default.removeItem(at: url) }
        guard operation.run(), let data = try? Data(contentsOf: url) else { return Data() }
        return data
    }

    /// A fresh view over `doc`, paginated to `page` on paper — no gap, no
    /// backdrop — in `theme` and with the images `documentDirectory` resolves.
    /// The sheet printing and the PDF both draw from. Light, whatever the
    /// screen's appearance: the theme's semantic colours resolve against the
    /// view's, and a dark one would print white.
    static func paperSheet(of doc: LeafDoc, theme: EditorTheme, documentDirectory: URL?,
                           page: PageSetup) -> LeafTextView {
        let sheet = LeafTextView(doc: doc, theme: theme)
        sheet.appearance = NSAppearance(named: .aqua)
        sheet.frame = NSRect(origin: .zero, size: CGSize(width: page.size.width, height: 0))
        sheet.documentDirectory = documentDirectory
        sheet.pageSetup = page.paper
        return sheet
    }

    /// `paperSheet(of:…)` over this view's own document, theme and images.
    func paperSheet(_ page: PageSetup) -> LeafTextView {
        Self.paperSheet(of: doc, theme: theme, documentDirectory: documentDirectory, page: page)
    }
}

#elseif canImport(UIKit)
import UIKit

extension LeafTextView {
    /// The document as a PDF, one page per sheet of `page`, in this view's theme
    /// and with its images. See `LeafEditorModel.pdfData(theme:page:title:)`.
    ///
    /// At the theme's stated size, not the Dynamic Type size the screen shows:
    /// a point is a point on paper, and the reader's text-size setting is about
    /// their screen.
    public func pdfData(page: PageSetup = .usLetter, title: String? = nil) -> Data {
        Self.pdf(of: doc, theme: theme, documentDirectory: documentDirectory, page: page, title: title)
    }

    static func pdf(of doc: LeafDoc, theme: EditorTheme, documentDirectory: URL?,
                    page: PageSetup, title: String?) -> Data {
        let sheet = LeafTextView(doc: doc, theme: theme)
        sheet.isPaper = true
        // Light, whatever the screen's: the sheet's layout resolves a formula's
        // ink under its own traits (the AppKit sheet's `aqua`), and its drawing
        // below runs under the same. Ink on paper, not a dark window's white.
        sheet.overrideUserInterfaceStyle = .light
        sheet.documentDirectory = documentDirectory
        sheet.frame = CGRect(origin: .zero, size: CGSize(width: page.size.width, height: 0))
        sheet.pageSetup = page.paper

        let format = UIGraphicsPDFRendererFormat()
        if let title { format.documentInfo = [kCGPDFContextTitle as String: title] }
        let renderer = UIGraphicsPDFRenderer(bounds: CGRect(origin: .zero, size: page.size),
                                             format: format)
        // Ink on paper, whatever the screen's appearance: the theme's colours are
        // dynamic, and resolve against whichever traits are current when drawn.
        let light = UITraitCollection(userInterfaceStyle: .light)
        return renderer.pdfData { ctx in
            light.performAsCurrent {
                // One PDF page per sheet, the sheet's corner moved to the page's.
                // `draw` paints in layout coordinates and reads its context from
                // UIKit, which the renderer has made current.
                for rect in sheet.pages {
                    ctx.beginPage()
                    ctx.cgContext.saveGState()
                    ctx.cgContext.translateBy(x: -rect.minX, y: -rect.minY)
                    ctx.cgContext.clip(to: rect)
                    sheet.draw(rect)
                    ctx.cgContext.restoreGState()
                }
            }
        }
    }
}
#endif
