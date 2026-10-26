//  TextSearch.swift
//
//  What the system's find UI needs of a document, on either platform: the text
//  it searches, the crossing between its UTF-16 ranges and core's byte
//  offsets, the boxes a range is drawn in, and the search itself.
//
//  The system's ranges — `NSTextFinderClient`'s and `NSTextInputClient`'s on the
//  Mac, the find panel's on iOS — are in UTF-16 units of the text as it is
//  *shown*: the visible text, delimiters hidden, a block gap spelled as one
//  `\n`. That is not a scale away from core's byte offsets (hidden markup,
//  characters outside the BMP), so every range crosses through core's
//  `utf16IndexForOffset` / `offsetForUtf16Index` at the boundary, and inside a
//  view offsets stay bytes. The AppKit view answers the find bar with these;
//  the UIKit view answers `UITextSearching` with the same ones and searches
//  with `TextSearch.matches`, since on iOS the searching is the client's job.

import Foundation
import CoreGraphics
import LeafFFI

extension LeafDoc {
    /// The document's whole visible text — the string the system's UTF-16
    /// ranges index, and the one find searches: in the rendered view the words
    /// without their markup, in the source view the source.
    func visibleText() -> String { textInRange(from: 0, to: docEndOffset()) }

    /// A source byte range as the system's UTF-16 `NSRange` into `visibleText()`.
    func utf16Range(fromByte: Int, toByte: Int) -> NSRange {
        let lo = Int(utf16IndexForOffset(off: UInt32(max(0, fromByte))))
        let hi = Int(utf16IndexForOffset(off: UInt32(max(fromByte, toByte))))
        return NSRange(location: lo, length: hi - lo)
    }

    /// The system's UTF-16 `NSRange` as source byte bounds, both ends caret stops.
    func byteBounds(_ range: NSRange) -> (from: Int, to: Int) {
        let from = Int(offsetForUtf16Index(index: UInt32(max(0, range.location))))
        let to = Int(offsetForUtf16Index(index: UInt32(max(0, range.location + range.length))))
        return (from, max(from, to))
    }

    /// A match's UTF-16 `NSRange` as the source bytes of the characters it
    /// covers, and not a byte more — the range a find replaces.
    ///
    /// `byteBounds` ends a range at the caret stop of the character *after*
    /// it, and where markup is hidden between the two that stop is past the
    /// markup: "bold" in `**bold**` ends after the closing `**`, and replacing
    /// it took the delimiter with it. Here the end is just past the last
    /// character's own bytes, before anything hidden that follows. The start
    /// needs nothing done: a character's offset is already its own, inside
    /// whatever opens before it.
    func matchBounds(_ range: NSRange) -> (from: Int, to: Int) {
        let from = Int(offsetForUtf16Index(index: UInt32(max(0, range.location))))
        guard range.length > 0 else { return (from, from) }
        let end = range.location + range.length
        let last = Int(offsetForUtf16Index(index: UInt32(end - 1)))
        let next = Int(stepOffset(off: UInt32(last), delta: 1))
        guard next > last else { return (from, max(from, next)) }
        // The last character as the visible text spells it, which is how many
        // bytes it has in the source — unless it is the one `\n` a block gap is
        // shown as, which has no bytes of its own to end in, only the next stop.
        let shown = textInRange(from: UInt32(last), to: UInt32(next))
        guard let char = shown.first, char != "\n" else { return (from, max(from, next)) }
        var to = last + String(char).utf8.count
        // An escape (`\*` shown as `*`) is longer in the source than it shows:
        // walk on to the first byte the visible text counts past the character.
        while to < next, Int(utf16IndexForOffset(off: UInt32(to))) < end { to += 1 }
        return (from, max(from, min(to, next)))
    }
}

extension EditorLayout {
    /// The boxes a byte range of `doc` occupies, in layout coordinates — one per
    /// visual line it touches, plus any table cells it crosses. A collapsed
    /// range is the caret's box at that place.
    func rangeRects(fromByte from: Int, toByte to: Int, in doc: LeafDoc) -> [CGRect] {
        guard to > from else {
            let p = doc.posForOffset(off: UInt32(from))
            return rect(row: Int(p.row), ch: Int(p.ch)).map { [$0] } ?? []
        }
        let s = doc.posForOffset(off: UInt32(from)), e = doc.posForOffset(off: UInt32(to))
        let lines = rangeRects(from: (Int(s.row), Int(s.ch)), to: (Int(e.row), Int(e.ch))).map(\.rect)
        let cells = tableSelectionRects(from: from, to: to).map(\.rect)
        return lines + cells
    }
}

/// Finding a query in the visible text, as the iOS find panel asks for it.
enum TextSearch {
    /// How much of a word a match has to be — the find panel's Contains,
    /// Starts With and Full Word.
    enum WordMatch {
        case contains
        case startsWith
        case fullWord
    }

    /// Every non-overlapping occurrence of `query` in `text`, first to last, as
    /// UTF-16 ranges. `options` are `NSString`'s (case- and diacritic-insensitivity
    /// are what the panel sets); `word` then drops a match that starts, or
    /// starts or ends, inside a word.
    static func matches(of query: String, in text: String,
                        options: NSString.CompareOptions = [.caseInsensitive],
                        word: WordMatch = .contains) -> [NSRange] {
        guard !query.isEmpty else { return [] }
        let ns = text as NSString
        // A search walks forward from each match's end; direction and anchoring
        // are the walk's to decide, not the caller's.
        let options = options.subtracting([.backwards, .anchored])
        var found: [NSRange] = []
        var from = 0
        while from < ns.length {
            let r = ns.range(of: query, options: options, range: NSRange(location: from, length: ns.length - from))
            guard r.location != NSNotFound, r.length > 0 else { break }
            if accepts(r, in: ns, word: word) { found.append(r) }
            from = NSMaxRange(r)
        }
        return found
    }

    private static func accepts(_ r: NSRange, in ns: NSString, word: WordMatch) -> Bool {
        switch word {
        case .contains:
            return true
        case .startsWith:
            return !isWord(before: r.location, in: ns)
        case .fullWord:
            return !isWord(before: r.location, in: ns) && !isWord(at: NSMaxRange(r), in: ns)
        }
    }

    /// Whether the character ending just before `index` is part of a word.
    private static func isWord(before index: Int, in ns: NSString) -> Bool {
        guard index > 0 else { return false }
        return isWordCharacter(ns.substring(with: ns.rangeOfComposedCharacterSequence(at: index - 1)))
    }

    /// Whether the character starting at `index` is part of a word.
    private static func isWord(at index: Int, in ns: NSString) -> Bool {
        guard index < ns.length else { return false }
        return isWordCharacter(ns.substring(with: ns.rangeOfComposedCharacterSequence(at: index)))
    }

    private static func isWordCharacter(_ s: String) -> Bool {
        guard let scalar = s.unicodeScalars.first else { return false }
        return CharacterSet.alphanumerics.contains(scalar) || scalar == "_"
    }
}
