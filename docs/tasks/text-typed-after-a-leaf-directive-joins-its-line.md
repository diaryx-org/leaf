---
status: open
created: 2026-09-30
updated: 2026-09-30
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Text typed after a leaf directive joins the directive's line

**Where.** `crates/leaf-core`: text insertion at a caret that sits at the end
of a block leaf directive's placeholder row (`::name[label]{attrs}`, drawn as
`⧉ label`).

**What.** The placeholder row takes a caret at its end. Typing there inserts
the text into the directive's own source line, so

```
::page[Page 1 of 2]{card="id:58wnxd5"}
```

becomes

```
::page[Page 1 of 2]{card="id:58wnxd5"}TWO
```

which Markdown no longer reads as a directive: the mark turns into a
paragraph of raw source, and the typed text sits inside it. Pressing Return
first does the right thing — it opens a paragraph under the directive and the
text goes there — so the trap is only for a person who types straight after
the mark, which is what anyone does when the mark is a heading-like label
over a place to write.

Two marks in a row with nothing between them make it the only way in: there
is no paragraph between them to click into, so the end of the first mark's
row is where the caret lands.

**Seen.** LeafUI on iOS at leaf 0.4.11, in a document whose body is
`::page[Page 1 of 2]{…}\n\n::page[Page 2 of 2]{…}\n`: tap the end of the
first mark's row and type.

**Done when.** Text typed at the end of a block leaf directive's row opens a
paragraph under it, as Return does, and the directive's source line is never
extended. A test in `doc.rs` inserts at that caret and checks the directive
still parses.
