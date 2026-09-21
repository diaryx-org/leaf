# leaf-web

The web editor for [leaf](../../README.md): `LeafEditor`, a framework-agnostic
rich-text editor over `leaf-core`'s document model, compiled to WebAssembly by
the [`leaf-wasm`](../../crates/leaf-wasm) binding. On npm as
[`@diaryx/leaf`](https://www.npmjs.com/package/@diaryx/leaf), at the
workspace's version: an `exports` map, types, and two directories of wasm-pack
output — `pkg/`, the editor's module, and `pkg-math/`, the typesetter's — that
`npm run build:wasm` regenerates and that are never committed. Publishing
is by hand — `npm run build:wasm && npm publish --access public` — and
`prepack` refuses a `pkg/` built for another version.

```js
import { LeafEditor } from "@diaryx/leaf";

await LeafEditor.init();                              // load the wasm once
const editor = new LeafEditor(document.getElementById("editor"), {
  source: "# Hello\n\nType here.",
  format: "markdown",                                 // markdown | djot | html | xml
  onChange: (state) => updateToolbar(state),          // marks, heading, dirty, history
});

editor.toggleBold();                                  // the commands leaf-gpui has
editor.setHeading(2);
const md = editor.source();                           // persist however you like
editor.markSaved();
```

The editor owns the editing surface and input; the host owns the chrome — a
toolbar, a footer, the save affordance. `capabilities()` says which buttons the
format can spell, `onChange` lights them, and the demo in
[`apps/leaf-web-demo`](../../apps/leaf-web-demo) is a complete host in one
file.

`init()` with no argument fetches the wasm relative to the package's own
module, which is right when the page loads it unbundled. A bundler moves the
module and not the binary, so a bundled host asks it for the binary's URL —
the package exports it as `@diaryx/leaf/wasm` — and hands that to `init`:

```js
import { LeafEditor } from "@diaryx/leaf";
import wasmUrl from "@diaryx/leaf/wasm?url";        // Vite; other bundlers have their own spelling
import mathUrl from "@diaryx/leaf/math/wasm?url";

await LeafEditor.init(wasmUrl, { mathUrl });
```

The second URL is the **typesetter**, a wasm module of its own that the editor
fetches the first time a frame shows a formula — see *Math* below. `init`'s
second argument also says when: `{ math: "lazy" }` is that default,
`"eager"` fetches it now and `init` waits for both, `"off"` never fetches it
and a formula stays its placeholder. `LeafEditor.loadMath()` is `"eager"`
for a host that decides later.

## What it does

- **Proportional rendering.** A real body font, headings by size, code in a
  monospace panel; core wraps to a column budget the editor fits against what
  the browser actually drew.
- **Native selection and input.** One `contenteditable` surface the browser
  owns the caret in, with every edit intent intercepted and translated into a
  core operation, so IME, dictation, autocorrect, and mobile selection handles
  all work and the DOM never drifts from the model.
- **Tables as a grid**, built from core's structural view rather than its
  box-glyph picture; Tab walks the cells.
- **Block media** — images, video, audio — as real elements with a caret stop
  on either side.
- **Clickable things.** A task box ticks; ⌘-click follows a link (or lands in
  the document for a fragment), or a footnote to its note; hovering a link
  shows its destination.
- **Coloured highlights.** `highlight(color)` is one press of a swatch: it
  colours the highlight at the caret, or — over a selection that isn't
  highlighted yet — makes one and colours it, as a single undo step. The
  palette is `MARK_COLORS`, the names core reads and writes (`==🔴 text==`)
  and the suffixes of the `.leaf-mk-*` classes the stylesheet already paints
  with. Dim a picker on `capabilities().mark_color` — Markdown spells a colour
  and Djot doesn't — together with `caretInMark()` or `state.hasSelection`,
  which say whether there is anything for the colour to belong to.
  `setMarkColor` is the exact one-splice gesture underneath.
- **The presentation vocabulary.** `setAlignment`, `setLineSpacing`,
  `setFontSize`, `setFontFamily`, `setTextColor` and `insertPageBreak`, each
  taking one of a closed list of *names* (`ALIGNMENTS`, `LINE_SPACINGS`,
  `SIZE_STEPS`, `FONT_FAMILIES`, `MARK_COLORS`) or `null` to clear it, and each
  with an `…AtCaret()` query a control lights itself by and a
  `capabilities()` flag it dims by. Alignment and spacing are the caret's
  block; size, face and colour are the selection, or the block when there is
  none. The row element carries the alignment class and `data-line-height`, the
  run element `data-size`, `data-font` and `data-color` — the document's own
  spelling, so the editor's DOM and a published page's are the same.
  Every key but alignment also takes the **exact value** the names cannot
  spell — `"14pt"`, `"Garamond"`, `"#c03030"`, `"1.3"` — which a control offers
  under its names as an *Other…* row (the demo's is a `prompt()`). The
  attribute is the document's either way; a value is drawn by an inline style
  beside it, because no stylesheet can enumerate one. An exact size is that
  many points and not the heading's ramp scaled, and an exact colour is painted
  as written in both appearances: that is what exact means, and it is the
  portability the author traded away.
- **Math.** A `$…$` formula is its picture in the line, stretched over the
  one-character atom core counts the row by, and a `$$` block a centred
  picture in a row of its own — a self-contained SVG from `leaf-math`, the
  same bytes every leaf frontend draws. TeX the typesetter cannot read keeps
  its `∑`, tinted, with the fault in its title. The typesetter is a second
  wasm module, `@diaryx/leaf/math`, that the editor fetches on the first
  formula: until it lands the formula is drawn as core's placeholder — the
  atom in a line, the `∑ tex` row for a block — and the rows that carried one
  repaint when it does. A page with no formulas never loads it; a host that
  knows its documents have them asks for it up front (`init`'s `math:
  "eager"`); one that never wants the weight says `"off"`. The module is also
  importable on its own, for a host that renders leaf documents outside the
  editor: `typeset_math(tex, display, sizePx, "#rrggbb")` gives the SVG and
  its baseline metrics.
- **Syntax highlighting** in a fenced block whose language a grammar knows:
  each run carries a `token` — one of core's eight classes — and arrives as a
  `.leaf-t-<token>` class beside `.leaf-r-code`, coloured by the
  `--leaf-syn-<token>` custom properties in both appearances. A stylesheet
  that knows nothing of them still draws the block as code. The grammars
  weigh about 0.7 MB of the editor's module; build `leaf-wasm` with
  `--no-default-features` to leave them out.
- **Host highlights** (`setHighlights`), a **read-only** gate, **drag and
  drop** of text and files, `load()` for the next document, and `goTo` /
  `reveal` for landing a reader somewhere.
- **Incremental repaint.** A frame reuses every row it did not change, and
  crosses the wasm boundary as only the rows that did (`set_incremental_frames`
  in `leaf-wasm`; a caret move lifts none, a keystroke lifts one).

The keyboard mirrors leaf-gpui: ⌘B/I/U, ⌘⇧C code, ⌘⇧M highlight, ⌘⌥0–6
paragraph and headings, ⌘⇧7/8 lists, ⌘[ / ⌘] outdent and indent, ⌘E toggles
the source view, ⌘⇧V pastes as plain text. Ctrl stands in for ⌘ off a Mac, and
on a Mac Control is left to the system's own bindings.

## `presentation.css`

[`src/presentation.css`](src/presentation.css) is the stylesheet **a page built
from a leaf document links to**. A document carries alignment, spacing, size,
face and colour as names rather than measurements — `class="center"`,
`data-size="large"`, `data-color="red"` — which survive in every format leaf
writes (a `<div>`/`<span>` in Markdown, a `{.center}` line in djot, an
attribute in HTML). This file is one rule per token and nothing else, so a
page rendered by anything that passes that markup through draws what the author
meant, with no leaf and no JavaScript present:

```html
<link rel="stylesheet" href="node_modules/@diaryx/leaf/src/presentation.css" />
```

```js
import "@diaryx/leaf/presentation.css";   // the same file, through the exports map
```

It holds the *names* and only the names: there is no rule to write for every
point size, family and hex triple, so a page rendered with this file alone
draws `data-size="14pt"` at its own size and `data-color="#c03030"` in its own
ink. A page that wants the values too renders its runs the way the editor does
— the value inline, beside the attribute — and the file's header says so.

The seven colours are `var(--leaf-red, …)` and friends, so a page with a
palette overrides them and a page without still reads in light and in dark. The
editor injects these very rules, scoped to its own surface and with two
corrections of its own (sizes relative to the text around them, so a step on a
heading scales the heading; spacing as a multiple of the theme's line height);
`PRESENTATION_CSS` is the same text as a string, for a host that renders leaf
documents outside the editor. A test holds the file and the constant level.

## What it weighs

Two modules, so a page pays for math only if it shows some. Measured on
2026-09-19, at leaf 0.3.1, built as `build:wasm` builds them — the
workspace's `wasm` profile (`opt-level = "s"`, fat LTO, one codegen unit,
`panic = "abort"`) and `wasm-opt -Oz`:

| module | file | raw | gzip | brotli |
|---|---|---|---|---|
| the editor (`leaf-wasm`, grammars in) | `pkg/leaf_wasm_bg.wasm` | 3.41 MB | 1.78 MB | 1.50 MB |
| the typesetter (`leaf-math-wasm`) | `pkg-math/leaf_math_wasm_bg.wasm` | 2.77 MB | 1.06 MB | 0.83 MB |

Before the split and the profile, one module carried both at 5.81 MB raw,
2.66 gzip, 2.15 brotli; and before math, 3.69 MB raw. So a page without a
formula now fetches 30% less than it did with math inside, and one with a
formula about 8% more in total — the two modules each carry a copy of the
runtime they would have shared (std, the regex engine both syntect and RaTeX
lean on, serde_json), about a megabyte raw between them, which is the price
of splitting at the module boundary rather than inside one. The profile
alone took the editor's module from 3.69 to 3.41 MB: the binary is half
data — fonts, grammars, tables — that no optimiser shrinks. `opt-level =
"z"` would take another 3–5% off each for an inlining cost the frame path
should not pay.

## Building and testing

```sh
cargo xtask web              # build both modules, serve the demo, open it
cargo xtask web --test       # serve the editor tests instead
cargo xtask web --headless   # run them in Chrome and exit with the outcome
npm run build:wasm           # just the two modules (from this directory)
```

The tests ([`test/editor.test.html`](test/editor.test.html)) run in a real
browser against the real wasm, because what they cover — a `TreeWalker`, a
`Range`, a native selection, a laid-out proportional font — is exactly what a
stub DOM would get wrong. `--headless` finds Chrome or Chromium on its own
(`$LEAF_BROWSER` overrides), reads the outcome the page reports into its
title, and fails the way a test runner should.
