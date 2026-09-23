//  WritingToolsTests.swift
//
//  The Writing Tools coordinators' delegates, answered without Apple
//  Intelligence: which text a context holds, how its ranges cross into the
//  source, and that a session's rewrites are applied through core as one
//  undo step.

import XCTest
import LeafFFI
@testable import LeafUI

/// The mapping both views share.
final class WritingToolsTextTests: XCTestCase {
    private let text = "Title\nOne two three. Four.\nNext para."

    func testTheSelectionScopeIsTheSelectionsParagraphs() {
        let ns = text as NSString
        let selection = ns.range(of: "two three")
        let span = WritingToolsText.span(for: .selection, in: text, selection: selection, visible: .init())
        XCTAssertEqual(ns.substring(with: NSRange(location: span.origin, length: span.length)), "One two three. Four.\n")
        XCTAssertEqual(span.range, NSRange(location: 4, length: 9), "the selection, relative to its paragraph")
    }

    func testAnEmptySelectionIsTheWholeDocument() {
        let span = WritingToolsText.span(for: .selection, in: text, selection: NSRange(location: 3, length: 0),
                                         visible: .init())
        XCTAssertEqual(span, WritingToolsSpan(origin: 0, length: (text as NSString).length,
                                              range: NSRange(location: 0, length: (text as NSString).length)))
    }

    func testTheVisibleScopeWidensToParagraphs() {
        let ns = text as NSString
        let visible = NSRange(location: ns.range(of: "three").location, length: 16)
        let span = WritingToolsText.span(for: .visible, in: text, selection: .init(), visible: visible)
        XCTAssertEqual(span.origin, ns.range(of: "One").location)
        XCTAssertEqual(NSMaxRange(NSRange(location: span.origin, length: span.length)), ns.length)
        XCTAssertEqual(span.range.location, visible.location - span.origin)
    }

    func testAParagraphBreakInARewriteIsABlankLineInTheSource() {
        XCTAssertEqual(WritingToolsText.sourceText("One.\nTwo."), "One.\n\nTwo.")
        XCTAssertEqual(WritingToolsText.sourceText("One."), "One.")
    }

    func testARangeCrossesToTheWordsOwnBytes() throws {
        let doc = try LeafDoc(source: "# Title\n\nA **bold** word.\n", format: "markdown")
        let visible = doc.visibleText() as NSString
        let origin = visible.range(of: "A ").location
        let bold = visible.range(of: "bold")
        let range = NSRange(location: bold.location - origin, length: bold.length)
        let (from, to) = doc.writingToolsBytes(range, origin: origin)
        XCTAssertEqual(String(decoding: Array(doc.source().utf8)[from..<to], as: UTF8.self), "bold")
        _ = doc.applyWritingTools(range, origin: origin, text: "brave")
        XCTAssertEqual(doc.source(), "# Title\n\nA **brave** word.\n", "the markup around it stays")
    }
}

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit

final class WritingToolsTests: XCTestCase {
    private func editor(_ source: String) throws -> LeafTextView {
        let view = LeafTextView(doc: try LeafDoc(source: source, format: "markdown"), theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.layout()
        return view
    }

    /// Select `word`'s bytes — the sources here are ASCII, so its UTF-16
    /// location in the source is its byte offset.
    private func select(_ word: String, in view: LeafTextView) {
        let at = (view.sourceText() as NSString).range(of: word).location
        view.command { $0.selectRange(start: UInt32(at), end: UInt32(at + word.utf8.count)) }
    }

    /// The single context for `scope`, and its text.
    @available(macOS 15.2, *)
    private func context(_ view: LeafTextView, _ scope: NSWritingToolsCoordinator.ContextScope) throws
        -> NSWritingToolsCoordinator.Context {
        let contexts = view.writingToolsContexts(for: scope)
        XCTAssertEqual(contexts.count, 1, "one text storage, one context")
        return try XCTUnwrap(contexts.first)
    }

    /// `word`'s range in `context`.
    @available(macOS 15.2, *)
    private func range(of word: String, in context: NSWritingToolsCoordinator.Context) -> NSRange {
        (context.attributedString.string as NSString).range(of: word)
    }

    func testTheViewHasACoordinatorForTheInlineExperience() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("Writing Tools' coordinator is macOS 15.2") }
        let view = try editor("prose\n")
        let coordinator = try XCTUnwrap(view.writingToolsCoordinator)
        XCTAssertTrue(coordinator.delegate === view.writingToolsDelegate)
        XCTAssertTrue(coordinator.decorationContainerView === view)
        XCTAssertEqual(coordinator.preferredBehavior, .complete)
        XCTAssertEqual(coordinator.preferredResultOptions, [.plainText])
        view.isReadOnly = true
        XCTAssertEqual(coordinator.preferredBehavior, NSWritingToolsBehavior.none, "nothing to rewrite on a reader")
        // The panel's path stays for everything else: a selection is offered as a string.
        view.isReadOnly = false
        select("prose", in: view)
        XCTAssertTrue(view.validRequestor(forSendType: .string, returnType: .string) as AnyObject === view)
    }

    func testAContextIsTheVisibleTextNotTheMarkup() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("# Title\n\nA **bold** word, *mostly*.\n\nNext.\n")
        let whole = try context(view, .fullDocument)
        XCTAssertEqual(whole.attributedString.string, view.string)
        XCTAssertEqual(whole.range, NSRange(location: 0, length: (view.string as NSString).length))
        XCTAssertNotNil(whole.attributedString.attribute(.font, at: 0, effectiveRange: nil))

        select("bold", in: view)
        let selected = try context(view, .userSelection)
        XCTAssertEqual(selected.attributedString.string, "A bold word, mostly.\n", "the selection's paragraph")
        XCTAssertEqual((selected.attributedString.string as NSString).substring(with: selected.range), "bold")
    }

    func testAReplacementGoesThroughCoreAndKeepsTheMarkup() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("# Title\n\nA **bold** word.\n")
        select("bold", in: view)
        let ctx = try context(view, .userSelection)
        let proposed = NSAttributedString(string: "brave")
        let applied = view.writingToolsReplace(range(of: "bold", in: ctx), in: ctx, with: proposed)
        XCTAssertEqual(applied?.string, "brave", "the text that went in is the answer")
        XCTAssertEqual(view.sourceText(), "# Title\n\nA **brave** word.\n")
    }

    func testARewriteOfTwoParagraphsStaysTwoParagraphs() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("One.\n\nTwo.\n")
        let ctx = try context(view, .fullDocument)
        _ = view.writingToolsReplace(ctx.range, in: ctx, with: NSAttributedString(string: "First.\nSecond."))
        XCTAssertEqual(view.sourceText(), "First.\n\nSecond.\n")
    }

    func testASessionIsOneUndoStep() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("\n")
        view.command { $0.insert(text: "Their ") }
        view.command { $0.insert(text: "is a mistake here, and their too.") }
        let before = view.sourceText()
        let ctx = try context(view, .fullDocument)
        // Proofreading: the suggestions arrive one by one, last to first here so
        // the ranges stay as the context has them.
        view.writingToolsWillChange(to: .interactiveResting)
        view.writingToolsWillChange(to: .interactiveStreaming)
        let text = ctx.attributedString.string as NSString
        _ = view.writingToolsReplace(text.range(of: "their", options: .backwards), in: ctx,
                                     with: NSAttributedString(string: "there"))
        _ = view.writingToolsReplace(text.range(of: "Their"), in: ctx, with: NSAttributedString(string: "There"))
        view.writingToolsWillChange(to: .interactiveResting)
        view.writingToolsWillChange(to: .inactive)
        let after = "There is a mistake here, and there too.\n"
        XCTAssertEqual(view.sourceText(), after)
        XCTAssertFalse(view.writingToolsGroupOpen)

        view.command { $0.undo() }
        XCTAssertEqual(view.sourceText(), before, "one undo takes back every suggestion")
        view.command { $0.redo() }
        XCTAssertEqual(view.sourceText(), after, "one redo puts them all back")
        view.command { $0.undo() }
        view.command { $0.undo() }
        XCTAssertEqual(view.sourceText(), "Their \n", "the typing before is its own steps, not folded in")
        view.command { $0.undo() }
        XCTAssertEqual(view.sourceText(), "\n")
    }

    func testTypingAfterASessionIsAStepOfItsOwn() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("teh cat\n")
        let ctx = try context(view, .fullDocument)
        view.writingToolsWillChange(to: .interactiveResting)
        _ = view.writingToolsReplace(range(of: "teh", in: ctx), in: ctx, with: NSAttributedString(string: "the"))
        // The coordinator never said it was done; the next keystroke closes it.
        view.command { $0.selectRange(start: 7, end: 7) }
        view.keyDown(with: try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
            characters: "s", charactersIgnoringModifiers: "s", isARepeat: false, keyCode: 1)))
        XCTAssertFalse(view.writingToolsGroupOpen)
        view.insertText("s", replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertTrue(view.sourceText().hasPrefix("the cat"), view.sourceText())
        view.command { $0.undo() }
        XCTAssertEqual(view.sourceText(), "the cat\n")
    }

    func testTypingDuringASessionIsAStepOfItsOwn() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("teh cat\n")
        let ctx = try context(view, .fullDocument)
        view.writingToolsWillChange(to: .interactiveResting)
        _ = view.writingToolsReplace(range(of: "teh", in: ctx), in: ctx, with: NSAttributedString(string: "the"))
        view.command { $0.selectRange(start: 7, end: 7) }
        // Typed while Writing Tools is still at work — `keyDown` leaves a
        // working session's group alone, and the input system then inserts.
        view.insertText("s", replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertFalse(view.writingToolsGroupOpen, "the user's edit is not the session's")
        view.doCommand(by: #selector(NSResponder.deleteBackward(_:)))
        view.insertText("!", replacementRange: NSRange(location: NSNotFound, length: 0))
        view.writingToolsWillChange(to: .inactive)
        XCTAssertEqual(view.sourceText(), "the cat!\n")
        view.command { $0.undo() }
        XCTAssertEqual(view.sourceText(), "the cat\n", "the typing goes first, alone")
        view.command { $0.undo() }
        XCTAssertEqual(view.sourceText(), "the cats\n", "the deletion")
        view.command { $0.undo() }
        XCTAssertEqual(view.sourceText(), "the cat\n", "the first keystroke")
        view.command { $0.undo() }
        XCTAssertEqual(view.sourceText(), "teh cat\n", "and then the rewrite, alone")
    }

    func testAReaderTakesNoReplacement() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("teh cat\n")
        let ctx = try context(view, .fullDocument)
        view.isReadOnly = true
        XCTAssertNil(view.writingToolsReplace(range(of: "teh", in: ctx), in: ctx, with: NSAttributedString(string: "the")))
        XCTAssertEqual(view.sourceText(), "teh cat\n")
    }

    func testSelectingARangeSelectsItsBytes() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("A **bold** word.\n")
        let ctx = try context(view, .fullDocument)
        view.writingToolsSelect([NSValue(range: range(of: "bold", in: ctx))], in: ctx)
        XCTAssertEqual(view.firstSelectedRange, (view.string as NSString).range(of: "bold"))
    }

    func testTheGeometryIsTheLayoutsRangeRects() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("# Title\n\nA **bold** word.\n")
        let ctx = try context(view, .fullDocument)
        let bold = range(of: "bold", in: ctx)
        let boxes = view.writingToolsBoxes(bold, in: ctx)
        XCTAssertEqual(boxes.count, 1)
        let expected = try XCTUnwrap(view.rects(forCharacterRange: (view.string as NSString).range(of: "bold"))?.first?.rectValue)
        let paths = view.writingToolsBoundingPaths(bold, in: ctx)
        XCTAssertEqual(paths.map(\.bounds), [expected], "the same box find lights, in view coordinates")
        let underline = try XCTUnwrap(view.writingToolsUnderlinePaths(bold, in: ctx).first).bounds
        XCTAssertEqual(underline.minX, expected.minX)
        XCTAssertEqual(underline.width, expected.width)
        XCTAssertEqual(underline.maxY, expected.maxY, accuracy: 1, "along the foot of the word")
    }

    func testAnAnimationHidesItsTextUntilItFinishes() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("A **bold** word.\n")
        let ctx = try context(view, .fullDocument)
        let bold = range(of: "bold", in: ctx)
        XCTAssertTrue(view.writingToolsEffectRects().hidden.isEmpty)
        view.writingToolsPrepare(.anticipate, for: bold, in: ctx)
        XCTAssertEqual(view.writingToolsEffectRects().hidden, view.writingToolsBoxes(bold, in: ctx))
        view.writingToolsPrepare(.anticipateInactive, for: bold, in: ctx)
        XCTAssertEqual(view.writingToolsEffectRects().dimmed.count, 1, "greyed while Writing Tools waits")
        view.writingToolsFinish(.anticipate, for: bold, in: ctx)
        view.writingToolsFinish(.anticipateInactive, for: bold, in: ctx)
        XCTAssertTrue(view.writingToolsEffectRects().hidden.isEmpty)
        XCTAssertTrue(view.writingToolsEffectRects().dimmed.isEmpty)
    }

    func testAPreviewIsTheWordsOverTheirBoxes() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("A **bold** word.\n")
        let ctx = try context(view, .fullDocument)
        let bold = range(of: "bold", in: ctx)
        let preview = try XCTUnwrap(view.writingToolsPreviews(of: bold, in: ctx)?.first)
        let boxes = preview.candidateRects.map(\.rectValue)
        XCTAssertEqual(boxes, view.writingToolsBoundingPaths(bold, in: ctx).map(\.bounds))
        XCTAssertTrue(preview.presentationFrame.contains(boxes[0]))
        XCTAssertGreaterThan(preview.previewImage.width, 0)
        // Something was drawn: the words are ink on a transparent ground.
        XCTAssertTrue(hasInk(preview.previewImage), "the preview has the word in it")

        let rect = view.writingToolsPreview(for: preview.presentationFrame)
        XCTAssertNotNil(rect)
    }

    func testTheDelegateAnswersThroughTheView() throws {
        guard #available(macOS 15.2, *) else { throw XCTSkip("macOS 15.2") }
        let view = try editor("teh cat\n")
        let coordinator = try XCTUnwrap(view.writingToolsCoordinator)
        let delegate = try XCTUnwrap(view.writingToolsDelegate as? LeafWritingToolsDelegate)
        var contexts: [NSWritingToolsCoordinator.Context] = []
        delegate.writingToolsCoordinator(coordinator, requestsContextsFor: .fullDocument) { contexts = $0 }
        let ctx = try XCTUnwrap(contexts.first)
        var answer: NSAttributedString?
        delegate.writingToolsCoordinator(coordinator, replace: NSRange(location: 0, length: 3), in: ctx,
                                         proposedText: NSAttributedString(string: "the"), reason: .interactive,
                                         animationParameters: nil) { answer = $0 }
        XCTAssertEqual(answer?.string, "the")
        XCTAssertEqual(view.sourceText(), "the cat\n")
        var paths: [NSBezierPath] = []
        delegate.writingToolsCoordinator(coordinator, requestsBoundingBezierPathsFor: NSRange(location: 4, length: 3),
                                         in: ctx) { paths = $0 }
        XCTAssertEqual(paths.count, 1)
        var finished = false
        delegate.writingToolsCoordinator(coordinator, willChangeTo: .inactive) { finished = true }
        XCTAssertTrue(finished)
        XCTAssertFalse(view.writingToolsGroupOpen)
    }

    private func hasInk(_ image: CGImage) -> Bool {
        let w = image.width, h = image.height
        var pixels = [UInt8](repeating: 0, count: w * h * 4)
        guard let space = CGColorSpace(name: CGColorSpace.sRGB),
              let ctx = CGContext(data: &pixels, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                  space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return false }
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        return stride(from: 3, to: pixels.count, by: 4).contains { pixels[$0] > 0 }
    }
}
#endif

#if canImport(UIKit)
import UIKit

final class WritingToolsTests: XCTestCase {
    private func editor(_ source: String) throws -> (LeafDoc, LeafTextView) {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = CGRect(x: 0, y: 0, width: 402, height: 700)
        view.layoutIfNeeded()
        return (doc, view)
    }

    @available(iOS 18.2, *)
    private func context(_ view: LeafTextView, _ scope: UIWritingToolsCoordinator.ContextScope) throws
        -> UIWritingToolsCoordinator.Context {
        let contexts = view.writingToolsContexts(for: scope)
        XCTAssertEqual(contexts.count, 1, "one text storage, one context")
        return try XCTUnwrap(contexts.first)
    }

    @available(iOS 18.2, *)
    private func range(of word: String, in context: UIWritingToolsCoordinator.Context) -> NSRange {
        (context.attributedString.string as NSString).range(of: word)
    }

    func testTheViewHasACoordinatorForTheInlineExperience() throws {
        guard #available(iOS 18.2, *) else { throw XCTSkip("Writing Tools' coordinator is iOS 18.2") }
        let (_, view) = try editor("prose\n")
        let coordinator = try XCTUnwrap(view.writingToolsCoordinator)
        XCTAssertTrue(view.interactions.contains { $0 === coordinator }, "an interaction on the view")
        XCTAssertTrue(coordinator.delegate === view.writingToolsDelegate)
        XCTAssertEqual(coordinator.preferredBehavior, .complete)
        view.isReadOnly = true
        XCTAssertEqual(coordinator.preferredBehavior, UIWritingToolsBehavior.none, "nothing to rewrite on a reader")
    }

    func testAContextIsTheVisibleTextAroundTheSelection() throws {
        guard #available(iOS 18.2, *) else { throw XCTSkip("iOS 18.2") }
        let (doc, view) = try editor("# Title\n\nA **bold** word, *mostly*.\n\nNext.\n")
        XCTAssertEqual(try context(view, .fullDocument).attributedString.string, doc.visibleText())
        let at = UInt32(("# Title\n\nA **bold**" as NSString).range(of: "bold").location)
        view.command { $0.selectRange(start: at, end: at + 4) }
        let selected = try context(view, .userSelection)
        XCTAssertEqual(selected.attributedString.string, "A bold word, mostly.\n")
        XCTAssertEqual((selected.attributedString.string as NSString).substring(with: selected.range), "bold")
    }

    func testAReplacementGoesThroughCoreAndKeepsTheMarkup() throws {
        guard #available(iOS 18.2, *) else { throw XCTSkip("iOS 18.2") }
        let (doc, view) = try editor("# Title\n\nA **bold** word.\n")
        let ctx = try context(view, .fullDocument)
        let applied = view.writingToolsReplace(range(of: "bold", in: ctx), in: ctx, with: NSAttributedString(string: "brave"))
        XCTAssertEqual(applied?.string, "brave")
        XCTAssertEqual(doc.source(), "# Title\n\nA **brave** word.\n")
        view.isReadOnly = true
        XCTAssertNil(view.writingToolsReplace(range(of: "word", in: ctx), in: ctx, with: NSAttributedString(string: "x")))
    }

    func testASessionIsOneUndoStep() throws {
        guard #available(iOS 18.2, *) else { throw XCTSkip("iOS 18.2") }
        let (doc, view) = try editor("Their is a mistake, and their too.\n")
        let ctx = try context(view, .fullDocument)
        let text = ctx.attributedString.string as NSString
        view.writingToolsWillChange(to: .interactiveResting)
        _ = view.writingToolsReplace(text.range(of: "their", options: .backwards), in: ctx,
                                     with: NSAttributedString(string: "there"))
        _ = view.writingToolsReplace(text.range(of: "Their"), in: ctx, with: NSAttributedString(string: "There"))
        view.writingToolsWillChange(to: .inactive)
        XCTAssertEqual(doc.source(), "There is a mistake, and there too.\n")
        XCTAssertFalse(view.writingToolsGroupOpen)
        view.command { $0.undo() }
        XCTAssertEqual(doc.source(), "Their is a mistake, and their too.\n", "one undo takes back every suggestion")
        XCTAssertFalse(doc.view().canUndo)
        view.command { $0.redo() }
        XCTAssertEqual(doc.source(), "There is a mistake, and there too.\n")
    }

    func testTypingDuringASessionIsAStepOfItsOwn() throws {
        guard #available(iOS 18.2, *) else { throw XCTSkip("iOS 18.2") }
        let (doc, view) = try editor("teh cat\n")
        let ctx = try context(view, .fullDocument)
        view.writingToolsWillChange(to: .interactiveResting)
        _ = view.writingToolsReplace(range(of: "teh", in: ctx), in: ctx, with: NSAttributedString(string: "the"))
        view.command { $0.selectRange(start: 7, end: 7) }
        view.insertText("s")
        XCTAssertFalse(view.writingToolsGroupOpen, "the user's edit is not the session's")
        view.writingToolsWillChange(to: .interactiveResting)
        view.deleteBackward()
        XCTAssertFalse(view.writingToolsGroupOpen)
        view.writingToolsWillChange(to: .inactive)
        XCTAssertEqual(doc.source(), "the cat\n")
        view.command { $0.undo() }
        XCTAssertEqual(doc.source(), "the cats\n", "the deletion alone")
        view.command { $0.undo() }
        XCTAssertEqual(doc.source(), "the cat\n", "the typing alone")
        view.command { $0.undo() }
        XCTAssertEqual(doc.source(), "teh cat\n", "and then the rewrite")
    }

    func testTheGeometryIsTheLayoutsRangeRects() throws {
        guard #available(iOS 18.2, *) else { throw XCTSkip("iOS 18.2") }
        let (doc, view) = try editor("A **bold** word.\n")
        let ctx = try context(view, .fullDocument)
        let bold = range(of: "bold", in: ctx)
        let boxes = view.writingToolsBoxes(bold, in: ctx)
        XCTAssertEqual(boxes, view.layoutEngine.rangeRects(fromByte: 4, toByte: 8, in: doc))
        XCTAssertEqual(view.writingToolsBoundingPaths(bold, in: ctx).map(\.bounds), boxes)
        view.writingToolsPrepare(.anticipate, for: bold, in: ctx)
        XCTAssertEqual(view.writingToolsEffectRects().hidden, boxes, "hidden under the animation")
        view.writingToolsFinish(.anticipate, for: bold, in: ctx)
        XCTAssertTrue(view.writingToolsEffectRects().hidden.isEmpty)
        XCTAssertNil(view.writingToolsPreview(of: bold, in: ctx), "no preview for a view in no window")
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 402, height: 700))
        window.addSubview(view)
        defer { view.removeFromSuperview() }
        let preview = try XCTUnwrap(view.writingToolsPreview(of: bold, in: ctx))
        XCTAssertTrue(preview.target.container === view)
        XCTAssertEqual(preview.view.frame, boxes[0].integral)
    }

    func testTheDelegateAnswersThroughTheView() throws {
        guard #available(iOS 18.2, *) else { throw XCTSkip("iOS 18.2") }
        let (doc, view) = try editor("teh cat\n")
        let coordinator = try XCTUnwrap(view.writingToolsCoordinator)
        let delegate = try XCTUnwrap(view.writingToolsDelegate as? LeafWritingToolsDelegate)
        var contexts: [UIWritingToolsCoordinator.Context] = []
        delegate.writingToolsCoordinator(coordinator, requestsContextsFor: .fullDocument) { contexts = $0 }
        let ctx = try XCTUnwrap(contexts.first)
        var answer: NSAttributedString?
        delegate.writingToolsCoordinator(coordinator, replace: NSRange(location: 0, length: 3), in: ctx,
                                         proposedText: NSAttributedString(string: "the"), reason: .interactive,
                                         animationParameters: nil) { answer = $0 }
        XCTAssertEqual(answer?.string, "the")
        XCTAssertEqual(doc.source(), "the cat\n")
    }
}
#endif
