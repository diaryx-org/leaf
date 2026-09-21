---
status: open
created: 2026-09-20
updated: 2026-09-20
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# The Apple editor does work proportional to the document on every interaction

**Where.** `packages/leaf-swift`, `LeafTextView` (macOS) and `EditorLayout`;
`crates/leaf-core/src/wysiwyg.rs`, `VisualMap`; `crates/leaf-ffi`.

**What.** On a 14,000-word Markdown document (a design document — long
paragraphs, inline code, a table or two) the macOS view is unusable: opening
it takes many seconds, a scroll beach-balls for ten or fifteen, and with
spell checking off a click still takes a second or two to show a caret and a
drag as long again to grow a selection. The document model is not the cost —
twig parses it in a millisecond and a keystroke's map rebuild is ten — but
three things around it are each proportional to the whole document, and two
of them run on every repaint. Measured with `cargo run -p leaf-ffi --example
bench`, which this task added.

1. **Misspellings are mapped to rects on every draw.** `drawMisspellings`
   walks every reported range — no cull to the dirty band — and for each
   makes four FFI calls: `offsetForUtf16Index` twice (`byteBounds`) and
   `posForOffset` twice (`rangeRects`). `offset_at_visible_utf16` allocates
   a `Vec` of every visible character in the document and scans it
   (`visible_items(0, len)`), and `pos_of_offset` is a scan over rows and
   glyphs that costs 3.7 ms on a document of long paragraphs — slower than
   a scan of 215 rows should be, so something in `col_of_glyph`/`width()`
   is superlinear in a row. Nine milliseconds a word, a hundred or two
   words of jargon a spell checker does not know, and every scroll repaints
   the lot. `spellCheckingText()` has the same shape once per edit: one
   `utf16IndexForOffset` per run to build the masked string, thousands of
   runs, half a millisecond each.

2. **A caret move rebuilds the whole layout.** Every gesture answers with a
   whole `DocView`, and `render` hands it to `EditorLayout.init`, which
   walks every row, hashes each `Row` (all its runs and their text) up to
   three times against the shape cache, and compares `rows` twice more on
   the way in. The shaping is cached, the bookkeeping is not, and a
   drag-select does it per mouse-moved event. The binding's share of a
   click is under 4 ms release and 25 ms dev; the seconds are this.

3. **Counts run after every repaint.** `repainted` schedules `counts()` +
   `selectionCounts()`, 24 ms release and 200 ms dev, debounced. Not the
   cause, but the same rule applies: a caret move changed no count.

The wasm binding has the boundary-crossing half of this written up already
in [The frame crosses the wasm boundary whole on every keystroke](wasm-frame-crosses-whole.md);
what is here is the Apple renderer's half and the core lookups both share.

**How.**

- Map misspelled ranges to rects once, when the checker answers and again on
  relayout, and cull the cached rects to the dirty band in `draw`. Build the
  spell-check mask from the rows the view already holds — row text and a
  per-row UTF-16 start — instead of one round trip per run.
- Give `VisualMap` a prefix table: cumulative UTF-16 length per stop, so
  `visible_utf16_len` and `offset_at_visible_utf16` are a binary search; and
  binary-search rows by `end_src` in `pos_of_offset` before scanning glyphs.
  Find and remove the per-row superlinearity while there.
- In `render`, when `rows`, `tables`, `media` and `math` are unchanged, keep
  the `EditorLayout` and update only `docView` and the caret. That is the
  plain click. A selection changes the rows it spans, so `EditorLayout`
  should diff against the previous frame and reuse `RowLayout`s outside the
  changed range, re-flowing from the first changed row — which makes a
  keystroke and a drag proportional to what changed, and covers the edit
  case the cache alone did not.
- Skip the recount when the frame's rows are the frame before's.

**Done when.** On the bench's generated document and a real one of the same
size, in the dev profile: opening is well under a second, a scroll with
spell checking on repaints at frame rate, and a click or a drag step shows
its caret or selection without perceptible delay. The bench's per-call rows
for `offset_for_utf16_index` and `pos_for_offset` no longer grow with the
document. `scripts/test-swift.sh` still passes.
