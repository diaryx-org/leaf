---
title: "Proposal: a presentation vocabulary"
status: implemented
created: 2026-09-18
updated: 2026-09-18
part_of: '[Proposals](/docs/proposals/proposals.md)'
---

# A presentation vocabulary

## Status

`implemented` on 2026-09-18, in the sequence's order, against the published
twig-doc 3.6.0 (`8c29ea2`): core in `edd3839`, with a review's four fixes in
`ca31104` and the caret kept on its text through a Markdown `<div>` wrap in
`0923823`; both bindings in `910aef4`; leaf-swift in `6913620` and `ae33dde`;
leaf-web in `4ce7e1d` and `4a7c7d2`; leaf-ratatui in `ff9be2f`. Four things
the text below did not foresee. `Capabilities::page_break` is true for
Markdown and djot only: twig spells the directive in HTML and AsciiDoc too,
but the walker cannot yet read `<page-break>` or `<<<` back, and the
[task](../tasks/page-break-in-html-and-asciidoc.md) says what would. A clear
that cannot reach its property — the caret's block is inside a `<div>` that
sets it and is not its sole child — sets a status rather than silently doing
nothing. The terminal draws text colour in its own inks rather than the
highlight's washes, and leaf-swift likewise keeps a contrast-checked pair per
name, because a wash that reads *under* text does not read *as* text. And
twig 3.6's new attribute axis of diagnostics reported an HTML link's `href`
and an image's `src` as attributes Markdown drops, which leaf's paste gate
read as a lossy paste; the gate now asks the question it meant to
(`c8a293b`), and the two twig defects behind it are tasks in twig.

`accepted` on 2026-09-18, on the twig side having landed: twig's
[presentation as attributes](https://github.com/diaryx-org/twig/blob/main/docs/proposals/presentation-as-attributes.md)
is `implemented` at `e956f9f`, with `Editor::set_block_attrs` and
`Editor::wrap_range_attrs` in the Rust binding and `html_elements` as the
Markdown spelling's gate — a flag leaf has had on since it first framed a
`<picture>`. Built against the twig checkout through
`~/diaryx/.cargo/config.toml`; the `twig-doc` pin moves once twig names the
version.

The names below are the proposal's, chosen for the reasons under **The
vocabulary**, and they are the part most likely to be renamed by whoever
reads this first. Everything else — which level each property lives at, how
a gesture edits one key and keeps the rest, where the theme comes in — holds
whatever the names end up being.

## The picture

leaf-swift's paginated view is a document editor in the shape a word
processor has, and a word processor lets an author centre a heading, set a
run of words larger, pick a face for a pull quote, open up a paragraph's
line spacing, colour a warning red, and put a page break where a chapter
ends. leaf draws none of these, because until twig 3.5 the document had
nowhere to carry them: a Markdown file has no attribute syntax, and leaf's
`Style` records what a glyph *is* — a heading, code, a link — and leaves what
it looks like to the frontend's theme.

Twig now carries the attribute. `set_block_attrs` replaces the attribute set
of the paragraph or heading the caret is in; `wrap_range_attrs` wraps a
selection in an attributed span or re-styles the one it already lies in;
both spell the pair in every format leaf opens — djot's `{.center}` line,
HTML's `class="center"`, AsciiDoc's `[.center]`, and for Markdown a raw
`<div class="center">` around the block and a `<span …>` around the run,
which every Markdown renderer passes through and twig's parser pairs back
into a container under `html_elements`. Twig interprets none of it. What
`center` means, which keys leaf writes, and how a frontend draws a run whose
`data-size` says `large` are leaf's to decide, and this proposal decides
them.

Page breaks are the one property that is not an attribute of an existing
block, because there is no block for it to be an attribute of. That was
decided separately (the `::page-break` leaf directive, through twig's
`insert_directive`) and is folded in here because it is the sixth button on
the same toolbar and lands in the same release.

## The principle: names, not values; the theme owns the number

Everything leaf writes into an attribute is a *name* — a token a native
renderer can switch on and a stylesheet can select — never a measurement.
That is the rule `MarkColor` already follows: twig records `red`, not a hex
triple, and each frontend picks the red that suits its palette and its
appearance. Three reasons to hold to it for the rest:

- **A document outlives its theme.** A run set to `14pt` in a theme whose
  body is 12pt is a step up; move the theme to 16pt and the run is now a step
  *down*, and the author's intent — "a little larger than the text around
  it" — has inverted. A run set to `large` stays a step up under every
  theme. The same holds for a face: a document that names `Georgia` is a
  document that renders in the fallback everywhere Georgia is not installed,
  which is every Linux terminal and most of the web.
- **A stylesheet can only enumerate.** leaf publishes a stylesheet so that
  HTML built from a leaf document — by plates, by GitHub, by anything that
  passes raw HTML through — renders these attributes without leaf present.
  CSS selects on an attribute's *value* and cannot read it into a property
  (`attr()` in `font-size` is one browser, this year), so a vocabulary the
  stylesheet can render is a closed one.
- **The dark side of the palette is real.** A colour named `red` has a dark
  appearance; a colour written `#c00` does not. `HighlightColors.swift`
  already keeps two versions of each of the seven names for the highlight
  background; a text colour named from the same seven gets the same care for
  free.

The cost is that leaf is not a general typesetter. An author who needs
exactly 13.5pt or exactly Garamond has a `style` attribute twig will carry
and leaf will ignore, and the last section says why that stays out.

## The vocabulary

Twig's recommendation, followed: **a class for an enumerated property with no
argument, a `data-` key for one that takes a value.** Every token below is
what CSS already calls the same thing wherever CSS has a name for it, so the
stylesheet is a transcription and nothing has to be learned twice.

### Block-level: alignment and line spacing

On a paragraph or a heading — the blocks `set_block_attrs` rewrites — and
read on any block beneath a `div` that carries them:

| property | attribute | tokens | absent means |
|---|---|---|---|
| alignment | `class` | `center`, `right`, `justify` | the theme's default, which is `left` |
| line spacing | `data-line-height` | `1.15`, `1.5`, `2` | the theme's `lineHeight`, which is `1` |

`class` is a space-separated token list in every format, and leaf reads the
one token it knows and leaves the rest — a paragraph that arrives as
`class="lead center"` is centred and keeps `lead`. There is no `left` token
because absence is left, and a document that wants to say so has no reason
to; a right-to-left default is a theme matter and the subject of the
[right-to-left task](../tasks/right-to-left-text.md), not of a class.

Line spacing is a multiple of the theme's line height, which is what every
word processor's spacing menu offers: single, 1.15, 1.5, double. The tokens
are the numbers rather than names (`loose`, `double`) so that the stylesheet
line is `line-height: 1.5` and the reader of the source sees the ratio.
`1` is not a token, because `1` is absence and a document should not carry a
key that says nothing.

### Run-level: size, face and colour

On an anonymous attributed span, or on the block itself when the whole block
is meant:

| property | attribute | tokens | absent means |
|---|---|---|---|
| size | `data-size` | `xx-small`, `x-small`, `small`, `large`, `x-large`, `xx-large`, `xxx-large` | the theme's `fontSize` |
| face | `data-font` | `serif`, `sans-serif`, `monospace`, `cursive` | the theme's `bodyFontName` |
| colour | `data-color` | `red`, `orange`, `yellow`, `green`, `blue`, `purple`, `brown` | the theme's `textColor` |

The size ramp is CSS's `<absolute-size>` keyword set with `medium` removed,
because `medium` is absence. The theme maps each step to a multiple of the
body size, and the default multiples are the ratios CSS's own user-agent
stylesheet uses for the same words — `small` is 0.8125 of medium, `large` is
1.125, `x-large` 1.5, `xx-large` 2, `xxx-large` 3 — so a browser given only
the stylesheet and a native renderer given the theme agree. A heading keeps
its own ramp: a `data-size` on a heading scales the heading's size, not the
body's.

The faces are CSS's generic families, less `fantasy` and `system-ui`,
neither of which an author asks for. The theme names the concrete face for
each — `serif` is Georgia on a Mac and Noto Serif on a Linux box, and
`monospace` is the theme's `monoFontName`, which inline code already uses.
A named family is *carried* (twig will spell `data-font="Garamond"`) but
not in the vocabulary: the native renderers resolve it through the platform's
font registry when it is installed and fall back to the theme's body face
when it is not, and the stylesheet knows only the four generics. The toolbar
offers the four.

The colour key is the one twig already owns. `data-color` on a `mark` node
is a highlight's *background*, in the seven names the `==🔴 text==` spelling
encodes; `data-color` on an attributed span is the text's *foreground*, in
the same seven names. Same key, same vocabulary, same `MarkColor` enum, and
the two never collide because a `mark` is a `mark` and a span is a span.
The one reason to reuse rather than mint a `data-text-color`: a frontend
that has a red for a highlight has a red for text, and both should be *that*
red.

### Page breaks

`::page-break`, a leaf directive with no label and no attributes. twig's
`insert_directive` writes it in every format that names a leaf container —
Markdown with `directives` on, which leaf has, and djot, where it spells as
an empty `::: page-break` fence whose name comes back as a class. leaf's
walker draws it as the `⧉ page-break` placeholder row every leaf directive
gets, and each frontend that paginates reads the row's `DirectiveMark` and
opens a page there. A break at the very start of a document is a row and no
page, so that the first page is never blank.

## Where a property lives, and how a gesture edits it

**Alignment and line spacing are the block's.** They describe how lines are
laid, and a line is a block's. The gesture is `set_block_attrs` on the
caret's block, whatever is selected.

**Size, face and colour are the run's, and the block's when no run is
chosen.** With a selection, the gesture is `wrap_range_attrs` on the
selection — a span, or a rewrite of the span the selection already is. With
no selection, it is `set_block_attrs` on the caret's block, so that "make
this paragraph larger" is a click with the caret in it rather than a select-
all first. The walker reads the run-level keys at both levels, the nearer
winning: a span's `data-size` inside a block with its own applies to the
span.

**Each gesture edits one key and keeps the rest.** Twig's replace-not-merge
contract is the reason leaf can: a gesture reads the node's attributes,
removes its own key (and for alignment, its own tokens from `class`), adds
the new value or nothing, and passes the list back whole. A paragraph that
came in as `class="lead center" id="intro" data-line-height="1.5"` and is
right-aligned goes out as `class="lead right" id="intro"
data-line-height="1.5"`. Nothing leaf did not write is touched, which is what
lets a document from elsewhere pass through the editor unharmed.

**Clearing is the same gesture with `None`.** Each takes an `Option`; `None`
removes the key, and an empty list at the end unwraps the span or the
Markdown div, which twig does. A block that has lost its last vocabulary key
and carries nothing else is spelled bare again.

**In Markdown, the block's attributes are on the div around it.** That is
twig's decision and the reason its `set_block_attrs` edits the div when the
caret's block is its sole child. The walker's side of the same rule: a
container named `div` with `Element` origin is transparent already (its
children draw as themselves, which is what the
[HTML containers proposal](html-containers-fold-to-atoms.md) argues against
for the rest of a page's chrome), and it now contributes its vocabulary keys
to every block it contains. So `<div class="center">` around three paragraphs
centres all three, which is what the author of that HTML meant, and around
one is the sole-child shape twig writes. `<p class="center">` in an HTML
document reads the same way with no div at all.

## What core carries, and what each frontend does with it

`Style` gains three fields beside `role`, each an `Option` of a small enum in
[`style.rs`](../../crates/leaf-core/src/style.rs) with a `from_attrs` and a
`name`, as `MarkColor` has: `size: Option<SizeStep>`, `font:
Option<FontFamily>`, `color: Option<MarkColor>`. `VRow` gains two, carried on
every row of the block the way `heading` is: `align: Option<Align>` and
`line_height: Option<LineSpacing>`. A `Capabilities` flag per property —
`alignment`, `line_spacing`, `font_size`, `font_family`, `text_color`,
`page_break` — answered by twig's `supports_with` under leaf's parse
extensions, and a query per property at the caret for the toolbar's state.
Both bindings export the same six gestures and five queries, and
`the_two_bindings_export_the_same_methods` holds them level.

What a frontend does with the row and glyph facts is its own, and the
honest answer differs by surface:

- **leaf-swift** honours all of it: alignment through the paragraph style's
  `alignment`, line spacing by scaling the row height, size and face by
  choosing the font, colour by `foregroundColor` in the appearance's version
  of the name. The paginated view opens a page at a `page-break` row and gives
  the row no height; the continuous view draws a dashed hairline there. PDF
  and print inherit both. The toolbar grows an alignment segment, a size and a
  face menu, a spacing menu, a colour swatch beside the highlight one, and a
  page-break button, each dimmed by its capability.
- **leaf-web** honours all of it through the same tokens as CSS classes and
  `data-` attributes on the row and run elements, and the package ships
  `presentation.css`, the stylesheet that renders those tokens — the same
  file a page built from a leaf document links to.
- **leaf-ratatui** honours alignment (by padding the row into the width),
  colour (an ANSI hue per name, the highlight's own), and the page break (a
  dashed rule). It ignores size and face, because a cell has one size and one
  face, and it says nothing about ignoring them, the way it says nothing
  about a heading's size.
- **leaf-gpui** is outside the workspace and gets the fields when it is next
  touched.

## The stylesheet

`packages/leaf-web/src/presentation.css`, one rule per token, nothing else:

```css
.center  { text-align: center }
.right   { text-align: right }
.justify { text-align: justify }
[data-line-height="1.15"] { line-height: 1.15 }
[data-line-height="1.5"]  { line-height: 1.5 }
[data-line-height="2"]    { line-height: 2 }
[data-size="xx-small"]  { font-size: xx-small }
…
[data-font="serif"]      { font-family: serif }
…
[data-color="red"]       { color: var(--leaf-red, #c0392b) }
…
```

The colours are custom properties with a fallback, so a page that has a
palette overrides them and one that does not still reads. The file is the
whole of what a consumer needs to render a leaf document's presentation in
a browser, and it is published with the package because that is where the
web frontend's own tokens are already defined.

## What this does not do

- **It does not carry a measurement.** No `14pt`, no `#c00`, no `Garamond`
  in the vocabulary, for the reasons under **The principle**. A `style`
  attribute is carried by twig and ignored by leaf.
- **It does not set a document's default.** A default face, size and line
  height are the theme's, and a document that wants its own has front matter
  for it. The attributes here are per-block and per-run overrides.
- **It does not add left.** Absence is the default alignment, and the default
  is the theme's.
- **It does not colour a heading's level or a link.** A `data-color` on a run
  inside a link colours the link's text; the link's own role still underlines
  it. Roles and the vocabulary compose rather than compete.
- **It does not touch AsciiDoc's inline gap.** Twig refuses
  `wrap_range_attrs` in AsciiDoc because `[.a]#text#` keeps a role and drops
  a `data-` key; leaf's three run-level buttons dim there, and the block-level
  form of the same properties still works.

## Sequence

1. Core: the vocabulary enums, the `Style` and `VRow` fields, the walker
   reading them at both levels and through a `div`, the six gestures, the five
   queries, and the capabilities. The page-break directive's insert gesture and
   the walker's acceptance of djot's fence form.
2. Bindings: leaf-ffi and leaf-wasm export the same surface, and the row and
   glyph views carry the new fields. The UniFFI binding regenerated.
3. leaf-swift: theme, `AttributedRow`, `EditorLayout` (alignment, line
   height, the page flow), and the toolbar.
4. leaf-web: `presentation.css`, the row and run rendering, and the toolbar.
5. leaf-ratatui: alignment, colour, and the page break.
