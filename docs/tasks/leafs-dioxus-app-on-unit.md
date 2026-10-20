---
title: Leaf's Dioxus app on unit
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-20
---
# Leaf's Dioxus app on unit

**Where.** `apps/leaf-dioxus` (the toolbar's menus, the context menu, the
Link prompt, Settings), and `crates/leaf-dioxus-ui`, which unit replaces.

**What.** [unit](https://github.com/diaryx-org/unit), diaryx-org's
component library for Dioxus on the web and on Blitz, grew out of
`leaf-dioxus-ui`: its `Unit` is `Layer` with a focus of the page's own, and
its menus, dialog and field are the same code with pointer events and ARIA.
The app would take it by git (`branch = "main"`), wrap each window's page in
`UnitRoot { keys: Keys::Host }`, call `Unit::key` and `Unit::text` where it
calls `Layer::key` and `Layer::text` now, and give keys to Blitz while
`Unit::typing()`; Settings could be unit's `Segmented`, `Switch` and
`Select`. Its toolbar's controls would then be buttons assistive tech can
press: today they are spans, which VoiceOver reads but cannot press.

It needs Blitz's changes unit was built against (diaryx-org/blitz#1) on the
fork's `leaf` branch, which the app builds on, and those change two things
the app may notice: a press focuses a focusable element, and a click goes to
the nearest element the press and the release share.

**What was found, 2026-10-20** (unit at 8bcc067, leaf at 77e3886b). The
app and unit already build on the same Blitz (the fork's `leaf` branch at
d78620c, diaryx-org/blitz#1 merged) and the same Dioxus (0.8.0-alpha.3),
and every name the app takes from `leaf-dioxus-ui` has a counterpart in
unit (`Layer` is `Unit`; `Entry`, `Item`, `Dialog`, `Answer`, `Chips`,
`Choice::styled`, `MenuButton`, `Clipboard`, `open_menu`, `open_dialog`
keep their shapes). What unit does not yet do for the app:

- **A pressed control keeps the keys.** In unit, a press on a `Button`,
  `ToggleButton` or `MenuButton` gives it the keys until a press lands
  elsewhere, and under `Keys::Host` `Unit::key` hands it Return, Space and
  the arrows. A toolbar's Bold pressed by the mouse would then take the
  next Space typed into the document, and a menu's button would open again
  on Return. The toolbar's spans now press on `mousedown` and keep the keys
  with the editor, as a Mac toolbar does. unit needs a control that does not
  take the keys on a press (or the app calls `Unit::blur` after each).
- **The overlay is drawn only by `UnitRoot`.** `use_unit_provider` is
  public but `layer::render` is not, so the editor has to sit inside
  `UnitRoot`'s `div`, whose stylesheet sets `box-sizing: border-box` on
  every descendant. The editor sets it where it lays out by measure; that
  nothing else in its rows moves is unchecked.
- **Two themes.** The app themes its chrome by rewriting
  `prefers-color-scheme` (`leaf_dioxus::themed`) and tests the toolbar's
  palette against the editor's; unit's colours are a `Theme` scoped to
  `.unit-root`, chosen by `data-appearance`. The app would pass its
  appearance to `UnitRoot` and a `Theme` in its own colours.
- **iOS** is unit's open task (unit's `docs/tasks/unit-on-ios.md`); the
  Dioxus app is Mac only, so it does not hold this move up.

**Done when.** The app's menus, prompt and Settings are unit's, work as they
do now with the mouse, the keys and VoiceOver, and `crates/leaf-dioxus-ui`
is gone.
