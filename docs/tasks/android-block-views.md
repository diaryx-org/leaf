---
status: open
created: 2026-09-25
updated: 2026-09-25
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Android block views

**Where.** `packages/leaf-android/leaf-compose` — `EditorLayout.kt` places
rows, `LeafEditor.kt` draws them.

**What.** The Compose editor draws every row core sends as text, including the
rows other frontends replace with a view of their own:

- **Tables** draw as core's box-drawn picture, monospaced and unwrapped, where
  the Apple and web editors draw a grid from `DocView.tables`. The box-drawing
  glyphs are wider than the monospace advance in Android's fallback font, so
  the rules overrun the column on the right.
- **Block media** (`DocView.media`) draw as their `🖼`/`🎬`/`🔊` placeholder rows.
- **Math** (`DocView.math`) draws as its stand-in glyph, inline, and as the
  placeholder rows for a display formula; `typeset_math` is in the binding and
  unused.
- **Directive containers** carry no dashed outline, and a `::page-break` draws
  its placeholder rather than a hairline.

**How.** leaf-swift's `EditorLayout` is the model: the rows in a view's
`start_row..end_row` collapse onto the first, which takes the view's height;
the caret and hit test over a table read the grid (`TableCellView`'s source
offsets), not the picture. A formula is an SVG (`MathPicture`) — Android draws
none natively, so it wants either the resvg component already linked into
`libleaf_ffi.so` or a raster from core.

**Done when** a table, an image, and a display formula in the sample draw as
they do in the Apple app, with the caret landing in them the same way.
