//  Spelling.swift
//
//  The system's spelling checker and automatic substitutions, for the editor:
//  misspellings underlined as the text settles, guesses in the context menu,
//  Check Document Now, and what `NSTextView` does to text as it is typed —
//  a misspelling corrected, a text replacement expanded, a quote curled, a
//  `--` made a dash. LeafUI's `LeafTextView` is where each rule here was
//  worked out; this is that, over the C ABI.
//
//  Every range is a UTF-16 range of the text the system sees
//  (`leaf_view_text`): the visible text in the rendered view, its markup
//  hidden. The checker reads the same text with all but prose blanked
//  (`leaf_view_text(_, true)`), so code and bullets are never flagged and an
//  index into one is an index into the other. Nothing is checked or
//  substituted in the source view, where a word is whatever the markup needs.

import AppKit
import Combine

/// The switches under Edit ▸ Spelling and Grammar and Edit ▸ Substitutions,
/// for every editor in the app. The substitutions start as the system's
/// settings (Keyboard ▸ Text Input) and follow them when they change there.
final class SpellingSettings: ObservableObject {
    static let shared = SpellingSettings()

    private static let whileTypingKey = "CheckSpellingWhileTyping"

    @Published var checkWhileTyping: Bool {
        didSet { UserDefaults.standard.set(checkWhileTyping, forKey: Self.whileTypingKey) }
    }
    @Published var correctAutomatically = NSSpellChecker.isAutomaticSpellingCorrectionEnabled
    @Published var textReplacement = NSSpellChecker.isAutomaticTextReplacementEnabled
    @Published var smartQuotes = NSSpellChecker.isAutomaticQuoteSubstitutionEnabled
    @Published var smartDashes = NSSpellChecker.isAutomaticDashSubstitutionEnabled

    private var watches: [NSObjectProtocol] = []

    private init() {
        checkWhileTyping = UserDefaults.standard.object(forKey: Self.whileTypingKey) as? Bool ?? true
        let center = NotificationCenter.default
        let follow: [(Notification.Name, ReferenceWritableKeyPath<SpellingSettings, Bool>, () -> Bool)] = [
            (NSSpellChecker.didChangeAutomaticSpellingCorrectionNotification, \.correctAutomatically,
             { NSSpellChecker.isAutomaticSpellingCorrectionEnabled }),
            (NSSpellChecker.didChangeAutomaticTextReplacementNotification, \.textReplacement,
             { NSSpellChecker.isAutomaticTextReplacementEnabled }),
            (NSSpellChecker.didChangeAutomaticQuoteSubstitutionNotification, \.smartQuotes,
             { NSSpellChecker.isAutomaticQuoteSubstitutionEnabled }),
            (NSSpellChecker.didChangeAutomaticDashSubstitutionNotification, \.smartDashes,
             { NSSpellChecker.isAutomaticDashSubstitutionEnabled }),
        ]
        for (name, key, read) in follow {
            watches.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                self?[keyPath: key] = read()
            })
        }
    }
}

/// One editor's spelling: its checker document, the misspellings last found,
/// and the substitutions a keystroke completes.
final class Spelling {
    private unowned let view: LeafEditorNSView
    private let settings = SpellingSettings.shared
    /// This editor's document in the checker, which is what Ignore Spelling
    /// remembers a word for.
    let tag = NSSpellChecker.uniqueSpellDocumentTag()
    /// The text last scheduled for a check: its revision, and whether it was
    /// the source view's.
    private var scheduled: (revision: UInt64, source: Bool)?
    /// The misspellings found in the text at `revision`.
    private var found: (revision: UInt64, ranges: [NSRange]) = (0, [])
    private var work: DispatchWorkItem?
    /// Advanced by every schedule, so an older answer is dropped.
    private var generation = 0
    private var watch: AnyCancellable?

    init(view: LeafEditorNSView) {
        self.view = view
        watch = settings.$checkWhileTyping.dropFirst().sink { [weak self] _ in
            DispatchQueue.main.async { self?.schedule(force: true) }
        }
    }

    deinit {
        work?.cancel()
        NSSpellChecker.shared.closeSpellDocument(withTag: tag)
    }

    private var handle: OpaquePointer? { view.handle }
    /// Read afresh: the model's copy is a turn behind a keystroke.
    private var state: LeafState { view.currentState() }
    private var checks: Bool { settings.checkWhileTyping && !state.source_view }

    // MARK: Checking as the text settles

    /// The editor repainted. A change of text, or of view, is checked once
    /// the typing pauses; a move of the caret is not.
    func editorChanged() {
        let now = (revision: state.revision, source: state.source_view)
        guard scheduled.map({ $0 != now }) ?? true else { return }
        scheduled = now
        schedule(force: false)
    }

    private func schedule(force: Bool) {
        work?.cancel()
        generation += 1
        guard checks else {
            found = (state.revision, [])
            leaf_view_set_misspellings(handle, state.revision, nil, 0)
            return
        }
        let generation = generation
        let item = DispatchWorkItem { [weak self] in self?.check(generation: generation) }
        work = item
        DispatchQueue.main.asyncAfter(deadline: .now() + (force ? 0 : 0.3), execute: item)
    }

    private func check(generation: Int) {
        let revision = state.revision
        let text = view.text(prose: true)
        let whole = NSRange(location: 0, length: (text as NSString).length)
        guard whole.length > 0 else { return }
        NSSpellChecker.shared.requestChecking(
            of: text, range: whole, types: NSTextCheckingResult.CheckingType.spelling.rawValue,
            options: nil, inSpellDocumentWithTag: tag
        ) { [weak self] _, results, _, _ in
            DispatchQueue.main.async {
                guard let self, generation == self.generation, self.checks else { return }
                let ranges = results.filter { $0.resultType.contains(.spelling) }.map(\.range)
                    .filter { !Self.isHyphenatedCompound($0, in: text) }
                self.found = (revision, ranges)
                let flat = ranges.flatMap { [$0.location, $0.length] }
                flat.withUnsafeBufferPointer {
                    leaf_view_set_misspellings(self.handle, revision, $0.baseAddress, ranges.count)
                }
            }
        }
    }

    /// The misspelled word the caret stands in, or at the end of. If the
    /// pass after the last edit has not answered yet, the checker is asked
    /// for the caret's paragraph there and then, so a word just typed still
    /// gets its guesses.
    func misspellingAtCaret() -> NSRange? {
        guard !state.source_view, let caret = view.selectedUTF16(), caret.length == 0 else {
            return nil
        }
        let at = caret.location
        let hit = { (r: NSRange) in at >= r.location && at <= NSMaxRange(r) }
        if found.revision == state.revision {
            return found.ranges.first(where: hit)
        }
        let text = view.text(prose: true) as NSString
        var start = text.paragraphRange(for: NSRange(location: min(at, text.length), length: 0)).location
        while start < text.length {
            let r = NSSpellChecker.shared.checkSpelling(
                of: text as String, startingAt: start, language: nil, wrap: false,
                inSpellDocumentWithTag: tag, wordCount: nil)
            guard r.location != NSNotFound, r.location <= at else { return nil }
            if hit(r), !Self.isHyphenatedCompound(r, in: text as String) { return r }
            start = max(start + 1, NSMaxRange(r))
        }
        return nil
    }

    /// The context menu's spelling items for the word at `range`: the
    /// checker's guesses, then Ignore Spelling and Learn Spelling.
    func addItems(for range: NSRange, to menu: NSMenu) {
        let text = view.text(prose: false) as NSString
        guard NSMaxRange(range) <= text.length else { return }
        let word = text.substring(with: range)
        let guesses = NSSpellChecker.shared.guesses(
            forWordRange: range, in: text as String, language: nil, inSpellDocumentWithTag: tag) ?? []
        if guesses.isEmpty {
            let none = menu.addItem(withTitle: "No Guesses Found", action: nil, keyEquivalent: "")
            none.isEnabled = false
        }
        for guess in guesses.prefix(8) {
            menu.addItem(ClosureMenuItem(title: guess) { [weak self] in
                self?.replace(range, word, with: guess)
            })
        }
        menu.addItem(.separator())
        menu.addItem(ClosureMenuItem(title: "Ignore Spelling") { [weak self] in
            guard let self else { return }
            NSSpellChecker.shared.ignoreWord(word, inSpellDocumentWithTag: self.tag)
            self.schedule(force: true)
        })
        menu.addItem(ClosureMenuItem(title: "Learn Spelling") { [weak self] in
            NSSpellChecker.shared.learnWord(word)
            self?.schedule(force: true)
        })
        menu.addItem(.separator())
    }

    private func replace(_ range: NSRange, _ word: String, with guess: String) {
        if !leaf_view_replace_range(handle, range.location, range.length, word, guess, false) {
            NSSound.beep()
        }
    }

    /// Edit ▸ Spelling and Grammar ▸ Check Document Now: select the next
    /// misspelled word after the selection, from the top once past the end.
    func checkDocumentNow() {
        guard !state.source_view, let sel = view.selectedUTF16() else { return }
        let text = view.text(prose: true)
        var start = NSMaxRange(sel)
        for _ in 0..<2 {
            let r = NSSpellChecker.shared.checkSpelling(
                of: text, startingAt: start, language: nil, wrap: true,
                inSpellDocumentWithTag: tag, wordCount: nil)
            guard r.location != NSNotFound else { break }
            if !Self.isHyphenatedCompound(r, in: text) {
                leaf_view_select_range(handle, r.location, r.length)
                return
            }
            start = NSMaxRange(r)
        }
        NSSound.beep()
    }

    /// Apple's checker flags some ordinary editorial compounds
    /// ("round-trippable"). A hyphen joins the token deliberately, so neither
    /// the compound nor a part of it is underlined.
    static func isHyphenatedCompound(_ range: NSRange, in text: String) -> Bool {
        let ns = text as NSString
        guard range.location != NSNotFound, NSMaxRange(range) <= ns.length else { return false }
        let token = CharacterSet.letters.union(.init(charactersIn: "-‐‑‒–—'’"))
        let isToken = { (i: Int) in
            ns.substring(with: NSRange(location: i, length: 1)).rangeOfCharacter(from: token) != nil
        }
        var start = range.location
        var end = NSMaxRange(range)
        while start > 0, isToken(start - 1) { start -= 1 }
        while end < ns.length, isToken(end) { end += 1 }
        return ns.substring(with: NSRange(location: start, length: end - start))
            .rangeOfCharacter(from: .init(charactersIn: "-‐‑‒–—")) != nil
    }

    // MARK: Substitutions as the text is typed

    /// One substitution the checker proposed for what was just typed.
    struct Substitution: Equatable {
        let range: NSRange
        let replacement: String
    }

    /// Make the substitutions the keystroke `typed` completes, as
    /// `NSTextView` makes them, through core's `substitute`: the caret stays
    /// where the typing left it, and they are one undo step, apart from the
    /// typing, so ⌘Z puts back exactly what was typed.
    func afterTyping(_ typed: String) {
        guard !state.source_view, !state.has_selection else { return }
        // Most keystrokes are letters, which complete nothing; only a quote,
        // the end of a word, or what follows a `--` is worth reading the text.
        let mayQuote = settings.smartQuotes && typed.contains { $0 == "\"" || $0 == "'" }
        let mayWord = Self.endsWord(typed) && (settings.textReplacement || settings.correctAutomatically)
        let mayDash = settings.smartDashes && typed.first != "-"
        guard mayQuote || mayWord || mayDash, let caret = view.selectedUTF16() else { return }
        let prose = view.text(prose: true)
        if mayDash, !mayQuote, !mayWord {
            let ns = prose as NSString
            let back = max(0, caret.location - (typed as NSString).length - 2)
            guard caret.location <= ns.length,
                  ns.substring(with: NSRange(location: back, length: caret.location - back)).contains("--")
            else { return }
        }
        let found = substitutions(afterTyping: typed, in: prose, caret: caret.location)
        guard !found.isEmpty else { return }
        let visible = view.text(prose: false) as NSString
        leaf_view_undo_group(handle, true)
        defer { leaf_view_undo_group(handle, false) }
        // Last first, so each earlier range still stands where it was found.
        for sub in found where NSMaxRange(sub.range) <= visible.length {
            let original = visible.substring(with: sub.range)
            _ = leaf_view_replace_range(handle, sub.range.location, sub.range.length,
                                        original, sub.replacement, true)
        }
    }

    /// What the system would substitute now that `typed` has been typed at
    /// UTF-16 `caret` of `text`, only in the caret's paragraph and only what
    /// the keystroke itself completes:
    ///
    /// - a straight quote is curled as it is typed, by what stands before it;
    /// - a `--` becomes a dash once the character after it is typed, and that
    ///   character is not another hyphen, so `---` is left for the rule it is;
    /// - a word is corrected, or a text replacement expanded, as the character
    ///   that ends it is typed — a space or punctuation, not an apostrophe or
    ///   a hyphen inside it.
    ///
    /// Last first.
    func substitutions(afterTyping typed: String, in text: String, caret: Int) -> [Substitution] {
        let ns = text as NSString
        let typedLength = (typed as NSString).length
        guard typedLength > 0, caret >= typedLength, caret <= ns.length else { return [] }
        let typedStart = caret - typedLength
        let typedRange = NSRange(location: typedStart, length: typedLength)
        let endsWord = Self.endsWord(typed)
        let checker = NSSpellChecker.shared
        let language = checker.language()
        let correcting = endsWord && settings.correctAutomatically
            && !checker.preventsAutocorrection(before: typed, language: language)
        var types: NSTextCheckingResult.CheckingType = []
        if settings.smartQuotes { types.insert(.quote) }
        if settings.smartDashes { types.insert(.dash) }
        if endsWord && settings.textReplacement { types.insert(.replacement) }
        if correcting { types.formUnion([.spelling, .correction]) }
        guard !types.isEmpty else { return [] }

        let paragraph = ns.paragraphRange(for: NSRange(location: typedStart, length: 0))
        let checked = NSRange(location: paragraph.location, length: caret - paragraph.location)
        let results = checker.check(text, range: checked, types: types.rawValue, options: nil,
                                    inSpellDocumentWithTag: tag, orthography: nil, wordCount: nil)
        let firstTyped = typed.first
        var found: [Substitution] = []
        // A text replacement is what the user asked for by name, so it wins
        // over a correction proposed for the same word.
        var wordTaken = false
        for result in results {
            let r = result.range
            guard let replacement = result.replacementString, !replacement.isEmpty else { continue }
            switch result.resultType {
            case .quote:
                guard NSIntersectionRange(r, typedRange).length > 0 else { continue }
            case .dash:
                // Two hyphens exactly: a third either side is a rule, or
                // someone's ASCII art, and not a dash.
                guard NSMaxRange(r) == typedStart, firstTyped != "-", ns.substring(with: r) == "--",
                      r.location == 0 || ns.substring(with: NSRange(location: r.location - 1, length: 1)) != "-"
                else { continue }
            case .replacement:
                guard NSMaxRange(r) == typedStart else { continue }
                wordTaken = true
            default:
                continue
            }
            found.append(Substitution(range: r, replacement: replacement))
        }
        if correcting, !wordTaken {
            // The checker reports a word it would correct as a correction
            // where the system's own setting is on, and as a plain misspelling
            // where it is off; the correction for the latter is asked by word.
            for result in results where NSMaxRange(result.range) == typedStart {
                let word = result.range
                let replacement: String?
                if result.resultType == .correction {
                    replacement = result.replacementString
                } else if result.resultType == .spelling, !Self.isHyphenatedCompound(word, in: text) {
                    replacement = checker.correction(forWordRange: word, in: text, language: language,
                                                     inSpellDocumentWithTag: tag)
                } else {
                    continue
                }
                guard let replacement, !replacement.isEmpty, replacement != ns.substring(with: word)
                else { continue }
                found.append(Substitution(range: word, replacement: replacement))
                break
            }
        }
        return found.sorted { $0.range.location > $1.range.location }
    }

    /// Whether typing `typed` ends the word before it: a space or
    /// punctuation, not an apostrophe or a hyphen, which a word goes on
    /// through.
    static func endsWord(_ typed: String) -> Bool {
        typed.first.map { c in
            (c.isWhitespace || c.isPunctuation || c.isSymbol) && !"'’-‐".contains(c)
        } ?? false
    }
}
