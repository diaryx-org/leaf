---
status: open
created: 2026-09-18
updated: 2026-09-18
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# A page break in HTML and AsciiDoc is a row the walker does not draw

**Where.** `crates/leaf-core`, the visual map's directive arms
(`block` in `wysiwyg.rs`) and `Capabilities::page_break` in `doc.rs`.

**What.** `Doc::insert_page_break` writes twig's `Gesture::InsertDirective`,
and twig spells that gesture in four formats, not two. Markdown's
`::page-break` is a `Leaf`-form directive named `page-break`, and djot's is an
empty anonymous `::: page-break` fence; the walker has an arm for each and
both reach a frontend as one placeholder row carrying a `page-break`
`DirectiveMark`. The other two spell it differently and the walker reads
neither:

- **HTML** writes `<page-break></page-break>` — a container named
  `page-break` with `Element` origin and *no* `directive_form` at all. It
  fails `container_is_directive`, has no children, and so draws **nothing**:
  no row, no caret home, no mark. The break is in the file and invisible in
  the editor.
- **AsciiDoc** writes `<<<` — `Directive` origin with no form either, so it
  falls to the general container arm, which walks its (empty) children and
  leaves an empty unlabelled row behind rather than the `⧉ page-break`
  placeholder.

Because of that, `Capabilities::page_break` is gated on the format being
Markdown or djot rather than on the gesture alone, which is what
`docs/proposals/presentation-vocabulary.md` claims and all the toolbars offer.
The gate is the honest answer while the walker cannot draw the row, and it is
the line to delete when it can.

**Repro.** Open `<p>hello</p>\n` as HTML in the WYSIWYG view, call
`Doc::insert_page_break` (it refuses today — take the capability gate off
first), and count the rows: the source gains
`<page-break></page-break>` and the map gains nothing, so the caret walks
straight over bytes that hold no row. In AsciiDoc the same call gives a blank
row with no label, which reads as an empty paragraph and paginates as
nothing.

**Done when.** A page break drawn from any of the four formats is one
placeholder row carrying a `page-break` `DirectiveMark` — a frontend that
paginates cannot tell which format the file is in, which is the rule the
Markdown and djot arms already keep. Then `page_break` is
`supports(Gesture::InsertDirective)` again with no `matches!` beside it, the
capabilities test drops its HTML and AsciiDoc assertions, and
`a_page_break_reads_the_same_in_markdown_and_in_djot` in `wysiwyg` grows the
other two formats.
