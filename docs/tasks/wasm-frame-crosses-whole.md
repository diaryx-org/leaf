---
status: done
created: 2026-09-04
updated: 2026-09-20
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# The frame crosses the binding whole on every keystroke

**Status: done**, in four commits: `a8c9b1c` (the change frame in
`leaf-core::frame`, both bindings, and the regenerated Swift binding),
`b623db0` (`leaf-web`'s editor splices each change), `b29f412` (both Apple
views splice each change and hand the span to `EditorLayout`), and the
commit closing this file (the bench rows). What was chosen, and why, is
under *How* below.

After, with one row lifted per keystroke and none per click however long
the document. leaf-web, release wasm, 3,600 rows, driven from Node: a click
19.9 ms → 7.0 ms, a keystroke 22.8 → 10.0 ms; on the document doubled 39.6
→ 14.0 and 45.4 → 20.2. The Mac view on the bench's 701-row document, debug
build, timed against `LeafTextView`: a click 52.6 → 14.0 ms, a keystroke
73.4 → 30.5 ms, a drag step (two gestures) 120 → 39 ms. What is left of each
is the Rust side building every row of the frame before it compares them
(`view()` alone is 11 ms dev / 2.3 ms release on 701 rows, and grows with
the document), plus a few milliseconds of layout below the change — not the
crossing, which is now proportional to the gesture. The bench's last three
rows (`cargo run -p leaf-ffi --example bench`) time the gestures as changes
and print the rows each lifts, which do not grow on the doubled document.

**Where.** `crates/leaf-core/src/frame.rs`; `crates/leaf-ffi/src/lib.rs` and
`crates/leaf-wasm/src/lib.rs`, `DocView` and `LeafDoc::view`; `packages/leaf-web/src/editor.js`,
`render` and `_whole`; `packages/leaf-swift/Sources/LeafUI`, `LeafTextView`,
`LeafTextViewiOS` and `EditorLayout`.

**What.** Every model method in both bindings answered with a complete
`DocView` — every row of the document and its `Vec<Run>` — and the binding
lifted the whole of it across the boundary: UniFFI records into Swift, serde
into JS objects. On the bench's 14,000-word document (701 rows, 6,332 runs)
that lift was ~40 ms of a 54 ms dev-profile click in `LeafTextView`, and on a
3,600-row document ~45 ms of a wasm keystroke against under 2 ms for the
repaint; the Rust-side `view()` itself was ~11 ms dev. Both renderers already
reused the rows a frame did not change — Swift's `EditorLayout(previous:)`,
the web's reconcile — so the crossing was the cost that was left, and it was
proportional to the document rather than to what changed. A caret move
should lift zero rows.

**How.** The *revisioned* frame of the two shapes first proposed here (the
windowed one was not built: it would have changed both reconciles into
virtualised lists, and the row count is not what makes a keystroke slow).
Implemented at the layer both bindings share and consumed by both renderers:

- **Additive and opt-in.** `set_incremental_frames(true)` on `LeafDoc`, in
  both bindings. A caller that does not opt in gets a whole `DocView` from
  every method, unchanged in meaning. Opted in, the same methods answer
  with a `DocView` whose `rows` are only the changed span, and six fields
  say where it goes: `frame` (this frame's number), `basis` (the number of
  the frame the change is against — `0` on a whole frame, which is how a
  frame says which kind it is), `row_start`, `replaced` (how many of the
  basis frame's rows the span replaces), `row_count` (the document's rows
  after), and `src_shift`. `view()` is whole at any time and every change
  after it is against it; `rows(from, to)` is a window back. The
  `DocView` record gaining fields is source-breaking for Swift code that
  spells a literal, which the LeafUI test fixture did.
- **How core knows what changed.** No revision bookkeeping: the binding
  keeps the rows it last handed out and takes the common prefix and suffix
  against the new ones, in memory — microseconds on 701 rows, and it lifts
  nothing. The wrinkle is `Run::src`: a keystroke moves the source offset
  of every run after it, so read by `==` every row below an edit is a
  different row and the "span" is half the document. The suffix is
  therefore matched as *the same row, shifted*: equal in everything, with
  its offsets moved by one constant, which the frame carries as `src_shift`
  and the renderer applies to its own copies. The shift is read off the rows
  and checked on every row it is claimed for, never assumed from the edit,
  so applying frames in order reproduces `view()` exactly — the tests do
  this for a hundred random gestures against a second document driven with
  whole frames. A change names its basis, so a renderer that dropped a
  frame on the floor (the web's drop path does) detects it and asks
  `view()` rather than splice onto the wrong rows. The algorithm is one
  generic function, `leaf_core::frame::row_delta`; each binding says what
  "the same row, shifted" means for its own `Row`, field by field, so a
  field added to `Row` or `Run` fails to compile there rather than being
  ignored by the delta.
- **`tables`, `media`, `math`, `directives`** are small and ride every frame
  complete, indexed by the document's rows — a whole frame's `rows`, and
  the rows a change has been applied to — so no frame has to say whether
  they changed and no index is relative to a span.
- **Swift.** Both views opt in; `render` splices a change into `docView`
  (`DocView.apply`: O(span) plus a move of the rows after it, the rest of
  the frame taken as it came), so `docView` stays whole for the misspelling
  map, the find bar, the counts and the tables. The span is handed to
  `EditorLayout` rather than rediscovered — `sameText` reads it against the
  rows it replaced, and the shape narrowing (`Row.sameShape`) runs only over
  the span — so no row outside it is lifted or compared. Each kept row is
  still hashed once into the shape cache, which is what a relayout under a
  picture that arrived reads; 1.3 ms of a keystroke in a debug build, and
  the cost of a `reflow` without it would be shaping every row again.
- **Web.** `editor.js` opts in and `_whole` splices a change into
  `_lastView`, moving the kept rows' `src` in place — the offset `_adoptRow`
  refreshes on a reused element — and every path that keeps a frame without
  painting it goes through the same splice. The reconcile is unchanged.

**Done when.** A keystroke on a document of a few thousand rows costs the
binding time proportional to the rows that changed, measured in the browser
tests, and the reconcile in `editor.js` still passes every reuse test. Met:
the browser tests assert that a keystroke on a 3,600-row document lifts one
row and that the count is the same on a document ten times longer; a count
rather than a clock, since the headless driver's virtual time stands still.
The times above are from Node against the same wasm.
