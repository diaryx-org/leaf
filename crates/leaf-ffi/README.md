# leaf-ffi

The C-ABI / UniFFI **Rust binding** for leaf: it wraps the filesystem-free
`leaf-core` `Doc` behind UniFFI so a native Apple app can drive the byte-offset
caret model and render the `VisualMap` as style runs. The native-Apple peer of
`leaf-wasm`.

This crate is only the Rust binding (`src/lib.rs` + the `uniffi-bindgen` bin).
The Swift side built on top of it lives elsewhere:

| Piece | Location | What it is |
|-------|----------|------------|
| Swift SDK | [`packages/leaf-swift`](../../packages/leaf-swift) | `Package.swift` + `Sources/LeafUI` (the AppKit/UIKit editor) + the UniFFI-`generated/` Swift. The importable Swift package. |
| Demo app | [`apps/leaf-editor`](../../apps/leaf-editor) | The runnable cross-platform (macOS + iOS) example (`bootstrap.sh`, xcodegen `project.yml`). |

The Swift bindings are (re)generated from this crate by
`apps/leaf-editor/bootstrap.sh` (dev) or `scripts/build-xcframework.sh`
(distributable xcframework), both writing into `packages/leaf-swift/generated/`.
