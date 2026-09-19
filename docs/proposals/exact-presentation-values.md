---
title: "Proposal: exact values beside the presentation names"
status: accepted
created: 2026-09-19
updated: 2026-09-19
part_of: '[Proposals](/docs/proposals/proposals.md)'
---

# Exact values beside the presentation names

## Status

`accepted` on 2026-09-19. Adam's decision: the vocabulary is extended, not
reversed. Each menu that offers the names keeps them, first, and grows one
more row — *Other…* — for an author who wants a number, a family, or a
colour the names do not cover, and who takes the portability cost knowingly.

## The picture

The [presentation vocabulary](presentation-vocabulary.md) shipped in leaf
0.2.0 writes names and never measurements: `data-size="large"`, not `14pt`;
`data-font="serif"`, not `Garamond`; `data-color="red"`, not `#c0392b`;
`data-line-height="1.5"` from a menu of three. The reasons still hold, and
this proposal does not argue with them. A step outlives a theme change, a
generic face opens on every machine, and a closed set is what a stylesheet
can render with no leaf present.

What the names cannot do is be exact. A letterhead is set in the firm's
face; a brief is 12 point because the court says so; a brand colour is a
hex triple and not "the nearest of seven". An author with one of those needs
has, today, a `style` attribute twig will carry and leaf will ignore, and a
document that draws differently in the editor than it will anywhere else.

That proposal's own doc comments already imagine the extension. `from_attr`
on each enum says a `data-size="14pt"` from elsewhere is *carried* and drawn
at the theme's size, and that a named family is carried and "the native
renderers may resolve it through the platform's font registry". This
proposal makes both true: the keys stay, the value grammar opens, and the
menus grow a way to write the open form.

## The principle: the names first, the number on request

Every property keeps its names as the *first* thing a menu offers, because
a name is still the right answer for "a little larger" and "in a serif",
which is most of what an author asks for. The exact form is one more row,
under a divider, and it is the author's choice to take it:

- **A name is portable. A value is exact.** A run set to `large` reads as
  a step up under every theme and on every surface; a run set to `14pt`
  reads as 14 points, which is what the author asked for and is all it is.
  The menu row that offers the second is labelled so that the trade is
  visible: *Other…*, Apple's own word for the row on a font-size popup
  that opens a field.
- **Same key, open value.** An exact size is `data-size="14pt"`, an exact
  face `data-font="Garamond"`, an exact colour `data-color="#c03030"`, an
  exact spacing `data-line-height="1.3"`. Not a `style` attribute: twig
  carries `style`, but one attribute holding several declarations would
  make every gesture a CSS parser, would break the "edit one key and keep
  the rest" contract the six gestures rely on, and in AsciiDoc `style` is
  a positional attribute with a meaning of its own. One key per property
  is what the walker already reads and the gestures already write.
- **The grammar is CSS's, cut down.** Each key accepts what the CSS
  property of the same name accepts, restricted to the form a word
  processor speaks, so that the inline style a browser would need is a
  transcription of the token: `font-size: 14pt`, `font-family: Garamond`,
  `color: #c03030`, `line-height: 1.3`.
- **The theme owns the names; the author owns the values.** A theme maps
  `large` to a ratio and `red` to two inks, one per appearance, as it does
  now. A theme does not touch `14pt` or `#c03030`: an exact colour is
  painted as written in both appearances, and an exact size is that many
  points before the zoom. That is the cost the *Other…* row buys, and the
  proposal does not soften it with a heuristic.

## The grammar

| key | names (unchanged) | exact form | canonical spelling |
|---|---|---|---|
| `data-size` | `xx-small` … `xxx-large` | `<number>pt` | shortest decimal, `pt` suffix: `14pt`, `13.5pt` |
| `data-font` | `serif`, `sans-serif`, `monospace`, `cursive` | any other string, a family name | as given, trimmed |
| `data-color` | the seven `MarkColor` names | `#rrggbb` (`#rgb` read, never written) | lowercase six-digit hex |
| `data-line-height` | `1.15`, `1.5`, `2` | any positive decimal | shortest decimal: `1.3`, `1.25` |

Alignment has no exact form. Left, centre, right and justify are the whole
of what a line can do.

Only `pt` is a size unit. `px`, `em`, `rem` and `%` are not read: `px` is a
screen's unit and a document is not a screen, and the relative units are
what the steps already are. A value the grammar does not cover — `huge`,
`14px`, `rgb(…)`, `1.3em` — is what it was before this proposal: carried
untouched, drawn at the theme's default, and rewritten only by a gesture on
that key.

The exact size is a *point size*, not a multiple. It is what the author
typed and what the paper will show: on the paginated view 14pt is 14
points of the sheet, and on screen it is 14 points before the zoom the
view applies, which is how every other point size on the page behaves. A
heading set to an exact size is that size, not the heading's ramp scaled —
the name scales the ramp, the value replaces it.

## What core carries

Each of the four run- and block-level properties gets an *open* type beside
its closed enum. The enums (`SizeStep`, `FontFamily`, `MarkColor`,
`LineSpacing`) are unchanged and still `Copy`, still `ALL`, still the
menus' first offer. The open types hold either a name or a value:

- **Size**: a step, or a point size. The point size is a fixed-point
  number so the type stays `Copy` and `Eq` (a `Style` is stamped on every
  glyph and compared for run-merging); hundredths of a point is enough for
  `13.25pt` and far more than any menu writes.
- **Line height**: a step, or a ratio, in the same fixed point.
- **Text colour**: one of the seven names, or an RGB triple. The
  highlight's colour stays `Option<MarkColor>` on `Role::Mark` — its seven
  names are encoded in twig's Markdown bytes and are twig's to keep closed.
- **Face**: a generic, or a family name. A name is a `String`, and `Style`
  cannot hold one, so the walker interns every named family it meets in a
  table on the `VisualMap` and the glyph carries an index. The gestures and
  queries carry the name itself, since neither is per-glyph.

`from_attrs` on each open type reads the name first and the value second;
the walker's `Presentation` fold and `run_style` read the open type so the
nearest-wins rule is unchanged. `VRow::line_height` and `Style::size`,
`font`, `color` change type; nothing else in the map moves.

The six gestures and five queries take and return the open types. Each
gesture still edits one key and keeps the rest; a value is written in its
canonical spelling. A `data-size="huge"` at the caret is not a size the
query can answer, so the query answers `None` and the menu ticks *Default*,
exactly as before.

`Capabilities` gains nothing. The exact form rides the same two twig
operations the names do, so a format that can spell `data-size="large"`
can spell `data-size="14pt"`.

## The bindings

leaf-ffi's four enums become enums with payloads, which UniFFI 0.28 spells
in Swift as enums with associated values: `.step(.large)` or
`.points(14.0)`, `.generic(.serif)` or `.named("Garamond")`, `.named(.red)`
or `.rgb(r, g, b)`, `.step(.oneHalf)` or `.ratio(1.3)`. The gesture and
query signatures change accordingly, which is a breaking change to the
Swift API and gets a `Behavioural-change:` trailer; which version that is,
is Adam's.

The *views* — `Run.size`, `Run.font`, `Run.textColor`, `Row.lineHeight` —
stay `String?`, carrying the canonical token, for the reason
`PresentationVocabulary.swift` gives: a renderer handed a string draws a
token a newer core knows as *something*, and a table keyed by string is
what the theme already is. The Swift theme learns to read the exact forms
when its table has no entry; the parse is a suffix, a hex triple and a
decimal, and stays in one place.

leaf-wasm already speaks strings both ways. `set_font_size("14pt")` works
once core parses it, and `font_size_at_caret()` answers `"14pt"`.

## What each frontend does

- **leaf-swift** honours all four. The theme's resolvers fall through
  from the table to the exact form: `sizeScale` becomes a size in points,
  `fontName` tries the family by name through the platform's registry and
  falls back to the body face when it is not installed, `textColor` reads
  a hex triple, `lineSpacing` reads a ratio. Each of the four menus grows
  an *Other…* row under its divider. Size and spacing open a small field
  (points, or a ratio) in a popover on the Mac and a sheet on iOS; face
  opens the platform's font picker — a searchable list of installed
  families on the Mac, `UIFontPickerViewController` on iOS; colour opens
  the system colour picker on both. The value at the caret, when it is an
  exact one, shows as a ticked row of its own above *Other…* — "14 pt",
  "Garamond", a swatch — so the menu always shows what is in force.
  *Bigger* and *Smaller* from an exact size move it by one point.
- **leaf-web** renders the exact form as an inline style on the run or
  row element, beside the `data-` attribute it already sets, because the
  published stylesheet can only enumerate. A page built from such a
  document and rendered with `presentation.css` alone shows the names and
  not the values; that is the portability cost stated, and the stylesheet's
  header says so. The editor's own surface is complete.
- **leaf-ratatui** honours an exact colour as a truecolour cell and keeps
  ignoring size, face and spacing, as it ignores their names. leaf-tui's
  command palette offers no field for a value — a palette is a list — and
  gains nothing; the document's exact colours draw.
- **leaf-gpui** is outside the workspace and gets the open types when it
  is next touched.

## What this does not do

- **It does not remove or rename a name.** Every document written under
  0.2.x parses, draws and round-trips as it did.
- **It does not read `style`.** A `style` attribute is still carried and
  ignored.
- **It does not adapt an exact colour to the dark appearance,** and does
  not scale an exact size with the theme. Those are what "exact" means.
- **It does not add a unit menu.** Points are the unit, as they are on the
  paper.
- **It does not teach the stylesheet to render a value.** CSS `attr()`
  into `font-size` is one browser this year; when it is every browser the
  stylesheet can grow four rules and the inline style can go.

## Sequence

1. Core: the four open types, `from_attrs` reading name-then-value, the
   canonical spellings, the face table on the map, the walker's fold and
   `run_style`, the gestures and queries, and tests for every form and for
   the untouched carry of a value outside the grammar.
2. Bindings: leaf-ffi's enums with payloads and the changed signatures;
   leaf-wasm's string surface parsing and printing the open forms; the
   views unchanged; `the_two_bindings_export_the_same_methods` held level.
3. leaf-swift: the theme's fall-through resolvers, the model's API on the
   new types, the four *Other…* rows and their pickers, the ticked row for
   an exact value, and *Bigger*/*Smaller* by a point.
4. leaf-web: the inline style beside the `data-` attribute, the stylesheet
   header's note, and the test page.
5. leaf-ratatui: the truecolour ink.
