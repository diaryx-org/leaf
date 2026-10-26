---
title: What you see is what twig-export sets
description: An exact match between leaf's page and twig-export's PDF needs one layout engine, so leaf's page view is drawn from twig-export's Typst output rather than made to imitate it — a preview first, a caret over it second — while the flowing editor stays leaf's own and promises the words, not the page
status: draft
created: 2026-10-26
updated: 2026-10-26
part_of: '[Proposals](/docs/proposals/proposals.md)'
---

# What you see is what twig-export sets

## Status

`draft`, argued 2026-10-26 from leaf `main` at `67555a78` and twig-export
`main` at `bf145b7`, which is on twig 6 and fig 7. The numbers are from the
spike in [`spikes/twig-export-live`](/spikes/twig-export-live/README.md). Nothing
else is built. The proposal belongs in leaf because the view that changes is
leaf's. What it needs from twig-export is listed under
**What twig-export needs**, for that repo to take as tasks.

## The ask

Adam asked to look at "possibly making the live view reflect the output of
twig-export exactly". The context is Diaryx's draft proposal
[A document leaves through one typesetter](https://github.com/diaryx-org/diaryx/blob/main/docs/proposals/a-document-leaves-through-one-typesetter.md),
which makes twig-export the route for every format that is set on pages,
and which says that leaf's PDF and twig-export's "could disagree". That
proposal keeps leaf's PDF as "this page, as I see it" and asks only for a
test that keeps the two honest on paper size, columns and body size. This
proposal asks whether the two can agree *exactly*, and what that costs.

## The short answer

**Exact needs one layout engine.** Two engines can share every input — the
face, the size, the paper, the margins, the columns — and still break a line
at a different word, and so a page at a different line. So there are two
ways to get an exact match, and one way to get a close one:

| | What matches | What it costs |
| --- | --- | --- |
| **A. Shared settings** | The face, the sizes, the paper, the margins, the columns. Not where lines and pages break. | Small. A table of numbers both sides read, and a test. |
| **B. twig-export sets leaf's layout** (export what leaf draws) | Everything, by construction. | leaf has no one layout to export: each frontend lays out text with its own toolkit. And leaf's layout has no contents page, no footnotes at the foot, no running heads. |
| **C. leaf draws twig-export's pages** | Everything, by construction. | Typst in the app, a span map in twig-export, and a page view that draws Typst frames. A read-only preview first, then a caret over it. |

The recommendation is **A now, then C in two steps**. B is ruled out below.

## What exists

**leaf lays out text in each toolkit's own engine.** leaf-core decides what
each glyph is and how it is styled; it does not decide where a line breaks.
leaf-swift breaks lines with Core Text (`CTTypesetterSuggestLineBreak` in
`EditorLayout.swift`), greedy and with no hyphenation, and justifies by
kerning the spaces of a finished line. The Mac app in Rust
(`apps/leaf-dioxus`) breaks them with Blitz's text layout. leaf-android uses
Compose's, and leaf-web the browser's. A page break falls between rows,
never through one (`PageSetup.swift`, `apps/leaf-dioxus/src/pdf.rs`). So two
leaf frontends can already disagree about a page by a line or two; the PDF
from each is "what this frontend shows", which is a true statement for each
one separately.

**twig-export lays out with Typst.** It emits a `.typ` file from the twig
AST and a selector-to-style config, and compiles it in-process
(`twig-export/DESIGN.md`). Typst breaks a justified paragraph with
Knuth–Plass and hyphenation, a ragged one greedily; it measures line spacing
from the bottom of one line to the top of the next (`leading`), not with the
font's ascent and descent as Core Text does; it keeps widows and orphans
off a page; it puts footnotes at the foot of the page; and a block can split
across a page. Its page carries running heads, page numbers and a contents
page, which leaf's does not.

**They do not draw the same things yet.** twig-export emits no math,
highlight, strikethrough, superscript or subscript, does not read the
presentation CSS that leaf writes in a node's `style` attribute
(`text-align`, `line-height`, `font-size`, `font-family`, `color`), and does
not draw Mermaid. leaf draws math through RaTeX (`leaf-math`); Typst has a
math engine of its own, with different spacing and faces.

**The four setting words are the shared contract.** Diaryx's
`workspace/src/setting.rs` reads `font`, `size`, `layout` and `columns` from
a document's frontmatter, and says that "every renderer answers it in its
own idiom". leaf's `PageSetup` has the paper, the margins (72 points), the
column count and the gutter (24 points). twig-export's config has the paper,
the margin, the size and the leading, but no column count.

## Why not B: twig-export sets what leaf draws

B means: the export *is* leaf's layout, as `apps/leaf-dioxus/src/pdf.rs`
already does for the Mac (an offscreen editor, painted into a PDF). It is
exact for one frontend. It fails the Diaryx proposal's two requirements:

- A PDF must be byte-identical across the CLI, the Mac, iOS and Android.
  leaf's layout is four toolkits' layouts, and the CLI has none.
- A book needs a contents page, footnotes at the foot, running heads and
  page numbers. Those are a typesetter's, and building them into an editor's
  row layout is building a second Typst, badly.

So the page that leaves has to be Typst's. The question is only whether
leaf can show it.

## A. Shared settings: close, now

Make every number that both sides set come from one place, and test it.

- **The four words** compile to twig-export rules, as the Diaryx proposal
  already says. `columns` needs a document setting in twig-export
  (`#set page(columns: n)`); it has none today.
- **The numbers under the words**: the margin (leaf's 72 points), the
  gutter (24 points), the body leading, and the size and spacing of each
  heading level. leaf's themes hold these per frontend (`EditorTheme` in
  Swift, `Look` in Rust). They become one table in leaf-core, which every
  frontend's theme reads and which Diaryx's shipped twig-export template is
  generated from.
- **The faces**: the generic families map to the same bundled files on both
  sides (`diaryx_faces`), so that `serif` is one font everywhere.
- **A test**: one document, set by leaf (the Rust Mac app's offscreen layout,
  which needs no simulator) and by twig-export, compared on paper size,
  column count, body size, and the line count of each paragraph. The last is
  a tolerance, not an equality: it is the measure of how close A gets.

What A gives: the editor and the PDF look like one document — the same face,
the same scale, the same paper. What A cannot give: the same line breaks and
page breaks. A ragged paragraph in the same face at the same width breaks
greedily on both sides and will often match; a justified one will not, and
line spacing differs by the leading model, so the page breaks drift down a
long document.

## C. leaf draws twig-export's pages: exact

A **page view** in leaf that draws Typst's frames, not leaf's rows. The
flowing editor stays as it is.

### C1. A preview, read-only

A pane or a mode beside the editor that shows the document's pages as
twig-export sets them: the PDF, before it is a PDF. Each edit in the editor
re-emits and recompiles, and the pane redraws the pages that changed.

- **Exact by construction**: the preview and the PDF come from one compile,
  and Export writes the bytes of the document the preview shows.
- **Fast enough.** The spike sets a document again after each keystroke, in
  one `World`, on an M3 Pro:

  | Document | Pages | Per keystroke (parse + emit + Typst) |
  | --- | --- | --- |
  | A journal entry, 2,800 words | 5 | 1.4 ms |
  | A long chapter, 22,000 words | 35 | 11–15 ms |
  | A book, 112,000 words | 172 | 62–69 ms |

  An entry fits in one frame. A book does not, so the compile runs off the
  main thread and the pane shows the last pages it has; the editor never
  waits for it. Both halves grow with the length of the document, because
  twig-export parses and emits the whole document each time. Emitting only
  the blocks that changed is an optimisation for later, not a precondition.
- **Click to source.** A click on the preview puts the caret in the editor
  at the word clicked. That needs the span map (below), and nothing else.
- **Where it lands first**: the Rust Mac app (`apps/leaf-dioxus`), which can
  link twig-export directly and draw a page as an image (`typst-render`) or
  as vectors. leaf-swift and leaf-android reach it through `leaf-ffi` as page
  images; leaf-web through a wasm build, later.

### C2. A caret on the page

The same page view, editable: the caret, the selection and the IME drawn on
Typst's glyphs, with every edit still going through leaf-core's `Doc`. Only
the *view* changes; the document model, the commands and the undo stack do
not.

- **Hit-testing and drawing the caret** go through spans. Each glyph Typst
  lays out carries a `Span` and an offset into the source it was made from.
  The spike traced 99.7% of glyphs to the generated `.typ`; the rest are
  glyphs Typst makes itself (bullets, footnote numbers), which a caret skips.
  twig-export then needs to map a `.typ` range back to a twig source range
  — the **span map** — which it can record as it emits, since it already
  holds each node's span.
- **What is harder than C1**: a caret in a place with no glyph (an empty
  paragraph, the end of a line), selection rectangles across a column or a
  page break, typing latency on a book (the 62 ms above, every key), and
  everything twig-export does not draw yet (math, highlights) — each of
  which C2 can only edit once Typst draws it.
- **What it replaces**: leaf's own paginated layout (`PageSetup`, the page
  stacking in `EditorLayout`). A document with `layout: a4` opens in the
  Typst page view; `layout: continuous` opens in the flowing editor.

C2 is real work, and it is worth deciding on after C1 has been used.

## What each promise is

After A and C, leaf makes two promises, and each is true:

- **The flowing editor** promises the *words* and the *look*: the same face,
  sizes and colours as the page. It does not promise where a line breaks.
  It has no page, so there is no page for it to disagree with.
- **The page view** promises the *page*: it is the PDF, drawn.

This replaces "leaf's PDF stays, for what it is good at" in the Diaryx
proposal. Once C1 exists, leaf's own PDF (`pdfData`, `pdf.rs`) prints a page
nobody will see anywhere else, and Export as PDF goes through twig-export on
every platform. Print as shown can stay as a separate command, if anyone
asks for it.

## What twig-export needs

None of this is built; each is a task for twig-export.

1. **Draw what leaf draws**: math (Typst's own, or leaf-math's SVG as an
   image, for an exact match with leaf's flowing view), highlight,
   strikethrough, superscript, subscript, the presentation CSS in `style`
   (`text-align`, `line-height`, `font-size`, `font-family`, `color`),
   the page-break directive, and pictures resolved against the document's
   folder. Until it does, C1 shows less than the editor.
2. **A column count** as a document setting.
3. **A span map** from the emitted `.typ` back to the twig source.
4. **A `World` the caller keeps**, so that a second compile reuses the
   first's layout. The spike had to write its own, because `ExportWorld`
   is private. A library entry point that takes a parsed document, as the
   Diaryx proposal also asks for.
5. **No network, bundled faces**: the `network` feature off, and the
   generic families as files in the app.

## Order of work

1. **A**: the shared table of numbers in leaf-core; `columns` in
   twig-export; the comparison test.
2. twig-export's items 1, 3 and 4.
3. **C1** in the Rust Mac app: a page preview beside the editor, click to
   source, and Export as PDF through twig-export.
4. C1 on iOS and Android, after the binary size is measured there.
5. **C2**, if C1 earns it.

## To settle

1. **Binary size.** The spike's binary is 46 MB stripped, with typst-kit's
   embedded fonts. The delta over leaf is unmeasured, and so is a phone. The
   Diaryx proposal has the same open question; one measurement answers both.
2. **Math.** Typst's math in the page view and RaTeX's in the flowing view
   are two pictures of one formula. Either the page uses leaf-math's SVG
   (exact with the editor, but no Typst math features), or the flowing view
   accepts that the page's formula looks different.
3. **Where the numbers live.** A's table could live in leaf-core (leaf's
   numbers, which the export follows) or in Diaryx's template (the
   template's numbers, which leaf follows). leaf-core is proposed because
   leaf is used without Diaryx; the template then reads leaf's numbers.
4. **The page view on the web.** Typst in wasm is large; a web page view
   may be the server's job, or not exist.
