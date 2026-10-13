---
title: Following a fragment lands before its heading
status: done
created: 2026-10-12
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
updated: 2026-10-12
---
# Following a fragment lands before its heading

**Status.** Done, in the leaf commit `fix(leaf-core): following a fragment
lands on its heading, not before it`. The cause was `place_caret`'s snap:
`locate` hands back the heading's first byte, the `#` of a hidden `## `, and
the nearest stop to that was the end of the paragraph above. WYSIWYG
placement now goes through `Doc::snap_to_stop_on_its_line`, which takes the
first stop on the line when the offset is in hidden markup that opens it (a
heading's `## `, a list's `- `) and the nearest stop otherwise, so a click in
the blank gap between blocks is unchanged. Every frontend lands through
`place_caret`, so none changed. The core test is
`following_a_fragment_puts_the_caret_on_the_heading_not_before_it`, and the
web link tap test now asserts the caret is on `End`.

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
