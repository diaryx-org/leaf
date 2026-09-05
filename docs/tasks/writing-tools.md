---
status: open
created: 2026-09-04
updated: 2026-09-04
---
# Writing Tools in the Apple views

**Where.** `packages/leaf-swift`, both `LeafTextView`s.

**What.** macOS 15.2 and iOS 18.2 offer Writing Tools to custom text views
through `NSWritingToolsCoordinator` / `UIWritingToolsCoordinator`. The Edit
menu already shows the item (AppKit adds it for every app); it does nothing
here. A user who has it in every other text field will expect it.

**How.** A coordinator delegate answering with the visible text as an
`NSAttributedString`, its range geometry (again `EditorLayout.rangeRects`),
and applying the rewritten text through core's `replaceRange` — with the
proofreading/rewrite previews drawn over the rows. The package's floor is
macOS 12 / iOS 16, so the whole thing is behind `#available`.

**Done when.** Selecting a paragraph and choosing Writing Tools ▸ Proofread
shows the suggestions in place and applies them as one undo step.
