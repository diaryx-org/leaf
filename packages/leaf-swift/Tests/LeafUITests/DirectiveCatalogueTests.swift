//  DirectiveCatalogueTests.swift
//
//  The host's catalogue of directives: its items join Insert as tools with
//  their own ids, dimmed where the format cannot write one, and an item writes
//  its directive, asking first when it asks — and nothing when the author
//  cancels.

import XCTest
import LeafFFI
@testable import LeafUI

final class DirectiveCatalogueTests: XCTestCase {
    func testAnEmptyCatalogueAddsNothing() throws {
        let model = try LeafEditorModel(source: "Text.\n")
        XCTAssertTrue(ToolCatalogue(editor: model, beginLink: {}).directives.isEmpty)
    }

    func testAnItemIsAToolDimmedByTheFormat() throws {
        let item = DirectiveItem(id: "directive.embed", name: "embed", title: "Embed…", icon: "link",
                                 content: DirectiveContent(attrs: ["src": "https://x.org"]))
        let md = try LeafEditorModel(source: "Text.\n")
        md.directiveCatalogue = [item]
        let tool = try XCTUnwrap(ToolCatalogue(editor: md, beginLink: {}).directives.first)
        XCTAssertEqual(tool.id, "directive.embed")
        XCTAssertEqual(tool.glyph, .symbol("link"))
        XCTAssertEqual(tool.title, "Embed…")
        XCTAssertTrue(tool.enabled)

        let html = try LeafEditorModel(source: "<p>Text.</p>", format: "html")
        html.directiveCatalogue = [item]
        XCTAssertFalse(ToolCatalogue(editor: html, beginLink: {}).directives[0].enabled,
                       "HTML reads no host's directive back")
    }

    func testAFixedItemWritesItsDirective() throws {
        let model = try LeafEditorModel(source: "Text.\n")
        model.insert(DirectiveItem(id: "d.embed", name: "embed", title: "Embed", icon: "link",
                                   content: DirectiveContent(attrs: ["src": "https://x.org", "wide": ""])))
        XCTAssertTrue(model.source().contains("::embed{src=\"https://x.org\" wide}"), model.source())
    }

    func testAnAskCanWriteOrCancel() throws {
        let model = try LeafEditorModel(source: "Text.\n")
        var answer: DirectiveContent? = nil
        let item = DirectiveItem(id: "d.embed", name: "embed", title: "Embed…", icon: "link") { done in
            done(answer)
        }
        let before = model.source()
        model.insert(item)
        XCTAssertEqual(model.source(), before, "cancelled: nothing written")

        answer = DirectiveContent(attrs: ["src": "https://y.org"])
        model.insert(item)
        XCTAssertTrue(model.source().contains("::embed{src=\"https://y.org\"}"), model.source())
    }
}
