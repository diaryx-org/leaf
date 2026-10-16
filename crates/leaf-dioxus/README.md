# leaf-dioxus

The editor widget for Dioxus's native renderer: `dioxus-native`
0.8.0-alpha.2, which draws HTML and CSS through Blitz on wgpu, with no webview
and no browser engine. It is everything `apps/leaf-dioxus` draws in its window
but the settings panel: the rows and their proportional layout, the keys and
the input method, find and replace, the source view, tables as a grid, block
pictures, links and footnotes, and the status line. The app is one host for
it, as `apps/leaf` is for `leaf-gpui`; Diaryx's Dioxus port is to be another.

```sh
cargo test --manifest-path crates/leaf-dioxus/Cargo.toml
```

`apps/leaf-dioxus/README.md` says what the editor does and how each part was
found to need doing; this file is how to host it.

## Hosting it

```rust
use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_native::winit::event::WindowEvent;
use leaf_dioxus::{Host, Look, pressed, use_editor};

struct Mine;
impl Host for Mine {} // the system clipboard; everything else ignored

#[allow(non_snake_case)]
fn App() -> Element {
    let ed = use_editor(
        || leaf_core::Doc::from_source("# Hello".into(), leaf_core::Format::Markdown).unwrap(),
        Look::default,
        || Rc::new(Mine) as Rc<dyn Host>,
    );
    let mut mods = Default::default();
    dioxus_native::use_window_event({
        let ed = ed.clone();
        move |event, _| match event {
            WindowEvent::ModifiersChanged(m) => mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some((key, code, m)) = pressed(event, mods) {
                    ed.key(key, code, m);
                }
            }
            event => ed.window_event(event),
        }
    });
    ed.render()
}
```

- **`use_editor`** makes the editor in the calling component: the document,
  the [`Look`](#the-look) it draws with, and the [`Host`](#the-host) it asks.
  Each is made once, on the first render; `Editor::install` puts in another
  document and `Editor::set_look` another look.
- **The window's events** are the host's to hand over, because the editor
  reads its keys from winit rather than from DOM focus (Blitz's focus is not
  reliable enough for an editor; `keys.rs` says why). `pressed` turns winit's
  key into the editor's terms; `Editor::key` takes it, and
  `Editor::window_event` everything else it reads: the input method, the
  surface's size, a click's release, the theme. A host that wants a key first
  — the app's settings panel takes Escape — looks at it before passing it on.
- **`Editor::render`** is the editor's part of the page. The host draws its
  own beside it; `Palette` checks that the host's chrome sets every colour it
  uses for both appearances, as the editor's own stylesheet test does.

Beyond those, a host drives the editor through its methods: `doc()` to read
or change the document, `table` for the Table menu's commands, `open_link`,
`copy_link` and `follow_footnote` for a context menu's choices, `has_family`
to offer only installed fonts, and `repaint`/`refresh` after changing the
document itself.

## The host

What the editor cannot do by itself, it asks of the `Host`. Every method has a
default, so an empty `impl Host for Mine {}` is a host.

| Method | When | Default |
|---|---|---|
| `request` | a key the editor knows but leaves to the host: `Save` (⌘S), `SaveAs`, `Zoom` (⌘+ / ⌘− / ⌘0), `Settings` (⌘,) | ignored |
| `write_clipboard` / `read_clipboard` | Copy, Cut, Paste, a link's Copy Link, ⌘V in the find bar | the system clipboard, with the `system-clipboard` feature (on by default) |
| `character_palette` | ⌃⌘Space | nothing |
| `open` | a link to a URL or a file beside the document | nothing |
| `context_menu` | a right-click, with what it should offer (`Context`) | nothing |
| `repainted` | after every repaint, with the document: a window title, a dirty dot | nothing |

The editor never reads or writes a file: Save is a `Request`, so a host whose
documents are not files can store them where it likes, and the crate depends
on `leaf-core` without its `fs` feature. A host that opens files turns `fs`
on in its own dependency on `leaf-core`.

## The look

A `Look` is the type (body and code faces as CSS family lists, the size, the
leading), the column's measure in characters of the body face, and the
appearance — light, dark, or the system's. The editor measures its rows with
parley in exactly the type Blitz then draws them in, so a click lands on the
glyph under it; `set_look` relays every row out for a new one. `Look::default`
is the Swift editor's defaults.

## What a host has to know

- **Patch winit on macOS.** Press-and-hold accents, the emoji picker and
  dictation need the patched `winit-appkit` that `apps/leaf-dioxus` vendors
  (`patches/winit-appkit/PATCH.md`). This crate cannot carry the `[patch]`: a
  `[patch]` applies only at the root of the workspace that has it.
- **Blitz features.** The crate turns on the dioxus-native features the
  editor needs to draw correctly — classic `vello` (the default
  `vello-hybrid` cannot draw a colour emoji), `system-fonts`, `net` and
  `svg` — and leaves the rest to the host.
- **The editor assumes the window is its own.** Its stylesheet clips at
  `body`, its viewport is the window's surface less the status line, and it
  tells the input method where the caret is relative to the window's
  top-left corner. Placing it in part of a window is the task "Place the
  Dioxus editor in part of a window" in `docs/tasks/`.
- **A debug build can trip a Blitz assertion.** With `debug_assertions`,
  blitz-dom checks its layout tree after every change, and closing the find
  bar after a search has been seen to fail it ("layout child … is not in the
  slab") on Linux, before and after this crate was split out. Release builds
  do not check.
