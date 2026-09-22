---
status: open
created: 2026-09-16
updated: 2026-09-22
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# `can_undo` counts edits, and twig counts steps

**Status.** Open: the leaf half is in, provui's is not. `fix(core):
can_undo answers exactly, and undo and redo say whether they moved` makes
both halves truthful. twig has no depth query, so `undo_steps` is now an
exact mirror of its stacks rather than a count of edits — it follows every
`coalesce_last_undo` under twig's two-step rule and twig's 200-step cap —
and `Doc::undo()` / `redo()` return `bool`. The frames leaf-ffi and the wasm
binding hand back carry the exact `can_undo` / `can_redo`; their `undo` and
`redo` still return a frame. The done-when's test is
`can_undo_is_false_once_a_coalesced_run_is_undone`, and
`can_undo_is_exact_across_the_gestures_that_coalesce` holds the mirror to
twig across the gestures that fold steps. What remains is provui's, after a
leaf release carrying it: `DocumentSession` drops its "walk on when the body
did not move" special case.

**Where.** `crates/leaf-core/src/doc.rs`, `undo_steps` / `redo_steps`, and
`Doc::undo` / `Doc::redo` beside them.

**What.** `Doc` keeps `undo_steps` as a counter it increments once per edit,
while the history it fronts is twig's, and twig coalesces a run of edits
into one step. So `can_undo()` answers `true` after every keystroke has
already been undone: the counter says three, the editor has one, and the
only way a host learns the truth is to call `undo()` and notice that
`revision()` did not move. `undo()` itself returns nothing, so a host cannot
ask it either.

provui hit this composing leaf with flower into one undo across both
editors (`c1122e6` on its `flower-next` branch): the session dispatches
each undo to the editor whose turn it is, and for the body it has to treat
"nothing moved" as "that run is exhausted, walk on" rather than as a
refusal, because the counter cannot be trusted and the call cannot say.
flower's `Model::undo` returns `bool` for exactly this reason.

**Shape.** Either ask twig — `Editor::undo` already returns
`Option<Change>`, so `can_undo` can be the editor's answer rather than a
shadow count — or make `Doc::undo() -> bool` (moved or not) and have the
counters follow what twig did. Both, ideally: a truthful predicate and a
truthful verb.

**Done when** a document that has had five characters typed and then
`undo()` called until `revision()` stops moving reports `can_undo() ==
false`, `undo()` reports whether it moved, and provui's `DocumentSession`
can drop its "walk on when the body did not move" special case.
