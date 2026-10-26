# Leaf in a browser

The leaf-dioxus editor (`crates/leaf-dioxus`) in a browser tab: compiled to
`wasm32-unknown-unknown`, laid out and drawn by Blitz into a canvas through
WebGPU. It is the same editor as the Mac app's, on a document in memory
(`demo.md`).

```sh
wasm-pack build apps/leaf-dioxus-web --target web --release --no-typescript --no-pack
python3 -m http.server -d apps/leaf-dioxus-web     # then open http://localhost:8000
```

wasm-pack fetches the `wasm-bindgen` that the lockfile names. The bundle is
about 25 MB; brotli (`brotli -q 11 pkg/*.wasm`) cuts it by three to four
times for serving.

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
- The page's own faces, bundled (`fonts/`, with their licences): Libertinus
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
