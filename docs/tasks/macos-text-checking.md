---
status: open
created: 2026-09-04
updated: 2026-09-04
---
# Spelling and autocorrect in the macOS view

**Where.** `packages/leaf-swift`, `LeafTextView` (macOS).

**What.** The iOS view gets spelling, autocorrect, and the smart substitutions
from `UITextInput` for free (and turns them off in the source view). The macOS
view has none: no red underline, no autocorrect, no Edit ▸ Spelling and Grammar.

**How.** Adopt `NSTextCheckingClient` — it extends `NSTextInputClient`, which
the view already speaks in UTF-16 units of the visible text — and hold an
`NSTextCheckingController` that drives it: `annotatedSubstring(forProposedRange:)`,
`setAnnotations(_:range:)`, `addAnnotations`, `removeAnnotation`,
`replaceCharacters(in:withAnnotatedString:)`, `selectAndShow(_:)`,
`view(for:firstRect:actualRange:)`, and `candidateListTouchBarItem`. The
annotations are the underlines, drawn from the same range boxes the find bar
uses (`EditorLayout.rangeRects`). Spell checking should follow the view the
way the iOS traits do: prose in WYSIWYG, off in the source view, where `**` is
not a misspelling.

**Done when.** A misspelled word gets the red dotted underline in the WYSIWYG
view and a right-click offers corrections; Edit ▸ Spelling and Grammar items
enable; nothing is underlined in the source view.
