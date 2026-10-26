---
part_of: id:dx4hxn744/kv2bv2m
title: leaf
contents:
- '[Sample](sample.md)'
- '[leaf on diaryx.org](/www/index.md)'
- '[Audiences](/vocab/audiences.md)'
- '[Proposals](/docs/proposals/proposals.md)'
- '[Tasks](/docs/tasks/tasks.md)'
- '[Archive](/archive/README.md)'
config: .config/prov.yaml
registry: registry.yaml
id: 6h2bs8f
slug: leaf
---
# leaf (Work in progress!!)

A caret-based rich-text editor for documents, built on [`twig`](https://github.com/diaryx-org/twig).

## Workspace layout

The caret/selection model and the AST→glyph mapping live in a
frontend-neutral **core**. One editor widget draws it: `leaf-dioxus`, on
Dioxus's native renderer, which draws HTML and CSS through
[the Blitz fork](https://github.com/diaryx-org/blitz) (branch `leaf`) with no
webview. Thin **apps** host the widget with a window or a terminal, a
clipboard and file I/O.

The other frontends (Swift for iOS and the Mac, the web package and its wasm
binding, gpui for the desktop, Android, and the ratatui terminal editor) are
archived under [`archive/`](archive/README.md). They stay readable there as a
guide to what each platform needs; the tasks under `docs/tasks` say which of
those behaviours the Dioxus path does not have yet.

### `crates/` — Rust libraries

| crate | what it is |
| ----- | ---------- |
| [`leaf-core`](crates/leaf-core)       | the document model — a `twig::Editor` with a byte-offset caret + selection, and the WYSIWYG `VisualMap`. Glyphs carry a **toolkit-agnostic `Style`**; no UI dependency. Fenced code is syntax-highlighted here too (syntect, two-face's grammars — the set `plates` publishes with), as an eight-way `Token` per glyph that each frontend colours its own way; the `syntax` feature carries the grammars and can be left out. |
| [`leaf-raster`](crates/leaf-raster)   | the **shared CPU rasterization layer**: image decode (SVG included), path resolution and aspect-fit policy, and oversized-heading rasters with the caret and selection painted into the pixels — how a terminal heading stays editable when cells can't draw over a graphics-protocol image.                    |
| [`leaf-dioxus`](crates/leaf-dioxus) | the **embeddable widget on Dioxus's native renderer** (`dioxus-native`, Blitz on wgpu): the editor, its rows, input method, find bar and status line, drawn in the `Look` its host gives it and asking a `Host` for the clipboard, Save, the zoom and the context menu. A standalone workspace, since `dioxus-native` is an alpha. |
| [`leaf-math`](crates/leaf-math)       | TeX to a **self-contained SVG picture** with baseline metrics, through RaTeX with KaTeX's outlines embedded — the one door every frontend draws a formula through, so the picture is byte-identical everywhere.                                                    |
| [`leaf-mermaid`](crates/leaf-mermaid) | Mermaid diagrams to SVG, through mmdr, for the editor to draw a fence with. |
| [`leaf-session`](crates/leaf-session) | the editors leaf runs itself for a region of a document (a formula today), behind one interface: keys in, splices on the region's content out. |
| [`leaf-math-edit`](crates/leaf-math-edit) | a structural formula editor whose file format stays TeX (a spike), which `leaf-session` runs. |

### `apps/` — runnable frontends

| app | what it is |
|-----|------------|
| [`leaf-dioxus`](apps/leaf-dioxus) | **Leaf**, in Rust from `main` up, for every platform Dioxus's native renderer reaches — the Mac's is `Leaf.app` (`org.diaryx.leaf`). A window per document, each a Dioxus page: the toolbar, `crates/leaf-dioxus`'s editor, the context menu and Settings, their buttons, menus and dialogs [unit](https://github.com/diaryx-org/unit)'s. What a platform does its own way is `platform/`: on the Mac an `NSDocument` per window (Open, autosave, Versions, Rename, recents, Finder's Open With), the menu bar, the spelling checker, Look Up and the print panel, through objc2. Export as PDF and Print from the editor's own layout. What it does not do yet is the task "What the Mac app does not do yet". A standalone workspace, since `dioxus-native` is an alpha and its macOS winit backend is vendored and patched. |
| [`leaf-dioxus-tui`](apps/leaf-dioxus-tui) | the terminal editor: the same `leaf-dioxus` editor, laid out by Blitz and painted into character cells by `blitz-tui`, with unit's menus and dialogs in their cell density. It replaces the archived `leaf-tui`; what it does not do yet is in [its proposal](docs/proposals/the-leaf-dioxus-editor-in-a-terminal.md) and the task "The terminal editor after leaf-tui". A standalone workspace, like the app's. |

```sh
cargo run --manifest-path apps/leaf-dioxus/Cargo.toml -- path/to/document.md
cargo run --manifest-path apps/leaf-dioxus-tui/Cargo.toml -- path/to/document.md
```

The Mac app is a bundle put together around its binary, so it has a task in
the runner in [`xtask/`](xtask):

```sh
cargo xtask mac              # build Leaf.app (apps/leaf-dioxus) and launch it, with a copy of the sample open
cargo xtask mac notes.md     # …with a document of your own
cargo xtask package          # a signed, notarised Leaf.app in a .dmg, in target/package/
```

`cargo xtask --help` lists every task and flag. `cargo xtask package` needs a
Developer ID identity in the keychain and notary credentials in the
environment; the head of [`xtask/src/package.rs`](xtask/src/package.rs) names
them, and `--no-notarize` does without the second.

The runner holds the tasks that leave Rust; the checks and releasing are
[`ship`](https://github.com/diaryx-org/ship). `ship ci` runs everything a
release has to pass — the follower versions, rustfmt, clippy with warnings
denied, the workspace tests, a `cargo check` of each crate alone in each
feature shape a host actually builds, and the Dioxus widget and app, each a
workspace of its own. `ship <patch|minor|major|X.Y.Z>` runs those checks,
moves the version through every manifest and the lockfile, cuts the generated
region of [`docs/CHANGELOG.md`](docs/CHANGELOG.md) into a released section,
and commits and tags — locally. Pushing takes `--push`, and a pushed tag is a
release: [`mac-app.yml`](.github/workflows/mac-app.yml) attaches a signed,
notarised `Leaf-<version>-aarch64.dmg` to the release and points the tap's
`leaf-editor` cask at it (`brew install --cask diaryx-org/tap/leaf-editor`).
The crates are not on crates.io (0.5.0 was the last upload): provui and the
Diaryx app name them from git, `{ git = "https://github.com/diaryx-org/leaf",
branch = "main" }`, and lock the commit they build.

`ship` is the org's release tool, which its other repositories cut releases
with too; what makes leaf leaf is [`.config/ship.figl`](.config/ship.figl) and
nothing else.

Sibling to [`bough`](https://github.com/diaryx-org/bough): **same backend, opposite model.** Where bough
moves a selection through the document's AST and edits the *tree*, leaf gives you
an ordinary text **caret**, mouse, selection, and a formatting toolbar — and
turns every keystroke into one of twig's offset-addressed edits. The document
stays a live, round-trippable AST the whole time you type into it, so a Markdown
file and a Djot file are edited through the exact same operations.

Two views:

- **source** — the raw document with the caret in source bytes.
- **wysiwyg** — the markup *resolved*: headings coloured, `**bold**` as real
  bold, `==🔴 text==` as a red highlighter wash, a `` ```rust `` fence
  syntax-highlighted, the `#` / `**` / `` ` `` delimiters hidden. The caret
  still works because
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
