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
| Model | `src/lib.rs` → `pkg/leaf_wasm.js` | `LeafDoc`: parse/edit/caret + a typed `DocView` frame of style runs. wasm-bindgen glue; view types generated from Rust by [tsify](https://github.com/madonoharu/tsify). |
| Editor | `web/src/editor.js` (+ `.d.ts`) | `LeafEditor`: a **framework-agnostic** class that renders those runs to the DOM, places the caret, and routes keys/clicks. The reusable piece. |
| Demo | `web/index.html` | A thin host: chrome (toolbar/footer) around a `LeafEditor`. |

## Proportional rendering

Headings step down in **size** (a per-level ramp, mirroring `leaf-gpui`'s
`EditorStyle`), body is a real proportional font, and code is monospace with a
tinted panel. Core still wraps each line to a *column* budget — a column is a
semantic position, not a pixel — so the renderer never multiplies
`col × cellWidth`. Instead the browser shapes each row and the pixel positions
are read back out of it: the caret's x from a collapsed DOM `Range` at the caret
column, and a click's `(row, col)` from `caretRangeFromPoint`. The browser
measures; core keeps the model.

## Input: web-native without contenteditable

The surface isn't `contenteditable` — it's a *projection* of core's model
(WYSIWYG hides markup; list markers and quote gutters are synthetic; the caret
rides a column grid), so letting the browser mutate it would bypass core's
offset↔position mapping. Instead a hidden `<textarea>` is parked at the caret as
an **input sink**: it captures IME composition (CJK, accents), mobile keyboards,
dictation, and autocorrect the way a native field does, and the resulting text is
forwarded to `doc.insert`. Control keys and shortcuts are handled on `keydown`;
plain text flows through the `input`/composition events so the IME is never
bypassed. This is how Monaco and CodeMirror drive a model-owned document — custom
caret and selection, a textarea purely for input. Wide glyphs stay correct: core
speaks display *columns* while a DOM `Range` counts UTF-16 units, so the caret is
communicated as `DocView.caret_ch` (a UTF-16 offset) and clicks come back through
`click_ch`, with the two mapped by core's own grapheme-width measure.

## Build

```sh
wasm-pack build crates/leaf-wasm --target web --out-dir web/pkg
# (--dev for a fast, unoptimized build during development)
```

This regenerates `web/pkg/` (git-ignored). Then serve `web/` over HTTP (wasm and
ES modules need a real origin, not `file://`):

```sh
cd crates/leaf-wasm/web && python3 -m http.server 8000
# open http://localhost:8000/
```

## Using `LeafEditor`

```js
import { LeafEditor } from "./src/index.js";

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
the heading ramp) — see `DEFAULT_THEME` in `web/src/editor.d.ts`.

## Not yet packaged

`LeafEditor` is written to be published as an npm package (framework-agnostic,
typed, self-contained styles), but the publish mechanics — `package.json`
`exports`, a chosen wasm-init strategy (`--target web` vs `bundler` vs inlined
base64), and semver — are deliberately deferred until the API has settled.
