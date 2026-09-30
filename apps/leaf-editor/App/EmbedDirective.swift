import SwiftUI
import LeafFFI
import LeafUI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// The one directive the demo has a meaning for: `::embed{src=…}`, drawn as a
/// titled card with its URL, and offered in Insert as "Embed…", which asks for
/// the URL. A demonstration of the host hook, not a vocabulary — the same card
/// the web, Android and terminal demos paint, so the apps show the hook without
/// leaf adopting a name. See `docs/proposals/host-directives.md`.
enum EmbedDirective {
    static let name = "embed"

    /// The catalogue entry: Insert ▸ Embed…, asking for a URL.
    static let item = DirectiveItem(
        id: "directive.embed", name: name, title: "Embed\u{2026}", icon: "link.circle"
    ) { done in
        promptForEmbed { url in
            done(url.map { DirectiveContent(attrs: ["src": $0]) })
        }
    }

    /// The drawing: a card for an `::embed`, and nothing — core's placeholder —
    /// for any other directive.
    static func view(for directive: DirectiveView) -> LeafView? {
        guard directive.name == name else { return nil }
        let src = directive.attrs.first { $0.key == "src" }?.value ?? ""
        return EmbedCardView(src: src)
    }
}

/// The card itself: bordered, titled "Embed", the URL beneath.
struct EmbedCard: View {
    let src: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Label("Embed", systemImage: "link.circle")
                .font(.headline)
            Text(src.isEmpty ? "No URL" : src)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color.secondary.opacity(0.08)))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color.secondary.opacity(0.5)))
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("embed-card")
    }
}

#if canImport(AppKit)
/// The card in a hosting view that lets every click through to the editor, so
/// a click on it places the caret in front of it or past it as a click on a
/// picture does. Its height is the SwiftUI card's own, which the hosting view
/// reports as its intrinsic size.
final class EmbedCardView: NSHostingView<EmbedCard> {
    convenience init(src: String) {
        self.init(rootView: EmbedCard(src: src))
    }

    required init(rootView: EmbedCard) {
        super.init(rootView: rootView)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
#elseif canImport(UIKit)
/// The card in a hosting controller's view, kept by this view so the
/// controller lives as long as the card does. Taps go through to the editor.
final class EmbedCardView: UIView {
    private let host: UIHostingController<EmbedCard>

    init(src: String) {
        host = UIHostingController(rootView: EmbedCard(src: src))
        host.sizingOptions = .intrinsicContentSize
        super.init(frame: .zero)
        isUserInteractionEnabled = false
        host.view.backgroundColor = .clear
        host.view.translatesAutoresizingMaskIntoConstraints = false
        addSubview(host.view)
        NSLayoutConstraint.activate([
            host.view.leadingAnchor.constraint(equalTo: leadingAnchor),
            host.view.trailingAnchor.constraint(equalTo: trailingAnchor),
            host.view.topAnchor.constraint(equalTo: topAnchor),
            host.view.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var intrinsicContentSize: CGSize {
        CGSize(width: UIView.noIntrinsicMetric, height: host.view.intrinsicContentSize.height)
    }
}
#endif

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
