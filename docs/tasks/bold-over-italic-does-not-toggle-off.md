---
title: Bold over italic does not toggle off
status: in-progress
created: 2026-09-16
updated: 2026-09-16
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Bold over italic does not toggle off

**Diagnosis.** twig: `***word***` parses as `emph > strong`, so the selected
range is the emph's span and `Splicer.inlineNodeCovering` — which asks for a
`strong` whose span or content span *equals* the range — finds none and
wraps again. The same rule turns a drag that ends past a hidden closing
delimiter (`[2,8)` of `**bold** plain`, interior plus `**`) into
`****bold****`. Fixed in twig by matching a mark whose content the range
covers and whose span covers the range, through single-child marks nested
around it; closes with the twig-doc pin that carries it.

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
