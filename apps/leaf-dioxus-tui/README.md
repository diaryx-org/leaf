# leaf-dioxus-tui

A prototype: the leaf-dioxus editor (`crates/leaf-dioxus`), the component the
Mac app draws, drawn in a terminal. Blitz builds the DOM, resolves the styles
and lays the page out as it does for a window; `src/paint.rs` walks the laid-out
tree and writes character cells where blitz-paint would draw pixels.

```sh
cargo run --manifest-path apps/leaf-dioxus-tui/Cargo.toml -- document.md
cargo run --manifest-path apps/leaf-dioxus-tui/Cargo.toml -- --dump document.md       # one 80×24 frame as text
cargo run --manifest-path apps/leaf-dioxus-tui/Cargo.toml -- --dump-html document.md  # the same, in its colours
```

⌃Q quits. On macOS the editor's ⌘ shortcuts are ⌃ here: ⌃S saves, ⌃B is bold.
`LEAF_THEME=light` for a light terminal. `LEAF_TRACE=1 LEAF_DTUI_LOG=<file>` logs
keys and frame timings to the file.

The case for it, what it does and does not do yet, and what replacing leaf-tui
would take: [the proposal](../../docs/proposals/the-leaf-dioxus-editor-in-a-terminal.md).

## How it fits together

- `window.rs`: a winit `Window` that is the terminal, as `apps/leaf-dioxus`'s
  `offscreen.rs` is a sheet. The editor asks it for size, scale and theme.
- `main.rs`: the document is `DioxusDocument::new` with no window, as
  `apps/leaf-dioxus/src/headless.rs` builds one for tests. Keys go to
  `Editor::key`; clicks go to Blitz as pointer events; the wheel goes to
  `Editor::scroll`. Each turn is a poll and a `resolve`, then a paint.
- `paint.rs`: the cell painter. A cell is one advance of a monospace face
  (Menlo, 16px) wide and one line (1.5 leading, 24px) high, so the editor's own
  parley layout counts in cells without knowing it. Each line's clusters take
  consecutive cells, so text set larger than a cell still reads.
- `term.rs`: crossterm. A frame is written as the cells that changed.
