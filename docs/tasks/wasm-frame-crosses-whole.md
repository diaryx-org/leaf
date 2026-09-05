---
status: open
created: 2026-09-04
updated: 2026-09-04
---
# The frame crosses the wasm boundary whole on every keystroke

**Where.** `crates/leaf-wasm/src/lib.rs`, `LeafDoc::view`.

**What.** Every model method answers with a complete `DocView` — every row of
the document, serialised through serde into JS objects — and the editor
repaints from it. Now that `packages/leaf-web` reuses the rows a frame did not
change, the boundary crossing is the cost that is left: on a 3,600-row document
a dev-profile `view()` is about 45 ms against under 2 ms for the repaint, and
the release profile only scales it down. A keystroke's cost is still
proportional to the document, just on the other side of the boundary.

**How.** Two shapes are worth measuring before choosing. A *revisioned* frame:
`view()` carries a revision and the range of rows that changed since the one
the caller last saw, and the editor asks for rows by range
(`rows(from, to)`) only where its keys no longer match — core already caches
`build_visual` on `(revision, width)`, so it knows. Or a *windowed* frame: the
editor says which rows are on screen and core serialises only those plus a
margin, with the row count and the caret's row so the scrollbar and the
scroll-into-view still work. The first keeps the editor's reconcile as it is;
the second changes it to a virtualised list. Either has to keep tables and
media, which are published beside the rows, in step.

**Done when.** A keystroke on a document of a few thousand rows costs the
binding time proportional to the rows that changed, measured in the browser
tests, and the reconcile in `editor.js` still passes every reuse test.
