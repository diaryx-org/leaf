---
title: Archive
part_of: '[leaf](/README.md)'
---
# Archive

The frontends leaf no longer builds, archived on 2026-10-26. The Dioxus
editor, `crates/leaf-dioxus`, drawn by the Blitz fork, is the one path
forward: in windows (`apps/leaf-dioxus`) and in a terminal
(`apps/leaf-dioxus-tui`). What is here stays readable as a guide to what
each platform needs: each frontend solved its platform's input, selection,
accessibility and system services its own way, and the Dioxus path has not
solved all of them yet.

Each kept its path under `archive/`: `packages/leaf-swift` is
`archive/packages/leaf-swift`. Nothing here builds as it stands. The
workspace, CI, `ship` and the xtask no longer name any of it, and the paths
from here to the live crates (`../../crates/leaf-core`) are one level short.
To build one, check out `v0.5.1`, the last release that built them all.

| archived | what it was |
| -------- | ----------- |
| [`Package.swift`](Package.swift), [`packages/leaf-swift`](packages/leaf-swift), [`crates/leaf-ffi`](crates/leaf-ffi), [`crates/leaf-mermaid-ffi`](crates/leaf-mermaid-ffi) | LeafUI, the AppKit and UIKit editor, over the UniFFI binding, as a Swift package resolved from the repository root |
| [`apps/leaf-editor`](apps/leaf-editor) | Leaf for iOS (and the Mac, until `apps/leaf-dioxus`), over LeafUI. Its icon, sample and media moved to `apps/leaf-dioxus/resources` |
| [`packages/leaf-web`](packages/leaf-web), [`crates/leaf-wasm`](crates/leaf-wasm), [`crates/leaf-math-wasm`](crates/leaf-math-wasm), [`crates/leaf-mermaid-wasm`](crates/leaf-mermaid-wasm), [`apps/leaf-web-demo`](apps/leaf-web-demo) | the web editor (`@diaryx/leaf` on npm, last 0.1.18), its wasm bindings, and the demo page |
| [`crates/leaf-gpui`](crates/leaf-gpui), [`apps/leaf`](apps/leaf) | the gpui widget and desktop app, kept for Linux and Windows |
| [`apps/leaf-ios`](apps/leaf-ios/README.md) | the gpui iOS host, archived before the rest, on 2026-10-10 |
| [`packages/leaf-android`](packages/leaf-android), [`apps/leaf-android`](apps/leaf-android) | the Compose editor over the binding generated as Kotlin, and Leaf for Android |
| [`crates/leaf-ratatui`](crates/leaf-ratatui), [`apps/leaf-tui`](apps/leaf-tui/README.md) | the ratatui terminal editor, the `leaf` Homebrew formula until 0.5.1 |
| [`xtask/`](xtask), [`scripts/`](scripts) | the tasks and scripts that built and tested them |
| [`.github/workflows`](.github/workflows) | `ci.yml` (the Swift binding's drift, leaf-tui's release builds, the toolchain pin), `homebrew.yml` (the `leaf` formula), `pages.yml` (the web demo) |

## What still points here

- **The Homebrew `leaf` formula** stays at 0.5.1, leaf-tui. The task "Ship
  the Dioxus terminal editor as leaf" replaces it.
- **The web demo** at <https://diaryx-org.github.io/leaf/> stays as the last
  release deployed it until the next tag, whose `pages.yml` deploys
  `apps/leaf-dioxus-web` there instead. The task "A web demo drawn by the
  Dioxus editor".
- **`@diaryx/leaf` on npm** stays at 0.1.18.
- **SwiftPM.** The repository root has no `Package.swift` from this commit
  on, so a version range that resolves to a later tag fails. A pin to an
  earlier commit or tag still resolves: Diaryx's archived Swift app
  (`apps/swift` there) pins `a5262ea`.

## What the Dioxus path does not do yet

The archived code is where to look first for these. Each task names the
archived files that did it:

- [What the Mac app does not do yet](../docs/tasks/what-the-mac-app-does-not-do-yet.md)
- [What iOS does not do yet](../docs/tasks/what-ios-does-not-do-yet.md)
- [What Android does not do yet](../docs/tasks/what-android-does-not-do-yet.md)
- [What Linux and Windows do not do yet](../docs/tasks/what-linux-and-windows-do-not-do-yet.md)
- [Ship the Dioxus terminal editor as leaf](../docs/tasks/ship-the-dioxus-terminal-editor.md)
- [A web demo drawn by the Dioxus editor](../docs/tasks/a-dioxus-web-demo.md)
