---
title: A web demo drawn by the Dioxus editor
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-26
updated: 2026-10-26
---
# A web demo drawn by the Dioxus editor

**Where.** `crates/leaf-dioxus`, a new web host, and a Pages workflow.

**What.** The web demo at <https://diaryx-org.github.io/leaf/> is the
archived leaf-web editor (`archive/packages/leaf-web`, over
`archive/crates/leaf-wasm`), as deployed from the last release. Its workflow
is at `archive/.github/workflows/pages.yml`, so no new tag replaces it. The
Dioxus editor has no web target: `dioxus-native` draws through Blitz on
wgpu, and nothing in `crates/leaf-dioxus` builds for `wasm32`.

Two routes, to be weighed first:

- **Blitz in the browser.** Blitz and wgpu on `wasm32`, drawing to a canvas.
  The editor stays one editor, but the browser's own text input, IME,
  accessibility and find do not reach a canvas.
- **dioxus-web.** The DOM. The editor depends on Blitz in several places
  (`keys.rs`, `view.rs`), and dioxus-html has no `onbeforeinput` or
  composition events (`ime.rs`).

What leaf-web did that a web host needs, all in
`archive/packages/leaf-web/src/editor.js`: a `contenteditable` surface with
`beforeinput` intercepted (`insertReplacementText` for autocorrect and
dictation, `getTargetRanges`, the browser's own undo and redo); composition
reconciled on `compositionend`; ARIA (`aria-multiline`, `aria-readonly`,
labelled media and page breaks, a live status region); the clipboard in HTML
and plain text; drag and drop, with files dropped in; `prefers-color-scheme`;
a print stylesheet; and touch pointers for links and footnotes.

**Since 2026-10-26.** Settled for Blitz in the browser, and begun:
`apps/leaf-dioxus-web` runs the editor in a canvas through WebGPU, its
README saying why and what does not work yet. The widget builds for wasm32
with `clock.rs` (no `std::time::Instant` or tokio timer there) and reads the
command key from the browser; the Blitz fork's `add_svg_fonts` gives SVG
text the page's faces. Left: the input method (a hidden `<textarea>` feeding
`ime.rs`), paste from outside the page, WebGL 2 for browsers without
WebGPU, accessibility, and the Pages workflow that would deploy it.

**Done when.** A release deploys a demo drawn by the leaf-dioxus editor, or
the web is deliberately left to the archived editor's last deploy.
