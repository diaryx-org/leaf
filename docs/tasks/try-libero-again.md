---
title: Try Libero again
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-19
---
# Try Libero again

**Where.** `apps/leaf-dioxus` (the toolbar's menus, the context menu, the
Link prompt, Settings) and `crates/leaf-dioxus-ui`, which Libero would
replace or shrink.

**What.** [Libero](https://github.com/tdymel/libero) (crates.io `libero`,
MIT/Apache) is a Dioxus component library — over a hundred components, with
accessibility, keyboard support and themes — pinned to Dioxus
`=0.8.0-alpha.3`, the version leaf's Dioxus work is on, with a `native`
feature that reaches elements through Blitz rather than JavaScript. It was
tried on 2026-10-19 as the toolbar's Style menu, and set aside so the Mac
app's move to Rust could finish on `leaf-dioxus-ui`. What the spike found:

- **It needs a `document` context.** Its theme is a stylesheet it puts in
  the page's head through `dioxus::document`. dioxus-native's `launch`
  provides one and keeps it private, and the app builds its windows itself;
  without one the menu opened unstyled. The app now provides its own
  (`apps/leaf-dioxus/src/head.rs`), which is what Libero needs.
- **With its styles, the menu closed as it opened.** The trigger lit and no
  menu stayed. The likely cause is its dismissal on focus leaving the menu,
  against Blitz dropping DOM focus on a press that is not on a form control
  — and against the page taking the press for the editor. Not chased.
- **Keys are DOM keys.** Libero hears keys bubbling through the page; the
  editor reads them from winit and keeps them from Blitz. While a Libero
  overlay is open the app would hand Blitz the keys instead (the spike did,
  through a flag the menu's open state set).
- **iOS is untried.** Libero's iOS runs are Dioxus's WebView renderer; its
  Blitz runs are the desktops'. Leaf on iOS is Blitz.
- It shares the Blitz bug `leaf-dioxus-ui` works around (blitz-dom
  0.3.0-beta.4 keeps a freed node in its layout tree), and turns blitz-dom's
  debug assertions off for it.

**Done when.** Either Libero's menus, dialog and choice controls work in the
Mac app — open, dismissed by a press outside and by Escape, walked by the
arrow keys, a dialog's field typed in — and on iOS, and `leaf-dioxus-ui` is
reduced to what Libero does not do; or it is decided against, with why.
