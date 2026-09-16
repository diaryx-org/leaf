---
title: Table clicks lose inline markup source offsets
status: open
created: 2026-09-16
updated: 2026-09-16
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Table clicks lose inline markup source offsets

In a table body cell containing `**bold** tail`, with markup hidden, click immediately before `tail`. Hit testing returns source offset 24 instead of 28 in the reproduction, even after `snapOffset`. `EditorLayout.tableHitOffset` adds visible text bytes to the line start without accounting for hidden delimiters. Preserve source mapping through shaping and wrapping.

Reproduced on macOS in `packages/leaf-swift/Tests/LeafUITests/TableBugTests.swift`,
`testClickAfterInlineMarkupResolvesToVisibleCharacter`. Run `scripts/test-swift.sh --filter TableBugTests`.
The assertion is a strict expected failure; remove the annotation when fixed.

Done when the reproduction passes without an expected failure.
