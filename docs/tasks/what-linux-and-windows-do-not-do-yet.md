---
title: What Linux and Windows do not do yet
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-26
updated: 2026-11-04
---
# What Linux and Windows do not do yet

**Where.** `apps/leaf-dioxus/src/platform/desktop` and
`crates/leaf-dioxus/src/input.rs`.

**Done (2026-10-27).** The app runs on Linux: `platform/desktop` keeps its
own files (Open and Save through the XDG portal's dialogs, a `*` in the
title while there are unsaved changes, and a Discard/Cancel question before
such a window closes), stores settings in `<config>/leaf/settings`, uses
the system clipboard, and exports a PDF through twig-export. A File menu
in the toolbar and the Ctrl keys stand in for the menu bar. Windows
compiles the same module, untried. Still to do:

- **Close's question has two answers.** Discard or Cancel; there is no
  Save in it, since unit's dialog has two buttons.
- **No Print.** Export as PDF, then print from the viewer.
- **No spelling checker and no Look Up.**

Since then (2026-10-27): the document reads and saves through leaf-core's
file API, as the terminal editor does, so a save is written beside the file
and renamed over it, undoing back to what was saved clears the title's `*`,
a window coming forward takes what another program wrote to a clean
document (and warns a dirty one), and Save over such a change asks first.
File ▸ PDF Preview (Ctrl+Alt+P, or the toolbar's PDF) shows the pages
Export as PDF writes beside the editor, set again as it is edited
(`crates/leaf-pages`); tried on Linux, not yet on the Mac or Windows.

**What.** The desktop app off the Mac was gpui's,
`archive/apps/leaf` over `archive/crates/leaf-gpui`. What it did that the
Dioxus path does not:

- **The app.** Done (above): the Open dialog, the guard before a window
  with unsaved changes closes, and the check for a change on disk when the
  window is activated.
- **Key conventions.** Done (2026-11-04): off a Mac's keys, Ctrl+Y redoes
  beside Ctrl+Shift+Z, Ctrl+←/→ goes by word, Ctrl+Backspace and Ctrl+Delete
  delete a word, and Ctrl+Home/End goes to the document's ends (`input.rs`,
  `command`).
- **The input method.** gpui's `EntityInputHandler` (marked text,
  `bounds_for_range`). The editor takes winit's `Ime` events; they are not
  checked on Wayland, X11 or Windows.

Neither archived frontend had these, and the Dioxus path does not have them
yet: AccessKit's AT-SPI and UIA adapters (the text area is untested off the
Mac), and the primary selection with middle-click paste on Linux.

**Done when.** Each is done in the Dioxus app on Linux and Windows or
deliberately left out, with this list updated as they go.
