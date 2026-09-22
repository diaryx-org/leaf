---
status: done
created: 2026-09-22
updated: 2026-09-22
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# Replace All is one undo step

**Status.** Done, in `feat: Replace All is one undo step, through an undo
group on Doc`. No twig change was needed: `Doc::begin_undo_group` and
`Doc::end_undo_group` fold each edit made between them into the group's
first step as it lands, through twig's `coalesce_last_undo`, so a group
holds one step on the history however many edits it makes and never meets
twig's 200-step cap. Groups nest, an undo or redo closes an open one, and
the `can_undo` mirror follows every fold
(`can_undo_is_exact_across_the_gestures_that_coalesce` has the group
gestures). leaf-ffi and the wasm binding expose the pair. The iOS
`replaceAll` wraps its run in one group, and the Mac opens one at the
finder's `shouldReplaceCharacters(inRanges:with:)` and closes it at
`didReplaceCharacters()`. leaf-web has no Replace All to use it.

**Where.** `crates/leaf-core` (`Doc`), `crates/leaf-ffi`, and both Swift
`LeafTextView`s.

**What.** Replace All in the find UI replaces every match through
`replaceRange`, one call per match: `replaceAll(queryString:options:withText:)`
on iOS, and on the Mac the find bar's own sequence of
`replaceCharacters(in:with:)`. Each call is its own undo step, so ⌘Z after
replacing twelve matches puts back one of them, and it takes twelve presses
to undo the whole thing. Every other editor undoes a Replace All at once.

**How.** Core coalesces edits only as a run of the same kind (typing,
deleting) through twig's `coalesce_last_undo`, and it offers a host no way
to group edits it makes one after another. That needs either a
`Doc::replace_ranges(ranges, text)` that splices every range and folds the
steps into one, or a begin/end grouping pair on `Doc`. Either way, leaf-ffi
and the wasm binding should expose it. The iOS `replaceAll` would then make
one call. The Mac would need `NSTextFinderClient`'s replace-all path to
batch, since the finder calls `replaceCharacters` once per match and
brackets the run with `shouldReplaceCharacters(inRanges:with:)` and
`didReplaceCharacters()`.

**Done when.** A Replace All on either platform, followed by one Undo,
restores every match, and one Redo replaces them all again.
