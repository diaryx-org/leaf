---
status: open
created: 2026-09-23
updated: 2026-09-23
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Text glued under a closing `</div>` draws until the next full rebuild

**Where.** `crates/leaf-core`: `wysiwyg::build_spliced` and `build_cached`
against `build`, and whatever twig reports for the shape below.

**What.** In Markdown, `</div>` followed directly by a line of text, with no
blank line between them, opens an HTML block that runs to the next blank
line. The text becomes raw HTML. twig's top level then holds the div's
opening tag as an empty container, and the text as a bare `str` node:

```
<div data-line-height="1.5">\n\nknees\n\n</div>\nafter\n
top: Container 0..28, Para 30..35, Str 44..49
```

The incremental map and a fresh build disagree about that document:

- `build_cached` / `build_spliced` draw `knees`, two stacked gap rows, and
  `after`.
- `build` draws `knees` and nothing else.

So after an edit that reaches this state, what is on screen changes the
next time anything rebuilds the map from scratch: a width change, a
reopen, or a host that reloads the text after saving. The text below the
div disappears, or its gap changes, with no key pressed.

**Reached by.** An edit that deletes exactly the blank line under
`</div>`: a selection delete, a cut, a paste, or a host's `set_source`.
Enter and Backspace no longer reach it. Preserve flow's Backspace on that
line did, until the line stopped being drawn as a place to type (the same
commit as this file).

**Repro.** `incremental_build_matches_a_fresh_build_across_edits` in
`doc.rs`, with
`"<div data-line-height=\"1.5\">\n\nI cry\n\nknees\n\n\n\n</div>\n\nafter\n"`
added to its corpus, fails at step 21.

**Done when.** The builders agree on the shape, and the div document is
back in that corpus. What they should agree on is a separate question:
whether text swallowed into an HTML block draws as its source, like any
other raw block, or is hidden. Either way both paths must give the same
answer.
