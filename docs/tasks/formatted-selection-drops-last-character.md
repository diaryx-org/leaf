---
title: Selecting a formatted word drops its final character
status: done
created: 2026-09-16
updated: 2026-09-16
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Selecting a formatted word drops its final character

**Status: done.** The content end of a hidden inline mark is now a caret home
(`VisualMap::mark_ends`): `place_caret` leaves a focus there rather than
snapping it a character back, and Left/Right step onto it — so the end of
`**bold**` has two homes at one spot on screen, inside the mark and past it,
which is what the delete paths (`settle_inside_close_delims`) and
`active_inline_marks`'s documentation already assumed. The walks a system
text input counts characters against (`step_offset`, `distance_offset`)
skip it, so `text(in:)` and `offset(from:to:)` still agree.

In a Markdown table body cell containing `**bold**`, request the visible
word's four-byte range through `LeafDoc.setSelectionOffsets`. The focus snaps
from source offset 32 to 31 in the reproduction. Pressing Bold then produces
`****bol**d**` instead of removing bold from the word. This is separate from
Swift's approximate hit testing: the reproduction supplies correct source
offsets directly to the FFI.

The exact-range `selectRange` API is a useful control: selecting the four
letters with it allows bold to be removed, reapplied, and removed correctly.
The problem also appeared with `**bold** plain` outside a table during the
audit, so investigate the shared visible caret-stop mapping.

Reproduced through the public Swift API in
`packages/leaf-swift/Tests/LeafUITests/TableFormattingTests.swift`,
`testInteractiveSelectionOfBoldWordCanRemoveBold`. Run `scripts/test-swift.sh --filter TableFormattingTests`.
The failing assertions are strict expected failures. These tests exercise the
Rust-backed editing API, not just Swift fixtures; the underlying fix may belong
in the shared editing or source-mapping implementation.

Done when the reproduction passes without expected-failure annotations and
the passing formatting roundtrip and undo/redo controls still pass.
