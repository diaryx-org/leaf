---
status: open
created: 2026-09-04
updated: 2026-09-04
---
# An empty last line inside a code fence gets no row

**Where.** `crates/leaf-core`, the visual map's layout of a code block.

**Repro.** Open

    prose

    ```
    alpha
    beta
    ```

    after

in the WYSIWYG view, put the caret at the end of `beta`, and press Return. The
source becomes `beta\n\n` inside the fence — the block now ends with an empty
line, which CommonMark keeps as content — but the map lays out the same rows
as before: no row for the empty line, and the caret lands on `after`, outside
the block. Typing then lands in the paragraph below. A second Return does give
the block a visible empty line, so only the last one is dropped. Found by a
`packages/leaf-web` test that meant to grow a code block by a line; the test
was rewritten to shrink one instead.

**Done when.** The empty last line of a fenced block is a row of its own, the
caret lands on it after the Return that made it, and a core test pins the row
count.
