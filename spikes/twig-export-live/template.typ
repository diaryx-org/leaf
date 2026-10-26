// The procedural half of the page, and nothing else: what no one writes by
// hand and every book needs. The words come in as `body`, content built from
// the twig tree in Rust; this file never sees Markdown or Typst markup for
// them. `src/bin/content.rs` calls `book` with it.
#let book(body) = {
  set page(
    paper: "a4",
    margin: 2.5cm,
    numbering: "1",
    // The section that starts on this page, else the one still running.
    // Asked as "the next heading" and "the last one before", not as "every
    // heading": a header that reads every heading's page is laid out again
    // after any edit, which cost a book 250 ms a keystroke instead of 40.
    header: context {
      let next = query(heading.where(level: 1).after(here()))
      let h = if next.len() > 0 and next.first().location().page() == here().page() {
        next.first()
      } else {
        let before = query(heading.where(level: 1).before(here()))
        if before.len() > 0 { before.last() }
      }
      if h != none { align(right, emph(h.body)) }
    },
  )
  set text(size: 11pt)
  set heading(numbering: "1.")
  show heading.where(level: 1): it => { pagebreak(weak: true); it }

  outline()
  pagebreak()
  body
}
