---
status: done
created: 2026-09-04
updated: 2026-09-09
---
# An empty last line inside a code fence gets no row

**Status: done.** The `"code_block"` arm cut the block's terminator with
`trim_end_matches('\n')`, which took a trailing empty line's newline along with
it. It strips exactly one newline now — the same one twig's `content_span`
omits, so `code_line_offsets` still lines the rows up with the source. Pinned by
`an_empty_last_line_in_a_code_block_is_a_row_of_its_own` in `wysiwyg`,
`wysiwyg_return_on_the_last_code_line_keeps_the_caret_in_the_block` in `doc`,
and the leaf-web test that found it, restored as *a code block's edges follow
the block as it grows*.

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
