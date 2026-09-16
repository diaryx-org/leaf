---
title: Table caret and selection drift after Unicode
status: open
created: 2026-09-16
updated: 2026-09-16
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Table caret and selection drift after Unicode

In a table body cell containing `caféXYZ`, place the caret after `é` or select `X`. The caret and selection start render after `X`, about 10.55 points too far right with the default font. Source UTF-8 byte offsets are used directly as CoreText UTF-16 indices in `EditorLayout.tableCaretRect` and `tableSelectionRects`.

Reproduced on macOS in `packages/leaf-swift/Tests/LeafUITests/TableBugTests.swift`,
`testUnicodeCaretAndSelectionUseSourceBytesAsUTF16`. Run `scripts/test-swift.sh --filter TableBugTests`.
The assertion is a strict expected failure; remove the annotation when fixed.

Done when the reproduction passes without an expected failure.
