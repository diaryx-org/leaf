---
config: colophon.yaml
contents:
- '[Sample](sample.md)'
---
# leaf (Work in progress!!)

A caret-based rich-text editor for documents, built on [`twig`](../twig).

## Workspace layout

leaf is a Cargo workspace in three tiers. The caret/selection model and the
AST→glyph mapping live in a frontend-neutral **core**; embeddable editor
**widgets** wrap it per toolkit; and thin **apps** host a widget with a window,
clipboard, and file I/O.

### `crates/` — Rust libraries

| crate | what it is |
|-------|------------|
| [`leaf-core`](crates/leaf-core) | the document model — a `twig::Editor` with a byte-offset caret + selection, and the WYSIWYG `VisualMap`. Glyphs carry a **toolkit-agnostic `Style`**; no UI dependency. |
| [`leaf-ratatui`](crates/leaf-ratatui) | the **embeddable terminal widget** (ratatui + crossterm): renders the editing surface into a `Rect` and turns key/mouse events into `Doc` edits, returning an `Outcome` for what the host owns (quit, save, clipboard, dialogs). The terminal peer of `leaf-gpui`. |
| [`leaf-gpui`](crates/leaf-gpui) | the **embeddable GUI widget** on [gpui](https://github.com/zed-industries/zed): the `Editor` view plus its input, pixel-wrapping renderer, and `register_keybindings`. Renders only the editing surface and leaves window chrome, file I/O, and quit to the host. |
| [`leaf-ffi`](crates/leaf-ffi) | the **UniFFI Rust binding** — wraps the filesystem-free `Doc` behind a C ABI so a native Apple app can drive it. Paired with the `leaf-swift` package. |
| [`leaf-wasm`](crates/leaf-wasm) | the **wasm-bindgen Rust binding** — wraps the `Doc` for the browser (`LeafDoc` + a typed `DocView`). Paired with the `leaf-web` package. |

### `packages/` — importable non-Rust widget packages

| package | what it is |
|---------|------------|
| [`leaf-swift`](packages/leaf-swift) | the Swift Package (`Package.swift`) — `LeafUI`, the AppKit/UIKit editor view, over the UniFFI `leaf-ffi` binding. The Apple peer of `leaf-ratatui`/`leaf-gpui`. |
| [`leaf-web`](packages/leaf-web) | the npm package — `LeafEditor`, a framework-agnostic web editor, over the `leaf-wasm` binding. |

### `apps/` — runnable frontends

| app | what it is |
|-----|------------|
| [`leaf-tui`](apps/leaf-tui) | the terminal editor (binary `leaf`) — a thin host around `leaf-ratatui` wiring a terminal, clipboard, and dialogs. The workspace default `cargo run`. |
| [`leaf`](apps/leaf) | the standalone gpui **application** (binary `leaf-gui`) — a thin host around `leaf-gpui`. |
| [`leaf-ios`](apps/leaf-ios) | the gpui iOS host (a standalone workspace on the gpui-mobile platform). |
| [`leaf-editor`](apps/leaf-editor) | the cross-platform (macOS + iOS) AppKit/UIKit/UniFFI demo app, consuming `packages/leaf-swift`. |
| [`leaf-web-demo`](apps/leaf-web-demo) | the web demo page, consuming `packages/leaf-web`. |

```sh
cargo run -- path/to/document.md            # the TUI (workspace default)
cargo run -p leaf -- path/to/document.md     # the GUI
```

`leaf` and `leaf-gpui` pin gpui to a specific Zed commit (gpui isn't published to
crates.io); the first build fetches and compiles the gpui tree, so it is slow.
It has **both views**, toggled with `⌘e`, just like the TUI's `⌥w`:

- **source** — the raw document, caret in source bytes.
- **wysiwyg** — `leaf-core`'s `VisualMap` resolved: `**bold**` painted bold,
  `_italic_` italic, `# ` / `` ` `` / `**` delimiters hidden, headings coloured,
  list markers as bullets. Each rendered glyph still maps back to its source
  byte, so the caret, selection, and clicks ride the *visible* text and step over
  hidden delimiters — the identical `VisualMap` the TUI renders, here with real
  proportional bold/italic via the per-glyph `to_gpui` styling in
  `leaf-gpui/src/style.rs`.

Both views share one rendering path (a `RowLayout` per visual row carrying each
character's source offset), so caret, selection, and mouse hit-testing are
written once and work in either view. Keys: arrows/Home/End (+`⇧` to select),
type to edit, `⌘b`/`⌘i` bold/italic, `⌘e` toggle view, `⌘s` save, `⌘q` quit.

```sh
cargo run -p leaf -- document.md            # opens in the source view
cargo run -p leaf -- document.md wysiwyg    # opens straight in wysiwyg
```

**gpui gotchas (macOS), learned the hard way:**

- The `gpui_platform` dependency **must** enable the `font-kit` feature. Without
  it, gpui's macOS backend uses a placeholder text system that lays text out but
  rasterizes *no glyphs* — the window, caret, and selection all render, but every
  character is invisible. This is not a version issue; it's a feature flag.
- gpui uses library features stabilized in Rust 1.95, and its macOS backend
  compiles Metal shaders at build time — so a full Xcode with the **Metal
  Toolchain** component is required (`xcodebuild -downloadComponent
  MetalToolchain`). The pinned toolchain lives in `rust-toolchain.toml`.

Sibling to [`bough`](../bough): **same backend, opposite model.** Where bough
moves a selection through the document's AST and edits the *tree*, leaf gives you
an ordinary text **caret**, mouse, selection, and a formatting toolbar — and
turns every keystroke into one of twig's offset-addressed edits. The document
stays a live, round-trippable AST the whole time you type into it, so a Markdown
file and a Djot file are edited through the exact same operations.

Two views, toggled with `⌥w`:

- **source** — the raw document with the caret in source bytes.
- **wysiwyg** — the markup *resolved*: headings coloured, `**bold**` as real
  bold, the `#` / `**` / `` ` `` delimiters hidden. The caret still works because
  every rendered glyph is tied back to the source byte it came from, so cursor
  motion, clicks, and selection ride the *visible* text and step right over the
  hidden delimiters. Because it reads the AST, Markdown and Djot that parse alike
  render identically — the [`mdfried`](https://github.com/benjajaja/mdfried)
  idea, made editable.

Every action maps onto twig's editor surface:

| action | twig op |
|--------|---------|
| type / delete | `edit_range(start, end, text)` |
| re-anchor the caret after an edit | the returned `Change` |
| breadcrumb / cursor context | `ancestors_at(offset)` |
| click to place the caret | `node_at` + the flat `nodes()` snapshot |
| **bold / italic / code / mark** | `toggle_inline(range, kind)` |
| **heading / body** | `set_block(offset, kind)` |

## Usage

```sh
cargo run -- path/to/document.md
```

Formats are detected by extension: `.md`/`.markdown`, `.dj`/`.djot`,
`.html`/`.htm`, `.xml`. The formatting toolbar targets the lightweight-markup
formats (Markdown, Djot).

## Keys

| key | action |
|-----|--------|
| *(printable)* | insert at the caret (replacing any selection) |
| `Enter` / `Backspace` / `Delete` | the usual |
| arrows / `Home` / `End` | move the caret |
| `Shift`+move | extend the selection |
| click / drag | place / drag the caret |
| `⌥b` / `⌥i` / `⌥c` | toggle **bold** / *italic* / `code` on the selection |
| `⌥m` | toggle mark/highlight (Djot) |
| `⌥1`…`⌥6` | make the block at the caret a heading of that level |
| `⌥0` | make it a paragraph |
| `⌥w` | switch between the source and wysiwyg views |
| `^s` / `^q` | save / quit |

## Status

Both views work: caret editing, mouse, selection, the format-aware toolbar, and
live AST awareness (the breadcrumb), in either source or wysiwyg.

Known rough edges (next steps): no soft-wrap-aware width for wide/emoji glyphs
(columns are counted in chars); code blocks render read-styled but map coarsely,
so edit code in the source view; no inline-image rendering yet (kitty/sixel is
the natural follow-up now that the glyph map exists); and no undo/redo (that
belongs in twig, which owns the buffer).
