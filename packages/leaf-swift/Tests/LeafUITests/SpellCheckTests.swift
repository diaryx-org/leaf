#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class SpellCheckTests: XCTestCase {
    private func view(_ source: String) throws -> LeafTextView {
        LeafTextView(
            doc: try LeafDoc(source: source, format: "markdown"),
            theme: .default)
    }

    func testCheckerTextKeepsProseAndMasksCodeAtTheSameOffsets() throws {
        let editor = try view("""
        A prose mispelling and `inlinecode`.

        ```rust
        fn misspeled() {}
        ```
        """)
        let visible = editor.string as NSString
        let checked = editor.spellCheckingText() as NSString

        XCTAssertEqual(checked.length, visible.length)
        for word in ["prose", "mispelling"] {
            let range = visible.range(of: word)
            XCTAssertEqual(checked.substring(with: range), word)
        }
        for word in ["inlinecode", "fn", "misspeled"] {
            let range = visible.range(of: word)
            XCTAssertEqual(
                checked.substring(with: range),
                String(repeating: " ", count: range.length))
        }
    }

    func testCheckerTextUsesStructuralTableCellsInsteadOfPictureRows() throws {
        let editor = try view("""
        | Feature | Status |
        | --- | --- |
        | Tables | editable |
        """)
        let visible = editor.string as NSString
        let checked = editor.spellCheckingText() as NSString

        XCTAssertEqual(checked.length, visible.length)
        for word in ["Feature", "Status", "Tables", "editable"] {
            let range = visible.range(of: word)
            XCTAssertNotEqual(
                range.location, NSNotFound,
                "\(word) missing from \(String(reflecting: editor.string))")
            guard range.location != NSNotFound else { continue }
            XCTAssertEqual(checked.substring(with: range), word)
        }
    }

    func testCheckerIgnoresHyphenatedCompoundsButNotNearbyWords() throws {
        let editor = try view("A round-trippable mispelling.\n")
        let text = editor.spellCheckingText() as NSString

        XCTAssertTrue(editor.isHyphenatedCompound(text.range(of: "round-trippable"), in: text as String))
        XCTAssertFalse(editor.isHyphenatedCompound(text.range(of: "mispelling"), in: text as String))
    }

    func testSystemCheckerStillSeesAProseTypoAfterSemanticMasking() throws {
        let editor = try view("""
        A mispelling beside round-trippable and `inlineerr`.

        ```
        codeblockerr
        ```
        """)
        let text = editor.spellCheckingText()
        let ns = text as NSString
        var found: [String] = []
        var start = 0
        while start < ns.length {
            var wordCount = 0
            let range = NSSpellChecker.shared.checkSpelling(
                of: text, startingAt: start, language: "en",
                wrap: false, inSpellDocumentWithTag: 0, wordCount: &wordCount)
            guard range.location != NSNotFound else { break }
            if !editor.isHyphenatedCompound(range, in: text) {
                found.append(ns.substring(with: range))
            }
            start = max(start + 1, NSMaxRange(range))
        }

        XCTAssertEqual(found, ["mispelling"])
    }
}
#endif
