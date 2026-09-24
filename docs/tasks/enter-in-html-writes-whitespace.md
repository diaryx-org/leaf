---
status: open
created: 2026-09-23
updated: 2026-09-23
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Enter in an HTML document writes whitespace

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
in `doc.rs` runs its HTML cases, which were left out of it for this task.
