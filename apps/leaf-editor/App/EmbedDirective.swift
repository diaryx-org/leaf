import CoreText
import LeafFFI
import LeafUI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// The one directive the demo has a meaning for: `::embed{src=…}`, drawn as a
/// titled card with its URL, and offered in Insert as "Embed…", which asks for
/// the URL. A demonstration of the host's half, not a vocabulary — the same
/// card the web, Android and terminal demos paint, so the apps show it without
/// leaf adopting a name. The card is the renderer's picture for
/// `directive:embed`, laid out, paged and printed with the text. See
/// `docs/proposals/host-directives.md` and `docs/proposals/embedded-editors.md`.
enum EmbedDirective {
    static let name = "embed"
    static let kind = "directive:embed"

    /// The catalogue entry: Insert ▸ Embed…, asking for a URL.
    static let item = DirectiveItem(
        id: "directive.embed", name: name, title: "Embed\u{2026}", icon: "link.circle"
    ) { done in
        promptForEmbed { url in
            done(url.map { DirectiveContent(attrs: ["src": $0]) })
        }
    }

    /// The picture: a card as wide as the column, titled "Embed", its URL
    /// beneath, in the text's ink.
    static let renderer: RegionRenderer = { region, context in
        let src = region.attrs.first { $0.key == "src" }?.value ?? ""
        return card(title: "Embed", detail: src.isEmpty ? "No URL" : src, ink: context.ink.cgColor)
    }

    private static let pad: CGFloat = 12
    private static let spacing: CGFloat = 4

    /// A bordered card with a headline and a line of detail under it, filling
    /// whatever width it is drawn at.
    static func card(title: String, detail: String, ink: CGColor) -> RegionPicture {
        let headline = CTFontCreateUIFontForLanguage(.emphasizedSystem, 13, nil)
            ?? CTFontCreateWithName("Helvetica-Bold" as CFString, 13, nil)
        let callout = CTFontCreateUIFontForLanguage(.system, 12, nil)
            ?? CTFontCreateWithName("Helvetica" as CFString, 12, nil)
        let secondary = ink.copy(alpha: ink.alpha * 0.6) ?? ink
        let top = line(title, font: headline, color: ink)
        let bottom = line(detail, font: callout, color: secondary)
        let height = (pad * 2 + top.height + spacing + bottom.height).rounded(.up)
        let symbol = linkSymbol(size: top.height)
        return RegionPicture(width: nil, height: height) { ctx, rect in
            let box = rect.insetBy(dx: 0.5, dy: 0.5)
            let path = CGPath(roundedRect: box, cornerWidth: 8, cornerHeight: 8, transform: nil)
            ctx.saveGState()
            ctx.addPath(path)
            ctx.setFillColor(ink.copy(alpha: 0.06) ?? ink)
            ctx.fillPath()
            ctx.addPath(path)
            ctx.setStrokeColor(ink.copy(alpha: 0.35) ?? ink)
            ctx.setLineWidth(1)
            ctx.strokePath()
            var x = rect.minX + pad
            if let symbol {
                // A template image: its shape, in the ink.
                let r = CGRect(x: x, y: rect.minY + pad, width: top.height, height: top.height)
                ctx.saveGState()
                ctx.translateBy(x: r.minX, y: r.maxY)
                ctx.scaleBy(x: 1, y: -1)
                ctx.clip(to: CGRect(origin: .zero, size: r.size), mask: symbol)
                ctx.setFillColor(ink)
                ctx.fill(CGRect(origin: .zero, size: r.size))
                ctx.restoreGState()
                x += top.height + 6
            }
            draw(top, at: CGPoint(x: x, y: rect.minY + pad), width: rect.maxX - pad - x, in: ctx)
            draw(bottom, at: CGPoint(x: rect.minX + pad, y: rect.minY + pad + top.height + spacing),
                 width: rect.width - pad * 2, in: ctx)
            ctx.restoreGState()
        }
    }

    private struct Line {
        let text: NSAttributedString
        let height: CGFloat
        let ascent: CGFloat
    }

    private static func line(_ text: String, font: CTFont, color: CGColor) -> Line {
        let attributed = NSAttributedString(string: text, attributes: [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): color,
        ])
        let ascent = CTFontGetAscent(font), descent = CTFontGetDescent(font)
        return Line(text: attributed, height: (ascent + descent).rounded(.up), ascent: ascent)
    }

    /// One line set at `origin` (its top left) in a y-down context, cut short
    /// with an ellipsis in the middle where it runs past `width`.
    private static func draw(_ line: Line, at origin: CGPoint, width: CGFloat, in ctx: CGContext) {
        var ct = CTLineCreateWithAttributedString(line.text)
        if CTLineGetTypographicBounds(ct, nil, nil, nil) > width {
            let dots = CTLineCreateWithAttributedString(NSAttributedString(
                string: "\u{2026}", attributes: line.text.attributes(at: 0, effectiveRange: nil)))
            ct = CTLineCreateTruncatedLine(ct, Double(max(0, width)), .middle, dots) ?? ct
        }
        ctx.saveGState()
        ctx.textMatrix = .identity
        ctx.translateBy(x: origin.x, y: origin.y + line.ascent)
        ctx.scaleBy(x: 1, y: -1)
        ctx.textPosition = .zero
        CTLineDraw(ct, ctx)
        ctx.restoreGState()
    }

    /// The `link.circle` symbol as a mask, `size` points square.
    private static func linkSymbol(size: CGFloat) -> CGImage? {
        #if canImport(AppKit)
        let image = NSImage(systemSymbolName: "link.circle", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: size, weight: .semibold))
        var rect = CGRect(x: 0, y: 0, width: size * 2, height: size * 2)
        return image?.cgImage(forProposedRect: &rect, context: nil, hints: nil)
        #else
        return UIImage(systemName: "link.circle",
                       withConfiguration: UIImage.SymbolConfiguration(pointSize: size, weight: .semibold))?.cgImage
        #endif
    }
}

/// Ask for the URL to embed, and call back with it — or with nil when the
/// author cancels or leaves the field empty. The link prompt's twin.
private func promptForEmbed(done: @escaping (String?) -> Void) {
    #if canImport(AppKit)
    let alert = NSAlert()
    alert.messageText = "Embed"
    alert.informativeText = "The address of what to embed."
    alert.addButton(withTitle: "Insert")
    alert.addButton(withTitle: "Cancel")
    let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 24))
    field.placeholderString = "https://"
    alert.accessoryView = field
    alert.window.initialFirstResponder = field
    if alert.runModal() == .alertFirstButtonReturn, !field.stringValue.isEmpty {
        done(field.stringValue)
    } else {
        done(nil)
    }
    #elseif canImport(UIKit)
    guard let root = UIApplication.shared.connectedScenes
        .compactMap({ ($0 as? UIWindowScene)?.keyWindow?.rootViewController })
        .first
    else { return done(nil) }
    let top = sequence(first: root) { $0.presentedViewController }.reversed().first ?? root
    let alert = UIAlertController(title: "Embed", message: "The address of what to embed.",
                                  preferredStyle: .alert)
    alert.addTextField {
        $0.placeholder = "https://"
        $0.keyboardType = .URL
        $0.autocapitalizationType = .none
        $0.autocorrectionType = .no
    }
    alert.addAction(UIAlertAction(title: "Cancel", style: .cancel) { _ in done(nil) })
    alert.addAction(UIAlertAction(title: "Insert", style: .default) { _ in
        let text = alert.textFields?.first?.text ?? ""
        done(text.isEmpty ? nil : text)
    })
    top.present(alert, animated: true)
    #endif
}
