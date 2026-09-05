# leaf-web

The web editor for [leaf](../../README.md): `LeafEditor`, a framework-agnostic
rich-text editor over `leaf-core`'s document model, compiled to WebAssembly by
the [`leaf-wasm`](../../crates/leaf-wasm) binding. Not on npm yet — the package
stays `private` until there is a consumer — but shaped like a package: an
`exports` map, types, and a `pkg/` of wasm-pack output that `npm run
build:wasm` regenerates and that is never committed.

```js
import { LeafEditor } from "leaf-web";

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
- **Host highlights** (`setHighlights`), a **read-only** gate, **drag and
  drop** of text and files, `load()` for the next document, and `goTo` /
  `reveal` for landing a reader somewhere.
- **Incremental repaint.** A frame reuses every row it did not change.

The keyboard mirrors leaf-gpui: ⌘B/I/U, ⌘⇧C code, ⌘⇧M highlight, ⌘⌥0–6
paragraph and headings, ⌘⇧7/8 lists, ⌘[ / ⌘] outdent and indent, ⌘E toggles
the source view, ⌘⇧V pastes as plain text. Ctrl stands in for ⌘ off a Mac, and
on a Mac Control is left to the system's own bindings.

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
