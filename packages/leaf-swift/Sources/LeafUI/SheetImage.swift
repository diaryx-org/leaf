//  SheetImage.swift
//
//  One sheet of the document as a picture — for a host that shows a sheet it
//  is not editing: the leaf a book is turning to, a thumbnail of a page. A
//  surface (`LeafEditor`) would do it too, but a surface has a scroll view to
//  size, a fit to resolve and a sheet to scroll to before it shows the right
//  thing, and one made as a turn begins showed the wrong one for a frame or
//  two. A picture is right the moment it exists.
//
//  Laid out and painted as the PDF is (`DocumentPDF.swift`), by a second view
//  over the same document on paper, so nothing on screen moves: no caret, no
//  selection, no flash, no marker in the margin. Unlike the PDF it keeps the
//  screen's appearance (`dark`) and fills the sheet with the theme's page
//  colour, as the editor draws it, so the picture and the editor that takes
//  its place look alike.

import CoreGraphics
import LeafFFI

/// A sheet drawn: which one it turned out to be — a sheet past the last is
/// the last — and how many the document has on that paper.
public struct SheetImage {
    public let image: CGImage
    public let sheet: Int
    public let sheetCount: Int
}

extension LeafEditorModel {
    /// Sheet `index` of the document on `page`, in `theme`, `scale` pixels to
    /// the point; past the last sheet, the last. Nil off paper's terms — a
    /// document with no sheets, or a context that could not be made.
    public func sheetImage(_ index: Int, theme: EditorTheme, page: PageSetup,
                           scale: CGFloat, dark: Bool = false) -> SheetImage? {
        LeafTextView.sheetImage(of: doc, theme: theme, documentDirectory: documentDirectory,
                                page: page, index: index, scale: scale, dark: dark)
    }
}

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit

extension LeafTextView {
    static func sheetImage(of doc: LeafDoc, theme: EditorTheme, documentDirectory: URL?,
                           page: PageSetup, index: Int, scale: CGFloat, dark: Bool) -> SheetImage? {
        let sheet = paperSheet(of: doc, theme: theme, documentDirectory: documentDirectory, page: page)
        sheet.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        guard !sheet.pages.isEmpty else { return nil }
        let at = min(max(index, 0), sheet.pages.count - 1)
        let rect = sheet.pages[at]
        let width = Int((rect.width * scale).rounded(.up))
        let height = Int((rect.height * scale).rounded(.up))
        guard width > 0, height > 0,
              let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
                                      bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue)
        else { return nil }
        // Flipped to the view's top-down rows, scaled, and the sheet's corner
        // moved to the picture's.
        context.translateBy(x: 0, y: CGFloat(height))
        context.scaleBy(x: scale, y: -scale)
        context.translateBy(x: -rect.minX, y: -rect.minY)
        let graphics = NSGraphicsContext(cgContext: context, flipped: true)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = graphics
        sheet.effectiveAppearance.performAsCurrentDrawingAppearance {
            theme.pageColor.setFill()
            rect.fill()
            context.clip(to: rect)
            sheet.draw(rect)
        }
        NSGraphicsContext.restoreGraphicsState()
        guard let image = context.makeImage() else { return nil }
        return SheetImage(image: image, sheet: at, sheetCount: sheet.pages.count)
    }
}

#elseif canImport(UIKit)
import UIKit

extension LeafTextView {
    static func sheetImage(of doc: LeafDoc, theme: EditorTheme, documentDirectory: URL?,
                           page: PageSetup, index: Int, scale: CGFloat, dark: Bool) -> SheetImage? {
        let sheet = LeafTextView(doc: doc, theme: theme)
        sheet.isPaper = true
        let style: UIUserInterfaceStyle = dark ? .dark : .light
        sheet.overrideUserInterfaceStyle = style
        sheet.documentDirectory = documentDirectory
        sheet.frame = CGRect(origin: .zero, size: CGSize(width: page.size.width, height: 0))
        sheet.pageSetup = page.paper
        guard !sheet.pages.isEmpty else { return nil }
        let at = min(max(index, 0), sheet.pages.count - 1)
        let rect = sheet.pages[at]
        let format = UIGraphicsImageRendererFormat()
        format.scale = scale
        format.opaque = true
        let traits = UITraitCollection(userInterfaceStyle: style)
        let renderer = UIGraphicsImageRenderer(size: rect.size, format: format)
        let image = renderer.image { ctx in
            traits.performAsCurrent {
                ctx.cgContext.translateBy(x: -rect.minX, y: -rect.minY)
                theme.pageColor.setFill()
                ctx.cgContext.fill(rect)
                ctx.cgContext.clip(to: rect)
                sheet.draw(rect)
            }
        }
        guard let cgImage = image.cgImage else { return nil }
        return SheetImage(image: cgImage, sheet: at, sheetCount: sheet.pages.count)
    }
}
#endif
