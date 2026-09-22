---
title: Backspace at the start of a table cell eats the column separator
status: done
created: 2026-09-16
updated: 2026-09-22
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# Backspace at the start of a table cell eats the column separator

**Status.** Done, in `fix(core): a table cell's start is a wall to
Backspace, and its end to Delete`. The meaning chosen is the wall: in the
rich view Backspace at a cell's first stop — or anywhere in the padding
before it — does nothing, and Delete at its last stop or in the padding
after it is the mirror. An in-cell `<br>` at the edge is still taken whole.
Source view keeps the literal byte delete, as it does for a list marker.

**Where.** `crates/leaf-core`, `Doc::backspace`. Every frontend inherits it.

**What.** With the caret at the first character of a Markdown table cell,
Backspace is an ordinary character delete: the first press removes the
padding space before the caret, the second removes the `|`, and the two cells
are one — the row is a column short of its header and the grid the reader was
editing has quietly changed shape.

Repro, through core alone:

```rust
let src = "| a | b |\n| - | - |\n| c | d |\n";
let mut d = Doc::from_source(src.to_string(), Format::Markdown).unwrap();
d.place_caret(src.find("d |").unwrap(), false);
d.backspace();   // "| c |d |"
d.backspace();   // "| c d |"
```

Found on iOS (the touch surface makes the two presses easy to run together),
but the TUI and the macOS view do the same. `backspace` already gives the
structural keys their own meaning at a list item's start and a heading's
start, and steps over an in-cell `<br>` whole; a cell's start is the same kind
of place and has no case.

**How.** Decide what the key means there, then write it as a case beside
`backspace_list_start`. The choices other table editors make: nothing at all
(the cell's start is a wall), or move to the previous cell's end without
deleting (what Shift+Tab does, `cell_tab(false)`). Deleting the separator is
the one answer no editor gives, because it is a structural edit the writer
did not ask for. Delete-forward at a cell's end is the mirror and should take
the same rule.

**Done when.** The repro leaves the source unchanged (or moves the caret to
`c`'s end, if that is the meaning chosen) after any number of presses, with a
test in `doc.rs` beside the list-start and heading-start ones, and the
forward-delete peer is covered too.
