# leaf-ffi

The C-ABI / UniFFI **Rust binding** for leaf: it wraps the filesystem-free
`leaf-core` `Doc` behind UniFFI so a native Apple app can drive the byte-offset
caret model and render the `VisualMap` as style runs. The native-Apple peer of
`leaf-wasm`.

This crate is only the Rust binding (`src/lib.rs` + the `uniffi-bindgen` bin).
The Swift side built on top of it lives elsewhere:

| Piece | Location | What it is |
|-------|----------|------------|
| Swift SDK | [`packages/leaf-swift`](../../packages/leaf-swift) | `Sources/LeafUI` (the AppKit/UIKit editor) + the committed `uniffi-generated/` Swift, exposed by the `Package.swift` at the repo root. The importable Swift package. |
| Demo app | [`apps/leaf-editor`](../../apps/leaf-editor) | The runnable cross-platform (macOS + iOS) example (`bootstrap.sh`, xcodegen `project.yml`). |

The Swift bindings are (re)generated from this crate by
`scripts/gen-bindings.sh` into `packages/leaf-swift/uniffi-generated/`, which is
**committed** — the Swift package is consumed by version from a bare git
checkout, so the binding must build as-is. CI diffs the committed binding
against this crate on every push; regenerate and commit after changing the FFI
surface.
