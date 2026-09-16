---
title: Bold inside code inserts visible delimiters
status: open
created: 2026-09-16
updated: 2026-09-16
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Bold inside code inserts visible delimiters

**Diagnosis.** twig: `locate.inlineHostPieces` treats a verbatim's interior
as an inline host, so `applyInline` writes `**` inside the backticks, where
it is text. Fixed in twig by widening a piece inside a code span to the
whole span, so the mark closes around the code (`` **`word`** ``, which
toggles off again); closes with the twig-doc pin that carries it.

Select the inner letters of `` `word` `` and invoke Bold. The source becomes
`` `**word**` `` and the visible code text becomes `**word**`; no bold styling
is applied. Pressing Bold again produces `` `****word****` `` and adds more
visible asterisks. Both table cells and ordinary paragraphs reproduce this.

The formatting command should preserve the visible content, either applying
formatting outside the code span or declining an unsupported combination.
It should never silently insert markup as literal user text. Selecting the
entire code span, including backticks, is a passing control for wrapping bold
outside code and removing it again.

Reproduced through the public Swift API in
`packages/leaf-swift/Tests/LeafUITests/TableFormattingTests.swift`,
`testBoldInsideCodeDoesNotInsertLiteralAsterisks`. Run `scripts/test-swift.sh --filter TableFormattingTests`.
The failing assertions are strict expected failures. These tests exercise the
Rust-backed editing API, not just Swift fixtures; the underlying fix may belong
in the shared editing or source-mapping implementation.

Done when the reproduction passes without expected-failure annotations and
the passing formatting roundtrip and undo/redo controls still pass.
