# leaf-web

The web editor for [leaf](../../README.md): `LeafEditor`, a framework-agnostic
rich-text editor over `leaf-core`'s document model, compiled to WebAssembly by
the [`leaf-wasm`](../../crates/leaf-wasm) binding. On npm as
[`@diaryx/leaf`](https://www.npmjs.com/package/@diaryx/leaf), at the
workspace's version: an `exports` map, types, and a `pkg/` of wasm-pack output
that `npm run build:wasm` regenerates and that is never committed. Publishing
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

await LeafEditor.init(wasmUrl);
```

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
- **Syntax highlighting** in a fenced block whose language a grammar knows:
  each run carries a `token` — one of core's eight classes — and arrives as a
  `.leaf-t-<token>` class beside `.leaf-r-code`, coloured by the
  `--leaf-syn-<token>` custom properties in both appearances. A stylesheet
  that knows nothing of them still draws the block as code. The grammars
  weigh about 0.7 MB of the wasm; build `leaf-wasm` with
  `--no-default-features` to leave them out.
- **Host highlights** (`setHighlights`), a **read-only** gate, **drag and
  drop** of text and files, `load()` for the next document, and `goTo` /
  `reveal` for landing a reader somewhere.
- **Incremental repaint.** A frame reuses every row it did not change.

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

The seven colours are `var(--leaf-red, …)` and friends, so a page with a
palette overrides them and a page without still reads in light and in dark. The
editor injects these very rules, scoped to its own surface and with two
corrections of its own (sizes relative to the text around them, so a step on a
heading scales the heading; spacing as a multiple of the theme's line height);
`PRESENTATION_CSS` is the same text as a string, for a host that renders leaf
documents outside the editor. A test holds the file and the constant level.

## Building and testing

```sh
cargo xtask web              # build the wasm, serve the demo, open it
cargo xtask web --test       # serve the editor tests instead
cargo xtask web --headless   # run them in Chrome and exit with the outcome
npm run build:wasm           # just the wasm (from this directory)
```

The tests ([`test/editor.test.html`](test/editor.test.html)) run in a real
browser against the real wasm, because what they cover — a `TreeWalker`, a
`Range`, a native selection, a laid-out proportional font — is exactly what a
stub DOM would get wrong. `--headless` finds Chrome or Chromium on its own
(`$LEAF_BROWSER` overrides), reads the outcome the page reports into its
title, and fails the way a test runner should.
