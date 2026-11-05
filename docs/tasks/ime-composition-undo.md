---
status: done
created: 2026-09-25
updated: 2026-11-04
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# An IME composition undoes a keystroke at a time

**Status.** Done, in `fix(core): a composition writes what typing writes`.
`Doc::insert_composing` is the composing form of `insert`: each step puts the
document back as the composition found it, caret and armed marks included, and
types the text there, folded into the composition's one undo step. `Composer`
in `crates/leaf-dioxus/src/ime.rs` composes through it, and that is the one
path every Dioxus host takes, so an armed Bold takes the word and a `*` stays
literal. Not run on a device: the Done when below names Android and iOS, whose
Dioxus apps have no platform module yet and whose input is the task "What
Android does not do yet" and "What iOS does not do yet". `leaf-ffi` and
`leaf-wasm` are archived, so there is no binding to expose it through.

**Since the archive (2026-10-26).** The paths below to `packages/` are under
`archive/` now. The leaf-dioxus editor closes the undo half (`ime.rs`, through
`edit_composing` and `end_composition`). The other half is still open there: a
composition is a raw splice, so an armed Bold and a literal `*` are lost, and
fixing that needs a composing insert in core.

**What.** A word typed through an IME that composes — Gboard composes nearly
every word on Android — takes one Undo per letter to take back, stepping
through `hello worl`, `hello wor`, … on the way. Core has the fix,
`Doc::edit_composing` and `Doc::end_composition`, which fold a composition's
provisional steps into one; neither binding exposes them, and no frontend
calls them.

**Why it is not a one-line swap.** `edit_composing` is a splice, as `edit` is.
The Android editor composes through `insert` instead, because `insert` is what
makes composed text *typed* text: an armed Bold takes it, and in
`MarkupMode::None` a typed `*` stays literal. Moving composition to a raw
splice would fix Undo and lose both. iOS composes through `replace_range` and
loses both already, as well as having the undo granularity.

**How.** A composing form of `insert` in core — `insert`'s semantics, landing
as a step of the open composition — exposed in `leaf-ffi` and `leaf-wasm`
alike (the bindings' parity test holds them level), with `end_composition`
beside it; then `LeafInputConnection.write` (Android), `setMarkedText`
(`LeafTextViewiOS.swift`) and the web editor's `compositionupdate` use it.

**Done when** composing a word and pressing Undo once takes the whole word
back, on Android and iOS, with an armed Bold still applying to it.
