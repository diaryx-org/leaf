---
status: open
created: 2026-09-22
updated: 2026-09-22
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Replace All is one undo step

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
