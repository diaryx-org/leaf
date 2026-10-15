---
title: Split leaf-dioxus into a widget crate and its app
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-15
updated: 2026-10-15
---

# Split leaf-dioxus into a widget crate and its app

**Where.** `apps/leaf-dioxus/src`.

**What.** `apps/leaf-dioxus` is one binary: the editor and the app around it
share a crate, and their modules call each other. `dispatch.rs` runs file
commands (`file.rs`) and menu commands (`menu.rs`). `editor.rs` holds the
display settings (`prefs.rs`). `main.rs` installs the menu bar and reads the
window's events. A host that wants the editor in a window of its own —
Diaryx, being ported to Dioxus — cannot take it without the menu bar, the file
dialogs and the settings file.

The split mirrors `leaf-gpui` and `apps/leaf`. A `crates/leaf-dioxus` widget
would hold the editor, rows, blocks, text, IME, find, source view and bridge.
Its own workspace would be on the same `dioxus-native` alpha, with no
`[patch]`; a consumer patches winit itself. The app would keep `main.rs`,
`menu.rs`, `file.rs`, the settings file and the panel. The seam is what the
editor asks the host for: commands it cannot run itself (Open, Save As,
New), the clipboard and the character palette (`host.rs`, `input::Host`), and
the settings it draws with.

**Why deferred.** The app was promoted first so that it is built, tested and
linted as a supported frontend. The seam is best drawn against a second host,
and Diaryx's port is that host.
