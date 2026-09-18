---
title: The rule and table buttons at a paragraph's end split nothing and leave blank lines behind
status: open
created: 2026-09-18
updated: 2026-09-18
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# The rule and table buttons at a paragraph's end split nothing and leave blank lines behind

**Where.** `crates/leaf-core`, `Doc::insert_thematic_break` and
`Doc::insert_table`. Every frontend inherits it.

**What.** Both gestures part a bare paragraph around the caret with twig's
`split_block` first, so the block lands *at* the caret, and then aim the
block at the first half. With the caret at the paragraph's end there is
nothing to part, but the split still writes its separator — a blank line
and the empty slot the next paragraph would fill — and `insert_thematic_break`
then lands the rule above that slot, which nothing ever fills. The document
ends in two blank lines, or carries three between the rule and the next
block.

Repro, through core alone:

```rust
let mut d = Doc::from_source("para\n".into(), Format::Djot).unwrap();
d.caret = 4;
d.insert_thematic_break();   // "para\n\n* * *\n\n\n"  — wanted "para\n\n* * *\n"
d.insert_table(1, 1);        // likewise, two blank lines under the grid

let mut d = Doc::from_source("para\n\nnext\n".into(), Format::Djot).unwrap();
d.caret = 4;
d.insert_thematic_break();   // "para\n\n* * *\n\n\n\nnext\n" — three blank lines
```

Markdown reaches the same place by a narrower door. Its paragraph span stops
before the newline, so `caret_in_bare_paragraph` — half-open `ancestors_at` —
already says "not in the paragraph" at `"para\n"` with the caret at 4, and no
split happens. But a document being typed has no newline after its last line
yet, and there the caret at the end is the source's end, which is inside
everything: `"para"` at 4 splits, and gives `"para\n\n---\n\n"`. That is the
commonest caret of all — the end of what was just typed — so Markdown is
affected in practice as much as djot.

**Why not twig.** This is twig's
[block-inserted-after-a-split-at-paragraph-end-leaves-two-blank-lines](https://github.com/diaryx-org/twig/blob/main/docs/tasks/block-inserted-after-a-split-at-paragraph-end-leaves-two-blank-lines.md),
closed there as `dropped` in favour of this one. The alternative was for
twig's `insertBlockAfter` to fold a run of blank lines under the block to
one, which edits spacing an author may have written and the gesture was not
asked to touch. `split_block` at a paragraph's end is a documented shape —
Enter, then type — and `insertBlockAfter` already answers "after the block"
correctly on its own; it is only the composition here that is wrong.

**How.** Do not split when the caret sits at the paragraph's end: when what
remains of the paragraph after the caret is whitespace only, skip
`split_block` and let `insert_thematic_break` / `insert_table` place the block
after the paragraph, which is where it was going anyway. Test with both
formats and both trailing-newline shapes, `"para\n"` and `"para"`.

**Needs twig past 3.5.0.** Until twig `a08bcf3` (`fix(editor): a block
inserted after an unterminated last line is still blank-separated`),
`insert_thematic_break` on `"para"` at 4 with no split gave `"para\n---\n"` —
a setext heading that eats the paragraph — and `insert_table` a grid whose
header GFM reads out of `para`. The no-op split has been masking that by
terminating the line first. Bump the `twig-doc` pin to the release that
carries the fix before removing the split, and say so in the commit.

**Related, not this task.** The caret at the paragraph's *start* has the
mirror shape — `"para\n"` at 0 gives `"\n\n---\n\npara\n"`, a rule before the
paragraph with two blank lines above it — but not splitting there would move
the rule to *after* the paragraph, which is the wrong side. That wants a
placement twig does not offer yet (a block before the caret's block, or a
line end that stops at the newline rather than past it), and is a separate
decision.
