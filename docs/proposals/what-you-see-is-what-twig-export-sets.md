---
title: What you see is what twig-export sets
description: An exact match between leaf's page and twig-export's PDF needs one layout engine, so leaf's page view is drawn from Typst's layout rather than made to imitate it — a preview first, a caret over it second — with the words handed to Typst as content built from the twig tree, so Typst's language is left only the procedural part of a page, and the flowing editor stays leaf's own and promises the words, not the page
status: draft
created: 2026-10-26
updated: 2026-10-26
part_of: '[Proposals](/docs/proposals/proposals.md)'
---

# What you see is what twig-export sets

## Status

`draft`, argued 2026-10-26 from leaf `main` at `67555a78` and twig-export
`main` at `bf145b7`, which is on twig 6 and fig 7. The numbers are from the
spike in [`spikes/twig-export-live`](/spikes/twig-export-live/README.md), which
sets one document two ways: through twig-export's generated `.typ`, and as
Typst content built in Rust with a template for the procedural part only.
Nothing else is built. The proposal belongs in leaf because the view that changes is
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
| **C. leaf draws twig-export's pages** | Everything, by construction. | Typst in the app, and a page view that draws Typst frames. A read-only preview first, then a caret over it. |

The recommendation is **A now, then C in two steps**, with the words reaching
Typst as content built from the twig tree rather than as generated Typst
markup (**Typst for the procedural part only**, below). B is ruled out below.

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
- **Fast enough.** The spike sets a document again after each keystroke
  through twig-export's `.typ`, in one `World`, on an M3 Pro (the content
  route below is as fast, with a contents page and running heads added):

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
  at the word clicked. Through `.typ` that needs a span map in twig-export;
  through content built from the twig tree it needs nothing (below).
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
  Through `.typ`, the spike traced 99.7% of glyphs to the generated source,
  and twig-export would then need a **span map** from a `.typ` range back
  to the twig source. Through content built from the twig tree, a glyph's
  span names the Markdown byte itself (99.9% exactly; below). Either way,
  the rest are glyphs Typst makes itself (bullets, footnote numbers), which
  a caret skips.
- **What is harder than C1**: a caret in a place with no glyph (an empty
  paragraph, the end of a line), selection rectangles across a column or a
  page break, typing latency on a book (the 62 ms above, every key), and
  everything twig-export does not draw yet (math, highlights) — each of
  which C2 can only edit once Typst draws it.
- **What it replaces**: leaf's own paginated layout (`PageSetup`, the page
  stacking in `EditorLayout`). A document with `layout: a4` opens in the
  Typst page view; `layout: continuous` opens in the flowing editor.

C2 is real work, and it is worth deciding on after C1 has been used.

## Typst for the procedural part only

Adam asked whether Typst could be stripped down until it is only the
instructions for what is *generated* on a page, with the rest of the source
staying Markdown and directives. It can, and it is the better shape for C.

### What is procedural

The words, their marks, their headings and lists are the author's, and the
twig tree already holds them. What no author writes, and every book needs,
is the procedural part: the contents page, the running heads, the page
numbers, the numbering of headings and footnotes, a footnote's place at the
foot of its page, the title page. In Typst each of these is a short piece of
code that queries the laid-out document (`outline()`, `counter(page)`,
`query(heading)` in a header). Under this split they are the *only* Typst
code there is: a template, a file the author can read and replace, and
which the Diaryx proposal already wants a book to be able to name.

The spike's whole template is twenty lines
([`template.typ`](/spikes/twig-export-live/template.typ)): one function,
`book(body)`, that sets the paper, the size, the heading numbering and a
header, puts `outline()` and a page break first, and then `body`.

### How the words reach Typst without Typst markup

Typst's language is a front end. It parses `.typ` into a tree of
*content* — elements like `HeadingElem`, `TextElem`, `FootnoteElem` — and
everything after that (show and set rules, layout, introspection) works on
the content tree, not the text. Those elements are public Rust types. So the
body can be built in Rust, directly from the twig tree, and the Typst parser
never sees it:

```text
   Markdown ──twig──▶ tree ──[content.rs]──▶ Typst content ─┐
                                                            ▼
   template.typ ──Typst eval──▶ book(body) ─────────▶ layout ──▶ pages / PDF
```

The spike does this in `src/bin/content.rs`: about a hundred lines turn
headings, paragraphs, emphasis, strong, lists and footnotes into elements,
and a copy of `typst::compile`'s driver evaluates the template and calls
`book` with the body. Its pages have a contents page, running heads, page
numbers and footnotes at the foot.

### What it gains

- **No generated source, and no escaping.** twig-export's `escape()`, its
  `#raw(...)` workaround for column-sensitive fences, and the class of bug
  where a document's text is read as Typst markup are gone: a string goes
  into a `TextElem` as a string.
- **A glyph names its Markdown byte.** Each element carries a span. Built
  from the twig tree, the span is the twig node's own place in the source,
  so a glyph leads straight back to the Markdown: in a 205-page book, 606,760
  of 607,452 glyphs landed on the exact byte, with no span map. That is the
  whole of C1's click to source and most of C2's caret.
- **The template is the only Typst.** It runs in Typst's sandbox, it is
  small enough to read, and changing it changes every document the same way.
- **As fast.** Per keystroke, on an M3 Pro, Typst's share (the twig parse
  and content build are separate, and leaf already has the tree):

  | Document | Pages | Through `.typ` (no contents, no heads) | As content, with the template |
  | --- | --- | --- | --- |
  | 2,800 words | 5 / 6 | 1.0–1.4 ms | 1.9–3.9 ms |
  | 22,000 words | 35 / 41 | 8–11 ms | 10–16 ms |
  | 112,000 words | 172 / 205 | 45–51 ms | 40–48 ms |

  The content route lays out more (a contents page, a header on every page,
  and three layout passes for the introspection to settle) in about the
  same time.

### What it costs

- **Typst's Rust API, not its language, becomes the contract.** Elements and
  the compile driver are public but not stable; each Typst 0.x release can
  move them. The spike also packs a span by hand, because
  `Span::from_range` is crate-private in typst-syntax 0.15, and copies
  `compile_impl`'s layout loop to call the template with the body. Two
  requests to Typst would remove both: a public range-span constructor, and
  a compile entry point that takes content beside the main file. Until
  then, Typst is pinned and bumped on purpose, as twig-export already does.
- **Three ways to throw away Typst's cache**, each found by the spike:
  1. Handing the body in through the `Library` (as a global named `body`)
     made every keystroke a cold compile — 160 ms in a book — because every
     memoized layout depends on the library's hash. The body has to be an
     argument to the template, not part of the library.
  2. A span that is an absolute byte offset changes for every block after
     the caret when one letter is typed, and a span is part of an element's
     hash, so Typst laid out the rest of the document again: 453 ms in the
     middle of a book, 81 ms at its end. A span that is *a block and an
     offset into it* stays the same for every block the edit did not touch.
     The caller keeps where each block starts now. leaf's own block keys
     would serve, if they outlive an edit.
  3. A running head that reads the page of every heading is laid out again
     after any edit: 250 ms a keystroke in a book, against 45 ms for one
     that asks for the next heading and the last one before. A template's
     procedural part has a cost of its own, and the template Diaryx ships
     is written for it.
- **`.typ` stops being the artifact everything is tested through.**
  twig-export's `DESIGN.md` makes the emitted `.typ` the real output because
  a golden `.typ` is a readable diff. Under this split the content builder
  is the real output and `.typ` is an export format (the Diaryx proposal's
  "Typst source", for a person taking a book to a typesetter by hand). Both
  are walks of one tree, and one test can hold them together: the same
  document, set both ways, must give the same pages.
- **The config's raw-Typst escape hatch** (`[rule.raw]`: `before`, `after`,
  `wrap`) splices markup around a node, which content cannot do. Its uses
  move into the template, as show rules, or are evaluated as small Typst
  snippets once and applied as styles.

### What it does not strip

Typst's *engine* stays whole: line breaking, hyphenation, pagination,
footnote placement and introspection are what make the page exact, and what
leaf does not have. What is stripped is Typst's *language* from the body.
A route that kept the source Markdown but parsed it inside Typst (a Typst
package that reads Markdown, as `cmarker` does through a wasm plugin) was
not taken: its spans point at the plugin call, not at the Markdown, so it
loses click to source, and it parses in wasm on every compile.

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
3. **A content backend**: a sibling of `typst_emit.rs` that builds Typst
   content from the tree, with block-relative spans, the resolved styles
   applied as Typst styles, and a template that holds the procedural part.
   `typst_emit.rs` stays, for the `.typ` export, and a test sets the corpus
   both ways and compares the pages. With it, no span map is needed.
4. **A `World` and a driver the caller keeps**, so that a second compile
   reuses the first's layout, and an entry point that takes a parsed
   document, as the Diaryx proposal also asks for. The spike had to write
   both, because `ExportWorld` is private.
5. **Two requests upstream, to Typst**: a public range-span constructor,
   and a compile entry point that takes content beside the main file.
6. **No network, bundled faces**: the `network` feature off, and the
   generic families as files in the app.

## Order of work

1. **A**: the shared table of numbers in leaf-core; `columns` in
   twig-export; the comparison test.
2. twig-export's items 1, 3 and 4, and the requests to Typst (item 5).
3. **C1** in the Rust Mac app: a page preview beside the editor, click to
   source, and Export as PDF through twig-export.
4. C1 on iOS and Android, after the binary size is measured there.
5. **C2**, if C1 earns it.

## To settle

1. **Typst's Rust API as a contract.** The content route trades a stable
   text format for an unstable Rust API. If Typst declines both requests,
   twig-export carries a copy of the compile loop and a hand-packed span
   across each Typst release; whether that is acceptable decides between
   the content route and `.typ` plus a span map.

2. **Binary size.** The spike's binary is 46 MB stripped, with typst-kit's
   embedded fonts. The delta over leaf is unmeasured, and so is a phone. The
   Diaryx proposal has the same open question; one measurement answers both.
3. **Math.** Typst's math in the page view and RaTeX's in the flowing view
   are two pictures of one formula. Either the page uses leaf-math's SVG
   (exact with the editor, but no Typst math features), or the flowing view
   accepts that the page's formula looks different.
4. **Where the numbers live.** A's table could live in leaf-core (leaf's
   numbers, which the export follows) or in Diaryx's template (the
   template's numbers, which leaf follows). leaf-core is proposed because
   leaf is used without Diaryx; the template then reads leaf's numbers.
5. **The page view on the web.** Typst in wasm is large; a web page view
   may be the server's job, or not exist.
