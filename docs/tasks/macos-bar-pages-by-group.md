---
title: The macOS formatting bar pages by group instead of scrolling
status: done
created: 2026-09-18
updated: 2026-09-18
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# The macOS formatting bar pages by group instead of scrolling

**Status: done**, 2026-09-18. `LeafFormattingToolbar` in `.bar` style
measures its container and shows whole groups with chevrons to the rest;
`ToolbarPaging` is the arithmetic, under `ToolbarPagingTests`. A host's own
tools go on the row as `LeafFormattingToolbar.Tool` values (`.button` or
`.menu`), one more group after history. `apps/leaf-editor` takes the
package bar on both platforms, its display menus as host tools.

**Where.** `packages/leaf-swift`, `LeafFormattingToolbar`
(`Sources/LeafUI/FormattingToolbar.swift`). The demo host in
`apps/leaf-editor` has a private copy of the same shape and would take the
package's bar instead.

**What.** The bar is one `ScrollView(.horizontal, showsIndicators: false)`
on every platform. `Style.bar` gives macOS pointer-sized targets, but the
arrangement is still a keyboard accessory's: in a column too narrow for the
whole row, the tools past its edge are reachable by trackpad swipe and by
nothing else — a mouse has no sideways scroll — and nothing says they are
there beyond a button that happens to be cut in half. A finger flicks a row;
a pointer needs something to click.

The bar's own doc note says a host wanting another arrangement should build
it on the public commands, and diaryx did: its `FormattingStrip` is the same
tools, in the same groups, **paged by group** — the strip shows as many
whole groups as fit and a pair of chevrons at its trailing end turns to the
rest. A group is never split, so nothing reads as "scroll", and a page turn
is a click. That is not a diaryx opinion about its editor; it is the right
macOS behaviour for any host, and it should be the package's.

**Done when.**

- On macOS, `LeafFormattingToolbar` in `.bar` style pages by group rather
  than scrolling: whole groups only, chevrons when there is more than one
  page, the current page clamped (not reset) when the width changes so a
  page that has just become the last stays up. `.accessory` on iOS keeps
  the scroll.
- The width is measured on the container, not the row — measured on the
  row it can never be narrower than its own tools, so it always fits.
- The groups are the bar's existing ones (inline marks, block styles,
  indent, history); a host can append its own group of tools to the row,
  paged with the rest, so diaryx's paperclip has somewhere to go and diaryx
  can drop its copy.
- `apps/leaf-editor` uses the package bar on macOS.
- `EditorLayoutTests` (or a sibling) covers the paging arithmetic: which
  groups fit a width, and the clamp.
