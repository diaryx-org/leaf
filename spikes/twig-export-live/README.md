# Spike: twig-export, fast enough to be a live view?

Can twig-export and Typst set a page again after every keystroke? And can a
glyph that Typst laid out be traced back to the source it came from? A view
that shows twig-export's own pages, with a caret, needs both. See
[the proposal](../../docs/proposals/what-you-see-is-what-twig-export-sets.md).

Two binaries answer it two ways:

- `twig-export-live` sets the document through twig-export's generated
  `.typ`, as twig-export does today.
- `content` builds Typst content in Rust from the twig tree, so the words
  never become Typst markup, and evaluates [`template.typ`](template.typ),
  which holds only the procedural part: a contents page, running heads,
  page numbers, heading numbering. Footnotes go to the foot by Typst's own
  rule.

This directory is its own Cargo workspace, so leaf itself does not depend on
it. It takes twig-export from its `main` branch.

## Run it

```sh
cargo run --release --bin twig-export-live -- 40   # 40 sections; 5 is a short entry, 200 a book
cargo run --release --bin content -- 40
PDF=out.pdf cargo run --release --bin content -- 5  # and write the pages
PASSES=1 cargo run --release --bin content -- 5     # and print the layout passes
```

It makes a Markdown document of `N` sections (a heading, six paragraphs of
90 words, a list, and a footnote each) and sets it with `report.toml`, a
copy of twig-export's example config (A4, 11pt). Then it:

1. emits Typst through `twig_export::render` and compiles it once, cold;
2. types a seven-letter word into a paragraph at the middle of the
   document, one letter at a time, and at each letter parses and emits the
   whole document again and recompiles it in the same `World` (so Typst's
   memoized layout is reused); then does the same near the end;
3. walks every page's frames and counts the glyphs whose `Span` resolves to
   a range of the generated `.typ`.

`content` does the same, with `template.typ` in place of the config, and in
step 3 checks that each glyph's span leads to the very Markdown byte the
glyph was set from.

## What `twig-export-live` found (2026-10-26, Apple M3 Pro, release build)

| Document | Pages | Cold compile | Per keystroke: twig + emit | Per keystroke: Typst |
| --- | --- | --- | --- | --- |
| 5 sections, 2,800 words | 5 | 8–11 ms | 0.4 ms | 1.0 ms |
| 40 sections, 22,000 words | 35 | 51–58 ms | 3.2–4.1 ms | 8–11 ms |
| 200 sections, 112,000 words | 172 | 255–388 ms | 17–18 ms | 45–51 ms |

- Per keystroke, both halves grow with the length of the document. The
  emit is a full parse and a full emit; Typst's memoization saves layout but
  still walks the whole document. A journal entry is far inside a frame (one
  60 Hz frame is 16.7 ms). A long book is three frames on this machine.
- 99.7% of glyphs trace to the generated `.typ` (602,591 of 604,383 at 200
  sections). The rest are glyphs Typst makes itself: list bullets and
  footnote numbers.
- The release binary, stripped, is 46 MB with typst-kit's embedded fonts.
  That is the whole spike, not a delta over leaf, and no part of it is
  measured on a phone.

Not measured: a phone, a document with pictures, tables or math, and the
cost of drawing the frames.

## What `content` found (2026-10-26, same machine)

Three runs each; Typst's share of a keystroke (twig's parse and the content
build are separate: 0.4 ms, 3 ms and 18 ms, a full parse each time).

| Document | Pages | Cold compile | Per keystroke: Typst |
| --- | --- | --- | --- |
| 5 sections | 6 | 9–39 ms | 1.9–3.9 ms |
| 40 sections | 41 | 31–41 ms | 10–16 ms |
| 200 sections | 205 | 160–190 ms | 40–48 ms |

- It sets more than `twig-export-live` (a contents page, a running head on
  every page, three layout passes for the introspection to settle) in about
  the same time.
- A glyph's span names the Markdown byte: 606,760 of 607,452 glyphs at 200
  sections, with no span map. 48 glyphs come from the template, and 21,303
  are made by Typst (bullets, numbers, the contents page's dots).
- Three things threw Typst's cache away, and are worked around here:
  1. The body defined in the `Library`'s global scope: every keystroke was a
     cold compile (160–170 ms at 200 sections), since every memoized layout
     depends on the library's hash. The body is now an argument to the
     template's `book`, through a copy of `typst::compile`'s loop.
  2. Spans as absolute byte offsets: typing moves every later block's span,
     and a span is part of an element's hash, so the rest of the document
     was laid out again (453 ms at the middle of a book, 81 ms at the end).
     Spans are now a top-level block's index and an offset into it.
  3. A running head that filtered *every* heading by page: 250 ms a
     keystroke at 200 sections. Asking for the next heading and the last one
     before brought it back to 40–48 ms.
- `Span::from_range` is crate-private in typst-syntax 0.15, so `span()`
  packs the raw-range span by hand, following `span.rs`.

Not covered: tables, code, pictures, math, directives, and the resolved
styles from a twig-export config; the content builder handles headings,
paragraphs, emphasis, strong, bullet lists and footnotes only.
