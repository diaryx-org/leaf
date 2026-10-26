# Leaf in a browser

The leaf-dioxus editor (`crates/leaf-dioxus`) in a browser tab: compiled to
`wasm32-unknown-unknown`, laid out and drawn by Blitz into a canvas through
WebGPU. It is the same editor as the Mac app's, on a document in memory
(`demo.md`).

```sh
cargo xtask web --serve 8000     # then open http://localhost:8000
```

The page is `index.html`, `pkg/` and `fonts/`; serve all three. The first
build installs `wasm-bindgen` and `wasm-split-cli`, at the versions the
lockfile names, into the app's `target/tools`.

## Size

The main wasm is 17.5 MB, 4.5 MB with brotli (`brotli -q 11`); the fonts
another 0.9 MB, fetched in parallel with it; and the Mermaid renderer 2.7 MB
(0.6 MB), fetched the first time a diagram is drawn. Measured on 2026-10-26.

- **Split** (`xtask/src/web.rs`, the steps of `dx build --wasm-split`): a
  `#[wasm_split]` function in the widget's `picture.rs`, behind its
  `wasm-split` feature, is the only way in to the renderer, so the renderer
  goes into a module of its own. Until it arrives a diagram is an empty box,
  and every editor repaints when it does. Without the split the wasm was
  19.6 MB (5.0 MB).
- **Stripped**: names, DWARF and relocations, which the splitter needs and
  the page does not, go after the split; they were 3.7 MB of the wasm.
- **Fonts**: fetched by the page, not compiled in, so they download beside
  the wasm and stay cached when it changes.
- **No `wasm-opt`**: `-Oz` saved 1.7 MB before compression and added 0.1 MB
  after it, for a minute a build.

What is left is mostly the engine the first frame needs: Stylo (2.7 MB),
Blitz, parley, Vello and wgpu. Syntax highlighting (syntect and two-face's
definitions, about 1.8 MB) and math (RaTeX and the KaTeX faces, about
0.9 MB) could follow Mermaid, but both are called from inside a frame's
layout — the highlighting from leaf-core's row builder — so each needs to
draw plainly first and redraw when it arrives. The image decoders (0.7 MB)
and ICU's data are Blitz's dependencies, which a host cannot turn off.

## Why Blitz in a canvas, and not the DOM

The editor shapes and places every run of text itself, with parley
(`text.rs`), and moves the caret and hit-tests through those layouts
(`motion.rs`). In the DOM, through dioxus-web, the browser would shape the
text and parley's measurements would not match what it drew; the editor's
keys, input method and pointer code (`keys.rs`, `dispatch.rs`, `ime.rs`) read
winit's events, which dioxus-web does not have. Through Blitz, the widget
builds for wasm32 with one change (`clock.rs`: no `std::time::Instant` and no
tokio timer in a browser), and measures and draws with one engine, as on the
Mac.

What the canvas costs is what the browser does for a text field and does not
do for a canvas: its input method, its accessibility tree, its find, and the
clipboard's paste event. The list below says where each stands.

## What works

Tried in Chrome on a Mac, through WebGPU:

- Typing and Return, and the key chords: ⌘ on a Mac, iPhone or iPad, Ctrl
  elsewhere, read from the browser (`input.rs`). ⌘B, ⌘I, ⌘Z, ⌘C and ⌘V
  tried.
- The mouse: a click places the caret, a double-click selects a word; the
  wheel scrolls.
- Headings, lists, quotes, tables, code with highlighting, footnotes,
  formulas, and Mermaid diagrams with their labels.
- The page's own faces (`fonts/`, with their licences): Libertinus
  Serif for the body and DejaVu Sans Mono for code. A browser shows no system
  fonts to Blitz, so the same files go to parley, to Blitz's font collection
  and to SVG text (`blitz_dom::add_svg_fonts`, on the Blitz fork's `leaf`).
- Copy and Paste within the page, through a clipboard of its own. Copy and
  Cut write the text to the system's clipboard too, though that is not
  checked yet.

Not tried yet: the dark appearance, Safari, Firefox, a phone or tablet,
and a document long enough to test how fast it scrolls.

## What does not work yet

- **The input method.** winit's web backend has none, so a dead key (⌥E then
  E), a phone's keyboard, dictation and every CJK input method reach the
  editor as nothing. The fix is a hidden `<textarea>` that follows the caret
  and turns its `beforeinput` and composition events into the editor's
  `Ime` events (`ime.rs` already takes them); the archived web editor
  (`archive/packages/leaf-web/src/editor.js`) shows how those events behave.
- **Paste from outside the page.** winit's canvas cancels the key's default
  action, so the browser never fires `paste`, and reading the system
  clipboard otherwise needs a permission prompt.
- **WebGL.** Only WebGPU draws; where a browser has no WebGPU adapter, the
  page says so. Three things stand between the hybrid renderer and WebGL 2:
  wgpu takes WebGPU whenever `navigator.gpu` exists, even with no adapter
  behind it, and never tries GL; the renderer asks for WebGPU's limits,
  which WebGL refuses; and `anyrender_vello_hybrid` 0.11 asks for a
  `Bgra8Unorm` surface off Android, which wgpu's WebGL backend does not
  offer. The last is a change to the vendored renderer in the Blitz fork's
  `patches/`.
- **Accessibility.** AccessKit has no web adapter, so a screen reader finds
  an empty canvas.
- **Pictures.** The editor reads a picture's file from disk; a document in
  memory has none, so a picture draws as a chip with its alt text.
- **Saving and opening.** ⌘S and ⌘O do nothing: the page has nowhere to keep
  a document yet.
- **Colour emoji.** The bundled faces have none.
