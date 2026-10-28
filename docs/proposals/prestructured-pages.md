---
title: 'Prestructured pages: Typst sets every view, from templates'
description: Every view of a document that shows it as it reads, the flowing one and the paged one alike, is laid out by Typst from one template, which is the document's kind (a note, a letter, a book) and comes in a reflowed and a paged layout; the styles a person chooses from are the template's few named ones, in Diaryx's four faces, at sizes relative to the reader's own, never a font or a size; the source view stays plain text, and a formula opens in a dialog until it can be edited on the page
status: draft
created: 2026-10-27
updated: 2026-10-27
part_of: '[Proposals](/docs/proposals/proposals.md)'
---

# Prestructured pages: Typst sets every view, from templates

## Status

`draft`, 2026-10-27, argued from leaf `main` at `f64c4c6`. It follows
[What you see is what twig-export sets](/docs/proposals/what-you-see-is-what-twig-export-sets.md),
whose C1 (a preview set by twig-export) and a first C2 (the page view, edited
on Typst's glyphs) are built, and takes the step that proposal left open:
whether the flowing editor stays leaf's own. Adam has decided three things
(**Decisions**); the rest is argued here and listed under **To settle**.

## The ask

Adam, 2026-10-27: can Typst set a reflowable view as well as pages, and if it
can, should leaf follow "prestructured WYSIWYG"
([John Gruber, 2026-10](https://daringfireball.net/2026/10/prestructured_wysiwyg)):
a default set of styles rather than fonts and sizes, in the faces Diaryx
already sets every page in, with templates for different kinds of document,
each in a reflowable and a paged version.

Gruber's name is for the model Apple's Notes and Messages follow. A person
applies named styles (title, heading, subheading, monospaced, lists, bold,
italic, links) and sees them as they will read; no typeface and no size can
be chosen. The sizes are relative, so they follow the reader's text size.
The styles "largely map well to Markdown", which never cared about fonts
either. What it buys: a document reads the same on every device, since no
face it names can be missing there; most people's typography is better for
having no font menu; and the reader's size setting reaches every style.
"Constraints are sometimes freeing."

## Decisions

Adam, 2026-10-27:

- **The reflowed Typst view replaces the flowing editor**, once it does what
  the flowing editor does (**What the flowing editor still does**). There is
  then one layout engine for every view that shows a document as it reads.
- **The source view stays a different thing**: the document as plain text,
  edited as text, in leaf's own rows, beside the Typst views and not one of
  them.
- **Math is edited in a dialog for now.** The Swift app edited a formula in
  place, as it reads (leaf-math-edit is that editor, without its frontend);
  that comes back on the page later. Until then a formula on the page opens a
  dialog with its TeX and a live picture of it.

## The short answer

**Yes, Typst sets a reflowable view.** A page as wide as the view and
`height: auto` is one page as tall as the document: Typst breaks its lines
and nothing else, with no running heads and no page numbers, and its
footnotes go at the end of it. The page view (`apps/leaf-dioxus/src/pages.rs`)
already draws pages of any size with the caret on them, so a reflowed view
is the same view on one tall page whose width follows the window.

**And it makes the plan whole.** A template is a kind of document. It holds
the few styles a person chooses from, sized relative to the body, in faces
named by word; and it lays the document out two ways, reflowed for writing
and reading on a screen, paged for paper and for a PDF. A person chooses the
kind, the face, and the layout, and never a font or a size. The editor, the
preview, the PDF and a published page then all come from one template, set
by one engine.

## What exists

**Three views, two engines.** The flowing editor is leaf's rows, laid out by
parley (`crates/leaf-dioxus/src/text.rs`) in whatever faces Settings names.
The page view and the preview beside the editor are twig-export's pages, set
by Typst with the words as content spanned to the source, the caret drawn on
Typst's glyphs and every key still the editor's. The source view is the rows
again, as text.

**A style menu that is already prestructured, and a Settings window that is
not.** The toolbar's Style ▾ offers Body, Heading 1–6, Quote, Code and Verse,
and its marks are bold, italic, underline, strikethrough, code, highlight and
links: named styles, each a Markdown construct. But Settings takes any face
installed by name, for the body and for code, at a size in pixels, and the
presentation vocabulary (`docs/proposals/presentation-vocabulary.md`) lets a
document carry `font-family`, `font-size` and `color` in a node's `style`.

**Diaryx's words for how a document is set.** `diaryx_workspace::setting`
reads four keys from a document's frontmatter, each a word every renderer
answers in its own idiom: `font` (`serif`, `sans-serif`, `monospace`,
`handwritten`, or a family by name), `size` (the body, in points), `layout`
(`continuous`, `letter`, `a4`) and `columns` (1 or 2). The paper *is* the
layout there: `a4` has said "pages" already.

**Diaryx's faces.** `diaryx_faces` holds one face for each word, as WOFF2:
Literata for `serif` and for no word at all, Atkinson Hyperlegible Next for
`sans-serif`, IBM Plex Mono for `monospace`, and Caveat for `handwritten`. All
four are under the SIL Open Font License. The crate is in Diaryx, which is
private.

**twig-export's config**, a document's settings (paper, margins, size,
leading, title page, contents) and rules from a twig selector to a neutral
style, emitted as Typst; and a document may name a Typst Universe template
through `raw` (the folklore book example). There is no template that is a
kind of document, and no reflowed layout.

## The proposal

### One engine for every view that reads as the document will

The reflowed view and the page view are both Typst's, from the same
template, and differ only in the template's `layout`:

| | Reflowed | Paged |
|---|---|---|
| Width | the view's, less a margin | the paper's, less its margins |
| Height | `auto`: one page | the paper's |
| Running heads, page numbers, contents | none | the template's |
| Footnotes | at the end | at the foot of their page |
| Breaks | lines | lines and pages |

Both draw the caret, the selection and a press as the page view does now.
Every edit is still leaf-core's, through the editor; the editor's moves by
line already go by the pages' lines (`leaf_dioxus::Lines`), and its pointer
handling by the byte a press lands on (`Target::Offset`).

**What it costs.** A keystroke sets the document again, as the page view
does: about 18 ms in a 30-page document, more in a book, off the main thread
and never in the editor's way. Reflowing to a new window width lays every
line out again, so a resize sets once the width has settled, not at every
step of a drag.

**What does not change.** The source view, which is the document as plain
text and stays leaf's rows. And the terminal editor, which paints into
character cells and cannot draw Typst's pages: it keeps the rows too, and
their look follows the template's styles as far as cells can.

### Prestructured styles, in the template

A template names the styles a document is written in, and nothing else is
offered:

- **Blocks:** Title, Heading 1–3, Body, Quote, Code, Verse, the lists and a
  task list. Six heading levels stay readable, as Markdown has them; the
  menu offers three, as Notes does, and the rest come from a document that
  has them.
- **Marks:** bold, italic, underline, strikethrough, code, highlight (in the
  template's few colours), links, superscript and subscript.
- **Sizes relative to the body**, in em, and the body's size the reader's:
  Settings' text size (and its zoom) scales every style at once, which is
  the accessibility half of Gruber's case.
- **Faces by word**, Diaryx's four. A document says `font: sans-serif`, never
  a family; the template sets each word in its face; and the faces are in the
  app, so a document reads the same wherever it opens.

The toolbar's Style ▾ and Format ▾ become the template's list, so a menu
cannot offer what the template does not style. Settings keeps the text size,
the appearance and the paper, and loses its font pickers.

### Templates as kinds of document

A template is one Typst file for a kind of document, a function of the body
and a few settings, the layout among them:

```typst
#let note(body, layout: "reflowed", width: auto, face: "serif", size: 12pt) = { … }
```

One file for both layouts, not two: everything but the page (the styles, the
faces, the sizes, the spacing) is shared, and two files would drift. The
paged branch adds the paper, the margins, the running heads, the page numbers
and a contents page where the kind has one.

The kinds to start with, each a template twig-export ships:

| Kind | Reflowed | Paged |
|---|---|---|
| Note (the default) | the body and its headings | the same, numbered pages |
| Journal entry | a dated title | a dated title, the entry's own heading |
| Letter | — | the sender, the date, the addressee, the closing |
| Essay | a title and a byline | a title page, numbered sections |
| Book | chapters | a contents page, running heads, chapters on new pages |

A document names its kind as it names its face, by a word in its
frontmatter, `template: letter`, and its layout by Diaryx's `layout` word:
`continuous` is the reflowed layout, a paper the paged one. Neither is
written until someone chooses it; no word is a note, reflowed.

The styles move from twig-export's rules into the template, as Typst's show
and set rules over the elements twig-export builds (`show heading.where(level:
1): …`): a prestructured style is an element's, so the element is where it
belongs. twig-export's rules stay for what is not an element: a directive or
a class mapped to one of the template's styles.

### What the flowing editor still does

The reflowed view replaces the flowing editor only once it does what the rows
do. Today, on the pages:

- **Not drawn by twig-export:** formulas (the dialog first), highlights,
  strikethrough, superscript, subscript, a picture's media badge (video,
  audio), Mermaid diagrams, a date's chip, the presentation vocabulary.
- **Not drawn by the page view:** the find bar's matches, the spelling
  checker's underlines, a link's hover, a task box that ticks, a table's
  grid edited as cells.
- **A caret with no glyph:** an empty paragraph, a line's end after a hidden
  delimiter, a blank document. The page view draws it beside the nearest
  glyph; it needs the line's own box from the layout instead.
- **The input method:** the candidate window is placed by the editor's
  hidden rows, not by the caret on the page.
- **Assistive tech:** the rows give AccessKit the document's text and its
  lines' boxes (`crates/leaf-dioxus/src/access.rs`); the pages give nothing
  yet, and must give the same before the rows go.

Each is a task once this is accepted, and the flowing editor goes when the
last is done.

## Order of work

1. **Templates in twig-export:** the template function with its `layout`
   parameter, a note and a book first, the styles as show rules, and a
   reflowed layout tested against the paged one (the same lines, at the same
   width).
2. **The faces**, embedded in twig-export's typesetter and the app, once
   where they live is settled (**To settle**, 1).
3. **The reflowed view in leaf**, beside the page view, with the resize
   settling before it sets; Settings and the style menus prestructured.
4. **The math dialog.**
5. **The gaps** in **What the flowing editor still does**, accessibility
   first.
6. **The flowing editor's rendered view retired**: the source view and the
   terminal keep the rows.
7. **Formulas edited on the page**, as the Swift app edited them.

## To settle

1. **Where the faces live.** `diaryx_faces` is in Diaryx, which is private,
   and twig-export and leaf are public. The fonts are open (OFL), but moving
   code from Diaryx to a public repository is Adam's decision under the org's
   rules. The alternatives: a public repository of its own for the four
   faces, which Diaryx, leaf and twig-export all take; or twig-export taking
   the faces as an input, and each host bringing its own files.
2. **A face by name.** Diaryx's `font:` also takes a family by name. Under
   this proposal no menu writes one, and a template sets a name it does not
   know in its default face; whether a name is still honoured where the face
   is installed is a choice between Notes' hidden Fonts panel and none at
   all.
3. **The presentation vocabulary.** A node's `style` may carry
   `font-family`, `font-size` and `color`, which a prestructured template
   would not offer. Either a template reads them as it reads a face by name
   (above), or the vocabulary keeps only what the template names: alignment,
   and the template's colours.
4. **Columns.** Diaryx's `columns: 2` means something in a paged layout and
   little in a reflowed one on a narrow screen; the template may set two
   columns only where the width allows.
5. **The kinds and their names**, and whether a person can add a template of
   their own (Diaryx's library, a Typst file beside the document).
6. **On the web**, Typst in wasm is large: whether the web demo keeps the
   rows, as the terminal does, or sets pages in the browser too.
