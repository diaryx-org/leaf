# Leaf for the Mac

A SwiftUI document app whose editor is the leaf-dioxus editor
(`crates/leaf-dioxus`), drawn by Blitz on wgpu in an `NSView` of the app's
own, through the C ABI of `crates/leaf-dioxus-embed`. Everything around the
editor is native: `DocumentGroup`'s windows, autosave, Versions, recents and
Open With; the menus and the toolbar; the Settings window; the context menu;
and the text input client, so press-and-hold accents, the Character Viewer
and dictation are AppKit's.

```sh
cargo xtask swift            # build and launch, on a copy of the sample
cargo xtask swift notes.md   # on a document of your own
cargo xtask package          # signed, notarised Leaf.app and .dmg in target/package/
```

`project.yml` is the project (xcodegen; the `.xcodeproj` is generated and
ignored). Its pre-build script builds the Rust staticlib, so a Rust edit needs
only a rebuild. The icon, the sample and its media are the iOS app's, in
`apps/leaf-editor/App`, which stays on LeafUI: the two cannot share a target,
since each links a Rust staticlib and two in one binary carry two Rust
runtimes.

## Where things are

| File | What it does |
|---|---|
| `LeafApp.swift` | the scenes: a `DocumentGroup` per format, Settings, File ▸ New, Export as PDF, Page Setup, Print, Help ▸ Open Sample Document |
| `LeafDocument.swift` | the document classes, one per format, and the sample |
| `EditorModel.swift` | one window's editor as the chrome sees it: its `LeafState`, and every command, mostly the key chord it stands for |
| `LeafEditorView.swift` | the `NSView`, in an `NSScrollView` that does its scrolling: pointer, keys, the text input client, the Edit menu's actions, the context menu |
| `EditorCommands.swift` | Edit, Format and View, aimed at the window in front |
| `ContentView.swift` | the window: the editor and its toolbar |
| `DisplayChoice.swift`, `SettingsView.swift` | the reader's display choices and Settings |

A menu item with a shortcut is the editor's key binding: choosing it sends
the chord to `crates/leaf-dioxus/src/input.rs`, as the keyboard would, so a
command is defined once. What no key does is a `leaf_view_*` call.

What the LeafUI Mac app did and this one does not yet is the task "What the
Mac app does not do yet" in `docs/tasks/`.
