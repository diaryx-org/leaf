---
title: Following a fragment lands before its heading
status: open
created: 2026-10-12
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Following a fragment lands before its heading

**Repro.** In a document with a `## Notes` heading after a paragraph, follow a
`[the notes](#notes)` link — a tap in a reader on iOS (`ce41241`), a tap or
⌘-click on the web, or Open Link from the long-press menu. The view scrolls to
the heading, but the caret lands at the end of the paragraph *before* it, not
on the heading.

**Where.** Shared: the long-press menu's Open Link and the web show the same,
so the landing offset comes from core (`Doc::locate` / `self_landing`, and how
`reveal` maps it to a caret stop), not from either frontend. A heading's
hidden `## ` is the likely suspect: an offset at the block's first byte snaps
to the nearest visible stop, and the paragraph's end is as near as the
heading's start.

**Done when.** Following `#notes` puts the caret at the start of the heading's
text on every frontend, and the web test beside the link tap test
(`packages/leaf-web/test/editor.test.html`) asserts where the caret lands
rather than only that a reveal was asked for.
