---
title: What Linux and Windows do not do yet
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-26
updated: 2026-10-26
---
# What Linux and Windows do not do yet

**Where.** `apps/leaf-dioxus` (it has no Linux or Windows module under
`src/platform/` yet) and `crates/leaf-dioxus/src/input.rs`.

**What.** The desktop app off the Mac was gpui's,
`archive/apps/leaf` over `archive/crates/leaf-gpui`. What it did that the
Dioxus path does not:

- **The app.** An Open dialog (`prompt_for_paths`), a guard before a window
  with unsaved changes closes or the app quits, and a check for a change on
  disk when the window is activated (`archive/apps/leaf/src/main.rs`).
- **Key conventions.** Ctrl+Y redoes, and the keys follow the CUA map
  (`archive/crates/leaf-gpui/src/lib.rs`, near line 378). Off Apple the editor
  takes Ctrl as ⌘ (`input.rs`, `CMD`), so Ctrl+←/→ goes to the line's edge
  rather than by word, Ctrl+Backspace deletes to the line's start, and there
  is no Ctrl+Home/End or Ctrl+Y. Android has the same divergence.
- **The input method.** gpui's `EntityInputHandler` (marked text,
  `bounds_for_range`). The editor takes winit's `Ime` events; they are not
  checked on Wayland, X11 or Windows.

Neither archived frontend had these, and the Dioxus path does not have them
yet: AccessKit's AT-SPI and UIA adapters (the text area is untested off the
Mac), and the primary selection with middle-click paste on Linux.

**Done when.** Each is done in the Dioxus app on Linux and Windows or
deliberately left out, with this list updated as they go.
