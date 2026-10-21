# leaf-dioxus-tui

A prototype: the leaf-dioxus editor (`crates/leaf-dioxus`), the component the
Mac app draws, drawn in a terminal, with unit's menus and dialogs round it as
the Mac app has them. Blitz builds the DOM, resolves the styles and lays the
page out as it does for a window; `src/paint.rs` walks the laid-out tree and
writes character cells where blitz-paint would draw pixels.

```sh
cargo run --manifest-path apps/leaf-dioxus-tui/Cargo.toml -- document.md
cargo run --manifest-path apps/leaf-dioxus-tui/Cargo.toml -- -r -w 60 document.md     # read-only, a 60-character column
cargo run --manifest-path apps/leaf-dioxus-tui/Cargo.toml -- --dump document.md       # one 80×24 frame as text
cargo run --manifest-path apps/leaf-dioxus-tui/Cargo.toml -- --dump-html document.md  # the same, in its colours
```

- ⌃P or F10 opens the command menu, which lists every command with its keys.
  A right-click opens the context menu.
- ⌃Q quits, and asks first when there are unsaved changes.
- On macOS the editor's ⌘ shortcuts are ⌃ here (a terminal never sees ⌘): ⌃S
  saves, ⌃B is bold, ⌃Z undoes. ⌃K stays the editor's delete to the end of
  the line.
- `-w N` sets the column to N characters, 0 to the terminal's width; the
  default is 80, as leaf-tui's.
- The terminal is asked whether it is light or dark; `LEAF_THEME=light|dark`
  overrides the answer.
- `LEAF_TRACE=1 LEAF_DTUI_LOG=<file>` logs keys and frame timings to the file.

The case for it, what it does and does not do yet, and what replacing leaf-tui
would take: [the proposal](../../docs/proposals/the-leaf-dioxus-editor-in-a-terminal.md).

## How it fits together

- `window.rs`: a winit `Window` that is the terminal, as `apps/leaf-dioxus`'s
  `offscreen.rs` is a sheet. The editor asks it for size, scale and theme.
- `main.rs`: the document is `DioxusDocument::new` with no window, as
  `apps/leaf-dioxus/src/headless.rs` builds one for tests. Every key goes to
  unit first, then to the command menu and Quit, then to `Editor::key`; the
  mouse goes to Blitz as pointer events, the wheel to `Editor::scroll`. Each
  turn is a poll and a `resolve`, then a paint.
- `paint.rs`: the cell painter. A cell is one advance of a monospace face
  (Menlo, 16px) wide and one line (1.5 leading, 24px) high, and the editor's
  `Type::uniform` sets every run at that size, so the editor's own parley
  layout counts in cells without knowing it.
- `pictures.rs`: pictures, formulas and diagrams through the terminal's
  graphics protocol (ratatui-image), over the cells of their boxes.
- `chrome.rs` and `commands.rs`: unit's root in its cell density
  (`Density::Cell`), the context menu, the command menu built from one
  command table, the Link prompt, and the question before quitting.
- `files.rs`: Save and Save As, the question before a save writes over a
  file changed on disk, and the watch that reloads a clean document — leaf-tui's
  rules, over the same leaf-core calls.
- `term.rs`: crossterm and ratatui. A frame is written as the cells that changed.
