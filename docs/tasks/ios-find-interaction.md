---
status: open
created: 2026-09-04
updated: 2026-09-04
---
# Find on iOS

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
