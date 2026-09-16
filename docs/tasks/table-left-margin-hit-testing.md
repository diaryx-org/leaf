---
title: Clicks left of a table jump to its last cell
status: done
created: 2026-09-16
updated: 2026-09-16
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Clicks left of a table jump to its last cell

**Status: done.** `TableLayout.locate(atX:y:)` sends a click left of the first
column to the first cell; the right margin still lands in the last.

Click in the left margin at the vertical position of a two-column table row. `TableLayout.locate(atX:y:)` falls back to the last cell when no column matches, including negative x. Clamp left-margin clicks to the first cell; retain last-cell behavior for clicks beyond the right edge.

Reproduced on macOS in `packages/leaf-swift/Tests/LeafUITests/TableBugTests.swift`,
`testClickLeftOfGridUsesFirstCell`. Run `scripts/test-swift.sh --filter TableBugTests`.
The assertion is a strict expected failure; remove the annotation when fixed.

Done when the reproduction passes without an expected failure.
