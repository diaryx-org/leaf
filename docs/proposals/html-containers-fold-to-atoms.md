---
title: HTML containers leaf does not author fold to atoms
status: draft
created: 2026-09-09
updated: 2026-09-09
part_of: '[Proposals](/docs/proposals/proposals.md)'
---
# HTML containers leaf does not author fold to atoms

## The picture

Open diaryx's `www/about/index.md` in the macOS view. It is Markdown, and it
parses under `html_elements`, so its `<section class="hero">`, `<div
class="wrap">` and `<p class="eyebrow">` all reach the visual map. What the
surface shows is:

- the eyebrow, `About Diaryx`, as a stray first paragraph above the heading;
- the section and its wrap divs as nothing at all, since an element-origin
  container falls into the walker's default arm and renders its children
  transparently;
- the ledger strip's `<span class="label">` words as ordinary inline text
  beside the figures they label.

Open a real `.html` document and the same thing happens one layer down: twig
keeps `<title>` as text (it is rcdata), keeps `<script>` and `<style>` as raw
text, folds `<html>`, `<body>`, `<main>` and `<section>` to a transparent
`section`, and hands `<nav>`, `<header>`, `<footer>`, `<aside>`, `<article>`
and a classed `<div>` back as the generic `container` kind. So a page's whole
chrome is on the editable surface, spelled as prose, and the title of the page
is its first line. The clipboard path already knows this list: `html.rs` drops
`head`, `title`, `script` and `style` before a paste. The open path runs none
of it.

## Two ends to pull from

**Extract.** Treat the file the way a reader mode does: find the content,
discard the rest, show a clean document. That is the templating step run
backwards, and it is wrong for an editor twice over. Which node is "the
content" is a heuristic, and the heuristic is wrong often enough that
Readability ships a corpus of exceptions. Worse, leaf is a caret editor over
source rows: every visual row maps to a source span and back. A transform
that produces a cleaner document has no map to the file it will be asked to
save.

**Fold.** Keep every node in the map and change only how the ones leaf does
not author are drawn. This is what leaf already does for a directive: a
`:::name{.class}` container draws as a tinted panel with a label, its rows
marked `directive`, its prose still editable inside; a leaf `::name{…}`
directive draws as a `⧉ name` placeholder atom that the caret steps over and a
frontend may paint over, with `rows_span` reserving its room. Since twig 2.8 an
unknown HTML element and a directive are one `container` kind, told apart by
`ContainerOrigin`, so the fold is not a new mechanism. It is the existing one,
applied to the other origin.

## The proposal

One rule, in `wysiwyg.rs`'s block walker, for a container whose origin is
`Element` and whose tag is neither `video` nor `audio` (those already fold to
a media placeholder):

- **A chrome element folds to an atom.** `head`, `title`, `script`, `style`,
  `nav`, `header`, `footer`, `form`, `svg`, `iframe`, `noscript`: one
  placeholder row labelled `⧉ nav`, a `DirectiveMark` carrying the tag and its
  attributes, rows reserved through `rows_span`, and the caret stepping over
  it in one keypress each way. The list is the clipboard's `DROP_TREE` plus the
  sectioning elements whose content is navigation rather than prose. It is a
  fixed list, not a judgement about the page.
- **Any other element-origin container draws as a panel.** `section`, `div`,
  `article`, `aside`, `main` when it carries attributes: rows marked
  `directive`, the first row labelled with the tag and its classes, `section
  .hero`, through the same `directive_label` a `:::` block uses. The prose
  inside stays editable, because it is prose.
- **An attributed paragraph stays a paragraph.** `<p class="eyebrow">` is a
  paragraph leaf authors. Its class may ride the row as a label, the way a
  code fence's language does, but it is not a container and does not fold.

The Swift view already paints a directive panel and its label
(`EditorLayout.swift`) and steps over a media placeholder, so the frontend
work is the label text. The web editor's atom mapping is the open task on a
directive hook, and the same `_domPoint` / `_rangeForRow` work serves both:
an HTML `<nav>` and a `::nav` directive should be indistinguishable to the
frontend.

## A content view on top, later

With the fold in place a stricter view is a rule rather than a heuristic:
show inline only what is inside `<main>`, else `<article>`, else `<body>`,
and draw everything outside it as an atom. A page you control always
satisfies it; a page saved into a book from elsewhere degrades honestly, with
what leaf does not understand opaque and steppable, never silently editable or
silently dropped. This is not part of the proposal's core and can wait until
someone asks for it.

## What this does not do

It does not sanitize on open. A document that goes through `sanitize` on the
way in is not the document on disk, and saving it would rewrite markup the
user never touched.

It does not make HTML authorable. `Capabilities::of(Format::Html)` stays
narrow: inline marks are spellable, a heading, a list, a quote, a link and a
fence are not. Whether twig should widen that is twig's question, and is
filed there as a proposal on editable HTML block elements. The fold is worth
doing whichever way that goes, because it is about what a page's chrome looks
like, not about what can be typed into it.

## Cost

A Markdown document with a raw `<div>` today renders it transparently and
would gain a labelled panel. That is a visible change for an existing
document and needs a `Behavioural-change:` trailer on the commit. The
diaryx site's pages are the first documents that will show it, and the
task in diaryx to move those pages onto directives is the other half of
this: once the site's chrome is a `:::hero` block, the panel is the intended
rendering rather than a fallback.
