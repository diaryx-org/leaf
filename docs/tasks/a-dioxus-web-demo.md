---
title: A web demo drawn by the Dioxus editor
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-26
updated: 2026-10-27
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
text the page's faces.

**Since 2026-10-27.** Most of leaf-web's list is back. A hidden text field
at the caret holds the focus and brings the input method, the clipboard in
both flavours and the browser's `beforeinput` (`field.rs`); it holds the
text as shown around the caret, so a screen reader reads the document's
words. Pictures load through the new `Host::picture`, and text, HTML,
pictures and documents can be dropped. ⌘S, ⌘O and `localStorage` keep the
document (`files.rs`); a tap follows a link or footnote; the dark appearance
follows the system's. `pages.yml` deploys it on the next tag.

Left:

- **WebGL 2**, for a browser without WebGPU, which gets a message instead
  of the editor. dioxus-native fixes the renderer at compile time, and
  `anyrender_vello_hybrid` has a WebGL scene painter but no WebGL window
  renderer: one would be a patch in the Blitz fork's
  `patches/anyrender_vello_hybrid`, chosen at resume when WebGPU has no
  adapter.
- **Print and Export as PDF.** leaf-web printed its DOM; a canvas prints a
  bitmap of what is on screen. The Mac app's `pdf.rs` lays the document out
  offscreen and draws it with krilla; on the web its synchronous picture
  loading would need to become async, and it would move to a crate both
  apps share.
- **Accessibility beyond the text**: the structure (headings, lists,
  links) and a live region for the status line, which AccessKit has no web
  adapter to give.
- **Faces for other scripts and colour emoji**: CJK and emoji draw as boxes,
  since Blitz sees only the faces the page brings.
- **The split** (`cargo xtask web --split`), off until it is sound: code
  leaf-core shares with the Mermaid renderer moves into its module and does
  nothing until it arrives (`xtask/src/web.rs`).
- **Tried only in headless Chrome on Linux**: Safari, Firefox, a phone's
  keyboard and a screen reader are not.

**Done when.** A release deploys a demo drawn by the leaf-dioxus editor, or
the web is deliberately left to the archived editor's last deploy.
