---
status: done
created: 2026-10-02
updated: 2026-10-03
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# A host cannot insert a link with a label of its own

**Status.** Done, after leaf 0.4.13, in the leaf commit `feat(leaf-core):
insert a link with a label of its own`, as a new verb so `insert_link`'s
callers are untouched: `Doc::insert_link_labelled(destination, label)`,
exported as `LeafDoc.insert_link_labelled` (leaf-ffi; `insertLinkLabelled`
in Swift and Kotlin), `LeafDoc.insert_link_labelled` in leaf-wasm,
`insertLinkLabelled(dest, label)` on the web editor, and
`LeafEditorModel.insertLink(_:label:)`. At a bare caret the label goes in
through twig's `insert_literal` and is wrapped by `insert_link`, one undo
step, caret after the link. With a selection, an empty label, or a caret
standing inside a link or autolink, it is `insert_link` and the label is
ignored — the last so a host's Edit Link can call the one verb and still
re-point. The tests are the `insert_link_labelled_*` group in `doc.rs` and
`testInsertLinkWithALabelWritesTheLabel` in leaf-swift.

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
