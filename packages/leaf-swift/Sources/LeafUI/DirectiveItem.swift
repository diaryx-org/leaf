//  DirectiveItem.swift
//
//  The host's catalogue of directives: the leaf directives (`::name{…}`) an
//  app has a meaning for, offered where Insert's other rows are — the iOS short
//  row's Insert menu, the key panel's Insert key, the Mac bar's Insert menu and
//  the Format menu in the menu bar. leaf offers no free-form "insert a
//  directive": it would write markup nobody draws. The vocabulary is the host's,
//  so the host names it, and `LeafEditorModel.directiveView` draws it. See
//  `docs/proposals/host-directives.md`.

import Foundation
import LeafFFI

/// What one directive is written with: its `[label]` and its `{attributes}`.
/// An empty attribute value writes a bare attribute (`{wide}`), as an empty one
/// reads back — the `DirectiveAttr` list a `DirectiveView` hands out is the
/// list this takes.
public struct DirectiveContent {
    /// The directive's `[label]`, or nil for none. djot has nowhere to put one
    /// and refuses the whole directive rather than dropping it.
    public var label: String?
    public var attrs: [DirectiveAttr]

    public init(label: String? = nil, attrs: [DirectiveAttr] = []) {
        self.label = label
        self.attrs = attrs
    }

    /// The same, with the attributes as pairs in order: `["src": url]` would
    /// lose the order a document keeps.
    public init(label: String? = nil, attrs: KeyValuePairs<String, String>) {
        self.init(label: label, attrs: attrs.map { DirectiveAttr(key: $0.key, value: $0.value) })
    }
}

/// One directive a host offers in Insert.
public struct DirectiveItem: Identifiable {
    /// How the item is written when it is chosen.
    public enum Source {
        /// Always the same label and attributes.
        case fixed(DirectiveContent)
        /// Asked of the author each time — an embed's URL. The host presents
        /// whatever it asks with and calls the completion once, on the main
        /// thread, with what to write, or with nil to write nothing.
        case ask((@escaping (DirectiveContent?) -> Void) -> Void)
    }

    /// Stable, as a tool's id is (`"directive.embed"`), so an arrangement of
    /// tools saved by id survives a release.
    public let id: String
    /// The directive's name — what is written after the `::`.
    public var name: String
    /// The row's text, localised by the host (`"Embed…"`).
    public var title: String
    /// An SF Symbol name for the row and the key.
    public var icon: String
    public var source: Source

    public init(id: String, name: String, title: String, icon: String, source: Source) {
        self.id = id
        self.name = name
        self.title = title
        self.icon = icon
        self.source = source
    }

    /// An item that always writes the same directive.
    public init(id: String, name: String, title: String, icon: String,
                content: DirectiveContent = DirectiveContent()) {
        self.init(id: id, name: name, title: title, icon: icon, source: .fixed(content))
    }

    /// An item that asks the author what to write each time — see
    /// `Source.ask`.
    public init(id: String, name: String, title: String, icon: String,
                ask: @escaping (@escaping (DirectiveContent?) -> Void) -> Void) {
        self.init(id: id, name: name, title: title, icon: icon, source: .ask(ask))
    }
}
