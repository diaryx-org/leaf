---
status: done
created: 2026-09-21
updated: 2026-09-21
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# A block can be moved — by keyboard, and by dragging it — and an attachment is a block

**Status.** Done, on twig-doc 3.9.1's offset-addressed `move_block`, which
landed before leaf called `move_before`/`move_after` at all — so the
cross-container cases are in from the start rather than deferred. What
shipped differs from the shape below in one place: `drop_target_at` and
`block_range_at` are `Doc`'s, not `VisualMap`'s, because a block is a fact
about the tree and the map has no tree. The boundary above a block is its
first byte for every kind; the one below a container is the start of the
line after it, since the end of a quote's last line is where a block joins
the quote. Three boundaries 3.9.0 refused or misread were filed with twig
as `move-block-boundary-gaps` and fixed in 3.9.1.

**Where.** `crates/leaf-core/src/doc.rs` (the op), `wysiwyg.rs` (the drop
target), then each frontend's gesture.

**What.** Restructuring a document today is cut and paste: select the whole
paragraph (triple-click), cut, find the place, paste, fix the blank lines.
The thing people most want to move is a picture — an attachment dropped in
the wrong place — and a block image *is* a paragraph holding one image node,
so "move an attachment" and "move a paragraph" are one operation on the
tree. An inline image in the middle of a sentence is not: that is a span cut
and pasted, which `edit`/`paste` already do, and stays caret-driven.

twig already has the tree op. `Editor::move_before`/`move_after`
(twig-doc 3.8.1) move a node next to another in one splice and one undo
step, and the anchor need not be a sibling. leaf pins 3.7.0 and uses neither;
they are the first locator-addressed ops it would call, so the index path is
derived from `nodes()` (`parent`, `next_sibling`) at the moment of the move,
never held across a reparse. Their stated limit — *bytes, not structure*:
a `> ` prefix does not travel, a list item's continuation indent is not
added — is twig's
[move-block-as-an-offset-addressed-gesture](https://github.com/diaryx-org/twig/blob/main/docs/tasks/closed/move-block-as-an-offset-addressed-gesture.md),
and until it lands a move whose source and destination are in different
containers is refused here with a status line, not attempted.

**Shape.**

1. `Doc::move_block(from: usize, to: usize)` — `from` inside the block to
   move (the same block `select_block_at` finds), `to` a block boundary it
   lands before, or the document's end. Goes through the same
   refresh/caret/dirty path as `apply_table`, with the caret riding the moved
   block to its new place. `move_block_up()`/`move_block_down()` are the
   same call with `to` at the previous/next sibling's boundary, and are the
   first slice: they prove the op with no hit-testing, and every frontend
   binds them (Alt+↑/↓, and the Format menu on macOS).
2. `VisualMap::drop_target_at(row) -> Option<usize>` — the nearest block
   boundary offset to a row, so a frontend drawing a drop indicator asks the
   map rather than measuring text, and `block_range_at(offset)` — what
   `select_block_at` computes, without moving the caret — for the outline of
   the block being carried.
3. The drag itself, per frontend: `UIDragInteraction`/`NSDraggingSession`
   in swift, pointer events in the web view; ratatui has the keyboard pair
   only. A drag that starts on a block image or on a paragraph's margin
   picks the block up; a drag inside text stays a selection.

**Done when** the twig pin is ≥ 3.8.1, `move_block` has tests for a
paragraph, an image block, a table, and a code block moved between two
top-level paragraphs and to the document's end, each one undo step; the
keyboard pair works in all three frontends; and macOS and web can drag an
image block to a new place with the indicator drawn from `drop_target_at`.
Cross-container drops become part of the done state once twig's gesture is
released and the pin moves again.
