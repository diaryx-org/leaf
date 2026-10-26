# Spike: twig-export, fast enough to be a live view?

Can twig-export and Typst set a page again after every keystroke? And can a
glyph that Typst laid out be traced back to the source it came from? A view
that shows twig-export's own pages, with a caret, needs both. See
[the proposal](../../docs/proposals/what-you-see-is-what-twig-export-sets.md).

This directory is its own Cargo workspace, so leaf itself does not depend on
it. It takes twig-export from its `main` branch.

## Run it

```sh
cargo run --release -- 40    # 40 sections; 5 is a short entry, 200 a book
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

## What it found (2026-10-26, Apple M3 Pro, release build)

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
