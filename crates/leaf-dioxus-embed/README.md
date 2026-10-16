# leaf-dioxus-embed

The leaf-dioxus editor (`crates/leaf-dioxus`) in a native view its host owns,
through a C ABI: `include/leaf_dioxus_embed.h`, written by hand beside
`src/ffi.rs`, which is the authority. `apps/leaf-mac` is the host.

```sh
cargo test --manifest-path crates/leaf-dioxus-embed/Cargo.toml
```

dioxus-native runs the editor in a window winit makes, inside winit's event
loop. Here the host makes the view and runs the loop:

- **`window.rs`** — a winit `Window` that is the host's `NSView`. Blitz and
  the editor ask it for the size, the scale, a redraw and the input method's
  caret; wgpu draws into the view it names.
- **`runtime.rs`** — the event loop that is not there. Blitz's proxy wakes a
  main-queue block that reads its channel; a redraw is another, coalesced. The
  tokio runtime Blitz's network provider and the editor's timers need is
  entered once on the main thread.
- **`view.rs`** — one editor in one view: what dioxus-native's `launch` and
  blitz-shell's `View::init` put together around a winit window.
- **`host.rs`** — the editor's `Host`, answered through the host's callbacks,
  each sent on the main queue's next turn so a nested run loop (a context
  menu, a save panel) never finds the editor borrowed.
- **`pdf.rs`** — the document laid out offscreen at a paper's column, Blitz's
  paint recorded and drawn again as PDF pages with krilla, breaking between
  rows.
- **`guard.rs`** — a panic is caught at every way in and logged to
  `~/Library/Logs/Leaf/panics.log`, since it cannot unwind through C.

The host is the first responder and the text input client, and sends text as
it arrives (`leaf_view_insert_text`, `leaf_view_set_marked_text`) and every
other key as a DOM key and code (`leaf_view_key`). So the patched
winit-appkit `apps/leaf-dioxus` needs is not needed here.
