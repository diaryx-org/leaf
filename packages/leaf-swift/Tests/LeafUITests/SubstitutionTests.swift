//  SubstitutionTests.swift
//
//  The macOS view's automatic substitutions: what the system does *to* text
//  as it is typed — corrections, text replacements, smart quotes and dashes —
//  made the way `NSTextView` makes them, in the rendered view only.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class SubstitutionTests: XCTestCase {
    /// A laid-out view with every substitution on, whatever this Mac's own
    /// settings say.
    private func editor(_ source: String, caret: Int? = nil) throws -> LeafTextView {
        let view = LeafTextView(doc: try LeafDoc(source: source, format: "markdown"), theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.layout()
        view.isAutomaticSpellingCorrectionEnabled = true
        view.isAutomaticTextReplacementEnabled = true
        view.isAutomaticQuoteSubstitutionEnabled = true
        view.isAutomaticDashSubstitutionEnabled = true
        // A reverted correction is learned by the checker for every app on
        // this Mac; a test must not teach it anything.
        view.recordsCorrectionResponses = false
        if let caret { view.command { $0.selectRange(start: UInt32(caret), end: UInt32(caret)) } }
        return view
    }

    /// Type `text` a character at a time, as the keyboard does.
    private func type(_ text: String, into view: LeafTextView) {
        for c in text {
            view.insertText(String(c), replacementRange: NSRange(location: NSNotFound, length: 0))
        }
    }

    private func undo(_ view: LeafTextView) { view.command { $0.undo() } }

    private var correctsTeh: Bool {
        let text = "I saw teh "
        return NSSpellChecker.shared.correction(
            forWordRange: (text as NSString).range(of: "teh"), in: text,
            language: NSSpellChecker.shared.language(), inSpellDocumentWithTag: 0) == "the"
    }

    func testSmartQuotesCurlAsTheyAreTyped() throws {
        let view = try editor("\n", caret: 0)
        type("He said \"hi\" and it's done", into: view)
        XCTAssertEqual(view.sourceText(), "He said \u{201C}hi\u{201D} and it\u{2019}s done\n")
    }

    func testASmartQuoteBesideHiddenMarkupLeavesValidSource() throws {
        // The caret's two homes at the end of a bold run: inside its closing
        // delimiter and past it. Either way the quote is curled, the bold is
        // still bold, and nothing else in the source moved.
        for caret in [8, 10] {
            let view = try editor("A **bold** word\n", caret: caret)
            type("\"", into: view)
            let source = view.sourceText()
            XCTAssertTrue(source == "A **bold\u{201D}** word\n" || source == "A **bold**\u{201D} word\n",
                          "caret \(caret): \(source)")
            XCTAssertEqual(view.string, "A bold\u{201D} word", "caret \(caret): the delimiters are still markup")
        }
        // And opening one: typed in front of a bold run, a quote opens.
        let view = try editor("A **bold** word\n", caret: 2)
        type("\"", into: view)
        XCTAssertTrue(view.sourceText().contains("\u{201C}"), view.sourceText())
        XCTAssertTrue(view.sourceText().contains("**bold**"), view.sourceText())
        XCTAssertEqual(view.string, "A \u{201C}bold word")
    }

    func testSmartDashesWaitForTheCharacterAfterThem() throws {
        let view = try editor("\n", caret: 0)
        type("wait--", into: view)
        XCTAssertEqual(view.sourceText(), "wait--\n", "not yet: a third hyphen could follow")
        type("then", into: view)
        XCTAssertEqual(view.sourceText(), "wait\u{2014}then\n")
    }

    func testThreeHyphensAreLeftAlone() throws {
        let view = try editor("\n", caret: 0)
        type("a---b", into: view)
        XCTAssertFalse(view.sourceText().contains("\u{2014}"), view.sourceText())
    }

    func testUndoPutsBackWhatWasTypedAsAStepOfItsOwn() throws {
        let view = try editor("\n", caret: 0)
        type("a\"", into: view)
        XCTAssertEqual(view.sourceText(), "a\u{201D}\n")
        undo(view)
        XCTAssertEqual(view.sourceText(), "a\"\n", "the straight quote that was typed")
        undo(view)
        XCTAssertEqual(view.sourceText(), "\n")
    }

    func testAMisspellingIsCorrectedAsTheWordEnds() throws {
        try XCTSkipUnless(correctsTeh, "this Mac's checker does not correct teh")
        let view = try editor("\n", caret: 0)
        type("I saw teh", into: view)
        XCTAssertEqual(view.sourceText(), "I saw teh\n", "not while the word is still being typed")
        type(" ", into: view)
        XCTAssertEqual(view.sourceText(), "I saw the \n")
        XCTAssertEqual(view.autocorrections.map(\.original), ["teh"], "underlined, and offered back")
        type("cat", into: view)
        XCTAssertEqual(view.sourceText(), "I saw the cat\n", "the caret stayed after the space")

        undo(view)
        XCTAssertEqual(view.sourceText(), "I saw the \n")
        undo(view)
        XCTAssertEqual(view.sourceText(), "I saw teh \n", "the correction is its own step")
        XCTAssertTrue(view.autocorrections.isEmpty, "and its underline goes with it")
    }

    func testACorrectionCanBeRevertedToTheOriginal() throws {
        try XCTSkipUnless(correctsTeh, "this Mac's checker does not correct teh")
        let view = try editor("\n", caret: 0)
        type("I saw teh ", into: view)
        let correction = try XCTUnwrap(view.autocorrections.first)
        view.command { $0.selectRange(start: 9, end: 9) }
        XCTAssertEqual(view.autocorrectionAtCaret(), correction, "the caret on the word offers it back")
        view.revert(correction, to: correction.original)
        XCTAssertEqual(view.sourceText(), "I saw teh \n")
        XCTAssertTrue(view.autocorrections.isEmpty)
        undo(view)
        XCTAssertEqual(view.sourceText(), "I saw the \n", "the reversion is a step of its own")
    }

    func testATextReplacementExpands() throws {
        let replacements = NSSpellChecker.shared.userReplacementsDictionary
        guard let (short, long) = replacements
            .filter({ $0.key.allSatisfy(\.isLetter) && !$0.value.contains("\n") && !$0.value.contains("*") })
            .min(by: { $0.key < $1.key })
        else { throw XCTSkip("this Mac has no text replacements") }
        let view = try editor("\n", caret: 0)
        type("x \(short) ", into: view)
        XCTAssertEqual(view.sourceText(), "x \(long) \n")
        undo(view)
        XCTAssertEqual(view.sourceText(), "x \(short) \n")
    }

    func testNothingIsSubstitutedInTheSourceView() throws {
        let view = try editor("\n", caret: 0)
        view.command { $0.toggleView() }
        type("\"q\" a--b teh ", into: view)
        XCTAssertEqual(view.sourceText(), "\"q\" a--b teh \n")
        XCTAssertTrue(view.autocorrections.isEmpty)
    }

    func testNothingIsSubstitutedInCode() throws {
        let view = try editor("`x`\n", caret: 2)
        type("\"", into: view)
        XCTAssertEqual(view.sourceText(), "`x\"`\n")
    }

    func testEachToggleTurnsItsSubstitutionOff() throws {
        let view = try editor("\n", caret: 0)
        view.isAutomaticQuoteSubstitutionEnabled = false
        view.isAutomaticDashSubstitutionEnabled = false
        view.isAutomaticSpellingCorrectionEnabled = false
        view.isAutomaticTextReplacementEnabled = false
        type("\"q\" a--b teh ", into: view)
        XCTAssertEqual(view.sourceText(), "\"q\" a--b teh \n")
    }

    func testTheMenuItemsAreCheckedAndDisabledInTheSourceView() throws {
        let view = try editor("prose\n")
        let actions: [(Selector, () -> Bool)] = [
            (#selector(LeafTextView.toggleAutomaticSpellingCorrection(_:)), { view.isAutomaticSpellingCorrectionEnabled }),
            (#selector(LeafTextView.toggleAutomaticTextReplacement(_:)), { view.isAutomaticTextReplacementEnabled }),
            (#selector(LeafTextView.toggleAutomaticQuoteSubstitution(_:)), { view.isAutomaticQuoteSubstitutionEnabled }),
            (#selector(LeafTextView.toggleAutomaticDashSubstitution(_:)), { view.isAutomaticDashSubstitutionEnabled }),
        ]
        for (action, isOn) in actions {
            let item = NSMenuItem(title: "", action: action, keyEquivalent: "")
            XCTAssertTrue(view.validateUserInterfaceItem(item), "\(action)")
            XCTAssertEqual(item.state, .on, "\(action)")
            view.perform(action, with: item)
            XCTAssertFalse(isOn(), "\(action) toggles")
            XCTAssertTrue(view.validateUserInterfaceItem(item))
            XCTAssertEqual(item.state, .off, "\(action)")
        }
        view.command { $0.toggleView() }
        for (action, _) in actions {
            let item = NSMenuItem(title: "", action: action, keyEquivalent: "")
            XCTAssertFalse(view.validateUserInterfaceItem(item), "\(action) does nothing in the source view")
        }
    }

    func testTheViewFollowsTheSystemSetting() throws {
        let view = try editor("\n")
        view.isAutomaticQuoteSubstitutionEnabled = !NSSpellChecker.isAutomaticQuoteSubstitutionEnabled
        NotificationCenter.default.post(name: NSSpellChecker.didChangeAutomaticQuoteSubstitutionNotification,
                                        object: nil)
        XCTAssertEqual(view.isAutomaticQuoteSubstitutionEnabled, NSSpellChecker.isAutomaticQuoteSubstitutionEnabled)
    }
}
#endif
