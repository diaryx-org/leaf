# leaf-dioxus-ui

Menus, a dialog with a one-line text field, a select and chips for a Dioxus
page — built to work on dioxus-native (Blitz) as they do on the web, and so
on every platform Dioxus draws to. The toolbar's menus, the context menu,
the Link prompt and the Settings window of `apps/leaf-dioxus` are made of
them.

```sh
cargo test --manifest-path crates/leaf-dioxus-ui/Cargo.toml
```

It needs only `dioxus`, not the native renderer, so it builds in seconds and
a web page can use it as it is.

## Why not Dioxus's own

`dioxus-primitives` is written for a page in a browser, and under Blitz:

| The primitives | Under Blitz | Here |
|---|---|---|
| close on Escape and on a press outside through `document::eval`, a JavaScript listener | `eval` is a no-op: an open menu never closes | a backdrop under an open menu takes the press outside; the host gives the layer its keys first |
| move DOM focus with `MountedData::set_focus` from a task | Blitz polls tasks while it holds its document borrowed: a panic | a highlighted item in the menu's own state, which the arrow keys move |
| read keys from the focused element's `onkeydown` | Blitz drops DOM focus on any press that is not on a form control | the host's keys, from winit, routed to the layer while it has something open |
| make the menu when it opens, inside the button | Blitz lays out an absolutely positioned element on the first paint and not when a later frame inserts it, and skips painting a child that hangs past its parent's box | one overlay at the page's root, mounted from the first frame |

[Libero](https://github.com/tdymel/libero) targets Blitz too, and was tried in
the toolbar: its menu opened unstyled until the page had a `document`
context for its stylesheet, then closed as it opened. The task "Try Libero
again" in `docs/tasks/` keeps what was found.

## Using it

```rust
use leaf_dioxus_ui::{Item, MenuButton, use_layer_provider};

fn Page() -> Element {
    let layer = use_layer_provider();
    rsx! {
        style { {leaf_dioxus_ui::CSS} }
        MenuButton {
            entries: vec![Item::new(1, "Bold").shortcut("⌘B").into()],
            onpick: move |id| { /* … */ },
            "Format"
        }
        {layer.render()}
    }
}
```

and in the host's key handler, before anything else has the key:

```rust
if layer.is_open() && layer.key(&key, mods) {
    return;
}
```

— and `layer.text(&s)` for the input method's text, which a dialog's field
types. `Layer::set_viewport` keeps menus inside the window, and
`Layer::set_clipboard` gives a field the system's clipboard.

## What is in it

- `Layer` (`layer.rs`): the overlay, its backdrop, a fixed number of menu
  levels and the dialog, and the keys while one is open. The overlay itself
  is never hidden — showing a box in the same frame as rows arrive in an
  absolutely positioned child of it leaves blitz-dom 0.3.0-beta.4 a freed
  node in its layout tree, and every layout after panics — so it lets the
  pointer through (`pointer-events: none`), and what it holds is shown and
  hidden.
- `Menu` (`menu.rs`): entries, submenus, ticks, shortcuts, and every level's
  geometry, computed rather than measured, so a submenu knows where its
  parent's item is without reading the DOM. Unit-tested as a state machine.
- `Dialog` and `Field` (`dialog.rs`): a title, a message, one line to type
  and two buttons; the field keeps its own text and caret.
- `MenuButton`, `Select`, `Chips` (`controls.rs`).
- `CSS`: every colour a custom property, set for the light and the dark
  appearance, so a host that forces one rewrites both alike.
