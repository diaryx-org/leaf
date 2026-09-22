---
status: done
created: 2026-09-04
updated: 2026-09-22
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# Find on iOS

**Status.** Done, in `feat(swift): find and replace on iOS through the
system find panel`. The iOS `LeafTextView` holds a `UIFindInteraction` over
its own `UITextSearching` conformance. The search runs over the visible
text (`TextSearch.matches`, with the panel's case and word options), so the
rendered view finds words and not their markup, and the source view finds
the source. Matches cross into source bytes through core's UTF-16 mapping,
now shared with the AppKit find bar in `TextSearch.swift`. The lit matches
are drawn from `EditorLayout.rangeRects`. The current match is the
selection, and a replace goes through `replaceRange`. The search is run
again when the text changes under an open panel. Find Selection in the
selection's edit menu opens the panel on a phone with no keyboard, and
`LeafEditorCommands` has the iPad Find menu. A match now ends before any
markup that closes around it (`matchBounds`), which fixed the Mac's find bar
too: replacing a bold word there used to take its closing `**` with it.
Replace All is one undo step per match, because core's history cannot group
edits. That is deferred to
[Replace All is one undo step](/docs/tasks/closed/replace-all-is-one-undo-step.md).

**Where.** `packages/leaf-swift`, `LeafTextView` (iOS).

**What.** The macOS view has the system find bar; the iOS view has no find at
all, so ⌘F on an iPad keyboard and the Find rotor do nothing.

**How.** `UIFindInteraction` (iOS 16, the package floor) over a `UITextSearching`
conformance: `performTextSearch(queryString:using:resultAggregator:)` walking the
visible text through core's UTF-16 mapping, `decorate(foundTextRange:document:usingStyle:)`
drawing from `EditorLayout.rangeRects`, `scrollRangeToVisible`, and replace
through `replaceRange`. `LeafEditorCommands`' Find menu is macOS-only today
and should gain the iOS items alongside.

**Done when.** ⌘F on iPad brings up the system find panel, matches highlight,
next/previous move the selection, and replace edits the document.
