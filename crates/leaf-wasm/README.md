# leaf-wasm

The WebAssembly frontend for [leaf](../../README.md): it wraps `leaf-core`'s
filesystem-free `Doc` — the byte-offset caret model and the AST→glyph
`VisualMap` — and drives a **proportional**, browser-rendered rich-text editor.

It's the web peer of `leaf-tui` and `leaf-gpui`: core stays the single source of
truth for text, wrapping, and caret math, and the web side only paints glyphs
and forwards input — exactly as the TUI and native GUI do.

## Layers

| Layer | File | What it is |
|-------|------|------------|
| Model | `crates/leaf-wasm/src/lib.rs` → `pkg/leaf_wasm.js` | `LeafDoc`: parse/edit/caret + a typed `DocView` frame of style runs. wasm-bindgen glue; view types generated from Rust by [tsify](https://github.com/madonoharu/tsify). This crate. |
| Editor | [`packages/leaf-web`](../../packages/leaf-web) `src/editor.js` (+ `.d.ts`) | `LeafEditor`: a **framework-agnostic** class that renders those runs to the DOM, places the caret, and routes keys/clicks. The reusable, importable npm-shaped package (not yet published). |
| Demo | [`apps/leaf-web-demo`](../../apps/leaf-web-demo) `index.html` | A thin host: chrome (toolbar/footer) around a `LeafEditor`. |

This crate is only the **Model** layer — the Rust→wasm binding. The importable
editor and its demo were extracted into `packages/leaf-web` and
`apps/leaf-web-demo`; the binding's `pkg/` output is built *into* that package.

## Proportional rendering

Headings step down in **size** (a per-level ramp, mirroring `leaf-gpui`'s
`EditorStyle`), body is a real proportional font, and code is monospace with a
tinted panel. Core still wraps each line to a *column* budget — a column is a
semantic position, not a pixel — so the renderer never multiplies
`col × cellWidth`. Instead the browser shapes each row and the pixel positions
are read back out of it: the caret's x from a collapsed DOM `Range` at the caret
column, and a click's `(row, col)` from `caretRangeFromPoint`. The browser
measures; core keeps the model.

## Input: native selection, intercepted intents

The surface is one `contenteditable` element, so the browser owns the caret
and the selection natively — which is what makes word and line selection, drag,
right-click Look Up, mobile selection handles, and IME behave like a real
field. But the DOM is a *projection* of core's model (WYSIWYG hides markup;
list markers and quote gutters are synthetic), so the browser is never allowed
to mutate it: every `beforeinput` is prevented and its intent — insertText,
deleteContentBackward, insertParagraph, formatBold, insertReplacementText with
its target range — is translated into a core operation, after which the editor
repaints and restores the native selection to core's caret. IME composition is
the one exception the browser won't let anyone prevent; it composes into the
DOM and is reconciled into core on `compositionend`. This is the CodeMirror 6
shape rather than Monaco's hidden textarea, and it is the only way to have
native selection and IME on the same focused element. Wide glyphs stay
correct: core speaks display *columns* while a DOM `Range` counts UTF-16
units, so the caret crosses as `DocView.caret_ch` (a UTF-16 offset) and
selections come back through `set_selection`, with the two mapped by core's
own grapheme-width measure.

## Build

```sh
# from packages/leaf-web (wraps the wasm-pack invocation):
npm --prefix packages/leaf-web run build:wasm       # or build:wasm:dev for a fast build
# equivalently, directly:
wasm-pack build crates/leaf-wasm --target web --out-dir packages/leaf-web/pkg
```

This regenerates `packages/leaf-web/pkg/` (git-ignored). Then serve the **repo
root** over HTTP (the demo imports the package across directories; wasm and ES
modules need a real origin, not `file://`):

```sh
python3 -m http.server 8000            # from the repo root
# open http://localhost:8000/apps/leaf-web-demo/
```

## Using `LeafEditor`

```js
import { LeafEditor } from "leaf-web";               // the packages/leaf-web package

await LeafEditor.init();                              // load the wasm once
const editor = new LeafEditor(document.getElementById("editor"), {
  source: "# Hello\n\nType here.",
  format: "markdown",                                 // md | djot | html | xml
  onChange: (state) => updateToolbar(state),          // reflect active marks etc.
});

editor.toggleBold();                                  // imperative commands
editor.setHeading(2);
const md = editor.source();                           // persist however you like
```

The editor owns the editing surface and input; the host owns chrome (toolbar,
footer, save). Presentation is themeable via the `theme` option (fonts, sizes,
the heading ramp) — see `DEFAULT_THEME` in `packages/leaf-web/src/editor.d.ts`,
and [`packages/leaf-web/README.md`](../../packages/leaf-web/README.md) for the
rest of the surface: highlights, read-only, navigation, drag and drop.

## Packaged

`LeafEditor` lives in the `packages/leaf-web` npm-shaped package — framework-agnostic,
typed, with a `package.json` `exports` map over `--target web` wasm. The `pkg/`
wasm output builds into that package (see **Build** above). A chosen wasm-init
strategy beyond `--target web` (bundler vs inlined base64) and semver are still
free to evolve as the API settles.
