---
status: done
created: 2026-09-23
updated: 2026-10-21
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Enter in an HTML document writes whitespace

**Status: done.** In an HTML document's prose, `Doc::newline` writes
elements (`html_newline`). At a paragraph's or heading's end, or in an
empty paragraph, it writes an empty `<p></p>` after it. At a block's start,
or on the whitespace between two blocks, the `<p></p>` goes there instead.
Inside a paragraph, Preserve flow writes a `<br>` and Fold parts the element
at the caret, closing and reopening the inline elements around it. A
heading's second half is a paragraph. Preserve flow opens a paragraph at a
paragraph's end too, because a `<br>` with nothing after it in its block
draws no line in a browser. The map draws an empty `<p></p>` as an empty
paragraph row. Backspace in one, or at the start of a block under one,
takes it whole, the inverse of the Enter that opened it: twig's join had
left a newline in the element. Lists, quotes and tables keep their own
branches.
`enter_and_backspace_anywhere_show_what_they_write_and_undo_in_one_step`
runs two HTML documents, and
`enter_in_html_writes_elements_and_backspace_takes_them_back` pins the
rest. What is still open: Backspace between two paragraphs of text is
twig's join, which spells it as a newline, a space to HTML, drawn as a line
break in Preserve flow. Hand-written whitespace between or inside elements
is still drawn as Markdown would draw it.

**Where.** `crates/leaf-core`: `Doc::newline` and the visual map.

**What.** In an HTML document, Enter writes what it writes in Markdown:
blank lines, or in Preserve flow a lone newline. HTML keeps neither.
Between `<p>` elements a blank line is insignificant whitespace, and inside
one a newline is a space. The two flows then disagree about what is on the
page:

- **Fold.** The map counts blank source lines the way it does for Markdown,
  so it draws an empty row that the saved file does not have. Enter at the
  end of `<p>I cry</p>` gives `<p>I cry\n\n</p>`, drawn as an empty
  paragraph. Reopened in a browser, or anywhere else, that paragraph is
  gone.
- **Preserve.** Enter writes `\n`, which neither the map nor HTML draws.
  The key looks dead, and each press leaves another invisible newline in
  the file.

**Repro.** `Doc::from_source("<p>I cry</p>\n<p>knees</p>\n", Format::Html)`,
WYSIWYG view, caret after `cry`, `newline()`. In Preserve flow the drawn
rows are unchanged and the source has grown by one newline.

**Done when.** Enter in HTML writes elements: `<p></p>` for an empty
paragraph and `<br>` for a soft line. The map draws only what those
elements draw. `enter_and_backspace_anywhere_show_what_they_write_and_undo_in_one_step`
in `doc/tests/enter.rs` runs its HTML cases, which were left out of it for this task.
