---
status: open
created: 2026-10-02
updated: 2026-10-02
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# A host cannot insert a link with a label of its own

**Where.** `crates/leaf-core/src/doc.rs`, `Doc::insert_link`, and its
exports in leaf-ffi, the wasm binding and `LeafEditorModel.insertLink`.

**What.** `insert_link(destination)` wraps the selection, or, with none,
writes `[dest](dest)` and selects the label so typing replaces it. That fits
an author pressing the link button. It does not fit a host that links
something it has just made and knows the name of. A host that adds a
page and links it from the caret gets `[2026-10-02.md](2026-10-02.md)`, then
navigates away from the selected label, and the reader is left with a file
name where a title should be. leaf has no verb that inserts text at the caret,
so the host cannot write the label first and link it either.

**Shape.** `insert_link_labelled(destination, label)`, or an optional
`label` on `insert_link`: with no selection, the label is written as the
link's text, escaped in the body's own grammar the way the destination
already is, and the caret lands after the link with nothing selected. With a
selection, the selection is linked and the label is ignored, as today.

**Done when** a test inserts a labelled link at a bare caret in Markdown and
djot and reads back `[Title](dest)` with the caret after it, a label
containing `]` round-trips, and the Swift, Kotlin and web bindings export it.
