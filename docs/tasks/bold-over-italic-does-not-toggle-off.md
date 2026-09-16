---
title: Bold over italic does not toggle off
status: open
created: 2026-09-16
updated: 2026-09-16
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Bold over italic does not toggle off

Select the complete source span `*word*`, including its delimiters, through
`selectRange`. Toggle Bold twice. The first command produces `***word***`;
the second produces `*****word*****` instead of returning to `*word*`.
Repeated commands keep adding delimiters. This reproduces in both table cells
and ordinary paragraphs.

Selecting only the inner letters is a passing control. The full-span case
matters for selection ranges that include existing formatting syntax; diagnose
how the toggle locates nested emphasis and preserves its selected range.

Reproduced through the public Swift API in
`packages/leaf-swift/Tests/LeafUITests/TableFormattingTests.swift`,
`testBoldOverItalicCanBeToggledOffAgain`. Run `scripts/test-swift.sh --filter TableFormattingTests`.
The failing assertions are strict expected failures. These tests exercise the
Rust-backed editing API, not just Swift fixtures; the underlying fix may belong
in the shared editing or source-mapping implementation.

Done when the reproduction passes without expected-failure annotations and
the passing formatting roundtrip and undo/redo controls still pass.
