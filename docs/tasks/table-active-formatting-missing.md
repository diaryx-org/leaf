---
title: Table cells omit active formatting from the toolbar
status: open
created: 2026-09-16
updated: 2026-09-16
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Table cells omit active formatting from the toolbar

**Diagnosis.** twig: the Markdown parser gives every `cell` in a row the
*row's* span (only `content_span` is the cell's — see the note on
`holdsInlinesDirectly` in twig's `locate.zig`), and `nodes_at` descends into
the *last* child whose span holds the offset, so in any row of two or more
columns the chain ends at the last cell and never reaches the mark in the
first. Djot cells carry their own spans and answer correctly; so does a
one-column Markdown table. `marks_at` is `ancestors_at` and inherits the
fault. Fixed in twig by giving a Markdown cell its own span; closes with the
twig-doc pin that carries it.

Place the caret inside `**word**`, `*word*`, `~~word~~`, `==word==`, or
`` `word` `` in a Markdown table body cell. Structural cell runs correctly
carry the corresponding style, but `DocView.active` is empty. The same
caret placement in an ordinary paragraph reports the style correctly.

`FormattingToolbar` uses these active marks to light its buttons, so the
buttons show the wrong state. A further manual API probe found that toggling
Bold inside bold table text reports Bold as active even though subsequent
typing exits bold. Check both existing and pending marks when fixing this.

Reproduced through the public Swift API in
`packages/leaf-swift/Tests/LeafUITests/TableFormattingTests.swift`,
`testTableCaretReportsExistingStylesToToolbar`. Run `scripts/test-swift.sh --filter TableFormattingTests`.
The failing assertions are strict expected failures. These tests exercise the
Rust-backed editing API, not just Swift fixtures; the underlying fix may belong
in the shared editing or source-mapping implementation.

Done when the reproduction passes without expected-failure annotations and
the passing formatting roundtrip and undo/redo controls still pass.
