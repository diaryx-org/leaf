---
title: What Android does not do yet
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-26
updated: 2026-10-26
---
# What Android does not do yet

**Where.** `apps/leaf-dioxus` (it has no Android module under
`src/platform/` yet) and `crates/leaf-dioxus`.

**What.** Leaf for Android was `archive/apps/leaf-android` over the Compose
editor in `archive/packages/leaf-android/leaf-compose`. What it did that the
Dioxus path does not, with where the archived code shows how:

- **The soft keyboard's `InputConnection`** (`LeafInputConnection.kt`):
  composition through core's insert; `deleteSurroundingText`, one character
  of it taken as Backspace; `getCursorCapsMode` for auto-capitals; a batch
  edit as one undo group; `performContextMenuAction`; the `EditorInfo` flags
  (multi-line, auto-correct, no full-screen, no enter action); and dead keys.
  The editor has only winit's `Ime` events. Whether winit's Android backend
  delivers composing or surrounding text at all is not checked. The task
  "Ctrl+Z under Gboard" is the same question for undo.
- **Selection gestures** (`LeafEditor.kt`): long press selects a word and a
  drag extends it, a double tap selects a word, a tap on the caret offers
  Paste, and the system's floating text toolbar (`LocalTextToolbar`). The
  editor has its own handles, and probably the double tap; nothing else.
- **The keyboard's inset** (`imePadding`, edge to edge, the caret kept in
  view; `MainActivity.kt`, `LeafEditor.kt`).
- **Keys from a hardware keyboard** (`LeafEditorState.kt`, `key()` and
  `chord()`): Ctrl or Alt with ←/→ moves by word, Ctrl+Home/End goes to the
  ends of the document, Ctrl+Backspace deletes a word, Ctrl+Y redoes. Off
  Apple the editor takes Ctrl as ⌘ (`input.rs`, `CMD`), so Ctrl+← goes to the
  line's edge and Ctrl+Backspace deletes to its start. Shared with Linux and
  Windows: the task "What Linux and Windows do not do yet".
- **The clipboard**: HTML `ClipData`, and paste as plain text
  (`LeafEditorState.kt`). arboard has no Android backend, so the host must
  supply the clipboard.
- **TalkBack.** The archived editor exposed only its toolbar. The editor's
  AccessKit text area is untested through the Android adapter.
- **The app.** Open and create through the Storage Access Framework, a
  persistable URI permission, opening from an intent, and the embed card
  (`archive/apps/leaf-android/app/.../MainActivity.kt`, `Document.kt`,
  `Embed.kt`).

**Done when.** Each is done in the Dioxus app on Android or deliberately
left out, with this list updated as they go.
