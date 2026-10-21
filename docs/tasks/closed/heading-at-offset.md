---
status: done
created: 2026-09-16
updated: 2026-10-21
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# The heading the caret is under is a question `Doc` cannot answer

**Status: done.** The leaf half is `feat(core): heading_at — the heading a
place in the document is under` (`d15bbb7`), which adds `Doc::heading_at` /
`heading_at_caret` and the `Heading` type, in the shape below, and exposes
both through leaf-ffi and the wasm binding as a `HeadingView` (`text`,
`level`, `start`, `end`); `heading_at_caret_is_the_nearest_heading_above` in
`doc/tests/links.rs` is the test the done-when asks for. provui's half is
`refactor(core)!: ask leaf for the heading under the caret instead of parsing
the body again` (provui `d512e25`, diaryx-org/provui#5):
`DocumentSession::heading_at_caret` delegates to leaf, and `twig` has left
`provui-core`'s manifest.

**Where.** `crates/leaf-core/src/doc/under_caret.rs`, beside `link_destination_at_caret`
and `locate`.

**What.** `Doc` answers "which link is the caret in" and "where does this
fragment land", but not the question between them: **which heading is the
caret under**. A host writing a link *to* the caret's position needs the
nearest heading at or above it, its level, and its text, so that
`#its-slug` can be attached to a reference. `Doc::nodes()` holds exactly the
`FlatNode`s that answer it and is private, so a host has no route to the
answer short of parsing the body a second time through its own twig — which
is what provui does today (`DocumentSession::heading_at_caret`, landed with
`c11733a` there): the same twig-doc, the same tree, one extra parse per call.

**Shape.** `pub fn heading_at(&mut self, off: usize) -> Option<Heading>` with
`Heading { text: String, level: u32, span: Range<usize> }`, and a
`heading_at_caret()` twin, following the pair `link_destination_at` /
`link_destination_at_caret` already draws. The text is the heading's inline
content with markup stripped, which is what a slug is made from; the span is
the whole block, which is what a peek draws. Whether `nodes()` itself should
become public is a separate question — it is the whole arena, and every
caller so far has wanted one node out of it.

**Done when** provui's second parse is gone: `heading_at_caret` there
delegates to leaf, the `twig` dependency it grew for this leaves
`provui-core`'s manifest, and a test here places the caret in a paragraph
under a level-2 heading and gets that heading back, and under no heading gets
`None`.
