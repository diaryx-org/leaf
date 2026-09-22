---
status: in-progress
created: 2026-09-04
updated: 2026-09-22
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Writing Tools in the Apple views

**Status.** Implemented, not yet seen working end to end, in
`feat(swift): inline Writing Tools in both views through the system's coordinator`.
Both views install a coordinator (`NSWritingToolsCoordinator` on macOS 15.2,
`UIWritingToolsCoordinator` on iOS 18.2) and fall back to the panel path
below that. The shared mapping is in `WritingTools.swift`:
- the context is the visible text, widened to paragraphs;
- ranges cross into the source through `matchBounds`;
- rewrites go through `replaceRange`, with a paragraph break written as a
  blank line;
- a session is one core undo group (`Doc::begin_undo_group`), open from the
  moment the coordinator leaves `inactive` until it returns.

Geometry and previews come from `EditorLayout.rangeRects`, and animated text
is hidden or dimmed under a clip. The Mac's Edit menu and context menu carry
the system's Writing Tools items. `WritingToolsTests` covers the delegate's
answers without Apple Intelligence.

What remains is to watch it work. On the Mac this was written on, the
coordinator took over as it should: the inline Proofread bar, contexts,
animations and previews all arrived. Writing Tools then answered "Writing
Tools Unavailable", and TextEdit showed no Writing Tools menu at all, so no
suggestion was ever delivered. Close this once, on a Mac or iPhone where
Writing Tools returns results, inline Proofread shows its suggestions in the
text, each can be accepted or rejected, and accepting all is one undo. One
thing to check then: after that failed session, the spelling underline stayed
missing until the next redraw of the rows, although the view's misspellings
were intact.

**Where.** `packages/leaf-swift`, both `LeafTextView`s.

**What.** The basic Writing Tools experience already works on both
platforms, without either view doing anything for Writing Tools as such. On
macOS the view is an `NSServicesMenuRequestor`: `validRequestor(forSendType:returnType:)`
accepts a string both ways while there is a selection, `writeSelection(to:types:)`
hands over the selected text, and `readSelection(from:)` puts the result
back over the selection through core (`pasteRich`). That is the path the
Writing Tools panel takes for a custom view. On iOS the view is a
`UITextInput`: `text(in:)` gives the selected text and `replace(_:withText:)`
puts the result back through `replaceRange`. So selecting a paragraph and
choosing Proofread or Rewrite shows the result in the panel, and accepting
it replaces the selection.

What is missing is the inline experience, the one a native text view gets:
the rewrite animating in place over the rows, proofreading suggestions
underlined in the text and reviewable one by one, and the original kept
alongside until the user accepts. That needs
`NSWritingToolsCoordinator` / `UIWritingToolsCoordinator` (macOS 15.2,
iOS 18.2).

**How.** A coordinator delegate that answers with the visible text as an
`NSAttributedString`, maps its ranges to geometry through
`EditorLayout.rangeRects` (as find does), and applies each rewritten range
through core's `replaceRange`. The proofreading marks and the rewrite
previews are drawn over the rows. The package's floor is macOS 12 / iOS 16,
so all of it sits behind `#available`, and the panel stays the fallback
below it.

**Done when.** Selecting a paragraph and choosing Writing Tools ▸ Proofread
shows the suggestions in place in the text rather than in a panel, each can
be accepted or rejected there, and accepting all is one undo step.
